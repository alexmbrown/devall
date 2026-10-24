//! Reverse tunnel: public requests to `https://<sub>.<domain>` arrive as multiplexed streams on
//! one WebSocket and are forwarded to a local target (`http://127.0.0.1:PORT`), byte-for-byte.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::{Semaphore, broadcast, mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use url::Url;

use crate::error::{RelayError, Result, chain};
use crate::protocol::{
    ErrorPayload, Frame, GoAway, HEARTBEAT_PING, HEARTBEAT_PONG, HeaderList, Limits, OfflineInbox, QuotaPayload, RequestHead, ResetPayload, ResponseHead, TunnelConfig, Usage, WsAccept, WsClose, WsOpen,
    WsReject, ft,
};
use crate::session::{ConnState, Connector, Exit, ReconnectPolicy, Source, binary, exit_for_close, now_ms, read_hello, spawn_writer, last_error_hint};
use crate::transport;

/// Request headers never forwarded to the local target (RFC 9110 hop-by-hop).
const HOP_BY_HOP: &[&str] = &["connection", "keep-alive", "proxy-connection", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"];

#[derive(Debug, Clone)]
pub struct TunnelOptions {
    /// Where public requests go, e.g. `http://127.0.0.1:3000` (a path prefix is kept).
    pub local: Url,
    /// Reserved subdomain (Supporter) or any name (self-hosted). `None` = random.
    pub subdomain: Option<String>,
    /// Send `Host: 127.0.0.1:PORT` instead of the public host (the public host is always in
    /// `X-Forwarded-Host`). Off by default, like ngrok, so absolute URLs and Twilio-style
    /// signatures over the public URL work.
    pub rewrite_host: bool,
    /// Buffer requests under these paths into a hosted inbox while the tunnel is offline.
    pub offline_inbox: Option<OfflineInbox>,
    pub reconnect: ReconnectPolicy,
    /// Accept self-signed certificates when the local target is `https://` (dev servers).
    pub accept_invalid_local_certs: bool,
}

impl TunnelOptions {
    pub fn new(local: Url) -> Self {
        Self { local, subdomain: None, rewrite_host: false, offline_inbox: None, reconnect: ReconnectPolicy::default(), accept_invalid_local_certs: true }
    }

    pub fn for_port(port: u16) -> Self {
        Self::new(Url::parse(&format!("http://127.0.0.1:{port}")).expect("valid loopback url"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub state: ConnState,
    pub local: String,
    pub public_url: Option<String>,
    /// `public_url` plus every custom domain serving the tunnel.
    #[serde(default)]
    pub public_urls: Vec<String>,
    pub subdomain: Option<String>,
    pub plan: Option<String>,
    pub session_expires_at: Option<String>,
    pub limits: Option<Limits>,
    pub usage: Option<Usage>,
    pub quota: Option<QuotaPayload>,
    pub requests: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub active_streams: u32,
    pub websockets: u32,
    pub reconnects: u32,
    pub last_error: Option<String>,
    pub connected_at_ms: Option<u64>,
}

impl TunnelStatus {
    fn new(local: &Url) -> Self {
        Self {
            state: ConnState::Connecting,
            local: local.to_string(),
            public_url: None,
            public_urls: Vec::new(),
            subdomain: None,
            plan: None,
            session_expires_at: None,
            limits: None,
            usage: None,
            quota: None,
            requests: 0,
            bytes_in: 0,
            bytes_out: 0,
            active_streams: 0,
            websockets: 0,
            reconnects: 0,
            last_error: None,
            connected_at_ms: None,
        }
    }
}

/// One proxied request (or pass-through WebSocket) for the live request log.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLog {
    pub stream: u32,
    pub at_ms: u64,
    pub method: String,
    pub path: String,
    pub status: Option<u16>,
    pub duration_ms: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub client_ip: Option<String>,
    pub websocket: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TunnelEvent {
    Status(Box<TunnelStatus>),
    Request(RequestLog),
}

pub(crate) struct Shared {
    status: watch::Sender<TunnelStatus>,
    events: broadcast::Sender<TunnelEvent>,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut TunnelStatus)) {
        self.status.send_modify(f);
        let s = self.status.borrow().clone();
        let _ = self.events.send(TunnelEvent::Status(Box::new(s)));
    }

    fn record(&self, log: RequestLog) {
        self.status.send_modify(|s| {
            s.requests += 1;
            s.bytes_in += log.bytes_in;
            s.bytes_out += log.bytes_out;
        });
        let _ = self.events.send(TunnelEvent::Request(log));
        let s = self.status.borrow().clone();
        let _ = self.events.send(TunnelEvent::Status(Box::new(s)));
    }
}

/// A running tunnel. Dropping the handle does not stop it; call [`TunnelHandle::stop`].
pub struct TunnelHandle {
    status: watch::Receiver<TunnelStatus>,
    events: broadcast::Sender<TunnelEvent>,
    stop: watch::Sender<bool>,
    task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl TunnelHandle {
    pub fn status(&self) -> TunnelStatus {
        self.status.borrow().clone()
    }

    /// Latest status, updated on every change.
    pub fn watch(&self) -> watch::Receiver<TunnelStatus> {
        self.status.clone()
    }

    /// Status changes and the live request log.
    pub fn subscribe(&self) -> broadcast::Receiver<TunnelEvent> {
        self.events.subscribe()
    }

    pub fn is_running(&self) -> bool {
        self.task.lock().expect("task lock").as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Waits until the tunnel is connected (or failed/stopped, or `timeout`).
    pub async fn wait_connected(&self, timeout: Duration) -> Result<TunnelStatus> {
        let mut rx = self.status.clone();
        let fut = rx.wait_for(|s| matches!(s.state, ConnState::Connected | ConnState::Failed | ConnState::Stopped));
        let s = tokio::time::timeout(timeout, fut).await.map_err(|_| RelayError::Timeout(format!("waiting for the tunnel{}", last_error_hint(self.status.borrow().last_error.as_deref()))))?.map_err(|_| RelayError::Connect("tunnel task ended".into()))?.clone();
        match s.state {
            ConnState::Connected => Ok(s),
            _ => Err(RelayError::Connect(s.last_error.unwrap_or_else(|| "tunnel stopped".into()))),
        }
    }

    /// Signals the tunnel to stop (closes the relay socket) without waiting.
    pub fn signal_stop(&self) {
        let _ = self.stop.send(true);
    }

    /// Stops the tunnel and waits for it to wind down.
    pub async fn stop(&self) {
        self.signal_stop();
        let task = self.task.lock().expect("task lock").take();
        if let Some(t) = task {
            let abort = t.abort_handle();
            if tokio::time::timeout(Duration::from_secs(5), t).await.is_err() {
                abort.abort();
            }
        }
    }
}

pub(crate) fn spawn(connector: Arc<Connector>, opts: TunnelOptions) -> TunnelHandle {
    let (status_tx, status_rx) = watch::channel(TunnelStatus::new(&opts.local));
    let (events, _) = broadcast::channel(1024);
    let (stop_tx, stop_rx) = watch::channel(false);
    let shared = Arc::new(Shared { status: status_tx, events: events.clone() });
    let task = tokio::spawn(run(connector, opts, shared, stop_rx));
    TunnelHandle { status: status_rx, events, stop: stop_tx, task: std::sync::Mutex::new(Some(task)) }
}

/// HTTP client for the local target: no proxy, no redirects, no decompression (bodies stay byte-exact).
fn local_client(opts: &TunnelOptions) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .danger_accept_invalid_certs(opts.accept_invalid_local_certs)
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

async fn run(connector: Arc<Connector>, opts: TunnelOptions, shared: Arc<Shared>, mut stop: watch::Receiver<bool>) {
    let http = local_client(&opts);
    let mut name = opts.subdomain.clone();
    let mut attempt = 0u32;
    loop {
        if *stop.borrow() {
            break;
        }
        let exit = session(&connector, &opts, &http, &shared, &mut name, &mut stop).await;
        let delay = match exit {
            Exit::Stopped => break,
            Exit::Failed(e) if e.is_fatal() => {
                shared.update(|s| {
                    s.state = ConnState::Failed;
                    s.last_error = Some(e.to_string());
                    s.active_streams = 0;
                    s.websockets = 0;
                });
                return;
            }
            Exit::Failed(e) => {
                let retry = match &e {
                    RelayError::Rejected { retry_after, .. } => *retry_after,
                    _ => None,
                };
                shared.update(|s| s.last_error = Some(e.to_string()));
                retry
            }
            Exit::Closed { reconnect: false, reason, .. } => {
                shared.update(|s| {
                    s.state = ConnState::Stopped;
                    s.last_error = Some(reason);
                    s.active_streams = 0;
                    s.websockets = 0;
                });
                return;
            }
            Exit::Closed { reconnect: true, delay, reason } => {
                // It was connected: start the backoff from scratch.
                attempt = 0;
                shared.update(|s| s.last_error = Some(reason));
                delay
            }
        };
        let wait = delay.unwrap_or_else(|| opts.reconnect.delay(attempt));
        attempt = attempt.saturating_add(1);
        shared.update(|s| {
            s.state = ConnState::Reconnecting;
            s.reconnects += 1;
            s.active_streams = 0;
            s.websockets = 0;
        });
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stop.changed() => {}
        }
    }
    shared.update(|s| {
        s.state = ConnState::Stopped;
        s.active_streams = 0;
        s.websockets = 0;
    });
}

enum Entry {
    Http { body: Option<mpsc::UnboundedSender<Bytes>>, credit: Arc<Semaphore>, task: AbortHandle },
    Ws { tx: mpsc::UnboundedSender<Frame>, task: AbortHandle },
}

impl Entry {
    fn abort(&self) {
        match self {
            Entry::Http { task, .. } | Entry::Ws { task, .. } => task.abort(),
        }
    }
}

struct StreamCtx {
    out: mpsc::Sender<Message>,
    http: reqwest::Client,
    local: Url,
    rewrite_host: bool,
    accept_invalid: bool,
    window: u32,
    max_frame: usize,
    max_ws: usize,
    shared: Arc<Shared>,
}

async fn session(connector: &Connector, opts: &TunnelOptions, http: &reqwest::Client, shared: &Arc<Shared>, name: &mut Option<String>, stop: &mut watch::Receiver<bool>) -> Exit {
    let query: Vec<(&str, String)> = name.iter().map(|n| ("name", n.clone())).collect();
    let ws = tokio::select! {
        r = connector.open("/connect", &query) => match r {
            Ok(ws) => ws,
            Err(e) => return Exit::Failed(e),
        },
        _ = stop.changed() => return Exit::Stopped,
    };
    let (sink, mut src) = ws.split();
    let hello = match read_hello(&mut src).await {
        Ok(h) => h,
        Err(e) => return Exit::Failed(e),
    };
    // Reconnects ask for the same name: free tunnels resume their random URL, reserved stay put.
    if hello.subdomain.is_some() {
        name.clone_from(&hello.subdomain);
    }
    let (out, out_rx) = mpsc::channel::<Message>(1024);
    let writer = spawn_writer(sink, out_rx);
    if let Some(oi) = &opts.offline_inbox {
        let _ = out.send(binary(Frame::json(ft::TUNNEL_CONFIG, 0, &TunnelConfig { offline_inbox: Some(oi.clone()), tcp: None }))).await;
    }
    shared.update(|s| {
        s.state = ConnState::Connected;
        s.public_url = Some(hello.public_url.clone());
        s.public_urls = if hello.public_urls.is_empty() { vec![hello.public_url.clone()] } else { hello.public_urls.clone() };
        s.subdomain.clone_from(&hello.subdomain);
        s.plan = Some(hello.plan.clone());
        s.limits = Some(hello.limits.clone());
        s.usage = Some(hello.usage.clone());
        s.session_expires_at.clone_from(&hello.session_expires_at);
        s.quota = None;
        s.last_error = None;
        s.connected_at_ms = Some(now_ms());
    });

    let ctx = Arc::new(StreamCtx {
        out: out.clone(),
        http: http.clone(),
        local: opts.local.clone(),
        rewrite_host: opts.rewrite_host,
        accept_invalid: opts.accept_invalid_local_certs,
        window: hello.window.max(1024),
        max_frame: hello.max_frame.clamp(1024, hello.window.max(1024) as usize),
        max_ws: hello.max_ws_message.max(1024),
        shared: shared.clone(),
    });
    let exit = pump(&ctx, &mut src, stop, Duration::from_secs(hello.heartbeat_seconds.clamp(5, 120))).await;
    drop(ctx);
    drop(out);
    if tokio::time::timeout(Duration::from_secs(2), writer).await.is_err() {
        // A stream task still holds the channel; the socket is going away anyway.
    }
    exit
}

async fn pump(ctx: &Arc<StreamCtx>, src: &mut Source, stop: &mut watch::Receiver<bool>, heartbeat: Duration) -> Exit {
    let mut streams: HashMap<u32, Entry> = HashMap::new();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<u32>();
    let mut hb = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    let mut last_seen = Instant::now();
    let mut goaway: Option<GoAway> = None;

    let exit = loop {
        tokio::select! {
            _ = stop.changed() => {
                if *stop.borrow() {
                    let _ = ctx.out.send(Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "stopped".into() }))).await;
                    break Exit::Stopped;
                }
            }
            _ = hb.tick() => {
                if last_seen.elapsed() > heartbeat * 3 {
                    break Exit::Closed { reconnect: true, delay: None, reason: "the relay stopped answering".into() };
                }
                let _ = ctx.out.try_send(Message::Text(HEARTBEAT_PING.into()));
            }
            Some(id) = done_rx.recv() => {
                if streams.remove(&id).is_some() {
                    let (h, w) = counts(&streams);
                    ctx.shared.status.send_modify(|s| { s.active_streams = h; s.websockets = w; });
                }
            }
            msg = src.next() => match msg {
                None => break exit_for_close(None, "", goaway.as_ref()),
                Some(Err(e)) => break Exit::Closed { reconnect: true, delay: None, reason: chain(&e) },
                Some(Ok(Message::Text(t))) => {
                    if t.as_str() == HEARTBEAT_PONG {
                        last_seen = Instant::now();
                    }
                }
                Some(Ok(Message::Binary(b))) => {
                    last_seen = Instant::now();
                    let res = match Frame::decode_bytes(b) {
                        Ok(f) => handle(f, ctx, &mut streams, &done_tx, &mut goaway).await,
                        Err(e) => Err(e),
                    };
                    if let Err(e) = res {
                        let _ = ctx.out.send(binary(Frame::json(ft::ERROR, 0, &ErrorPayload { code: "protocol_error".into(), message: e.to_string(), fatal: true }))).await;
                        break Exit::Closed { reconnect: true, delay: None, reason: e.to_string() };
                    }
                    let (h, w) = counts(&streams);
                    let changed = { let s = ctx.shared.status.borrow(); s.active_streams != h || s.websockets != w };
                    if changed {
                        ctx.shared.status.send_modify(|s| { s.active_streams = h; s.websockets = w; });
                    }
                }
                Some(Ok(Message::Close(cf))) => {
                    let (code, reason) = cf.map(|c| (Some(u16::from(c.code)), c.reason.to_string())).unwrap_or((None, String::new()));
                    break exit_for_close(code, &reason, goaway.as_ref());
                }
                Some(Ok(_)) => {}
            }
        }
    };
    for (_, e) in streams.drain() {
        e.abort();
    }
    exit
}

