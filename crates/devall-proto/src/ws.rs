//! WebSocket client (RFC 6455) on tokio-tungstenite, with our own TCP/TLS connector so custom
//! headers, subprotocols and "verify TLS" behave like the HTTP client.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use devall_core::Resolver;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, Interval};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self, Message};

use crate::error::{ProtoError, Result, chain};
use crate::event::{Direction, EventSink, ProtoEvent};
use crate::model::{MessageFormat, WsFile, resolve_pairs};
use crate::net::{self, NetStream, host_of, timeout_ms};
use crate::now_ms;

/// Largest message accepted from the server.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Concrete connection settings (every template resolved).
#[derive(Debug, Clone, PartialEq)]
pub struct WsOptions {
    /// `ws://` or `wss://` URL.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub subprotocols: Vec<String>,
    pub verify_tls: bool,
    pub connect_timeout: Duration,
    pub ping_interval: Option<Duration>,
}

impl WsOptions {
    pub fn resolve(file: &WsFile, r: &mut Resolver) -> Result<Self> {
        Ok(Self {
            url: normalize_ws_url(&r.resolve(file.url.trim()))?,
            headers: resolve_pairs(&file.headers, r),
            subprotocols: file.subprotocols.iter().map(|s| r.resolve(s.trim())).filter(|s| !s.is_empty()).collect(),
            verify_tls: file.settings.verify_tls,
            connect_timeout: timeout_ms(file.settings.connect_timeout_ms),
            ping_interval: (file.settings.ping_interval_ms > 0).then(|| Duration::from_millis(file.settings.ping_interval_ms)),
        })
    }
}

/// `http(s)://` → `ws(s)://`, bare `host/path` → `ws://host/path`; anything else is rejected.
pub fn normalize_ws_url(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ProtoError::InvalidUrl("URL is empty".into()));
    }
    let with_scheme = if raw.contains("://") { raw.to_string() } else { format!("ws://{raw}") };
    let mut url = url::Url::parse(&with_scheme).map_err(|e| ProtoError::InvalidUrl(format!("{raw} ({e})")))?;
    let scheme = match url.scheme() {
        "ws" | "http" => "ws",
        "wss" | "https" => "wss",
        other => return Err(ProtoError::InvalidUrl(format!("{raw}: unsupported scheme {other}:// (use ws:// or wss://)"))),
    };
    if url.scheme() != scheme {
        // `set_scheme` refuses special→special changes only between incompatible kinds; rebuild.
        let rest = &with_scheme[with_scheme.find("://").unwrap_or(0)..];
        url = url::Url::parse(&format!("{scheme}{rest}")).map_err(|e| ProtoError::InvalidUrl(format!("{raw} ({e})")))?;
    }
    if url.host().is_none() {
        return Err(ProtoError::InvalidUrl(format!("{raw} has no host")));
    }
    Ok(url.to_string())
}

/// A message to send.
#[derive(Debug, Clone, PartialEq)]
pub enum WsOutgoing {
    Text(String),
    Binary(Vec<u8>),
}

impl WsOutgoing {
    /// Builds a frame from a composer body (already template-resolved).
    pub fn from_body(body: &str, format: MessageFormat) -> Result<Self> {
        let bytes = format.encode(body)?;
        Ok(match format {
            MessageFormat::Text | MessageFormat::Json => WsOutgoing::Text(body.to_string()),
            MessageFormat::BinaryBase64 => WsOutgoing::Binary(bytes),
        })
    }
}

enum Cmd {
    Send(WsOutgoing),
    Close(Option<u16>, Option<String>),
}

/// A live connection. Dropping it closes the connection.
#[derive(Debug)]
pub struct WsSession {
    tx: mpsc::UnboundedSender<Cmd>,
    open: Arc<AtomicBool>,
    done: watch::Receiver<bool>,
}

impl std::fmt::Debug for Cmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Cmd::Send(_) => "Send",
            Cmd::Close(..) => "Close",
        })
    }
}

