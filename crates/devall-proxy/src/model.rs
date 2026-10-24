//! Captured traffic: exchanges (request + response + timing), the views sent to the UI, and the
//! live events. Raw bodies are kept exactly as they crossed the wire (still compressed); views
//! decode `Content-Encoding` for display.

use base64::Engine;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::body::decode_body;
use crate::util::{header_value, mime_essence};

pub type ExchangeId = u64;

/// Header name/value pairs in wire order (names are lower-case: HTTP/2 and hyper normalize them).
pub type HeaderList = Vec<(String, String)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExchangeState {
    /// Request received, waiting for the upstream response.
    Pending,
    /// Held at a breakpoint (see `paused` for the phase).
    Paused,
    /// Response headers received, body streaming.
    Receiving,
    Complete,
    /// Upstream/connect/TLS failure or a client disconnect; see `error`.
    Failed,
    /// Aborted at a breakpoint.
    Aborted,
    /// Blocked by a rule.
    Blocked,
    /// CONNECT tunnel passed through without decryption (bypass list / non-HTTP).
    Tunnel,
    /// Upgraded to a WebSocket; frames are captured until it closes.
    WebSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Request,
    Response,
}

/// Body bytes as captured. `raw` holds at most the configured cap; `size` is the full wire size.
#[derive(Debug, Clone, Default)]
pub struct CapturedBody {
    pub raw: Bytes,
    pub size: u64,
    pub truncated: bool,
    /// Dropped to stay under the capture's total memory budget.
    pub evicted: bool,
}

