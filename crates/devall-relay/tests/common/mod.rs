//! Test doubles: an in-process relay speaking the protocol, local HTTP/WebSocket targets and a
//! fake Google securetoken endpoint. Everything binds 127.0.0.1:0.
#![allow(dead_code)]
// The handshake callback signature is fixed by tungstenite (its error type is a full HTTP response).
#![allow(clippy::result_large_err)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use devall_relay::protocol::{Frame, HEARTBEAT_PING, HEARTBEAT_PONG, Hello, Limits, Usage, ft};
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame as BodyFrame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request as WsRequest, Response as WsResponse};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};

pub const T: Duration = Duration::from_secs(10);

// ---------- mock relay ----------

/// What the relay saw when a client connected.
#[derive(Debug, Clone)]
pub struct Accepted {
    pub path: String,
    pub query: String,
    pub authorization: Option<String>,
    pub protocol: Option<String>,
}

pub struct MockRelay {
    pub addr: SocketAddr,
    conns: tokio::sync::Mutex<mpsc::UnboundedReceiver<MockConn>>,
    /// Bearer tokens accepted; anything else gets 401 with a JSON error.
    pub accept_tokens: Arc<Mutex<Vec<String>>>,
    /// When set, the next upgrade is refused with (status, code).
    pub reject_next: Arc<Mutex<Option<(u16, String)>>>,
    pub attempts: Arc<AtomicUsize>,
}

pub struct MockConn {
    pub info: Accepted,
    ws: WebSocketStream<TcpStream>,
    pending: Vec<Frame>,
}

impl MockRelay {
    pub async fn start(tokens: &[&str]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let accept_tokens = Arc::new(Mutex::new(tokens.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
        let reject_next = Arc::new(Mutex::new(None::<(u16, String)>));
        let attempts = Arc::new(AtomicUsize::new(0));
        {
            let accept_tokens = accept_tokens.clone();
            let reject_next = reject_next.clone();
            let attempts = attempts.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else { continue };
                    let tx = tx.clone();
                    let accept_tokens = accept_tokens.clone();
                    let reject_next = reject_next.clone();
                    attempts.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        let seen = Arc::new(Mutex::new(None::<Accepted>));
                        let seen2 = seen.clone();
                        let cb = move |req: &WsRequest, mut res: WsResponse| -> Result<WsResponse, ErrorResponse> {
                            let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string);
                            let protocol = req.headers().get("sec-websocket-protocol").and_then(|v| v.to_str().ok()).map(str::to_string);
                            *seen2.lock().unwrap() = Some(Accepted {
                                path: req.uri().path().to_string(),
                                query: req.uri().query().unwrap_or("").to_string(),
                                authorization: auth.clone(),
                                protocol: protocol.clone(),
                            });
                            let reject = |status: u16, code: &str| {
                                let body = format!(r#"{{"error":{{"code":"{code}","message":"refused: {code}"}}}}"#);
                                let mut r = ErrorResponse::new(Some(body));
                                *r.status_mut() = StatusCode::from_u16(status).unwrap();
                                r
                            };
                            if let Some((status, code)) = reject_next.lock().unwrap().take() {
                                return Err(reject(status, &code));
                            }
                            let ok = auth.as_deref().and_then(|a| a.strip_prefix("Bearer ")).is_some_and(|t| accept_tokens.lock().unwrap().iter().any(|x| x == t));
                            if !ok {
                                return Err(reject(401, "unauthorized"));
                            }
                            if let Some(p) = protocol {
                                res.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_str(p.split(',').next().unwrap().trim()).unwrap());
                            }
                            Ok(res)
                        };
                        if let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, cb).await {
                            let info = seen.lock().unwrap().clone().unwrap();
                            let _ = tx.send(MockConn { info, ws, pending: Vec::new() });
                        }
                    });
                }
            });
        }
        Self { addr, conns: tokio::sync::Mutex::new(rx), accept_tokens, reject_next, attempts }
    }

    pub fn endpoint(&self) -> String {
        format!("ws://{}", self.addr)
    }

    pub async fn next_conn(&self) -> MockConn {
        tokio::time::timeout(T, self.conns.lock().await.recv()).await.expect("client connected").expect("relay running")
    }
}

