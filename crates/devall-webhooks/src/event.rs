//! Captured webhook events and the small helpers that read them.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::model::ProviderKind;
use crate::verify::VerifyResult;

/// Request bodies above this are captured truncated and answered with 413 (as webhook.site does).
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Where an event came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventSource {
    /// The local receiver (directly, over the LAN, or through a tunnel).
    #[default]
    Local,
    /// The hosted relay (buffered while the app was offline).
    Relay,
    /// A replay sent from the app (`replayOf` points at the original).
    Replay,
    /// Imported from a file or another tool.
    Import,
}

impl EventSource {
    pub fn as_str(self) -> &'static str {
        match self {
            EventSource::Local => "local",
            EventSource::Relay => "relay",
            EventSource::Replay => "replay",
            EventSource::Import => "import",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "relay" => EventSource::Relay,
            "replay" => EventSource::Replay,
            "import" => EventSource::Import,
            _ => EventSource::Local,
        }
    }
}

/// A request captured by the receiver or handed to [`crate::WebhookHub::ingest`] by another
/// source (the hosted relay). Nothing here is resolved or verified yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEvent {
    pub method: String,
    /// Full URL as received, e.g. `http://127.0.0.1:4590/stripe?x=1` (the relay passes its public URL).
    pub url: String,
    /// Path used for inbox routing, e.g. `/stripe`.
    pub path: String,
    /// Raw query string without `?` (still percent-encoded).
    #[serde(default)]
    pub query: String,
    /// In received order, duplicates preserved.
    pub headers: Vec<(String, String)>,
    #[serde(with = "b64_bytes")]
    pub body: Vec<u8>,
    #[serde(default)]
    pub body_truncated: bool,
    /// Peer address (`ip:port`).
    #[serde(default)]
    pub remote_addr: Option<String>,
    /// Unix milliseconds; `0` = now.
    #[serde(default)]
    pub received_at: i64,
    #[serde(default)]
    pub source: EventSource,
    /// The URL the sender called, when known (relay URL, tunnel URL). Twilio signs it.
    #[serde(default)]
    pub public_url: Option<String>,
    /// The source's own id (e.g. the relay's event id). Ingesting the same id twice is a no-op,
    /// so at-least-once delivery doesn't create duplicates.
    #[serde(default)]
    pub external_id: Option<String>,
}

/// Outcome of forwarding an event (or sending a replay) to a target.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardResult {
    pub url: String,
    /// Unix milliseconds when the request was sent.
    pub at: i64,
    pub status: Option<u16>,
    #[serde(default)]
    pub status_text: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// First 64 KB of the response body (lossy UTF-8).
    #[serde(default)]
    pub body_excerpt: Option<String>,
    #[serde(default)]
    pub body_size: usize,
    pub duration_ms: f64,
    /// Set when no response was received (connection refused, timeout, invalid URL).
    pub error: Option<String>,
}

impl ForwardResult {
    pub fn failed(url: impl Into<String>, error: impl Into<String>) -> Self {
        Self { url: url.into(), at: now_ms(), error: Some(error.into()), ..Default::default() }
    }
}

/// A stored event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEvent {
    /// ULID (sortable by arrival).
    pub id: String,
    /// Inbox id (file stem under `webhooks/`); `None` = matched no inbox (catch-all).
    pub inbox: Option<String>,
    pub received_at: i64,
    pub source: EventSource,
    pub method: String,
    pub url: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    #[serde(skip)]
    pub body: Vec<u8>,
    pub body_size: usize,
    pub body_truncated: bool,
    pub content_type: Option<String>,
    pub remote_addr: Option<String>,
    pub public_url: Option<String>,
    /// Event type when the payload says (`payment_intent.succeeded`, `push`, `orders/create`, …).
    pub label: Option<String>,
    /// The inbox's provider when the event was received.
    pub provider: Option<ProviderKind>,
    /// Status the receiver answered with.
    pub response_status: Option<u16>,
    pub verification: Option<VerifyResult>,
    pub forward: Option<ForwardResult>,
    /// For replays: the event that was replayed.
    pub replay_of: Option<String>,
    /// The source's id for events from the relay/imports (deduplication key).
    pub external_id: Option<String>,
}

impl WebhookEvent {
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// The URL the sender called: the public URL when known, else the local one.
    pub fn signed_url(&self) -> &str {
        self.public_url.as_deref().filter(|u| !u.is_empty()).unwrap_or(&self.url)
    }