impl CapturedBody {
    pub fn new(raw: Bytes, cap: usize) -> Self {
        let size = raw.len() as u64;
        if raw.len() > cap {
            Self { raw: raw.slice(..cap), size, truncated: true, evicted: false }
        } else {
            Self { raw, size, truncated: false, evicted: false }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    /// Absolute URL as the client asked for it (before Map Remote).
    pub url: String,
    pub http_version: String,
    pub headers: HeaderList,
    pub body: CapturedBody,
}

#[derive(Debug, Clone)]
pub struct CapturedResponse {
    pub status: u16,
    pub reason: String,
    pub http_version: String,
    /// As delivered to the client (after header rules and breakpoint edits).
    pub headers: HeaderList,
    pub body: CapturedBody,
}

/// HAR-style timing in milliseconds. Connection phases are only known for the exchange that
/// opened the connection; reused connections report `connectionReused: true` instead.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    /// Time held at breakpoints or delayed by throttle/mock rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_ms: Option<f64>,
    /// Request sent → first response byte (server think time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<f64>,
    /// First response byte → last body byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_ms: Option<f64>,
    /// Request received by the proxy → response fully delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_reused: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WsDirection {
    /// Client → server.
    Send,
    /// Server → client.
    Receive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsFrame {
    pub direction: WsDirection,
    /// Milliseconds since the exchange started.
    pub at_ms: f64,
    /// `text`, `binary`, `ping`, `pong` or `close`.
    pub opcode: String,
    pub size: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// Findings of a "Validate against OpenAPI" rule for one exchange.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecValidation {
    pub rule: String,
    /// Spec file name.
    pub spec: String,
    /// Answered with a problem response instead of the real one (block mode).
    pub blocked: bool,
    pub request: Vec<devall_openapi::Violation>,
    pub response: Vec<devall_openapi::Violation>,
    /// The response was checked (it may still be pending).
    pub response_checked: bool,
    /// The response body was too large or not fully captured: only its status was checked.
    pub response_body_skipped: bool,
}

impl SpecValidation {
    pub fn count(&self) -> usize {
        self.request.len() + self.response.len()
    }
}

/// One request/response pair (or tunnel / WebSocket session).
#[derive(Debug, Clone)]
pub struct Exchange {
    pub id: ExchangeId,
    /// Unix epoch milliseconds.
    pub started_at: i64,
    pub client_addr: Option<String>,
    /// Best effort; not determined yet on any platform.
    pub client_process: Option<String>,
    pub request: CapturedRequest,
    pub response: Option<CapturedResponse>,
    pub state: ExchangeState,
    pub error: Option<String>,
    pub timing: Timing,
    /// Upstream socket address.
    pub server_addr: Option<String>,
    /// Where Map Remote sent the request, when it differs from `request.url`.
    pub mapped_url: Option<String>,
    /// Names of the rules that matched.
    pub rules: Vec<String>,
    /// Free-form markers: `edited`, `mocked`, `mapLocal`, `mapRemote`, `throttled`, `replay`,
    /// `bypassed`, `imported`, `breakpointTimeout`, …
    pub tags: Vec<String>,
    pub ws_frames: Vec<WsFrame>,
    /// Total frames seen (frames beyond the per-exchange cap are counted but not kept).
    pub ws_frame_count: usize,
    pub paused: Option<Phase>,
    /// Set when a Validate against OpenAPI rule checked this exchange.
    pub validation: Option<SpecValidation>,
}

impl Exchange {
    pub fn new(method: impl Into<String>, url: impl Into<String>, http_version: impl Into<String>, headers: HeaderList) -> Self {
        Self {
            id: 0,
            started_at: crate::util::now_ms(),
            client_addr: None,
            client_process: None,
            request: CapturedRequest { method: method.into(), url: url.into(), http_version: http_version.into(), headers, body: CapturedBody::default() },
            response: None,
            state: ExchangeState::Pending,
            error: None,
            timing: Timing::default(),
            server_addr: None,
            mapped_url: None,
            rules: Vec::new(),
            tags: Vec::new(),
            ws_frames: Vec::new(),
            ws_frame_count: 0,
            paused: None,
            validation: None,
        }
    }

    pub fn tag(&mut self, tag: &str) {
        if !self.tags.iter().any(|t| t == tag) {
            self.tags.push(tag.to_string());
        }
    }

    pub fn parsed_url(&self) -> Option<url::Url> {
        url::Url::parse(&self.request.url).ok()
    }

    /// Response content type, else the request's.
    pub fn content_type(&self) -> Option<String> {
        self.response
            .as_ref()
            .and_then(|r| header_value(&r.headers, "content-type"))
            .or_else(|| header_value(&self.request.headers, "content-type"))
            .map(|s| s.to_string())
    }

    pub fn body_bytes(&self) -> usize {
        self.request.body.raw.len() + self.response.as_ref().map_or(0, |r| r.body.raw.len())
    }

    pub fn summary(&self) -> ExchangeSummary {
        let (scheme, host, path) = split_url(&self.request.url);
        ExchangeSummary {
            id: self.id,
            started_at: self.started_at,
            method: self.request.method.clone(),
            url: self.request.url.clone(),
            scheme,
            host,
            path,
            http_version: self.request.http_version.clone(),
            status: self.response.as_ref().map(|r| r.status),
            state: self.state,
            content_type: self.content_type().map(|c| mime_essence(&c)),
            request_size: self.request.body.size,
            response_size: self.response.as_ref().map(|r| r.body.size),
            duration_ms: self.timing.total_ms,
            rules: self.rules.clone(),
            tags: self.tags.clone(),
            paused: self.paused,
            error: self.error.clone(),
            ws_frames: self.ws_frame_count,
            client_addr: self.client_addr.clone(),
            mapped_url: self.mapped_url.clone(),
            violations: self.validation.as_ref().map_or(0, SpecValidation::count),
        }
    }

    pub fn detail(&self) -> ExchangeDetail {
        let req_ct = header_value(&self.request.headers, "content-type").map(str::to_string);
        let request = MessageView {
            method: Some(self.request.method.clone()),
            url: Some(self.request.url.clone()),
            status: None,
            reason: None,
            http_version: self.request.http_version.clone(),
            headers: self.request.headers.clone(),
            body: BodyView::build(&self.request.body, header_value(&self.request.headers, "content-encoding"), req_ct),
        };
        let response = self.response.as_ref().map(|r| MessageView {
            method: None,
            url: None,
            status: Some(r.status),
            reason: Some(r.reason.clone()),
            http_version: r.http_version.clone(),
            headers: r.headers.clone(),
            body: BodyView::build(&r.body, header_value(&r.headers, "content-encoding"), header_value(&r.headers, "content-type").map(str::to_string)),
        });
        ExchangeDetail {
            summary: self.summary(),
            request,
            response,
            timing: self.timing.clone(),
            server_addr: self.server_addr.clone(),
            client_process: self.client_process.clone(),
            frames: self.ws_frames.clone(),
            validation: self.validation.clone(),
        }
    }
}

/// `(scheme, host[:port], path?query)` of an absolute URL, tolerant of odd inputs (tunnels use
/// `host:port`).
pub(crate) fn split_url(url: &str) -> (String, String, String) {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = match rest.find('/') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => match rest.find('?') {
                    Some(i) => (&rest[..i], &rest[i..]),
                    None => (rest, "/"),
                },
            };
            (scheme.to_ascii_lowercase(), authority.to_string(), path.to_string())
        }
        None => (String::new(), url.to_string(), String::new()),
    }
}

/// One row of the exchange list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeSummary {
    pub id: ExchangeId,
    pub started_at: i64,
    pub method: String,
    pub url: String,
    pub scheme: String,
    /// `host[:port]` as requested.
    pub host: String,
    /// Path and query.
    pub path: String,
    pub http_version: String,
    pub status: Option<u16>,
    pub state: ExchangeState,
    /// MIME essence (`application/json`), response first.
    pub content_type: Option<String>,
    pub request_size: u64,
    pub response_size: Option<u64>,
    pub duration_ms: Option<f64>,
    pub rules: Vec<String>,
    pub tags: Vec<String>,
    pub paused: Option<Phase>,
    pub error: Option<String>,
    pub ws_frames: usize,
    pub client_addr: Option<String>,
    pub mapped_url: Option<String>,
    /// OpenAPI violations found by a validation rule (0 = none or not checked).
    pub violations: usize,
}