fn counts(streams: &HashMap<u32, Entry>) -> (u32, u32) {
    let ws = streams.values().filter(|e| matches!(e, Entry::Ws { .. })).count() as u32;
    (streams.len() as u32 - ws, ws)
}

async fn handle(f: Frame, ctx: &Arc<StreamCtx>, streams: &mut HashMap<u32, Entry>, done: &mpsc::UnboundedSender<u32>, goaway: &mut Option<GoAway>) -> Result<()> {
    match f.kind {
        ft::OPEN => {
            let head: RequestHead = f.parse()?;
            let (body_tx, body_rx) = mpsc::unbounded_channel::<Bytes>();
            let credit = Arc::new(Semaphore::new(ctx.window as usize));
            let task = tokio::spawn(http_stream(ctx.clone(), f.stream, head, body_rx, credit.clone(), done.clone()));
            if let Some(old) = streams.insert(f.stream, Entry::Http { body: Some(body_tx), credit, task: task.abort_handle() }) {
                old.abort();
            }
        }
        ft::DATA => {
            if let Some(Entry::Http { body: Some(tx), .. }) = streams.get(&f.stream) {
                let _ = tx.send(f.payload);
            }
        }
        ft::END => {
            if let Some(Entry::Http { body, .. }) = streams.get_mut(&f.stream) {
                *body = None;
            }
        }
        ft::WINDOW => {
            let n = f.parse_u32()?;
            if let Some(Entry::Http { credit, .. }) = streams.get(&f.stream) {
                credit.add_permits((n as usize).min(64 * 1024 * 1024));
            }
        }
        ft::RESET => {
            if let Some(e) = streams.remove(&f.stream) {
                e.abort();
            }
        }
        ft::WS_OPEN => {
            let open: WsOpen = f.parse()?;
            let (tx, rx) = mpsc::unbounded_channel::<Frame>();
            let task = tokio::spawn(ws_stream(ctx.clone(), f.stream, open, rx, done.clone()));
            if let Some(old) = streams.insert(f.stream, Entry::Ws { tx, task: task.abort_handle() }) {
                old.abort();
            }
        }
        ft::WS_TEXT | ft::WS_BINARY | ft::WS_CLOSE => {
            if let Some(Entry::Ws { tx, .. }) = streams.get(&f.stream) {
                let _ = tx.send(f);
            }
        }
        ft::QUOTA => {
            let q: QuotaPayload = f.parse()?;
            ctx.shared.update(|s| s.quota = Some(q));
        }
        ft::USAGE => {
            let u: Usage = f.parse()?;
            ctx.shared.update(|s| s.usage = Some(u));
        }
        ft::GOAWAY => *goaway = Some(f.parse()?),
        ft::ERROR => {
            let e: ErrorPayload = f.parse()?;
            ctx.shared.update(|s| s.last_error = Some(format!("{} ({})", e.message, e.code)));
        }
        ft::PING => {
            let _ = ctx.out.send(binary(Frame::new(ft::PONG, 0, f.payload))).await;
        }
        _ => {} // HELLO repeats, PONG, frames for other kinds/versions: ignore
    }
    Ok(())
}