impl WsSession {
    /// Connects and completes the handshake; afterwards every frame is reported to `sink`.
    pub async fn connect(opts: WsOptions, sink: EventSink) -> Result<Self> {
        sink.emit(ProtoEvent::Connecting { at: now_ms(), target: opts.url.clone() });
        let (ws, hs) = handshake(&opts.url, &opts.headers, &opts.subprotocols, opts.verify_tls, opts.connect_timeout).await?;
        let detail = match &hs.protocol {
            Some(p) => format!("HTTP {} · subprotocol {p}", hs.status),
            None => format!("HTTP {}", hs.status),
        };
        sink.emit(ProtoEvent::Opened { at: now_ms(), detail: Some(detail), headers: hs.headers });

        let (tx, rx) = mpsc::unbounded_channel();
        let open = Arc::new(AtomicBool::new(true));
        let (done_tx, done) = watch::channel(false);
        let flag = open.clone();
        tokio::spawn(async move {
            run(ws, rx, sink.clone(), opts.ping_interval).await;
            flag.store(false, Ordering::SeqCst);
            let _ = done_tx.send(true);
        });
        Ok(Self { tx, open, done })
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    pub fn send(&self, msg: WsOutgoing) -> Result<()> {
        if !self.is_open() {
            return Err(ProtoError::Closed);
        }
        self.tx.send(Cmd::Send(msg)).map_err(|_| ProtoError::Closed)
    }

    /// Starts the closing handshake (default code 1000).
    pub fn close(&self, code: Option<u16>, reason: Option<String>) {
        let _ = self.tx.send(Cmd::Close(code, reason));
    }

    /// Resolves once the connection has fully closed.
    pub async fn wait_closed(&self) {
        let mut rx = self.done.clone();
        let _ = rx.wait_for(|d| *d).await;
    }
}

/// What the server answered to the upgrade request.
#[derive(Debug, Clone)]
pub(crate) struct Handshake {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub protocol: Option<String>,
}

pub(crate) type WsStream = tokio_tungstenite::WebSocketStream<NetStream>;

/// TCP (+TLS for `wss://`) connect and the HTTP upgrade, with custom headers and subprotocols.
pub(crate) async fn handshake(url: &str, headers: &[(String, String)], subprotocols: &[String], verify_tls: bool, timeout: Duration) -> Result<(WsStream, Handshake)> {
    let parsed = url::Url::parse(url).map_err(|e| ProtoError::InvalidUrl(format!("{url} ({e})")))?;
    let host = host_of(&parsed)?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let tls = if parsed.scheme() == "wss" { Some(net::tls_config(verify_tls, &[b"http/1.1"])?) } else { None };
    let stream = net::connect(&host, port, tls, timeout).await?;
    let mut req = url.into_client_request().map_err(|e| ProtoError::InvalidUrl(e.to_string()))?;
    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes()).map_err(|_| ProtoError::Invalid(format!("invalid header name {k:?}")))?;
        let value = HeaderValue::from_str(v).map_err(|_| ProtoError::Invalid(format!("invalid value for header {k}")))?;
        req.headers_mut().append(name, value);
    }
    if !subprotocols.is_empty() {
        let value = HeaderValue::from_str(&subprotocols.join(", ")).map_err(|_| ProtoError::Invalid("invalid subprotocol".into()))?;
        req.headers_mut().insert("Sec-WebSocket-Protocol", value);
    }
    let cfg = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE_BYTES)).max_frame_size(Some(MAX_MESSAGE_BYTES));
    let (ws, resp) = tokio::time::timeout(timeout, tokio_tungstenite::client_async_with_config(req, stream, Some(cfg)))
        .await
        .map_err(|_| ProtoError::Timeout("WebSocket handshake".into()))?
        .map_err(handshake_error)?;
    let hs = Handshake {
        status: resp.status().as_u16(),
        headers: resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect(),
        protocol: resp.headers().get("sec-websocket-protocol").and_then(|v| v.to_str().ok()).map(str::to_string),
    };
    Ok((ws, hs))
}

