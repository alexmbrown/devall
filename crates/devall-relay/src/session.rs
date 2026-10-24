//! Pieces shared by tunnel and inbox sessions: connecting with auth, the socket writer task,
//! HELLO, close handling and reconnect backoff.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::auth::{RelayAuth, TokenProvider};
use crate::error::{RelayError, Result, chain};
use crate::protocol::{DEFAULT_MAX_WS_MESSAGE, Frame, GoAway, Hello, close, ft, subprotocol};
use crate::transport::{self, WsStream};

pub const USER_AGENT: &str = concat!("devall-relay/", env!("CARGO_PKG_VERSION"));
pub(crate) const HELLO_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) type Sink = SplitSink<WsStream, Message>;
pub(crate) type Source = SplitStream<WsStream>;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Connection state shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ConnState {
    #[default]
    Connecting,
    Connected,
    Reconnecting,
    /// Stopped by the user or by the relay (replaced, blocked).
    Stopped,
    /// Gave up: an error that reconnecting won't fix (signed out, plan, reserved name…).
    Failed,
}

/// Exponential backoff with jitter for reconnects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self { initial: Duration::from_millis(500), max: Duration::from_secs(30) }
    }
}

impl ReconnectPolicy {
    /// Delay before attempt `n` (0-based): initial × 2ⁿ, capped, ±20 % jitter.
    pub fn delay(&self, attempt: u32) -> Duration {
        let base = self.initial.saturating_mul(1u32 << attempt.min(16)).min(self.max);
        let jitter = 0.8 + fastrand::f64() * 0.4;
        base.mul_f64(jitter).min(self.max)
    }
}

/// Opens authenticated relay sockets.
pub(crate) struct Connector {
    pub endpoint: Url,
    pub tokens: Arc<TokenProvider>,
    pub verify_tls: bool,
    pub connect_timeout: Duration,
}

impl Connector {
    pub fn url(&self, path: &str, query: &[(&str, String)]) -> Url {
        let mut u = self.endpoint.clone();
        let base = u.path().trim_end_matches('/').to_string();
        u.set_path(&format!("{base}{path}"));
        u.set_query(None);
        if !query.is_empty() {
            let mut q = u.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
        }
        u
    }

    /// Connects to `path`; after a 401 the cached ID token is dropped and the attempt repeated once.
    pub async fn open(&self, path: &str, query: &[(&str, String)]) -> Result<WsStream> {
        let url = self.url(path, query);
        let mut retried = false;
        loop {
            let token = self.tokens.bearer().await?;
            let headers = vec![("authorization".to_string(), format!("Bearer {token}")), ("user-agent".to_string(), USER_AGENT.to_string())];
            match transport::websocket(&url, &headers, &[subprotocol()], self.verify_tls, self.connect_timeout, DEFAULT_MAX_WS_MESSAGE * 4).await {
                Ok(h) => {
                    if h.protocol.as_deref() != Some(subprotocol().as_str()) {
                        return Err(RelayError::Protocol(format!("relay picked protocol {:?}, expected {}", h.protocol, subprotocol())));
                    }
                    return Ok(h.ws);
                }
                Err(RelayError::Rejected { status: 401, .. }) if !retried && matches!(self.tokens.auth(), RelayAuth::FirebaseRefreshToken { .. }) => {
                    retried = true;
                    self.tokens.invalidate().await;
                }
                Err(RelayError::Rejected { status: 401, message, .. }) => return Err(RelayError::Auth(message)),
                Err(e) => return Err(e),
            }
        }
    }
}

impl Connector {
    /// Connects to `path` with an explicit bearer (e.g. a TCP tunnel's `dtcp_…` access secret).
    pub async fn open_with_bearer(&self, path: &str, query: &[(&str, String)], bearer: &str) -> Result<WsStream> {
        let url = self.url(path, query);
        let headers = vec![("authorization".to_string(), format!("Bearer {bearer}")), ("user-agent".to_string(), USER_AGENT.to_string())];
        let h = transport::websocket(&url, &headers, &[subprotocol()], self.verify_tls, self.connect_timeout, DEFAULT_MAX_WS_MESSAGE * 4).await?;
        if h.protocol.as_deref() != Some(subprotocol().as_str()) {
            return Err(RelayError::Protocol(format!("relay picked protocol {:?}, expected {}", h.protocol, subprotocol())));
        }
        Ok(h.ws)
    }
}

/// Owns the socket's write half; everything else sends through the channel.
pub(crate) fn spawn_writer(mut sink: Sink, mut rx: mpsc::Receiver<Message>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let is_close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || is_close {
                break;
            }
        }
        let _ = sink.close().await;
    })
}

pub(crate) fn binary(f: Frame) -> Message {
    Message::Binary(f.encode().into())
}