/// Joins the local base URL and the public path (query kept verbatim).
fn local_url(base: &Url, path: &str) -> Result<Url> {
    let prefix = base.as_str().trim_end_matches('/');
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    Url::parse(&format!("{prefix}{path}")).map_err(|e| RelayError::Config(format!("local URL for {path}: {e}")))
}

fn authority(u: &Url) -> String {
    let host = u.host_str().unwrap_or("localhost");
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
    match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    }
}

/// Headers sent to the local app: original minus hop-by-hop, plus `X-Forwarded-*`.
fn local_headers(head_headers: &HeaderList, public_url: &str, client_ip: Option<&str>, local: &Url, rewrite_host: bool, websocket: bool) -> Vec<(String, String)> {
    let public = Url::parse(public_url).ok();
    let public_host = public.as_ref().map(authority).unwrap_or_default();
    let mut out: Vec<(String, String)> = Vec::with_capacity(head_headers.len() + 5);
    let mut xff: Option<String> = None;
    for (k, v) in head_headers {
        let lk = k.to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lk.as_str()) || lk == "host" || lk.starts_with("x-forwarded-") {
            if lk == "x-forwarded-for" {
                xff = Some(v.clone());
            }
            continue;
        }
        if websocket && lk.starts_with("sec-websocket-") {
            continue;
        }
        out.push((lk, v.clone()));
    }
    let host = if rewrite_host || public_host.is_empty() { authority(local) } else { public_host.clone() };
    out.push(("host".into(), host));
    let xff = match (xff, client_ip) {
        (Some(prev), Some(ip)) => Some(format!("{prev}, {ip}")),
        (None, Some(ip)) => Some(ip.to_string()),
        (prev, None) => prev,
    };
    if let Some(x) = xff {
        out.push(("x-forwarded-for".into(), x));
    }
    out.push(("x-forwarded-proto".into(), public.as_ref().map(|u| u.scheme().to_string()).unwrap_or_else(|| "https".into())));
    if !public_host.is_empty() {
        out.push(("x-forwarded-host".into(), public_host));
    }
    out.push(("x-forwarded-port".into(), public.as_ref().and_then(|u| u.port_or_known_default()).unwrap_or(443).to_string()));
    out
}