fn handshake_error(e: tungstenite::Error) -> ProtoError {
    match e {
        tungstenite::Error::Http(resp) => {
            let body = resp.body().as_ref().map(|b| String::from_utf8_lossy(b).chars().take(500).collect::<String>()).unwrap_or_default();
            let status = resp.status();
            ProtoError::Protocol(format!(
                "server refused the WebSocket upgrade: HTTP {} {}{}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                if body.is_empty() { String::new() } else { format!(" — {body}") }
            ))
        }
        tungstenite::Error::Io(e) => ProtoError::Connect(e.to_string()),
        tungstenite::Error::Tls(e) => ProtoError::Tls(e.to_string()),
        other => ProtoError::Protocol(chain(&other)),
    }
}

async fn tick(i: &mut Option<Interval>) {
    match i {
        Some(i) => {
            i.tick().await;
        }
        None => std::future::pending().await,
    }
}

async fn until(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

async fn run(ws: tokio_tungstenite::WebSocketStream<NetStream>, mut rx: mpsc::UnboundedReceiver<Cmd>, sink: EventSink, ping: Option<Duration>) {
    let (mut write, mut read) = ws.split();
    let mut ping = ping.map(|d| tokio::time::interval_at(Instant::now() + d, d));
    let mut close_deadline: Option<Instant> = None;
    let mut closed: Option<(Option<i64>, Option<String>)> = None;
    let mut client_close: Option<(Option<i64>, Option<String>)> = None;
    loop {
        tokio::select! {
            cmd = rx.recv(), if close_deadline.is_none() => match cmd {
                Some(Cmd::Send(m)) => {
                    let (event, frame) = match m {
                        WsOutgoing::Text(t) => (ProtoEvent::text(Direction::Out, None, t.clone(), None), Message::text(t)),
                        WsOutgoing::Binary(b) => (ProtoEvent::binary(Direction::Out, None, &b, None), Message::binary(b)),
                    };
                    match write.send(frame).await {
                        Ok(()) => sink.emit(event),
                        Err(e) => {
                            sink.error(format!("send failed: {}", chain(&e)));
                            break;
                        }
                    }
                }
                Some(Cmd::Close(code, reason)) => {
                    let code = code.unwrap_or(1000);
                    let frame = CloseFrame { code: CloseCode::from(code), reason: reason.clone().unwrap_or_default().into() };
                    client_close = Some((Some(code as i64), reason));
                    if write.send(Message::Close(Some(frame))).await.is_err() {
                        break;
                    }
                    close_deadline = Some(Instant::now() + Duration::from_secs(5));
                }
                None => {
                    // Handle dropped: close politely without waiting.
                    let _ = write.send(Message::Close(None)).await;
                    client_close = Some((Some(1000), Some("session closed".into())));
                    break;
                }
            },
            msg = read.next() => match msg {
                Some(Ok(Message::Text(t))) => sink.emit(ProtoEvent::text(Direction::In, None, t.as_str(), None)),
                Some(Ok(Message::Binary(b))) => sink.emit(ProtoEvent::binary(Direction::In, None, &b, None)),
                Some(Ok(Message::Ping(p))) => sink.info(format!("ping received ({} bytes), pong sent", p.len())),
                Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {}
                Some(Ok(Message::Close(frame))) => {
                    closed = Some(match frame {
                        Some(f) => (Some(u16::from(f.code) as i64), Some(f.reason.to_string()).filter(|r| !r.is_empty())),
                        None => (Some(1005), None),
                    });
                    // Keep reading: tungstenite answers the close and then ends the stream.
                }
                Some(Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed)) | None => break,
                Some(Err(e)) => {
                    sink.error(chain(&e));
                    break;
                }
            },
            _ = tick(&mut ping) => {
                if write.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
            _ = until(close_deadline) => break,
        }
    }
    let (code, reason) = closed.or(client_close).unwrap_or((Some(1006), Some("connection lost".into())));
    sink.emit(ProtoEvent::closed(code, reason));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    #[test]
    fn urls_are_normalized() {
        assert_eq!(normalize_ws_url("http://localhost:8080/ws").unwrap(), "ws://localhost:8080/ws");
        assert_eq!(normalize_ws_url("https://example.com/s?x=1").unwrap(), "wss://example.com/s?x=1");
        assert_eq!(normalize_ws_url("localhost:9000").unwrap(), "ws://localhost:9000/");
        assert!(normalize_ws_url("ftp://x").is_err());
        assert!(normalize_ws_url("").is_err());
    }

    #[test]
    fn resolves_templates() {
        let mut f = WsFile::new("t");
        f.url = "{{host}}/ws".into();
        f.headers.push(devall_core::KeyValue::new("Authorization", "Bearer {{secret.T}}"));
        f.subprotocols = vec!["{{proto}}".into(), " ".into()];
        struct S;
        impl devall_core::SecretLookup for S {
            fn lookup(&self, _: &str) -> Option<String> {
                Some("tok-123456".into())
            }
        }
        let mut r = Resolver::new(&S);
        r.push_layer([("host".to_string(), "https://api.test".to_string()), ("proto".to_string(), "graphql-ws".to_string())].into_iter().collect());
        let o = WsOptions::resolve(&f, &mut r).unwrap();
        assert_eq!(o.url, "wss://api.test/ws");
        assert_eq!(o.headers, vec![("Authorization".to_string(), "Bearer tok-123456".to_string())]);
        assert_eq!(o.subprotocols, vec!["graphql-ws"]);
        assert!(r.secret_values.contains("tok-123456"));
    }

    /// Echo server: checks the custom header, picks the first offered subprotocol, echoes frames,
    /// and closes with 4000 "bye" when it receives `close-me`.
    // The handshake callback's `Result<Response, ErrorResponse>` is fixed by tungstenite.
    #[allow(clippy::result_large_err)]
    async fn echo_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let cb = |req: &Request, mut resp: Response| {
                        assert_eq!(req.headers().get("x-test").and_then(|v| v.to_str().ok()), Some("yes"));
                        if let Some(p) = req.headers().get("sec-websocket-protocol").and_then(|v| v.to_str().ok()) {
                            let first = p.split(',').next().unwrap().trim().to_string();
                            resp.headers_mut().insert("sec-websocket-protocol", first.parse().unwrap());
                        }
                        Ok(resp)
                    };
                    let mut ws = tokio_tungstenite::accept_hdr_async(stream, cb).await.unwrap();
                    while let Some(Ok(m)) = ws.next().await {
                        match m {
                            Message::Text(t) if t.as_str() == "close-me" => {
                                let _ = ws.send(Message::Close(Some(CloseFrame { code: CloseCode::from(4000), reason: "bye".into() }))).await;
                            }
                            Message::Text(_) | Message::Binary(_) => ws.send(m).await.unwrap(),
                            _ => {}
                        }
                    }
                });
            }
        });
        format!("ws://{addr}/echo")
    }

    fn opts(url: String) -> WsOptions {
        WsOptions {
            url,
            headers: vec![("X-Test".into(), "yes".into())],
            subprotocols: vec!["chat.v1".into(), "chat.v0".into()],
            verify_tls: true,
            connect_timeout: Duration::from_secs(5),
            ping_interval: None,
        }
    }

    async fn next_message(rx: &mut mpsc::UnboundedReceiver<ProtoEvent>) -> ProtoEvent {
        loop {
            let e = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event timeout").expect("sink closed");
            if matches!(e, ProtoEvent::Message { direction: Direction::In, .. } | ProtoEvent::Closed { .. }) {
                return e;
            }
        }
    }

    #[tokio::test]
    async fn echo_text_binary_and_server_close() {
        let url = echo_server().await;
        let (sink, mut rx) = EventSink::channel();
        let s = WsSession::connect(opts(url), sink).await.unwrap();
        assert!(matches!(rx.recv().await, Some(ProtoEvent::Connecting { .. })));
        match rx.recv().await {
            Some(ProtoEvent::Opened { detail, headers, .. }) => {
                assert_eq!(detail.as_deref(), Some("HTTP 101 · subprotocol chat.v1"));
                assert!(headers.iter().any(|(k, _)| k == "sec-websocket-accept"));
            }
            other => panic!("{other:?}"),
        }

        s.send(WsOutgoing::from_body("{\"a\":1}", MessageFormat::Json).unwrap()).unwrap();
        match next_message(&mut rx).await {
            ProtoEvent::Message { body, binary, size, .. } => {
                assert_eq!(body, "{\"a\":1}");
                assert!(!binary);
                assert_eq!(size, 7);
            }
            other => panic!("{other:?}"),
        }

        s.send(WsOutgoing::from_body("AAEC", MessageFormat::BinaryBase64).unwrap()).unwrap();
        match next_message(&mut rx).await {
            ProtoEvent::Message { body, binary, size, .. } => {
                assert_eq!((body.as_str(), binary, size), ("AAEC", true, 3));
            }
            other => panic!("{other:?}"),
        }

        s.send(WsOutgoing::Text("close-me".into())).unwrap();
        match next_message(&mut rx).await {
            ProtoEvent::Closed { code, reason, .. } => {
                assert_eq!(code, Some(4000));
                assert_eq!(reason.as_deref(), Some("bye"));
            }
            other => panic!("{other:?}"),
        }
        tokio::time::timeout(Duration::from_secs(5), s.wait_closed()).await.unwrap();
        assert!(!s.is_open());
        assert!(matches!(s.send(WsOutgoing::Text("x".into())), Err(ProtoError::Closed)));
    }

    #[tokio::test]
    async fn client_close_reports_code() {
        let url = echo_server().await;
        let (sink, mut rx) = EventSink::channel();
        let s = WsSession::connect(opts(url), sink).await.unwrap();
        s.close(Some(1001), Some("going away".into()));
        tokio::time::timeout(Duration::from_secs(5), s.wait_closed()).await.unwrap();
        let mut last = None;
        while let Ok(e) = rx.try_recv() {
            last = Some(e);
        }
        match last {
            Some(ProtoEvent::Closed { code, .. }) => assert_eq!(code, Some(1001)),
            other => panic!("{other:?}"),
        }
    }

    /// `wss://` against a self-signed certificate: rejected with verification on (OS trust
    /// store), accepted with it off — exercises the shared TLS connector.
    #[tokio::test]
    async fn wss_self_signed_needs_verify_off() {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(ck.signing_key.serialize_der().into());
        let cfg = rustls::ServerConfig::builder_with_provider(net::crypto_provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![ck.cert.der().clone()], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(stream).await else { return };
                    let Ok(mut ws) = tokio_tungstenite::accept_async(tls).await else { return };
                    while let Some(Ok(m)) = ws.next().await {
                        if m.is_text() {
                            let _ = ws.send(m).await;
                        }
                    }
                });
            }
        });
        // The certificate is for "localhost", so only the (missing) trust makes verification fail.
        let mut o = WsOptions {
            url: format!("wss://localhost:{port}/"),
            headers: Vec::new(),
            subprotocols: Vec::new(),
            verify_tls: true,
            connect_timeout: Duration::from_secs(5),
            ping_interval: None,
        };
        let e = WsSession::connect(o.clone(), EventSink::noop()).await.unwrap_err();
        assert!(matches!(e, ProtoError::Tls(_)), "{e}");

        o.verify_tls = false;
        let (sink, mut rx) = EventSink::channel();
        let s = WsSession::connect(o, sink).await.unwrap();
        s.send(WsOutgoing::Text("over tls".into())).unwrap();
        match next_message(&mut rx).await {
            ProtoEvent::Message { body, .. } => assert_eq!(body, "over tls"),
            other => panic!("{other:?}"),
        }
        s.close(None, None);
    }

    #[tokio::test]
    async fn refused_upgrade_is_reported() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 4\r\n\r\nnope").await;
        });
        let e = WsSession::connect(opts(format!("ws://{addr}/")), EventSink::noop()).await.unwrap_err();
        assert!(e.to_string().contains("401"), "{e}");
    }
}