/// A body prepared for display: decoded when compressed, text when valid UTF-8 else base64.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyView {
    /// Size on the wire.
    pub size: u64,
    /// Bytes kept by the capture (≤ size).
    pub captured: usize,
    pub truncated: bool,
    pub evicted: bool,
    /// `Content-Encoding`, when present.
    pub encoding: Option<String>,
    /// True when the content below is the decoded form of a compressed body.
    pub decoded: bool,
    pub decode_error: Option<String>,
    pub content_type: Option<String>,
    pub text: Option<String>,
    pub base64: Option<String>,
}

impl BodyView {
    pub fn build(body: &CapturedBody, encoding: Option<&str>, content_type: Option<String>) -> Self {
        let mut view = BodyView {
            size: body.size,
            captured: body.raw.len(),
            truncated: body.truncated,
            evicted: body.evicted,
            encoding: encoding.map(str::to_string),
            decoded: false,
            decode_error: None,
            content_type,
            text: None,
            base64: None,
        };
        if body.raw.is_empty() {
            if !body.evicted {
                view.text = Some(String::new());
            }
            return view;
        }
        let bytes: std::borrow::Cow<'_, [u8]> = match encoding.filter(|e| !e.trim().is_empty() && !e.eq_ignore_ascii_case("identity")) {
            Some(enc) => match decode_body(&body.raw, enc) {
                Ok(d) => {
                    view.decoded = true;
                    d.into()
                }
                Err(e) => {
                    view.decode_error = Some(e);
                    (&body.raw[..]).into()
                }
            },
            None => (&body.raw[..]).into(),
        };
        match std::str::from_utf8(&bytes) {
            Ok(t) => view.text = Some(t.to_string()),
            Err(_) => view.base64 = Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
        }
        view
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    /// Requests only.
    pub method: Option<String>,
    pub url: Option<String>,
    /// Responses only.
    pub status: Option<u16>,
    pub reason: Option<String>,
    pub http_version: String,
    pub headers: HeaderList,
    pub body: BodyView,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeDetail {
    #[serde(flatten)]
    pub summary: ExchangeSummary,
    pub request: MessageView,
    pub response: Option<MessageView>,
    pub timing: Timing,
    pub server_addr: Option<String>,
    pub client_process: Option<String>,
    /// WebSocket frames (the summary's `wsFrames` is the total count).
    pub frames: Vec<WsFrame>,
    /// Findings of a Validate against OpenAPI rule.
    pub validation: Option<SpecValidation>,
}

/// A request or response held at a breakpoint, as shown in the breakpoint editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PausedExchange {
    pub id: ExchangeId,
    pub phase: Phase,
    pub method: String,
    pub url: String,
    /// Response phase only.
    pub status: Option<u16>,
    pub headers: HeaderList,
    pub body: BodyView,
    /// Unix epoch milliseconds when the breakpoint resumes on its own.
    pub deadline: i64,
}

/// Changes applied when resuming a breakpoint (or replaying). Unset fields keep their value.
/// `method`/`url` apply to requests, `status` to responses. Setting `body` (text) or
/// `bodyBase64` replaces the body; the message is then sent uncompressed with a fresh
/// `Content-Length`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BreakpointEdit {
    pub method: Option<String>,
    pub url: Option<String>,
    pub status: Option<u16>,
    pub headers: Option<HeaderList>,
    pub body: Option<String>,
    pub body_base64: Option<String>,
}

impl BreakpointEdit {
    pub fn new_body(&self) -> Option<Bytes> {
        if let Some(b) = &self.body_base64 {
            return base64::engine::general_purpose::STANDARD.decode(b.trim()).ok().map(Bytes::from);
        }
        self.body.as_ref().map(|t| Bytes::from(t.clone()))
    }
}

/// Live notification for subscribers (the UI). `exchange` is sent for new exchanges and every
/// state change of an existing one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ProxyEvent {
    Exchange {
        exchange: ExchangeSummary,
    },
    Paused {
        paused: PausedExchange,
    },
    Resumed {
        id: ExchangeId,
    },
    /// Exchanges dropped from the ring buffer (or removed by the user).
    Removed {
        ids: Vec<ExchangeId>,
    },
    Cleared,
    Status {
        running: bool,
        port: Option<u16>,
        error: Option<String>,
    },
    /// The subscriber fell behind and missed events; reload the list.
    Resync,
}
