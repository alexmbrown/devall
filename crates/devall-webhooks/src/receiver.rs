//! The local receiver: one HTTP/1.1 server per project on `127.0.0.1:<port>` (default 4590, or
//! `0.0.0.0` for devices on the LAN) that captures every request and hands it to the
//! [`WebhookHub`]. Bodies are read up to [`MAX_BODY_BYTES`]; larger ones are kept truncated and
//! answered with 413.
//!
//! Header names arrive lowercased (hyper normalizes them; HTTP/2 and Go-based tunnels do too) in
//! received order with duplicates preserved. Signatures never cover header names.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::WebhookError;
use crate::event::{EventSource, IncomingEvent, MAX_BODY_BYTES, now_ms};
use crate::hub::{Reply, WebhookHub};

pub const DEFAULT_PORT: u16 = 4590;
/// Paths under this prefix are answered by the receiver itself and never captured.
pub const INTERNAL_PREFIX: &str = "/__devall";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiverConfig {
    /// `0` = any free port.
    pub port: u16,
    /// Listen on every interface (LAN devices, phones) instead of loopback only.
    pub bind_all: bool,
}

impl Default for ReceiverConfig {
    fn default() -> Self {
        Self { port: DEFAULT_PORT, bind_all: false }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverInfo {
    pub port: u16,
    pub bind_all: bool,
    /// `http://127.0.0.1:<port>`
    pub local_url: String,
    /// `http://<LAN IP>:<port>` when bound to all interfaces.
    pub lan_url: Option<String>,
    pub started_at: i64,
}

pub struct Receiver {
    info: ReceiverInfo,
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    hub: Arc<WebhookHub>,
}

impl Receiver {
    pub async fn start(hub: Arc<WebhookHub>, cfg: ReceiverConfig) -> Result<Self, WebhookError> {
        let ip = if cfg.bind_all { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
        let addr = SocketAddr::new(ip, cfg.port);
        let listener = TcpListener::bind(addr).await.map_err(|e| WebhookError::Bind {
            addr: addr.to_string(),
            message: if e.kind() == std::io::ErrorKind::AddrInUse {
                format!("port {} is already in use (another receiver, mock server or app?) — choose another port", cfg.port)
            } else {
                e.to_string()
            },
        })?;
        let port = listener.local_addr().map_err(|e| WebhookError::Io(e.to_string()))?.port();
        let local_url = format!("http://127.0.0.1:{port}");
        let lan_url = cfg.bind_all.then(|| lan_ip().map(|ip| format!("http://{ip}:{port}"))).flatten();
        hub.set_local_base(Some(local_url.clone()));
        let (shutdown, _) = watch::channel(false);
        let task = tokio::spawn(run(listener, hub.clone(), port, shutdown.subscribe()));
        Ok(Self { info: ReceiverInfo { port, bind_all: cfg.bind_all, local_url, lan_url, started_at: now_ms() }, shutdown, task, hub })
    }

    pub fn info(&self) -> &ReceiverInfo {
        &self.info
    }

    pub fn port(&self) -> u16 {
        self.info.port
    }

    pub fn is_running(&self) -> bool {
        !self.task.is_finished()
    }

    /// Stops accepting, lets in-flight requests finish (2 s max).
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
        self.hub.set_local_base(None);
    }
}

/// The address other machines on the LAN reach this one at (no packet is sent: connecting a
/// UDP socket only selects a route).
pub fn lan_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

async fn run(listener: TcpListener, hub: Arc<WebhookHub>, port: u16, mut shutdown: watch::Receiver<bool>) {
    let mut conns = JoinSet::new();
    let for_conns = shutdown.clone();
    loop {
        tokio::select! {
            res = listener.accept() => match res {
                Ok((stream, peer)) => {
                    conns.spawn(serve_connection(stream, peer, hub.clone(), port, for_conns.clone()));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            },
            _ = shutdown.changed() => break,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
    drop(listener);
    let drain = async { while conns.join_next().await.is_some() {} };
    if tokio::time::timeout(Duration::from_secs(2), drain).await.is_err() {
        conns.abort_all();
        while conns.join_next().await.is_some() {}
    }
}

async fn serve_connection(stream: TcpStream, peer: SocketAddr, hub: Arc<WebhookHub>, port: u16, mut shutdown: watch::Receiver<bool>) {
    let _ = stream.set_nodelay(true);
    let io = TokioIo::new(stream);
    let svc = service_fn(move |req| {
        let hub = hub.clone();
        async move { Ok::<_, Infallible>(handle(hub, req, peer, port).await) }
    });
    let mut builder = http1::Builder::new();
    builder.timer(TokioTimer::new()).keep_alive(true).header_read_timeout(Duration::from_secs(30)).max_buf_size(1024 * 1024);
    let conn = builder.serve_connection(io, svc);
    tokio::pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => {}
        _ = shutdown.changed() => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.as_mut().await;
        }
    }
}

/// Reads at most `limit` bytes; the flag says the body was longer (the rest is not read).
async fn read_capped(mut body: Incoming, limit: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else { break };
        if let Ok(data) = frame.into_data() {
            if out.len() + data.len() > limit {
                out.extend_from_slice(&data[..limit - out.len()]);
                return (out, true);
            }
            out.extend_from_slice(&data);
        }
    }
    (out, false)
}

fn respond(r: Reply) -> Response<Full<Bytes>> {
    let mut b = Response::builder().status(r.status);
    for (k, v) in &r.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    b.body(Full::new(Bytes::from(r.body))).unwrap_or_else(|e| {
        let mut resp = Response::new(Full::new(Bytes::from(format!("invalid configured response: {e}"))));
        *resp.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
        resp
    })
}

async fn handle(hub: Arc<WebhookHub>, req: Request<Incoming>, peer: SocketAddr, port: u16) -> Response<Full<Bytes>> {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    if path == INTERNAL_PREFIX || path.starts_with(&format!("{INTERNAL_PREFIX}/")) {
        let body = serde_json::json!({ "ok": true, "service": "devall-webhooks", "port": port }).to_string();
        return respond(Reply { status: 200, headers: vec![("Content-Type".into(), "application/json".into())], body: body.into_bytes() });
    }
    let (body, truncated) = read_capped(body, MAX_BODY_BYTES).await;
    let headers: Vec<(String, String)> = parts.headers.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let host = parts.headers.get(hyper::header::HOST).and_then(|h| h.to_str().ok()).filter(|h| !h.is_empty()).map(str::to_string).unwrap_or_else(|| format!("127.0.0.1:{port}"));
    let query = parts.uri.query().unwrap_or("").to_string();
    let url = if query.is_empty() { format!("http://{host}{path}") } else { format!("http://{host}{path}?{query}") };
    let incoming = IncomingEvent {
        method: parts.method.as_str().to_string(),
        url,
        path,
        query,
        headers,
        body,
        body_truncated: truncated,
        remote_addr: Some(peer.to_string()),
        received_at: now_ms(),
        source: EventSource::Local,
        public_url: None,
        external_id: None,
    };
    respond(hub.receive(incoming).await)
}
