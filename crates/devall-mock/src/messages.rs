//! The message journal shared by the WebSocket, gRPC and MQTT mocks: every frame / message /
//! connection event, newest last, capped, and broadcast as [`JournalEvent::Message`].

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::JournalEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Protocol {
    Websocket,
    Grpc,
    Mqtt,
}

impl Protocol {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "websocket" | "ws" => Some(Self::Websocket),
            "grpc" => Some(Self::Grpc),
            "mqtt" => Some(Self::Mqtt),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// From the client to the mock.
    In,
    /// From the mock to the client.
    Out,
    /// Connection lifecycle (open, close, subscribe, call status, …).
    Event,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEvent {
    pub seq: u64,
    /// Epoch milliseconds.
    pub at: i64,
    pub protocol: Protocol,
    /// Connection id (WebSocket), call id (gRPC) or client id (MQTT).
    pub connection: String,
    pub direction: Direction,
    /// WebSocket path, gRPC `/package.Service/Method` or MQTT topic.
    pub channel: String,
    /// Text, or Base64 when `binary`.
    pub body: String,
    pub binary: bool,
    /// Name (or id) of the stub / rule that produced or matched it.
    pub stub: Option<String>,
    /// Protocol details (`qos`, `retain`, `status`, `closeCode`, `event`, …).
    pub meta: Value,
}

pub(crate) struct Messages {
    events: Mutex<VecDeque<MessageEvent>>,
    seq: AtomicU64,
    cap: usize,
    tx: broadcast::Sender<JournalEvent>,
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Text when UTF-8, otherwise Base64.
pub(crate) fn body_of(bytes: &[u8]) -> (String, bool) {
    match std::str::from_utf8(bytes) {
        Ok(t) => (t.to_string(), false),
        Err(_) => (crate::util::b64_encode(bytes), true),
    }
}

impl Messages {
    pub fn new(cap: usize, tx: broadcast::Sender<JournalEvent>) -> Self {
        Self { events: Mutex::new(VecDeque::new()), seq: AtomicU64::new(0), cap: cap.max(10), tx }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record(&self, protocol: Protocol, connection: &str, direction: Direction, channel: &str, body: String, binary: bool, stub: Option<&str>, meta: Value) {
        let e = MessageEvent {
            seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
            at: now_ms(),
            protocol,
            connection: connection.to_string(),
            direction,
            channel: channel.to_string(),
            body,
            binary,
            stub: stub.map(str::to_string),
            meta,
        };
        {
            let mut q = self.events.lock().unwrap_or_else(|e| e.into_inner());
            q.push_back(e.clone());
            while q.len() > self.cap {
                q.pop_front();
            }
        }
        let _ = self.tx.send(JournalEvent::Message(e));
    }

    /// Newest first, optionally one protocol, at most `limit`.
    pub fn list(&self, protocol: Option<Protocol>, limit: usize) -> Vec<MessageEvent> {
        let q = self.events.lock().unwrap_or_else(|e| e.into_inner());
        q.iter().rev().filter(|e| protocol.is_none_or(|p| e.protocol == p)).take(limit).cloned().collect()
    }

    pub fn clear(&self, protocol: Option<Protocol>) {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).retain(|e| protocol.is_some_and(|p| e.protocol != p));
    }
}
