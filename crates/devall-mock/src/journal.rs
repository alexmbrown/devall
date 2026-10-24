//! Request journal: a bounded ring buffer of serve events in WireMock's JSON shape.

use std::collections::VecDeque;
use std::sync::Arc;

use bytes::Bytes;
use serde::Serialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::request::MockRequest;
use crate::stub::{Fault, Stub};
use crate::util::{b64_encode, canonical_header_name};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub added_delay: u64,
    pub process_time: u64,
    pub response_send_time: u64,
    pub serve_time: u64,
    pub total_time: u64,
}

#[derive(Debug, Clone)]
pub struct LoggedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub fault: Option<Fault>,
}

#[derive(Debug, Clone)]
pub struct ServeEvent {
    /// Monotonic sequence number (recordings use it to find events since they started).
    pub seq: u64,
    pub id: Uuid,
    pub request: MockRequest,
    pub stub: Option<Arc<Stub>>,
    pub response_definition: Value,
    pub response: LoggedResponse,
    pub was_matched: bool,
    pub timing: Timing,
    /// Upstream base URL when the response was proxied.
    pub proxied_from: Option<String>,
    pub path_params: Vec<(String, String)>,
    /// What the chaos layer did to this request, if anything.
    pub chaos: Option<Value>,
}

pub(crate) fn headers_json(headers: &[(String, String)]) -> Value {
    let mut map: Map<String, Value> = Map::new();
    for (k, v) in headers {
        let key = canonical_header_name(k);
        match map.get_mut(&key) {
            None => {
                map.insert(key, Value::String(v.clone()));
            }
            Some(Value::Array(a)) => a.push(Value::String(v.clone())),
            Some(existing) => {
                let first = existing.take();
                *existing = Value::Array(vec![first, Value::String(v.clone())]);
            }
        }
    }
    Value::Object(map)
}

impl ServeEvent {
    pub fn request_json(&self) -> Value {
        let mut r = self.request.to_json();
        if !self.path_params.is_empty()
            && let Value::Object(o) = &mut r
        {
            let params: Map<String, Value> = self.path_params.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
            o.insert("pathParameters".into(), Value::Object(params));
        }
        r
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("id".into(), json!(self.id.to_string()));
        m.insert("request".into(), self.request_json());
        if let Some(stub) = &self.stub {
            m.insert("stubMapping".into(), stub.to_json());
        }
        m.insert("responseDefinition".into(), self.response_definition.clone());
        let mut resp = Map::new();
        resp.insert("status".into(), json!(self.response.status));
        resp.insert("headers".into(), headers_json(&self.response.headers));
        resp.insert("bodyAsBase64".into(), json!(b64_encode(&self.response.body)));
        resp.insert("body".into(), json!(String::from_utf8_lossy(&self.response.body)));
        if let Some(f) = self.response.fault {
            resp.insert("fault".into(), json!(f.name()));
        }
        m.insert("response".into(), Value::Object(resp));
        m.insert("wasMatched".into(), json!(self.was_matched));
        m.insert("timing".into(), serde_json::to_value(&self.timing).unwrap_or(Value::Null));
        m.insert("subEvents".into(), json!([]));
        if let Some(c) = &self.chaos {
            m.insert("devallChaos".into(), c.clone());
        }
        Value::Object(m)
    }
}

#[derive(Debug)]
pub struct Journal {
    events: VecDeque<Arc<ServeEvent>>,
    capacity: usize,
    next_seq: u64,
}

impl Journal {
    pub fn new(capacity: usize) -> Self {
        Self { events: VecDeque::new(), capacity: capacity.max(1), next_seq: 1 }
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn push(&mut self, mut event: ServeEvent) -> Arc<ServeEvent> {
        event.seq = self.next_seq;
        self.next_seq += 1;
        let arc = Arc::new(event);
        self.events.push_back(arc.clone());
        while self.events.len() > self.capacity {
            self.events.pop_front();
        }
        arc
    }

    /// Oldest first.
    pub fn events(&self) -> Vec<Arc<ServeEvent>> {
        self.events.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn get(&self, id: &Uuid) -> Option<Arc<ServeEvent>> {
        self.events.iter().find(|e| e.id == *id).cloned()
    }

    pub fn remove(&mut self, id: &Uuid) -> bool {
        let before = self.events.len();
        self.events.retain(|e| e.id != *id);
        before != self.events.len()
    }

    pub fn retain(&mut self, keep: impl Fn(&ServeEvent) -> bool) -> Vec<Arc<ServeEvent>> {
        let mut removed = Vec::new();
        self.events.retain(|e| {
            let k = keep(e);
            if !k {
                removed.push(e.clone());
            }
            k
        });
        removed
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(url: &str) -> ServeEvent {
        ServeEvent {
            seq: 0,
            id: Uuid::new_v4(),
            request: MockRequest::new("GET", url, vec![], Vec::new()),
            stub: None,
            response_definition: json!({"status": 404}),
            response: LoggedResponse { status: 404, headers: vec![("content-type".into(), "text/plain".into())], body: Bytes::from_static(b"nope"), fault: None },
            was_matched: false,
            timing: Timing::default(),
            proxied_from: None,
            path_params: vec![],
            chaos: None,
        }
    }

    #[test]
    fn ring_buffer() {
        let mut j = Journal::new(2);
        let a = j.push(event("/a"));
        j.push(event("/b"));
        j.push(event("/c"));
        assert_eq!(j.len(), 2);
        assert!(j.get(&a.id).is_none());
        let urls: Vec<String> = j.events().iter().map(|e| e.request.url.clone()).collect();
        assert_eq!(urls, ["/b", "/c"]);
        assert_eq!(j.events()[1].seq, 3);
        let json = j.events()[0].to_json();
        assert_eq!(json["response"]["headers"]["Content-Type"], "text/plain");
        assert_eq!(json["response"]["body"], "nope");
        assert_eq!(json["wasMatched"], false);
        let removed = j.retain(|e| e.request.url != "/b");
        assert_eq!(removed.len(), 1);
        j.clear();
        assert_eq!(j.len(), 0);
    }
}