pub fn limits() -> Limits {
    Limits {
        session_seconds: Some(28_800),
        bandwidth_bytes_per_month: Some(1 << 30),
        max_request_body_bytes: Some(10 << 20),
        request_timeout_ms: 60_000,
        max_tunnels: Some(1),
        interstitial: true,
        inbox_retention_seconds: 86_400,
        inbox_events_per_day: Some(500),
        inbox_max_buffered: Some(500),
        inbox_max_body_bytes: 1 << 20,
        max_inboxes: Some(1),
        reserved_subdomains: Some(0),
        ip_requests_per_minute: Some(300),
        ..Default::default()
    }
}

pub fn hello(kind: &str, sub: &str) -> Hello {
    let (public_url, subdomain, inbox_id) = if kind == "inbox" {
        (format!("https://hooks.devall.test/{sub}/"), None, Some(sub.to_string()))
    } else {
        (format!("https://{sub}.devall.test"), Some(sub.to_string()), None)
    };
    Hello {
        version: 1,
        kind: kind.into(),
        session_id: "s_test".into(),
        public_urls: vec![public_url.clone()],
        public_url,
        subdomain,
        inbox_id,
        plan: "free".into(),
        limits: limits(),
        usage: Usage { bandwidth_bytes: 0, requests: 0, period_start: "2026-10-01T00:00:00.000Z".into(), period_end: "2026-11-01T00:00:00.000Z".into() },
        session_expires_at: None,
        window: 256 * 1024,
        max_frame: 64 * 1024,
        max_ws_message: 1 << 20,
        heartbeat_seconds: 25,
        server_time: "2026-10-21T00:00:00.000Z".into(),
    }
}

impl MockConn {
    pub async fn send(&mut self, f: Frame) {
        self.ws.send(Message::Binary(f.encode().into())).await.unwrap();
    }

    pub async fn send_hello(&mut self, kind: &str, sub: &str) {
        let h = hello(kind, sub);
        self.send(Frame::json(ft::HELLO, 0, &h)).await;
    }

    pub async fn close(&mut self, code: u16, reason: &str) {
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        let _ = self.ws.send(Message::Close(Some(CloseFrame { code: code.into(), reason: reason.to_string().into() }))).await;
    }

    async fn read_one(&mut self, timeout: Duration) -> Option<Frame> {
        loop {
            let m = tokio::time::timeout(timeout, self.ws.next()).await.ok()??;
            match m.ok()? {
                Message::Binary(b) => return Some(Frame::decode(&b).unwrap()),
                Message::Text(t) if t.as_str() == HEARTBEAT_PING => {
                    let _ = self.ws.send(Message::Text(HEARTBEAT_PONG.into())).await;
                }
                Message::Close(_) => return None,
                _ => {}
            }
        }
    }

    /// Next frame of `kind` (optionally on `stream`); other frames are kept for later.
    pub async fn expect(&mut self, kind: u8, stream: Option<u32>) -> Frame {
        let matches = |f: &Frame| f.kind == kind && stream.is_none_or(|s| f.stream == s);
        if let Some(i) = self.pending.iter().position(matches) {
            return self.pending.remove(i);
        }
        loop {
            let f = self.read_one(T).await.unwrap_or_else(|| panic!("connection ended waiting for {}", ft::name(kind)));
            if matches(&f) {
                return f;
            }
            self.pending.push(f);
        }
    }

    /// Whether a frame of `kind` arrives within `wait`.
    pub async fn sees(&mut self, kind: u8, stream: Option<u32>, wait: Duration) -> bool {
        let matches = |f: &Frame| f.kind == kind && stream.is_none_or(|s| f.stream == s);
        if self.pending.iter().any(matches) {
            return true;
        }
        let deadline = tokio::time::Instant::now() + wait;
        while let Some(left) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            match self.read_one(left).await {
                Some(f) => {
                    let m = matches(&f);
                    self.pending.push(f);
                    if m {
                        return true;
                    }
                }
                None => return false,
            }
        }
        false
    }

    /// Collects RESPONSE_DATA until RESPONSE_END on `stream`, granting credit as it goes.
    pub async fn read_body(&mut self, stream: u32, grant: bool) -> Vec<u8> {
        let mut body = Vec::new();
        loop {
            let f = self.expect_any(&[ft::RESPONSE_DATA, ft::RESPONSE_END], stream).await;
            if f.kind == ft::RESPONSE_END {
                return body;
            }
            body.extend_from_slice(&f.payload);
            if grant {
                self.send(Frame::u32(ft::WINDOW, stream, f.payload.len() as u32)).await;
            }
        }
    }

    pub async fn expect_any(&mut self, kinds: &[u8], stream: u32) -> Frame {
        let matches = |f: &Frame| kinds.contains(&f.kind) && f.stream == stream;
        if let Some(i) = self.pending.iter().position(matches) {
            return self.pending.remove(i);
        }
        loop {
            let f = self.read_one(T).await.expect("connection ended");
            if matches(&f) {
                return f;
            }
            self.pending.push(f);
        }
    }

    pub fn drop_conn(self) {}
}