async fn send_frame(ctx: &StreamCtx, f: Frame) -> bool {
    ctx.out.send(binary(f)).await.is_ok()
}

/// A complete short response (local target unreachable, bad request…).
async fn respond_plain(ctx: &StreamCtx, id: u32, status: u16, message: &str) {
    let head = ResponseHead { status, headers: vec![("content-type".into(), "text/plain; charset=utf-8".into()), ("x-devall-error".into(), "1".into())] };
    let _ = send_frame(ctx, Frame::json(ft::RESPONSE, id, &head)).await;
    let _ = send_frame(ctx, Frame::new(ft::RESPONSE_DATA, id, format!("{message}\n").into_bytes())).await;
    let _ = send_frame(ctx, Frame::empty(ft::RESPONSE_END, id)).await;
}

async fn http_stream(ctx: Arc<StreamCtx>, id: u32, head: RequestHead, body_rx: mpsc::UnboundedReceiver<Bytes>, credit: Arc<Semaphore>, done: mpsc::UnboundedSender<u32>) {
    let started = Instant::now();
    let at_ms = now_ms();
    let bytes_in = Arc::new(AtomicU64::new(0));
    let (status, bytes_out, error) = forward(&ctx, id, &head, body_rx, &credit, &bytes_in).await;
    ctx.shared.record(RequestLog {
        stream: id,
        at_ms,
        method: head.method.clone(),
        path: head.path.clone(),
        status,
        duration_ms: started.elapsed().as_millis() as u64,
        bytes_in: bytes_in.load(Ordering::Relaxed),
        bytes_out,
        client_ip: head.client_ip.clone(),
        websocket: false,
        error,
    });
    let _ = done.send(id);
}

