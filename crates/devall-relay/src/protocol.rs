//! Wire protocol v1 — the Rust side of `relay/PROTOCOL.md` (the contract; change it first).
//!
//! Every protocol message is a binary WebSocket message `[u8 type][u32 stream id, BE][payload]`.
//! Text messages carry only the hibernation-friendly heartbeat.

use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{RelayError, Result};

pub const PROTOCOL_VERSION: u32 = 1;
pub const SUBPROTOCOL_PREFIX: &str = "devall-relay.";
pub const HEADER_LEN: usize = 5;
pub const HEARTBEAT_PING: &str = "devall-relay:ping";
pub const HEARTBEAT_PONG: &str = "devall-relay:pong";

/// Defaults used until HELLO says otherwise.
pub const DEFAULT_WINDOW: u32 = 256 * 1024;
pub const DEFAULT_MAX_FRAME: usize = 64 * 1024;
pub const DEFAULT_MAX_WS_MESSAGE: usize = 1024 * 1024;
pub const DEFAULT_HEARTBEAT_SECS: u64 = 25;

/// `Sec-WebSocket-Protocol` value offered by this client.
pub fn subprotocol() -> String {
    format!("{SUBPROTOCOL_PREFIX}{PROTOCOL_VERSION}")
}

/// Frame type codes (PROTOCOL.md §2).
pub mod ft {
    pub const HELLO: u8 = 0x01;
    pub const ERROR: u8 = 0x02;
    pub const QUOTA: u8 = 0x03;
    pub const PING: u8 = 0x04;
    pub const PONG: u8 = 0x05;
    pub const GOAWAY: u8 = 0x06;
    pub const USAGE: u8 = 0x07;
    pub const TUNNEL_CONFIG: u8 = 0x08;

    pub const OPEN: u8 = 0x10;
    pub const DATA: u8 = 0x11;
    pub const END: u8 = 0x12;
    pub const RESPONSE: u8 = 0x13;
    pub const RESPONSE_DATA: u8 = 0x14;
    pub const RESPONSE_END: u8 = 0x15;
    pub const RESET: u8 = 0x16;
    pub const WINDOW: u8 = 0x17;

    pub const WS_OPEN: u8 = 0x20;
    pub const WS_ACCEPT: u8 = 0x21;
    pub const WS_REJECT: u8 = 0x22;
    pub const WS_TEXT: u8 = 0x23;
    pub const WS_BINARY: u8 = 0x24;
    pub const WS_CLOSE: u8 = 0x25;

    pub const INBOX_EVENT: u8 = 0x30;
    pub const INBOX_ACK: u8 = 0x31;
    pub const INBOX_STATUS: u8 = 0x32;
    pub const INBOX_CONFIG: u8 = 0x33;

    pub const TCP_OPEN: u8 = 0x40;
    pub const TCP_ACCEPT: u8 = 0x41;
    pub const TCP_DATA: u8 = 0x42;
    pub const TCP_CLOSE: u8 = 0x43;

    pub fn name(t: u8) -> &'static str {
        match t {
            HELLO => "HELLO",
            ERROR => "ERROR",
            QUOTA => "QUOTA",
            PING => "PING",
            PONG => "PONG",
            GOAWAY => "GOAWAY",
            USAGE => "USAGE",
            TUNNEL_CONFIG => "TUNNEL_CONFIG",
            OPEN => "OPEN",
            DATA => "DATA",
            END => "END",
            RESPONSE => "RESPONSE",
            RESPONSE_DATA => "RESPONSE_DATA",
            RESPONSE_END => "RESPONSE_END",
            RESET => "RESET",
            WINDOW => "WINDOW",
            WS_OPEN => "WS_OPEN",
            WS_ACCEPT => "WS_ACCEPT",
            WS_REJECT => "WS_REJECT",
            WS_TEXT => "WS_TEXT",
            WS_BINARY => "WS_BINARY",
            WS_CLOSE => "WS_CLOSE",
            INBOX_EVENT => "INBOX_EVENT",
            INBOX_ACK => "INBOX_ACK",
            INBOX_STATUS => "INBOX_STATUS",
            INBOX_CONFIG => "INBOX_CONFIG",
            TCP_OPEN => "TCP_OPEN",
            TCP_ACCEPT => "TCP_ACCEPT",
            TCP_DATA => "TCP_DATA",
            TCP_CLOSE => "TCP_CLOSE",
            _ => "UNKNOWN",
        }
    }
}

