//! Stub mappings: the JSON as the user wrote it (kept for round-tripping, in a stable key order)
//! plus the compiled request pattern and response definition.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::pattern::RequestPattern;
use crate::util::{b64_decode, json_text};

pub const DEFAULT_PRIORITY: i64 = 5;

/// Where a persistent stub lives, relative to `mocks/mappings/` (`/`-separated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubSource {
    pub rel_path: String,
    /// `true` when the file holds `{"mappings": [...]}`.
    pub multi: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Fault {
    ConnectionResetByPeer,
    EmptyResponse,
    MalformedResponseChunk,
    RandomDataThenClose,
}

impl Fault {
    pub const ALL: [Fault; 4] = [Fault::ConnectionResetByPeer, Fault::EmptyResponse, Fault::MalformedResponseChunk, Fault::RandomDataThenClose];

    pub fn name(self) -> &'static str {
        match self {
            Fault::ConnectionResetByPeer => "CONNECTION_RESET_BY_PEER",
            Fault::EmptyResponse => "EMPTY_RESPONSE",
            Fault::MalformedResponseChunk => "MALFORMED_RESPONSE_CHUNK",
            Fault::RandomDataThenClose => "RANDOM_DATA_THEN_CLOSE",
        }
    }

    pub fn parse(s: &str) -> Option<Fault> {
        Fault::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(s.trim()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DelayDistribution {
    Uniform { lower: u64, upper: u64 },
    Lognormal { median: f64, sigma: f64, #[serde(default, skip_serializing_if = "Option::is_none")] max_value: Option<f64> },
    Fixed { milliseconds: u64 },
}

impl DelayDistribution {
    /// Samples a delay in milliseconds.
    pub fn sample(&self) -> u64 {
        match self {
            Self::Uniform { lower, upper } => {
                let (lo, hi) = if lower <= upper { (*lower, *upper) } else { (*upper, *lower) };
                fastrand::u64(lo..=hi)
            }
            Self::Lognormal { median, sigma, max_value } => {
                // Box–Muller for a standard normal sample.
                let u1 = fastrand::f64().max(f64::MIN_POSITIVE);
                let u2 = fastrand::f64();
                let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
                let mut v = median * (sigma * z).exp();
                if let Some(max) = max_value {
                    v = v.min(*max);
                }
                v.max(0.0).round() as u64
            }
            Self::Fixed { milliseconds } => *milliseconds,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dribble {
    pub chunks: u32,
    pub total_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ProxySpec {
    pub base_url: String,
    pub prefix_to_remove: Option<String>,
    pub additional_headers: Vec<(String, String)>,
    pub remove_headers: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum BodySpec {
    Empty,
    Text(String),
    Json(Value),
    Binary(Bytes),
    File(String),
}

#[derive(Debug, Clone)]
pub struct ResponseDef {
    pub status: u16,
    pub status_message: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: BodySpec,
    pub fixed_delay_ms: Option<u64>,
    pub delay: Option<DelayDistribution>,
    pub dribble: Option<Dribble>,
    pub fault: Option<Fault>,
    pub proxy: Option<ProxySpec>,
    pub transformers: Vec<String>,
    pub transformer_parameters: Value,
    /// The `response` JSON as written.
    pub raw: Value,
}

fn headers_of(v: Option<&Value>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(Value::Object(m)) = v {
        for (k, v) in m {
            match v {
                Value::Array(a) => out.extend(a.iter().map(|x| (k.clone(), json_text(x)))),
                other => out.push((k.clone(), json_text(other))),
            }
        }
    }
    out
}

impl ResponseDef {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let empty = Map::new();
        let obj = match v {
            Value::Object(o) => o,
            Value::Null => &empty,
            _ => return Err("response must be an object".into()),
        };
        let status = match obj.get("status") {
            None | Some(Value::Null) => 200,
            Some(s) => json_text(s).trim().parse::<u16>().map_err(|_| format!("invalid status {s}"))?,
        };
        if !(100..=999).contains(&status) {
            return Err(format!("invalid status {status}"));
        }
        let body = if let Some(f) = obj.get("bodyFileName").filter(|v| !v.is_null()) {
            BodySpec::File(json_text(f))
        } else if let Some(b) = obj.get("base64Body").filter(|v| !v.is_null()) {
            BodySpec::Binary(Bytes::from(b64_decode(&json_text(b)).ok_or("base64Body is not valid base64")?))
        } else if let Some(j) = obj.get("jsonBody").filter(|v| !v.is_null()) {
            BodySpec::Json(j.clone())
        } else if let Some(b) = obj.get("body").filter(|v| !v.is_null()) {
            BodySpec::Text(json_text(b))
        } else {
            BodySpec::Empty
        };
        let delay = match obj.get("delayDistribution").filter(|v| !v.is_null()) {
            Some(d) => Some(serde_json::from_value::<DelayDistribution>(d.clone()).map_err(|e| format!("delayDistribution: {e}"))?),
            None => None,
        };
        let dribble = obj.get("chunkedDribbleDelay").filter(|v| !v.is_null()).map(|d| Dribble {
            chunks: d.get("numberOfChunks").and_then(Value::as_u64).unwrap_or(1).clamp(1, 10_000) as u32,
            total_ms: d.get("totalDuration").and_then(Value::as_u64).unwrap_or(0),
        });
        let fault = match obj.get("fault").filter(|v| !v.is_null()) {
            Some(f) => Some(Fault::parse(&json_text(f)).ok_or_else(|| format!("unknown fault {f}"))?),
            None => None,
        };
        let proxy = obj.get("proxyBaseUrl").filter(|v| !v.is_null() && v.as_str() != Some("")).map(|b| ProxySpec {
            base_url: json_text(b),
            prefix_to_remove: obj.get("proxyUrlPrefixToRemove").filter(|v| !v.is_null()).map(json_text),
            additional_headers: headers_of(obj.get("additionalProxyRequestHeaders")),
            remove_headers: obj.get("removeProxyRequestHeaders").and_then(Value::as_array).map(|a| a.iter().map(json_text).collect()).unwrap_or_default(),
        });
        let transformers = match obj.get("transformers") {
            Some(Value::Array(a)) => a.iter().map(json_text).collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        };
        Ok(Self {
            status,
            status_message: obj.get("statusMessage").filter(|v| !v.is_null()).map(json_text),
            headers: headers_of(obj.get("headers")),
            body,
            fixed_delay_ms: obj.get("fixedDelayMilliseconds").and_then(Value::as_u64),
            delay,
            dribble,
            fault,
            proxy,
            transformers,
            transformer_parameters: obj.get("transformerParameters").cloned().unwrap_or_else(|| json!({})),
            raw: Value::Object(obj.clone()),
        })
    }

    pub fn templated(&self) -> bool {
        self.transformers.iter().any(|t| t == "response-template")
    }
}

#[derive(Debug, Clone)]
pub struct Stub {
    pub id: Uuid,
    pub name: Option<String>,
    pub request: RequestPattern,
    pub response: ResponseDef,
    pub priority: Option<i64>,
    pub scenario_name: Option<String>,
    pub required_state: Option<String>,
    pub new_state: Option<String>,
    pub persistent: bool,
    pub metadata: Option<Value>,
    pub insertion_index: u64,
    pub source: Option<StubSource>,
    /// Canonical JSON without `uuid` / `insertionIndex` (those are added on output).
    json: Map<String, Value>,
}

/// Top-level keys in the order WireMock writes them; anything else follows in input order.
const KEY_ORDER: &[&str] = &[
    "id",
    "name",
    "request",
    "response",
    "persistent",
    "priority",
    "scenarioName",
    "requiredScenarioState",
    "newScenarioState",
    "postServeActions",
    "serveEventListeners",
    "metadata",
];

fn opt_string(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).filter(|v| !v.is_null()).map(json_text)
}

impl Stub {
    /// Compiles a stub from WireMock JSON. `fallback_id` is used when neither `id` nor `uuid` is set.
    pub fn from_json(v: &Value, fallback_id: impl FnOnce() -> Uuid) -> Result<Self, String> {
        let obj = v.as_object().ok_or("stub mapping must be a JSON object")?;
        let id = match obj.get("id").or_else(|| obj.get("uuid")).filter(|v| !v.is_null()) {
            Some(Value::String(s)) => Uuid::parse_str(s.trim()).map_err(|_| format!("invalid stub id {s:?} (must be a UUID)"))?,
            Some(other) => return Err(format!("invalid stub id {other}")),
            None => fallback_id(),
        };
        let request_json = obj.get("request").ok_or("request is required")?;
        let request = RequestPattern::from_json(request_json).map_err(|e| format!("request: {e}"))?;
        let response = ResponseDef::from_json(obj.get("response").unwrap_or(&Value::Null)).map_err(|e| format!("response: {e}"))?;
        let priority = match obj.get("priority").filter(|v| !v.is_null()) {
            Some(p) => Some(json_text(p).trim().parse::<i64>().map_err(|_| format!("invalid priority {p}"))?),
            None => None,
        };
        let persistent = matches!(obj.get("persistent"), Some(Value::Bool(true)));
        let mut json = Map::new();
        json.insert("id".into(), Value::String(id.to_string()));
        for key in KEY_ORDER.iter().skip(1) {
            if let Some(v) = obj.get(*key).filter(|v| !v.is_null()) {
                json.insert((*key).to_string(), v.clone());
            }
        }
        if !json.contains_key("response") {
            json.insert("response".into(), json!({ "status": 200 }));
        }
        for (k, v) in obj {
            if !json.contains_key(k) && !matches!(k.as_str(), "uuid" | "insertionIndex" | "id") && !v.is_null() {
                json.insert(k.clone(), v.clone());
            }
        }
        Ok(Self {
            id,
            name: opt_string(obj, "name"),
            request,
            response,
            priority,
            scenario_name: opt_string(obj, "scenarioName"),
            required_state: opt_string(obj, "requiredScenarioState"),
            new_state: opt_string(obj, "newScenarioState"),
            persistent,
            metadata: obj.get("metadata").filter(|v| !v.is_null()).cloned(),
            insertion_index: 0,
            source: None,
            json,
        })
    }

    pub fn effective_priority(&self) -> i64 {
        self.priority.unwrap_or(DEFAULT_PRIORITY)
    }

    pub fn set_persistent(&mut self, persistent: bool) {
        self.persistent = persistent;
        if persistent {
            // keep the canonical position of the key
            let mut out = Map::new();
            let mut inserted = false;
            for (k, v) in std::mem::take(&mut self.json) {
                if k == "persistent" {
                    continue;
                }
                let after_response = k != "id" && k != "name" && k != "request" && k != "response";
                if after_response && !inserted {
                    out.insert("persistent".into(), Value::Bool(true));
                    inserted = true;
                }
                out.insert(k, v);
            }
            if !inserted {
                out.insert("persistent".into(), Value::Bool(true));
            }
            self.json = out;
        } else {
            self.json.remove("persistent");
        }
    }

    /// JSON for the admin API (WireMock 3.x shape: includes the `uuid` alias and `insertionIndex`).
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        for (k, v) in &self.json {
            out.insert(k.clone(), v.clone());
            if k == "response" {
                out.insert("uuid".into(), Value::String(self.id.to_string()));
            }
        }
        out.insert("insertionIndex".into(), json!(self.insertion_index));
        Value::Object(out)
    }

    /// JSON written to `mocks/mappings/` (no runtime-only fields).
    pub fn to_file_json(&self) -> Value {
        Value::Object(self.json.clone())
    }

    /// `METHOD /path` for display and file names.
    pub fn summary(&self) -> String {
        let req = self.json.get("request");
        let method = req.and_then(|r| r.get("method")).map_or_else(|| "ANY".into(), json_text);
        let url = req
            .and_then(|r| ["url", "urlPath", "urlPathTemplate", "urlPattern", "urlPathPattern"].iter().find_map(|k| r.get(*k)))
            .map_or_else(|| "/".into(), json_text);
        format!("{method} {url}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_orders() {
        let v = json!({
            "metadata": {"tag": "x"},
            "response": {"status": 201, "jsonBody": {"ok": true}, "headers": {"X-A": ["1", "2"]}},
            "request": {"method": "POST", "urlPath": "/x"},
            "custom": 1,
            "uuid": "2f6a7c8e-0000-4000-8000-000000000001",
            "priority": 2,
            "name": "Create"
        });
        let s = Stub::from_json(&v, Uuid::new_v4).unwrap();
        assert_eq!(s.id.to_string(), "2f6a7c8e-0000-4000-8000-000000000001");
        assert_eq!(s.effective_priority(), 2);
        assert_eq!(s.response.status, 201);
        assert_eq!(s.response.headers.len(), 2);
        let keys: Vec<String> = s.to_file_json().as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["id", "name", "request", "response", "priority", "metadata", "custom"]);
        let admin = s.to_json();
        assert_eq!(admin["uuid"], admin["id"]);
        assert_eq!(s.summary(), "POST /x");

        let mut s2 = s.clone();
        s2.set_persistent(true);
        let keys: Vec<String> = s2.to_file_json().as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["id", "name", "request", "response", "persistent", "priority", "metadata", "custom"]);
    }

    #[test]
    fn rejects_bad_stubs() {
        assert!(Stub::from_json(&json!({"response": {}}), Uuid::new_v4).is_err());
        assert!(Stub::from_json(&json!({"request": {}, "response": {"status": 99}}), Uuid::new_v4).is_err());
        assert!(Stub::from_json(&json!({"id": "nope", "request": {}}), Uuid::new_v4).is_err());
        assert!(Stub::from_json(&json!({"request": {"urlPattern": "("}}), Uuid::new_v4).is_err());
        assert!(Stub::from_json(&json!({"request": {}, "response": {"fault": "BOOM"}}), Uuid::new_v4).is_err());
    }

    #[test]
    fn delay_distributions() {
        let u: DelayDistribution = serde_json::from_value(json!({"type": "uniform", "lower": 10, "upper": 20})).unwrap();
        for _ in 0..50 {
            assert!((10..=20).contains(&u.sample()));
        }
        let l: DelayDistribution = serde_json::from_value(json!({"type": "lognormal", "median": 50, "sigma": 0.5, "maxValue": 80})).unwrap();
        for _ in 0..50 {
            assert!(l.sample() <= 80);
        }
        let f: DelayDistribution = serde_json::from_value(json!({"type": "fixed", "milliseconds": 7})).unwrap();
        assert_eq!(f.sample(), 7);
    }
}