/// Reads frames until HELLO (anything else first is a protocol error).
pub(crate) async fn read_hello(src: &mut Source) -> Result<Hello> {
    let fut = async {
        loop {
            match src.next().await {
                Some(Ok(Message::Binary(b))) => {
                    let f = Frame::decode_bytes(b)?;
                    if f.kind == ft::HELLO {
                        return f.parse::<Hello>();
                    }
                    if f.kind == ft::ERROR {
                        let e: crate::protocol::ErrorPayload = f.parse()?;
                        return Err(RelayError::Protocol(format!("{}: {}", e.code, e.message)));
                    }
                    return Err(RelayError::Protocol(format!("expected HELLO, got {}", ft::name(f.kind))));
                }
                Some(Ok(Message::Close(cf))) => {
                    return Err(RelayError::WebSocket(format!("closed before HELLO ({})", cf.map(|c| c.reason.to_string()).unwrap_or_default())));
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(RelayError::WebSocket(chain(&e))),
                None => return Err(RelayError::WebSocket("closed before HELLO".into())),
            }
        }
    };
    tokio::time::timeout(HELLO_TIMEOUT, fut).await.map_err(|_| RelayError::Timeout("waiting for HELLO".into()))?
}

/// ` (last error: …)` for timeouts while (re)connecting, so a refusal the relay keeps answering
/// (e.g. `429 too_many_tunnels`) isn't hidden behind "timed out".
pub(crate) fn last_error_hint(e: Option<&str>) -> String {
    e.filter(|e| !e.is_empty()).map(|e| format!(" (last error: {e})")).unwrap_or_default()
}

/// How a session ended.
#[derive(Debug)]
pub(crate) enum Exit {
    Stopped,
    Closed { reconnect: bool, delay: Option<Duration>, reason: String },
    Failed(RelayError),
}

/// Maps a close (code + any GOAWAY seen before it) to the reconnect decision.
pub(crate) fn exit_for_close(code: Option<u16>, reason: &str, goaway: Option<&GoAway>) -> Exit {
    if let Some(g) = goaway {
        let msg = g.message.clone().unwrap_or_else(|| g.reason.replace('_', " "));
        return Exit::Closed { reconnect: g.reconnect, delay: g.retry_after_ms.map(Duration::from_millis), reason: msg };
    }
    match code {
        Some(close::REPLACED) => Exit::Closed { reconnect: false, delay: None, reason: "replaced by a newer connection".into() },
        Some(close::BLOCKED) => Exit::Closed { reconnect: false, delay: None, reason: "disabled after an abuse report".into() },
        Some(close::SESSION_EXPIRED) => Exit::Closed { reconnect: true, delay: Some(Duration::ZERO), reason: "session ended".into() },
        Some(c) => Exit::Closed { reconnect: true, delay: None, reason: if reason.is_empty() { format!("relay closed the connection ({c})") } else { format!("{reason} ({c})") } },
        None => Exit::Closed { reconnect: true, delay: None, reason: "connection lost".into() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let p = ReconnectPolicy { initial: Duration::from_millis(100), max: Duration::from_secs(2) };
        assert!(p.delay(0) >= Duration::from_millis(80) && p.delay(0) <= Duration::from_millis(120));
        assert!(p.delay(3) >= Duration::from_millis(640));
        for _ in 0..50 {
            assert!(p.delay(30) <= Duration::from_secs(2));
            assert!(p.delay(30) >= Duration::from_millis(1600));
        }
    }

    #[test]
    fn close_codes_drive_reconnects() {
        assert!(matches!(exit_for_close(Some(4009), "", None), Exit::Closed { reconnect: false, .. }));
        assert!(matches!(exit_for_close(Some(4051), "", None), Exit::Closed { reconnect: false, .. }));
        assert!(matches!(exit_for_close(Some(4008), "", None), Exit::Closed { reconnect: true, delay: Some(d), .. } if d.is_zero()));
        assert!(matches!(exit_for_close(Some(1001), "deploy", None), Exit::Closed { reconnect: true, delay: None, .. }));
        let g = GoAway { reason: "session_expired".into(), reconnect: true, retry_after_ms: Some(0), message: None };
        assert!(matches!(exit_for_close(Some(4008), "", Some(&g)), Exit::Closed { reconnect: true, .. }));
    }

    #[test]
    fn builds_endpoint_urls() {
        let c = Connector {
            endpoint: Url::parse("wss://relay.devall.dev").unwrap(),
            tokens: Arc::new(TokenProvider::new(RelayAuth::StaticToken(Some("t".into())), Arc::new(devall_core::MemorySecretStore::default()), reqwest::Client::new())),
            verify_tls: true,
            connect_timeout: Duration::from_secs(1),
        };
        assert_eq!(c.url("/connect", &[("name", "my app".into())]).as_str(), "wss://relay.devall.dev/connect?name=my+app");
        assert_eq!(c.url("/inbox", &[]).as_str(), "wss://relay.devall.dev/inbox");
    }
}