/// WebSocket close codes (PROTOCOL.md §4).
pub mod close {
    pub const PROTOCOL_ERROR: u16 = 4000;
    pub const SESSION_EXPIRED: u16 = 4008;
    pub const REPLACED: u16 = 4009;
    pub const QUOTA: u16 = 4029;
    pub const BLOCKED: u16 = 4051;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub stream: u32,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(kind: u8, stream: u32, payload: impl Into<Bytes>) -> Self {
        Self { kind, stream, payload: payload.into() }
    }

    pub fn empty(kind: u8, stream: u32) -> Self {
        Self { kind, stream, payload: Bytes::new() }
    }

    pub fn json<T: Serialize>(kind: u8, stream: u32, value: &T) -> Self {
        // Serializing our own payload structs can't fail.
        let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
        Self::new(kind, stream, bytes)
    }

    pub fn u32(kind: u8, stream: u32, n: u32) -> Self {
        Self::new(kind, stream, n.to_be_bytes().to_vec())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.push(self.kind);
        out.extend_from_slice(&self.stream.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < HEADER_LEN {
            return Err(RelayError::Protocol(format!("frame too short ({} bytes)", data.len())));
        }
        let stream = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        Ok(Self { kind: data[0], stream, payload: Bytes::copy_from_slice(&data[HEADER_LEN..]) })
    }

    /// Like [`Frame::decode`] without copying the payload.
    pub fn decode_bytes(data: Bytes) -> Result<Self> {
        if data.len() < HEADER_LEN {
            return Err(RelayError::Protocol(format!("frame too short ({} bytes)", data.len())));
        }
        let stream = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        Ok(Self { kind: data[0], stream, payload: data.slice(HEADER_LEN..) })
    }

    pub fn parse<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.payload).map_err(|e| RelayError::Protocol(format!("{}: invalid payload: {e}", ft::name(self.kind))))
    }

    pub fn parse_u32(&self) -> Result<u32> {
        let b: [u8; 4] = self.payload.as_ref().try_into().map_err(|_| RelayError::Protocol(format!("{}: expected 4 bytes", ft::name(self.kind))))?;
        Ok(u32::from_be_bytes(b))
    }
}