async fn forward(ctx: &Arc<StreamCtx>, id: u32, head: &RequestHead, body_rx: mpsc::UnboundedReceiver<Bytes>, credit: &Semaphore, bytes_in: &Arc<AtomicU64>) -> (Option<u16>, u64, Option<String>) {
    let target = match local_url(&ctx.local, &head.path) {
        Ok(u) => u,
        Err(e) => {
            respond_plain(ctx, id, 400, &format!("devall: {e}")).await;
            return (Some(400), 0, Some(e.to_string()));
        }
    };
    let method = match reqwest::Method::from_bytes(head.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            respond_plain(ctx, id, 400, "devall: invalid method").await;
            return (Some(400), 0, Some("invalid method".into()));
        }
    };
    let mut headers = reqwest::header::HeaderMap::new();
    for (k, v) in local_headers(&head.headers, &head.url, head.client_ip.as_deref(), &ctx.local, ctx.rewrite_host, false) {
        if let (Ok(name), Ok(value)) = (reqwest::header::HeaderName::from_bytes(k.as_bytes()), reqwest::header::HeaderValue::from_str(&v)) {
            headers.append(name, value);
        }
    }
    let has_body = match head.body_length {
        Some(n) => n > 0,
        None => !matches!(method, reqwest::Method::GET | reqwest::Method::HEAD | reqwest::Method::OPTIONS),
    };
    let mut req = ctx.http.request(method, target).headers(headers);
    if has_body {
        let out = ctx.out.clone();
        let half = (ctx.window / 2).max(1);
        let counter = bytes_in.clone();
        // Credit goes back to the relay as the local server consumes the body.
        let stream = futures_util::stream::unfold((body_rx, 0u32), move |(mut rx, mut consumed)| {
            let out = out.clone();
            let counter = counter.clone();
            async move {
                let chunk = rx.recv().await?;
                counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                consumed = consumed.saturating_add(chunk.len() as u32);
                if consumed >= half {
                    let _ = out.send(binary(Frame::u32(ft::WINDOW, id, consumed))).await;
                    consumed = 0;
                }
                Some((Ok::<Bytes, std::io::Error>(chunk), (rx, consumed)))
            }
        });
        req = req.body(reqwest::Body::wrap_stream(stream));
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("devall: could not reach {} — {}", ctx.local.as_str().trim_end_matches('/'), chain(&e));
            respond_plain(ctx, id, 502, &msg).await;
            return (Some(502), 0, Some(msg));
        }
    };
    let status = resp.status().as_u16();
    let headers: HeaderList = resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    if !send_frame(ctx, Frame::json(ft::RESPONSE, id, &ResponseHead { status, headers })).await {
        return (Some(status), 0, Some("relay disconnected".into()));
    }
    let mut bytes_out = 0u64;
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        let mut chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let msg = chain(&e);
                let _ = send_frame(ctx, Frame::json(ft::RESET, id, &ResetPayload { code: "local_unreachable".into(), message: msg.clone() })).await;
                return (Some(status), bytes_out, Some(msg));
            }
        };
        while !chunk.is_empty() {
            let n = chunk.len().min(ctx.max_frame);
            let piece = chunk.split_to(n);
            match credit.acquire_many(n as u32).await {
                Ok(p) => p.forget(),
                Err(_) => return (Some(status), bytes_out, Some("stream closed".into())),
            }
            if !send_frame(ctx, Frame::new(ft::RESPONSE_DATA, id, piece)).await {
                return (Some(status), bytes_out, Some("relay disconnected".into()));
            }
            bytes_out += n as u64;
        }
    }
    let _ = send_frame(ctx, Frame::empty(ft::RESPONSE_END, id)).await;
    (Some(status), bytes_out, None)
}