    pub fn body_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.body).ok()
    }

    pub fn summary(&self) -> EventSummary {
        EventSummary {
            id: self.id.clone(),
            inbox: self.inbox.clone(),
            received_at: self.received_at,
            source: self.source,
            method: self.method.clone(),
            path: self.path.clone(),
            query: self.query.clone(),
            label: self.label.clone(),
            body_size: self.body_size,
            content_type: self.content_type.clone(),
            remote_addr: self.remote_addr.clone(),
            response_status: self.response_status,
            signature: self.verification.as_ref().map(|v| SignatureBadge { valid: v.valid, status: v.status, reason: v.reason.clone() }),
            forward_status: self.forward.as_ref().and_then(|f| f.status),
            forward_error: self.forward.as_ref().and_then(|f| f.error.clone()),
            forward_ms: self.forward.as_ref().map(|f| f.duration_ms),
            replay_of: self.replay_of.clone(),
        }
    }

    /// JSON-friendly view: the body as text when it is UTF-8, base64 otherwise.
    pub fn into_view(self) -> EventView {
        let (body_text, body_base64) = match String::from_utf8(self.body.clone()) {
            Ok(t) => (Some(t), None),
            Err(_) => (None, Some(base64::engine::general_purpose::STANDARD.encode(&self.body))),
        };
        EventView { event: self, body_text, body_base64 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureBadge {
    pub valid: bool,
    pub status: crate::verify::VerifyStatus,
    pub reason: String,
}

/// List row (no headers/body).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSummary {
    pub id: String,
    pub inbox: Option<String>,
    pub received_at: i64,
    pub source: EventSource,
    pub method: String,
    pub path: String,
    pub query: String,
    pub label: Option<String>,
    pub body_size: usize,
    pub content_type: Option<String>,
    pub remote_addr: Option<String>,
    pub response_status: Option<u16>,
    pub signature: Option<SignatureBadge>,
    pub forward_status: Option<u16>,
    pub forward_error: Option<String>,
    pub forward_ms: Option<f64>,
    pub replay_of: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventView {
    #[serde(flatten)]
    pub event: WebhookEvent,
    pub body_text: Option<String>,
    pub body_base64: Option<String>,
}

/// Filter + page for [`crate::EventStore::list`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EventQuery {
    /// Inbox id; `""` = events that matched no inbox; absent = all.
    pub inbox: Option<String>,
    /// Case-insensitive match on path, method, label or body text.
    pub search: Option<String>,
    pub offset: usize,
    /// Default 100, max 1000.
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub events: Vec<EventSummary>,
    /// Matching events in total (for paging).
    pub total: usize,
}

/// Live notifications from the store.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StoreNotice {
    Added { event: EventSummary },
    /// Verification or forward result arrived.
    Updated { event: EventSummary },
    Deleted { id: String },
    Cleared { inbox: Option<String> },
    /// Something failed in the background (e.g. the database could not be written).
    Error { message: String },
}

/// Optional pruning. Without it history is unlimited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Retention {
    pub max_events: Option<u64>,
    pub max_age_days: Option<u64>,
}

impl Retention {
    pub fn is_unlimited(&self) -> bool {
        self.max_events.is_none() && self.max_age_days.is_none()
    }
}

pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// Replaces every `name` header (case-insensitively) with `value` at the first one's position, or appends.
pub fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    match headers.iter().position(|(k, _)| k.eq_ignore_ascii_case(name)) {
        Some(i) => {
            headers[i].1 = value.to_string();
            let mut j = i + 1;
            while j < headers.len() {
                if headers[j].0.eq_ignore_ascii_case(name) {
                    headers.remove(j);
                } else {
                    j += 1;
                }
            }
        }
        None => headers.push((name.to_string(), value.to_string())),
    }
}

pub fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
}

/// RFC 9110 §7.6.1 connection-specific headers (plus `Proxy-Connection`), never forwarded.
pub const HOP_BY_HOP: &[&str] = &["connection", "keep-alive", "proxy-connection", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"];

/// Headers to drop when re-sending a captured request: hop-by-hop ones, those named in
/// `Connection`, and `Content-Length` (recomputed from the body).
pub fn without_hop_by_hop(headers: &[(String, String)]) -> Vec<(String, String)> {
    let named: Vec<String> = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| v.split(',').map(|s| s.trim().to_ascii_lowercase()).collect::<Vec<_>>())
        .collect();
    headers
        .iter()
        .filter(|(k, _)| {
            let l = k.to_ascii_lowercase();
            !HOP_BY_HOP.contains(&l.as_str()) && l != "content-length" && !named.contains(&l)
        })
        .cloned()
        .collect()
}

pub fn is_json_content_type(ct: Option<&str>) -> bool {
    ct.map(|c| {
        let c = c.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        c == "application/json" || c.ends_with("+json") || c == "text/json"
    })
    .unwrap_or(false)
}