/// Ordered `[name, value]` pairs (JSON arrays on the wire).
pub type HeaderList = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub session_seconds: Option<u64>,
    pub bandwidth_bytes_per_month: Option<u64>,
    pub max_request_body_bytes: Option<u64>,
    pub request_timeout_ms: u64,
    pub max_tunnels: Option<u32>,
    pub interstitial: bool,
    pub inbox_retention_seconds: u64,
    pub inbox_events_per_day: Option<u64>,
    pub inbox_max_buffered: Option<u64>,
    pub inbox_max_body_bytes: u64,
    pub max_inboxes: Option<u32>,
    pub reserved_subdomains: Option<u32>,
    pub ip_requests_per_minute: Option<u32>,
    // ---- Batch D (absent on older relays: defaults = "not offered") ----
    #[serde(default)]
    pub custom_domains: Option<u32>,
    #[serde(default)]
    pub tcp_tunnels: bool,
    #[serde(default)]
    pub tcp_streams_per_tunnel: Option<u32>,
    #[serde(default)]
    pub hosted_mocks: Option<u32>,
    #[serde(default)]
    pub mock_max_stubs: Option<u32>,
    #[serde(default)]
    pub mock_max_bytes: Option<u64>,
    #[serde(default)]
    pub mock_requests_per_day: Option<u64>,
    #[serde(default)]
    pub mock_journal_size: u32,
    #[serde(default)]
    pub shares: Option<u32>,
    #[serde(default)]
    pub share_max_bytes: u64,
    #[serde(default)]
    pub share_ttl_seconds: u64,
    #[serde(default)]
    pub monitors: Option<u32>,
    #[serde(default)]
    pub monitor_min_interval_minutes: u32,
    #[serde(default)]
    pub monitor_max_requests: u32,
    #[serde(default)]
    pub monitor_history: u32,
    // ---- cloud load-generator regions (absent on older relays: not offered) ----
    /// Regions one cloud load run may use (0 = not on this plan).
    #[serde(default)]
    pub load_regions: u32,
    #[serde(default)]
    pub load_max_duration_seconds: u32,
    /// Requests per second across all regions (`None` = unlimited, self-host).
    #[serde(default)]
    pub load_max_rps: Option<f64>,
    #[serde(default)]
    pub load_runs_per_day: Option<u32>,
    /// Private / loopback targets allowed (self-host).
    #[serde(default)]
    pub load_private_targets: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub bandwidth_bytes: u64,
    pub requests: u64,
    pub period_start: String,
    pub period_end: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub version: u32,
    pub kind: String,
    pub session_id: String,
    pub public_url: String,
    #[serde(default)]
    pub public_urls: Vec<String>,
    pub subdomain: Option<String>,
    pub inbox_id: Option<String>,
    pub plan: String,
    pub limits: Limits,
    pub usage: Usage,
    pub session_expires_at: Option<String>,
    pub window: u32,
    pub max_frame: usize,
    pub max_ws_message: usize,
    pub heartbeat_seconds: u64,
    pub server_time: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub fatal: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPayload {
    pub kind: String,
    pub used: u64,
    pub limit: u64,
    #[serde(default)]
    pub reset_at: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoAway {
    pub reason: String,
    pub reconnect: bool,
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineInbox {
    pub inbox_id: String,
    pub path_prefixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfig {
    pub offline_inbox: Option<OfflineInbox>,
    /// TCP tunnels: who may connect besides the owner's account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp: Option<TcpConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TcpConfig {
    /// SHA-256 (lower-case hex) of the tunnel's access secret; `None` = owner's account only.
    pub secret_sha256: Option<String>,
}

/// TCP_OPEN: a consumer wants a connection through this TCP tunnel (PROTOCOL.md §2.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpOpen {
    pub client_ip: Option<String>,
    /// `owner` (the publisher's account) or `secret` (the tunnel's access secret).
    pub via: String,
}

/// TCP_CLOSE: ends a TCP stream in both directions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpClose {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Stream id used on a TCP consumer socket (the relay rewrites it).
pub const TCP_CONSUMER_STREAM: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestHead {
    pub method: String,
    pub url: String,
    pub path: String,
    pub headers: HeaderList,
    pub client_ip: Option<String>,
    pub body_length: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseHead {
    pub status: u16,
    pub headers: HeaderList,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsOpen {
    pub url: String,
    pub path: String,
    pub headers: HeaderList,
    #[serde(default)]
    pub protocols: Vec<String>,
    pub client_ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WsAccept {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsReject {
    pub status: u16,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsClose {
    pub code: u16,
    #[serde(default)]
    pub reason: String,
}

/// A webhook captured by the hosted inbox, exactly as received at the edge.
///
/// This is the shape handed to the app's webhook module (`devall-webhooks` `ingest`): method, the
/// exact public URL, ordered headers, the raw body (base64, byte-for-byte), receive time and the
/// sender's IP. `id` is unique per event; delivery is at-least-once, so consumers dedupe by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEvent {
    pub id: String,
    pub seq: u64,
    pub inbox_id: String,
    pub method: String,
    pub url: String,
    /// Path after the inbox id, with query (`/stripe?x=1`).
    pub path: String,
    pub headers: HeaderList,
    pub body_base64: String,
    pub body_size: u64,
    /// RFC 3339 receive time at the edge.
    pub received_at: String,
    pub source_ip: Option<String>,
}

impl InboxEvent {
    /// The raw request body.
    pub fn body(&self) -> Result<Vec<u8>> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(&self.body_base64).map_err(|e| RelayError::Protocol(format!("event {}: body is not base64: {e}", self.id)))
    }

    /// First header value with this (case-insensitive) name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// `received_at` as Unix milliseconds (the edge sends `YYYY-MM-DDTHH:MM:SS[.mmm]Z`).
    pub fn received_at_ms(&self) -> Option<i64> {
        parse_utc_ms(&self.received_at)
    }
}

/// Parses the UTC timestamps the relay emits (`2026-10-21T16:13:24.517Z`) to Unix milliseconds.
pub fn parse_utc_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || *b.last()? != b'Z' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let frac = &s[19..s.len() - 1];
    let ms = if frac.is_empty() {
        0
    } else {
        let digits = frac.strip_prefix('.')?;
        if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        format!("{:0<3}", &digits[..digits.len().min(3)]).parse::<i64>().ok()?
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + h * 3600 + mi * 60 + sec) * 1000) + ms)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxAck {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxResponse {
    pub status: u16,
    pub headers: HeaderList,
    pub body: String,
}

impl Default for InboxResponse {
    fn default() -> Self {
        Self { status: 200, headers: vec![("content-type".into(), "application/json".into())], body: r#"{"ok":true}"#.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxConfig {
    pub response: Option<InboxResponse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InboxStatus {
    pub buffered: u64,
    pub in_flight: u64,
    pub today_count: u64,
    pub daily_limit: Option<u64>,
    pub rejected_today: u64,
    pub retention_seconds: u64,
    pub oldest_received_at: Option<String>,
    pub response: Option<InboxResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout_matches_the_spec() {
        let f = Frame::new(ft::DATA, 0x0102_0304, vec![9, 8, 7]);
        assert_eq!(f.encode(), vec![0x11, 1, 2, 3, 4, 9, 8, 7]);
        assert_eq!(Frame::decode(&f.encode()).unwrap(), f);
        assert_eq!(Frame::decode_bytes(Bytes::from(f.encode())).unwrap(), f);
        assert_eq!(Frame::empty(ft::END, 7).encode().len(), HEADER_LEN);
        assert_eq!(Frame::decode(&Frame::new(ft::OPEN, u32::MAX, vec![]).encode()).unwrap().stream, u32::MAX);
    }

    #[test]
    fn json_and_u32_payloads_round_trip() {
        let head = ResponseHead { status: 201, headers: vec![("x-a".into(), "ü".into())] };
        let f = Frame::json(ft::RESPONSE, 3, &head);
        assert_eq!(std::str::from_utf8(&f.payload).unwrap(), r#"{"status":201,"headers":[["x-a","ü"]]}"#);
        assert_eq!(Frame::decode(&f.encode()).unwrap().parse::<ResponseHead>().unwrap(), head);
        assert_eq!(Frame::u32(ft::WINDOW, 5, 0xffff_fffe).parse_u32().unwrap(), 0xffff_fffe);
    }

    #[test]
    fn rejects_malformed_frames() {
        assert!(Frame::decode(&[1, 0, 0]).is_err());
        assert!(Frame::new(ft::HELLO, 0, b"nope".to_vec()).parse::<Hello>().is_err());
        assert!(Frame::new(ft::WINDOW, 1, vec![1, 2]).parse_u32().is_err());
    }

    #[test]
    fn parses_server_payloads_in_camel_case() {
        let ev: InboxEvent = serde_json::from_str(
            r#"{"id":"evt_1","seq":4,"inboxId":"abc","method":"POST","url":"https://hooks.x/abc/s?a=1","path":"/s?a=1",
                "headers":[["content-type","application/json"],["Stripe-Signature","t=1"]],"bodyBase64":"AAH+/w==","bodySize":4,
                "receivedAt":"2026-10-21T16:13:24.517Z","sourceIp":null}"#,
        )
        .unwrap();
        assert_eq!(ev.body().unwrap(), vec![0, 1, 254, 255]);
        assert_eq!(ev.header("stripe-signature"), Some("t=1"));
        let back = serde_json::to_value(&ev).unwrap();
        assert_eq!(back["bodyBase64"], "AAH+/w==");
        assert_eq!(back["receivedAt"], "2026-10-21T16:13:24.517Z");

        let hello: Hello = serde_json::from_str(
            r#"{"version":1,"kind":"tunnel","sessionId":"s_1","publicUrl":"https://a-b-1234.devall.dev","publicUrls":["https://a-b-1234.devall.dev"],
               "subdomain":"a-b-1234","inboxId":null,"plan":"free","limits":{"sessionSeconds":28800,"bandwidthBytesPerMonth":1073741824,
               "maxRequestBodyBytes":10485760,"requestTimeoutMs":60000,"maxTunnels":1,"interstitial":true,"inboxRetentionSeconds":86400,
               "inboxEventsPerDay":500,"inboxMaxBuffered":500,"inboxMaxBodyBytes":1048576,"maxInboxes":1,"reservedSubdomains":0,
               "ipRequestsPerMinute":300},"usage":{"bandwidthBytes":0,"requests":0,"periodStart":"2026-10-01T00:00:00.000Z",
               "periodEnd":"2026-11-01T00:00:00.000Z"},"sessionExpiresAt":null,"window":262144,"maxFrame":65536,"maxWsMessage":1048576,
               "heartbeatSeconds":25,"serverTime":"2026-10-21T16:13:24.517Z"}"#,
        )
        .unwrap();
        assert_eq!(hello.limits.max_tunnels, Some(1));
        assert_eq!(hello.window, DEFAULT_WINDOW);
    }

    #[test]
    fn parses_edge_timestamps() {
        assert_eq!(parse_utc_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_utc_ms("2026-10-21T16:13:24.517Z"), Some(1_792_599_204_517));
        assert_eq!(parse_utc_ms("2000-02-29T23:59:59Z"), Some(951_868_799_000));
        assert_eq!(parse_utc_ms("2026-10-21T16:13:24.5Z"), Some(1_792_599_204_500));
        assert_eq!(parse_utc_ms("2026-10-21 16:13:24Z"), None);
        assert_eq!(parse_utc_ms("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc_ms("nope"), None);
    }

    #[test]
    fn subprotocol_names_the_version() {
        assert_eq!(subprotocol(), "devall-relay.1");
        assert_eq!(ft::name(ft::INBOX_ACK), "INBOX_ACK");
    }
}
