//! Events streamed from a live session to the UI.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine;
use devall_core::vars::redact_values;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::now_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// Received from the server.
    In,
    /// Sent by us.
    Out,
}

/// Something that happened on a connection. `at` is milliseconds since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ProtoEvent {
    /// A connection attempt started.
    Connecting { at: i64, target: String },
    /// The connection is established (handshake headers when the protocol has them).
    Opened {
        at: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        headers: Vec<(String, String)>,
    },
    /// A message in either direction. `body` is text, or base64 when `binary` is set.
    Message {
        at: i64,
        direction: Direction,
        /// Event name (Socket.IO), topic (MQTT), method (gRPC); none for plain WebSocket frames.
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        body: String,
        binary: bool,
        /// Payload size in bytes.
        size: usize,
        /// Protocol details (QoS/retain, ack ids, …).
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<serde_json::Value>,
    },
    /// Response headers or trailers (gRPC).
    Metadata { at: i64, trailers: bool, entries: Vec<(String, String)> },
    Info { at: i64, message: String },
    Error { at: i64, message: String },
    /// The connection or call ended. `code` is the WebSocket close code or gRPC status code.
    Closed {
        at: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl ProtoEvent {
    pub fn info(message: impl Into<String>) -> Self {
        ProtoEvent::Info { at: now_ms(), message: message.into() }
    }

    pub fn error(message: impl Into<String>) -> Self {
        ProtoEvent::Error { at: now_ms(), message: message.into() }
    }

    pub fn closed(code: Option<i64>, reason: Option<String>) -> Self {
        ProtoEvent::Closed { at: now_ms(), code, reason }
    }

    /// A message event for a text payload.
    pub fn text(direction: Direction, label: Option<String>, body: impl Into<String>, meta: Option<serde_json::Value>) -> Self {
        let body = body.into();
        ProtoEvent::Message { at: now_ms(), direction, label, size: body.len(), body, binary: false, meta }
    }

    /// A message event for raw bytes: shown as text when valid UTF-8, base64 otherwise.
    pub fn bytes(direction: Direction, label: Option<String>, data: &[u8], meta: Option<serde_json::Value>) -> Self {
        match std::str::from_utf8(data) {
            Ok(t) => Self::text(direction, label, t, meta),
            Err(_) => Self::binary(direction, label, data, meta),
        }
    }

    /// A message event for binary data (always base64).
    pub fn binary(direction: Direction, label: Option<String>, data: &[u8], meta: Option<serde_json::Value>) -> Self {
        ProtoEvent::Message {
            at: now_ms(),
            direction,
            label,
            body: base64::engine::general_purpose::STANDARD.encode(data),
            binary: true,
            size: data.len(),
            meta,
        }
    }

    pub fn is_closed(&self) -> bool {
        matches!(self, ProtoEvent::Closed { .. })
    }

    /// Masks every secret value in the event's text (bodies, errors, headers).
    pub fn redact(&mut self, secrets: &BTreeSet<String>) {
        if secrets.is_empty() {
            return;
        }
        let r = |s: &mut String| *s = redact_values(s, secrets);
        match self {
            ProtoEvent::Connecting { target, .. } => r(target),
            ProtoEvent::Opened { detail, headers, .. } => {
                detail.as_mut().map(r);
                headers.iter_mut().for_each(|(_, v)| r(v));
            }
            ProtoEvent::Message { label, body, binary, meta, .. } => {
                label.as_mut().map(r);
                if !*binary {
                    r(body);
                }
                if let Some(m) = meta {
                    let text = m.to_string();
                    let red = redact_values(&text, secrets);
                    if red != text {
                        *m = serde_json::from_str(&red).unwrap_or(serde_json::Value::Null);
                    }
                }
            }
            ProtoEvent::Metadata { entries, .. } => entries.iter_mut().for_each(|(_, v)| r(v)),
            ProtoEvent::Info { message, .. } | ProtoEvent::Error { message, .. } => r(message),
            ProtoEvent::Closed { reason, .. } => {
                reason.as_mut().map(r);
            }
        }
    }
}

/// Where a session reports its events. Cheap to clone.
#[derive(Clone)]
pub struct EventSink {
    f: Arc<dyn Fn(ProtoEvent) + Send + Sync>,
}

impl std::fmt::Debug for EventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventSink")
    }
}

impl EventSink {
    pub fn new(f: impl Fn(ProtoEvent) + Send + Sync + 'static) -> Self {
        Self { f: Arc::new(f) }
    }

    /// A sink that drops everything.
    pub fn noop() -> Self {
        Self::new(|_| {})
    }

    /// A sink feeding an unbounded channel (tests, CLI).
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<ProtoEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self::new(move |e| drop(tx.send(e))), rx)
    }

    pub fn emit(&self, e: ProtoEvent) {
        (self.f)(e)
    }

    pub fn info(&self, message: impl Into<String>) {
        self.emit(ProtoEvent::info(message))
    }

    pub fn error(&self, message: impl Into<String>) {
        self.emit(ProtoEvent::error(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_tagged_camel_case() {
        let e = ProtoEvent::text(Direction::Out, Some("chat".into()), "hi", Some(serde_json::json!({ "ackId": 1 })));
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "message");
        assert_eq!(v["direction"], "out");
        assert_eq!(v["size"], 2);
        assert_eq!(v["meta"]["ackId"], 1);
        let c = serde_json::to_value(ProtoEvent::closed(Some(1000), None)).unwrap();
        assert_eq!(c["type"], "closed");
        assert!(c.get("reason").is_none());
    }

    #[test]
    fn binary_payloads_are_base64() {
        let ProtoEvent::Message { body, binary, size, .. } = ProtoEvent::bytes(Direction::In, None, &[0xff, 0x00, 0x10], None) else { panic!() };
        assert!(binary);
        assert_eq!(size, 3);
        assert_eq!(body, "/wAQ");
    }

    #[test]
    fn redaction_masks_text_and_meta() {
        let secrets: BTreeSet<String> = ["s3cr3t-token".to_string()].into_iter().collect();
        let mut e = ProtoEvent::text(Direction::Out, None, "{\"token\":\"s3cr3t-token\"}", Some(serde_json::json!({ "auth": "s3cr3t-token" })));
        e.redact(&secrets);
        let v = serde_json::to_value(&e).unwrap();
        assert!(!v.to_string().contains("s3cr3t-token"), "{v}");
        let mut err = ProtoEvent::error("bad token s3cr3t-token");
        err.redact(&secrets);
        assert_eq!(err, ProtoEvent::Error { at: match err { ProtoEvent::Error { at, .. } => at, _ => 0 }, message: "bad token ••••••".into() });
    }

    #[tokio::test]
    async fn channel_sink_delivers() {
        let (sink, mut rx) = EventSink::channel();
        sink.info("hello");
        assert!(matches!(rx.recv().await, Some(ProtoEvent::Info { message, .. }) if message == "hello"));
    }
}