async fn ws_stream(ctx: Arc<StreamCtx>, id: u32, open: WsOpen, mut rx: mpsc::UnboundedReceiver<Frame>, done: mpsc::UnboundedSender<u32>) {
    let started = Instant::now();
    let at_ms = now_ms();
    let mut log = RequestLog { stream: id, at_ms, method: "GET".into(), path: open.path.clone(), status: None, duration_ms: 0, bytes_in: 0, bytes_out: 0, client_ip: open.client_ip.clone(), websocket: true, error: None };
    let result = async {
        let mut url = local_url(&ctx.local, &open.path)?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme).map_err(|_| RelayError::Config("cannot build ws:// URL".into()))?;
        let headers = local_headers(&open.headers, &open.url, open.client_ip.as_deref(), &ctx.local, ctx.rewrite_host, true);
        // The handshake builder sets Host from the URL; send the chosen one instead.
        transport::websocket(&url, &headers, &open.protocols, !ctx.accept_invalid, Duration::from_secs(10), ctx.max_ws).await
    }
    .await;
    let hs = match result {
        Ok(h) => h,
        Err(e) => {
            let status = e.status().filter(|s| (400..=599).contains(s)).unwrap_or(502);
            let message = format!("devall: local WebSocket refused: {e}");
            let _ = send_frame(&ctx, Frame::json(ft::WS_REJECT, id, &WsReject { status, message: message.clone() })).await;
            log.status = Some(status);
            log.error = Some(message);
            log.duration_ms = started.elapsed().as_millis() as u64;
            ctx.shared.record(log);
            let _ = done.send(id);
            return;
        }
    };
    log.status = Some(101);
    let _ = send_frame(&ctx, Frame::json(ft::WS_ACCEPT, id, &WsAccept { protocol: hs.protocol.clone() })).await;
    let (mut sink, mut stream) = hs.ws.split();
    loop {
        tokio::select! {
            f = rx.recv() => match f {
                None => {
                    let _ = sink.send(Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "tunnel closed".into() }))).await;
                    break;
                }
                Some(f) => match f.kind {
                    ft::WS_TEXT => {
                        log.bytes_in += f.payload.len() as u64;
                        if sink.send(Message::Text(String::from_utf8_lossy(&f.payload).into_owned().into())).await.is_err() { break; }
                    }
                    ft::WS_BINARY => {
                        log.bytes_in += f.payload.len() as u64;
                        if sink.send(Message::Binary(f.payload)).await.is_err() { break; }
                    }
                    _ => {
                        let c: WsClose = f.parse().unwrap_or(WsClose { code: 1000, reason: String::new() });
                        let code = if c.code == 1005 || c.code == 1006 { 1000 } else { c.code };
                        let _ = sink.send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: c.reason.into() }))).await;
                        break;
                    }
                }
            },
            m = stream.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    log.bytes_out += t.len() as u64;
                    if !send_frame(&ctx, Frame::new(ft::WS_TEXT, id, t.as_bytes().to_vec())).await { break; }
                }
                Some(Ok(Message::Binary(b))) => {
                    log.bytes_out += b.len() as u64;
                    if !send_frame(&ctx, Frame::new(ft::WS_BINARY, id, b)).await { break; }
                }
                Some(Ok(Message::Close(cf))) => {
                    let (code, reason) = cf.map(|c| (u16::from(c.code), c.reason.to_string())).unwrap_or((1000, String::new()));
                    let _ = send_frame(&ctx, Frame::json(ft::WS_CLOSE, id, &WsClose { code, reason })).await;
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    log.error = Some(chain(&e));
                    let _ = send_frame(&ctx, Frame::json(ft::WS_CLOSE, id, &WsClose { code: 1011, reason: "local connection failed".into() })).await;
                    break;
                }
                None => {
                    let _ = send_frame(&ctx, Frame::json(ft::WS_CLOSE, id, &WsClose { code: 1011, reason: "local connection closed".into() })).await;
                    break;
                }
            }
        }
    }
    log.duration_ms = started.elapsed().as_millis() as u64;
    ctx.shared.record(log);
    let _ = done.send(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_local_urls_verbatim() {
        let base = Url::parse("http://127.0.0.1:3000").unwrap();
        assert_eq!(local_url(&base, "/a/b?x=1&y=%20").unwrap().as_str(), "http://127.0.0.1:3000/a/b?x=1&y=%20");
        let prefixed = Url::parse("http://127.0.0.1:3000/api/").unwrap();
        assert_eq!(local_url(&prefixed, "/v1").unwrap().as_str(), "http://127.0.0.1:3000/api/v1");
    }

    #[test]
    fn forwards_headers_with_x_forwarded() {
        let local = Url::parse("http://127.0.0.1:3000").unwrap();
        let hs: HeaderList = vec![
            ("content-type".into(), "application/json".into()),
            ("connection".into(), "keep-alive".into()),
            ("host".into(), "ignored".into()),
            ("X-Twilio-Signature".into(), "abc=".into()),
        ];
        let out = local_headers(&hs, "https://my-app.devall.dev/hook?x=1", Some("203.0.113.9"), &local, false, false);
        assert!(out.contains(&("content-type".into(), "application/json".into())));
        assert!(out.contains(&("x-twilio-signature".into(), "abc=".into())));
        assert!(out.contains(&("host".into(), "my-app.devall.dev".into())));
        assert!(out.contains(&("x-forwarded-for".into(), "203.0.113.9".into())));
        assert!(out.contains(&("x-forwarded-proto".into(), "https".into())));
        assert!(out.contains(&("x-forwarded-host".into(), "my-app.devall.dev".into())));
        assert!(out.contains(&("x-forwarded-port".into(), "443".into())));
        assert!(!out.iter().any(|(k, _)| k == "connection"));
        let rewritten = local_headers(&hs, "https://my-app.devall.dev/", None, &local, true, false);
        assert!(rewritten.contains(&("host".into(), "127.0.0.1:3000".into())));
        assert!(!rewritten.iter().any(|(k, _)| k == "x-forwarded-for"));
    }
}