// ---------- local HTTP target ----------

pub struct LocalHttp {
    pub addr: SocketAddr,
}

async fn local_handler(req: Request<Incoming>) -> Result<Response<http_body_util::combinators::BoxBody<Bytes, Infallible>>, Infallible> {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let method = req.method().to_string();
    let h = |n: &str| req.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let (host, xff, xfh, xfp, ctype, accept_enc, sig) = (h("host"), h("x-forwarded-for"), h("x-forwarded-host"), h("x-forwarded-proto"), h("content-type"), h("accept-encoding"), h("x-twilio-signature"));
    match path.as_str() {
        "/stream" => {
            // 512 KiB in 64 chunks of 8 KiB.
            let (tx, rx) = mpsc::channel::<Result<BodyFrame<Bytes>, Infallible>>(4);
            tokio::spawn(async move {
                for i in 0..64u8 {
                    if tx.send(Ok(BodyFrame::data(Bytes::from(vec![i; 8192])))).await.is_err() {
                        return;
                    }
                }
            });
            let body = StreamBody::new(tokio_stream_from(rx));
            Ok(Response::builder().status(200).header("content-type", "application/octet-stream").body(BodyExt::boxed(body)).unwrap())
        }
        "/gzip" => Ok(Response::builder().status(200).header("content-encoding", "gzip").header("content-type", "text/plain").body(Full::new(Bytes::from_static(&[0x1f, 0x8b, 8, 0, 1, 2, 3])).boxed()).unwrap()),
        "/status/404" => Ok(Response::builder().status(404).header("x-a", "1").header("set-cookie", "a=1").header("set-cookie", "b=2").body(Full::new(Bytes::from("nope")).boxed()).unwrap()),
        _ => {
            let body = req.into_body().collect().await.map(|b| b.to_bytes()).unwrap_or_default();
            let len = body.len();
            Ok(Response::builder()
                .status(200)
                .header("x-echo-method", method)
                .header("x-echo-path", format!("{path}?{query}"))
                .header("x-echo-host", host)
                .header("x-echo-xff", xff)
                .header("x-echo-xfh", xfh)
                .header("x-echo-xfp", xfp)
                .header("x-echo-ctype", ctype)
                .header("x-echo-accept-encoding", accept_enc)
                .header("x-echo-sig", sig)
                .header("x-echo-len", len.to_string())
                .body(Full::new(body).boxed())
                .unwrap())
        }
    }
}

fn tokio_stream_from<T: Send + 'static>(mut rx: mpsc::Receiver<T>) -> impl futures_util::Stream<Item = T> {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

impl LocalHttp {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else { continue };
                tokio::spawn(async move {
                    let _ = http1::Builder::new().serve_connection(TokioIo::new(s), service_fn(local_handler)).await;
                });
            }
        });
        Self { addr }
    }

    pub fn url(&self) -> url::Url {
        url::Url::parse(&format!("http://{}", self.addr)).unwrap()
    }
}

// ---------- local WebSocket echo ----------

pub async fn start_ws_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else { continue };
            tokio::spawn(async move {
                let cb = |req: &WsRequest, mut res: WsResponse| -> Result<WsResponse, ErrorResponse> {
                    if req.uri().path() == "/forbidden" {
                        let mut r = ErrorResponse::new(Some("no".into()));
                        *r.status_mut() = StatusCode::FORBIDDEN;
                        return Err(r);
                    }
                    if let Some(p) = req.headers().get("sec-websocket-protocol").and_then(|v| v.to_str().ok()) {
                        let first = p.split(',').next().unwrap().trim().to_string();
                        res.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_str(&first).unwrap());
                    }
                    Ok(res)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(s, cb).await else { return };
                while let Some(Ok(m)) = ws.next().await {
                    match m {
                        Message::Text(t) if t.as_str() == "close-me" => {
                            use tokio_tungstenite::tungstenite::protocol::CloseFrame;
                            let _ = ws.send(Message::Close(Some(CloseFrame { code: 4002u16.into(), reason: "asked to".into() }))).await;
                            break;
                        }
                        Message::Text(_) | Message::Binary(_) => {
                            if ws.send(m).await.is_err() {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            });
        }
    });
    addr
}