pub fn is_form_content_type(ct: Option<&str>) -> bool {
    ct.map(|c| c.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/x-www-form-urlencoded")).unwrap_or(false)
}

/// The event type the payload declares, for list labels and file names.
pub fn event_label(provider: Option<ProviderKind>, headers: &[(String, String)], body: &[u8]) -> Option<String> {
    for h in ["X-GitHub-Event", "X-Shopify-Topic", "X-Gitlab-Event", "X-Event-Key", "X-Event-Type", "X-Webhook-Event"] {
        if let Some(v) = header(headers, h).filter(|v| !v.is_empty()) {
            let action = if h == "X-GitHub-Event" {
                serde_json::from_slice::<serde_json::Value>(body).ok().and_then(|j| j.get("action").and_then(|a| a.as_str()).map(str::to_string))
            } else {
                None
            };
            return Some(match action {
                Some(a) => format!("{v}.{a}"),
                None => v.to_string(),
            });
        }
    }
    let ct = header(headers, "content-type");
    if is_form_content_type(ct) {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(body).into_owned().collect();
        let get = |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
        // Slack slash commands / interactivity, Twilio voice & messaging.
        if let Some(c) = get("command") {
            return Some(c);
        }
        if let Some(p) = get("payload")
            && let Ok(j) = serde_json::from_str::<serde_json::Value>(&p)
            && let Some(t) = j.get("type").and_then(|t| t.as_str())
        {
            return Some(t.to_string());
        }
        for k in ["SmsStatus", "MessageStatus", "CallStatus", "EventType"] {
            if let Some(v) = get(k) {
                return Some(v);
            }
        }
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    if provider == Some(ProviderKind::Slack)
        && let Some(t) = json.pointer("/event/type").and_then(|t| t.as_str())
    {
        return Some(t.to_string());
    }
    for p in ["/type", "/event_type", "/eventType", "/event", "/topic", "/action"] {
        if let Some(t) = json.pointer(p).and_then(|t| t.as_str()).filter(|t| !t.is_empty() && t.len() <= 120) {
            return Some(t.to_string());
        }
    }
    None
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Bodies cross JSON as base64 (so [`IncomingEvent`] can come from the relay as JSON).
mod b64_bytes {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(b))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn hop_by_hop_and_connection_named_headers_are_dropped() {
        let hs = h(&[("Host", "x"), ("Connection", "keep-alive, X-Secret-Hop"), ("x-secret-hop", "1"), ("Content-Length", "3"), ("Transfer-Encoding", "chunked"), ("X-A", "1"), ("x-a", "2")]);
        let out = without_hop_by_hop(&hs);
        assert_eq!(out, h(&[("Host", "x"), ("X-A", "1"), ("x-a", "2")]));
    }

    #[test]
    fn set_header_replaces_all_duplicates_in_place() {
        let mut hs = h(&[("a", "1"), ("Sig", "old"), ("b", "2"), ("sig", "old2")]);
        set_header(&mut hs, "SIG", "new");
        assert_eq!(hs, h(&[("a", "1"), ("Sig", "new"), ("b", "2")]));
        set_header(&mut hs, "c", "3");
        assert_eq!(hs.last().unwrap(), &("c".to_string(), "3".to_string()));
    }

    #[test]
    fn labels() {
        let gh = h(&[("X-GitHub-Event", "pull_request"), ("content-type", "application/json")]);
        assert_eq!(event_label(Some(ProviderKind::Github), &gh, br#"{"action":"opened"}"#).as_deref(), Some("pull_request.opened"));
        let stripe = h(&[("content-type", "application/json")]);
        assert_eq!(event_label(Some(ProviderKind::Stripe), &stripe, br#"{"id":"evt_1","type":"invoice.paid"}"#).as_deref(), Some("invoice.paid"));
        let slack = h(&[("content-type", "application/json")]);
        assert_eq!(event_label(Some(ProviderKind::Slack), &slack, br#"{"type":"event_callback","event":{"type":"app_mention"}}"#).as_deref(), Some("app_mention"));
        let form = h(&[("content-type", "application/x-www-form-urlencoded")]);
        assert_eq!(event_label(None, &form, b"command=%2Fdeploy&text=prod").as_deref(), Some("/deploy"));
        assert_eq!(event_label(None, &form, b"SmsStatus=received&Body=hi").as_deref(), Some("received"));
        assert_eq!(event_label(None, &stripe, b"not json"), None);
    }

    #[test]
    fn incoming_event_json_uses_base64_body() {
        let e = IncomingEvent { method: "POST".into(), body: vec![0, 159, 146, 150], ..Default::default() };
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["body"], "AJ+Slg==");
        let back: IncomingEvent = serde_json::from_value(j).unwrap();
        assert_eq!(back.body, vec![0, 159, 146, 150]);
    }
}