// ---------- fake securetoken ----------

pub struct FakeSecureToken {
    pub addr: SocketAddr,
    pub calls: Arc<AtomicUsize>,
}

impl FakeSecureToken {
    /// Exchanges `R<n>` for `ID-<n+1>` and rotates to `R<n+1>`; requires `key=test-key`.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else { continue };
                let c = c.clone();
                tokio::spawn(async move {
                    let svc = service_fn(move |req: Request<Incoming>| {
                        let c = c.clone();
                        async move {
                            c.fetch_add(1, Ordering::SeqCst);
                            let key_ok = req.uri().query().unwrap_or("").contains("key=test-key");
                            let ctype = req.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                            let body = req.into_body().collect().await.unwrap().to_bytes();
                            let form: Vec<(String, String)> = url::form_urlencoded::parse(&body).into_owned().collect();
                            let get = |k: &str| form.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap_or_default();
                            let rt = get("refresh_token");
                            let n = rt.strip_prefix('R').and_then(|n| n.parse::<u32>().ok());
                            let res = match (key_ok, ctype.starts_with("application/x-www-form-urlencoded"), get("grant_type").as_str(), n) {
                                (true, true, "refresh_token", Some(n)) => Response::builder()
                                    .status(200)
                                    .header("content-type", "application/json")
                                    .body(Full::new(Bytes::from(format!(
                                        r#"{{"access_token":"ID-{m}","expires_in":"3600","token_type":"Bearer","refresh_token":"R{m}","id_token":"ID-{m}","user_id":"uid-1","project_id":"p"}}"#,
                                        m = n + 1
                                    ))))
                                    .unwrap(),
                                _ => Response::builder().status(400).header("content-type", "application/json").body(Full::new(Bytes::from(r#"{"error":{"code":400,"message":"INVALID_REFRESH_TOKEN"}}"#))).unwrap(),
                            };
                            Ok::<_, Infallible>(res)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(TokioIo::new(s), svc).await;
                });
            }
        });
        Self { addr, calls }
    }

    pub fn url(&self) -> String {
        format!("http://{}/v1/token", self.addr)
    }
}

// ---------- fake account API ----------

pub async fn start_account_api(expect_token: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else { continue };
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| async move {
                    let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    let res = if req.uri().path() != "/v1/account" {
                        Response::builder().status(404).body(Full::new(Bytes::new())).unwrap()
                    } else if auth != format!("Bearer {expect_token}") {
                        Response::builder().status(401).body(Full::new(Bytes::from(r#"{"error":{"code":"unauthorized","message":"bad token"}}"#))).unwrap()
                    } else {
                        let limits = serde_json::to_string(&limits()).unwrap();
                        let body = format!(
                            r#"{{"uid":"static-root","email":null,"plan":"supporter","selfHosted":false,"domain":"devall.test","limits":{limits},
                            "usage":{{"bandwidthBytes":1234,"requests":5,"periodStart":"2026-10-01T00:00:00.000Z","periodEnd":"2026-11-01T00:00:00.000Z"}},
                            "tunnels":[{{"subdomain":"my-api","url":"https://my-api.devall.test","since":"2026-10-21T00:00:00.000Z"}}],
                            "inboxes":[{{"projectKey":"_default","inboxId":"abcdefghijklmnopqrst","url":"https://hooks.devall.test/abcdefghijklmnopqrst/"}}],
                            "reservedSubdomains":["my-api"],"upgradeUrl":"https://auth.devall.test/upgrade.html"}}"#
                        );
                        Response::builder().status(200).header("content-type", "application/json").body(Full::new(Bytes::from(body))).unwrap()
                    };
                    Ok::<_, Infallible>(res)
                });
                let _ = http1::Builder::new().serve_connection(TokioIo::new(s), svc).await;
            });
        }
    });
    addr
}
