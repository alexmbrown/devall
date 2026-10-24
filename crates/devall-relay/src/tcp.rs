//! Raw TCP tunnels (TCP over WebSocket, like `cloudflared access tcp`) — PROTOCOL.md §2.6.
//!
//! - [`spawn_publisher`]: `devall tunnel tcp --port 5432` — registers a TCP tunnel and, for every
//!   TCP_OPEN, connects to the local port and bridges bytes with per-stream credit.
//! - [`spawn_consumer`]: `devall tcp connect <name> --local-port 15432` — listens locally and opens
//!   one relay WebSocket per accepted connection (owner account token or the tunnel's
//!   `dtcp_…` access secret).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, broadcast, mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::error::{RelayError, Result, chain};
use crate::protocol::{
    DEFAULT_MAX_FRAME, DEFAULT_WINDOW, ErrorPayload, Frame, GoAway, HEARTBEAT_PING, HEARTBEAT_PONG, Limits, QuotaPayload, TCP_CONSUMER_STREAM, TcpClose, TcpConfig, TcpOpen, TunnelConfig, Usage, ft,
};
use crate::session::{ConnState, Connector, Exit, ReconnectPolicy, Source, binary, exit_for_close, now_ms, read_hello, spawn_writer, last_error_hint};

/// Prefix of TCP tunnel access secrets (the relay recognizes them in `Authorization`).
pub const SECRET_PREFIX: &str = "dtcp_";

/// A new random access secret, e.g. `dtcp_3kQ…` (40 chars of base62 ≈ 238 bits).
pub fn generate_access_secret() -> String {
    const B62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut s = String::from(SECRET_PREFIX);
    for _ in 0..40 {
        s.push(B62[fastrand::usize(..B62.len())] as char);
    }
    s
}

/// SHA-256 (lower-case hex) — what the publisher tells the relay instead of the secret.
pub fn secret_sha256(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

// ---------- shared bridge ----------

#[derive(Default)]
struct Counters {
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

/// Copies between a local TCP socket and one relay stream until either side closes.
/// `rx` gets TCP_DATA / TCP_CLOSE for the stream; `credit` is topped up by WINDOW frames.
/// Returns the error that ended the stream, if any.
async fn bridge(sock: TcpStream, stream: u32, out: mpsc::Sender<Message>, mut rx: mpsc::UnboundedReceiver<Frame>, credit: Arc<Semaphore>, max_frame: usize, counters: Arc<Counters>) -> Option<String> {
    let (mut rd, mut wr) = sock.into_split();
    let half = (DEFAULT_WINDOW / 2).max(1);
    let out_r = out.clone();
    let counters_r = counters.clone();
    let reader = async move {
        let mut buf = vec![0u8; max_frame.max(1024)];
        loop {
            let n = match rd.read(&mut buf).await {
                Ok(0) => return Ok::<(), String>(()),
                Ok(n) => n,
                Err(e) => return Err(e.to_string()),
            };
            match credit.acquire_many(n as u32).await {
                Ok(p) => p.forget(),
                Err(_) => return Err("stream closed".into()),
            }
            if out_r.send(binary(Frame::new(ft::TCP_DATA, stream, buf[..n].to_vec()))).await.is_err() {
                return Err("relay disconnected".into());
            }
            counters_r.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
        }
    };
    let out_w = out.clone();
    let writer = async move {
        let mut consumed = 0u32;
        while let Some(f) = rx.recv().await {
            match f.kind {
                ft::TCP_DATA => {
                    if let Err(e) = wr.write_all(&f.payload).await {
                        return Err(e.to_string());
                    }
                    counters.bytes_in.fetch_add(f.payload.len() as u64, Ordering::Relaxed);
                    consumed = consumed.saturating_add(f.payload.len() as u32);
                    if consumed >= half {
                        let _ = out_w.send(binary(Frame::u32(ft::WINDOW, stream, consumed))).await;
                        consumed = 0;
                    }
                }
                ft::TCP_CLOSE => {
                    let _ = wr.shutdown().await;
                    return Ok(true);
                }
                _ => {}
            }
        }
        let _ = wr.shutdown().await;
        Ok::<bool, String>(false)
    };
    tokio::select! {
        r = reader => {
            let (reason, err) = match r {
                Ok(()) => ("eof", None),
                Err(e) => ("reset", Some(e)),
            };
            let _ = out.send(binary(Frame::json(ft::TCP_CLOSE, stream, &TcpClose { reason: reason.into(), message: err.clone() }))).await;
            err
        }
        w = writer => match w {
            Ok(true) => None, // the other side closed the stream
            Ok(false) => Some("relay connection ended".into()),
            Err(e) => {
                let _ = out.send(binary(Frame::json(ft::TCP_CLOSE, stream, &TcpClose { reason: "reset".into(), message: Some(e.clone()) }))).await;
                Some(e)
            }
        },
    }
}

// ---------- publisher ----------

#[derive(Debug, Clone)]
pub struct TcpTunnelOptions {
    /// Local target, e.g. `127.0.0.1:5432`.
    pub local: String,
    /// Reserved subdomain (Supporter) or any name (self-hosted). `None` = random.
    pub subdomain: Option<String>,
    /// Lets teammates connect with this `dtcp_…` secret (only its SHA-256 is sent). `None` = only
    /// your own account can connect.
    pub access_secret: Option<String>,
    pub reconnect: ReconnectPolicy,
    pub connect_timeout: Duration,
}

impl TcpTunnelOptions {
    pub fn for_port(port: u16) -> Self {
        Self { local: format!("127.0.0.1:{port}"), subdomain: None, access_secret: None, reconnect: ReconnectPolicy::default(), connect_timeout: Duration::from_secs(10) }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpTunnelStatus {
    pub state: ConnState,
    pub local: String,
    /// Name consumers pass to `devall tcp connect`.
    pub name: Option<String>,
    pub plan: Option<String>,
    pub session_expires_at: Option<String>,
    pub limits: Option<Limits>,
    pub usage: Option<Usage>,
    pub quota: Option<QuotaPayload>,
    /// Whether an access secret is configured (the secret itself is never in the status).
    pub has_secret: bool,
    pub connections: u32,
    pub total_connections: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub reconnects: u32,
    pub last_error: Option<String>,
    pub connected_at_ms: Option<u64>,
}

/// One bridged connection, for the live log.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpConnLog {
    pub stream: u32,
    pub at_ms: u64,
    pub client_ip: Option<String>,
    /// `owner`, `secret` (publisher side) or `local` (consumer side).
    pub via: String,
    pub duration_ms: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TcpTunnelEvent {
    Status(Box<TcpTunnelStatus>),
    Connection(TcpConnLog),
}

struct PubShared {
    status: watch::Sender<TcpTunnelStatus>,
    events: broadcast::Sender<TcpTunnelEvent>,
}

impl PubShared {
    fn update(&self, f: impl FnOnce(&mut TcpTunnelStatus)) {
        self.status.send_modify(f);
        let s = self.status.borrow().clone();
        let _ = self.events.send(TcpTunnelEvent::Status(Box::new(s)));
    }
}

/// A running TCP tunnel. Dropping the handle does not stop it; call [`TcpTunnelHandle::stop`].
pub struct TcpTunnelHandle {
    status: watch::Receiver<TcpTunnelStatus>,
    events: broadcast::Sender<TcpTunnelEvent>,
    stop: watch::Sender<bool>,
    task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl TcpTunnelHandle {
    pub fn status(&self) -> TcpTunnelStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TcpTunnelEvent> {
        self.events.subscribe()
    }

    pub async fn wait_connected(&self, timeout: Duration) -> Result<TcpTunnelStatus> {
        let mut rx = self.status.clone();
        let fut = rx.wait_for(|s| matches!(s.state, ConnState::Connected | ConnState::Failed | ConnState::Stopped));
        let s = tokio::time::timeout(timeout, fut).await.map_err(|_| RelayError::Timeout(format!("waiting for the TCP tunnel{}", last_error_hint(self.status.borrow().last_error.as_deref()))))?.map_err(|_| RelayError::Connect("tunnel task ended".into()))?.clone();
        match s.state {
            ConnState::Connected => Ok(s),
            _ => Err(RelayError::Connect(s.last_error.unwrap_or_else(|| "tunnel stopped".into()))),
        }
    }

    pub fn signal_stop(&self) {
        let _ = self.stop.send(true);
    }

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

pub(crate) fn spawn_publisher(connector: Arc<Connector>, opts: TcpTunnelOptions) -> TcpTunnelHandle {
    let initial = TcpTunnelStatus {
        state: ConnState::Connecting,
        local: opts.local.clone(),
        name: None,
        plan: None,
        session_expires_at: None,
        limits: None,
        usage: None,
        quota: None,
        has_secret: opts.access_secret.is_some(),
        connections: 0,
        total_connections: 0,
        bytes_in: 0,
        bytes_out: 0,
        reconnects: 0,
        last_error: None,
        connected_at_ms: None,
    };
    let (status_tx, status_rx) = watch::channel(initial);
    let (events, _) = broadcast::channel(1024);
    let (stop_tx, stop_rx) = watch::channel(false);
    let shared = Arc::new(PubShared { status: status_tx, events: events.clone() });
    let task = tokio::spawn(run_publisher(connector, opts, shared, stop_rx));
    TcpTunnelHandle { status: status_rx, events, stop: stop_tx, task: std::sync::Mutex::new(Some(task)) }
}

async fn run_publisher(connector: Arc<Connector>, opts: TcpTunnelOptions, shared: Arc<PubShared>, mut stop: watch::Receiver<bool>) {
    let mut name = opts.subdomain.clone();
    let mut attempt = 0u32;
    loop {
        if *stop.borrow() {
            break;
        }
        let exit = publisher_session(&connector, &opts, &shared, &mut name, &mut stop).await;
        let delay = match exit {
            Exit::Stopped => break,
            Exit::Failed(e) if e.is_fatal() => {
                shared.update(|s| {
                    s.state = ConnState::Failed;
                    s.last_error = Some(e.to_string());
                    s.connections = 0;
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
                    s.connections = 0;
                });
                return;
            }
            Exit::Closed { reconnect: true, delay, reason } => {
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
            s.connections = 0;
        });
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stop.changed() => {}
        }
    }
    shared.update(|s| {
        s.state = ConnState::Stopped;
        s.connections = 0;
    });
}

struct PubStream {
    tx: mpsc::UnboundedSender<Frame>,
    credit: Arc<Semaphore>,
    task: AbortHandle,
}

async fn publisher_session(connector: &Connector, opts: &TcpTunnelOptions, shared: &Arc<PubShared>, name: &mut Option<String>, stop: &mut watch::Receiver<bool>) -> Exit {
    let mut query: Vec<(&str, String)> = vec![("kind", "tcp".into())];
    if let Some(n) = name.as_ref() {
        query.push(("name", n.clone()));
    }
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
    if hello.kind != "tcp" {
        return Exit::Failed(RelayError::Config("this relay doesn't support raw TCP tunnels yet (update the relay)".into()));
    }
    if hello.subdomain.is_some() {
        name.clone_from(&hello.subdomain);
    }
    let (out, out_rx) = mpsc::channel::<Message>(1024);
    let writer = spawn_writer(sink, out_rx);
    let cfg = TunnelConfig { offline_inbox: None, tcp: Some(TcpConfig { secret_sha256: opts.access_secret.as_deref().map(secret_sha256) }) };
    let _ = out.send(binary(Frame::json(ft::TUNNEL_CONFIG, 0, &cfg))).await;
    shared.update(|s| {
        s.state = ConnState::Connected;
        s.name.clone_from(&hello.subdomain);
        s.plan = Some(hello.plan.clone());
        s.limits = Some(hello.limits.clone());
        s.usage = Some(hello.usage.clone());
        s.session_expires_at.clone_from(&hello.session_expires_at);
        s.quota = None;
        s.last_error = None;
        s.connected_at_ms = Some(now_ms());
    });
    let max_frame = hello.max_frame.clamp(1024, DEFAULT_WINDOW as usize);
    let heartbeat = Duration::from_secs(hello.heartbeat_seconds.clamp(5, 120));
    let exit = publisher_pump(opts, shared, &out, &mut src, stop, heartbeat, max_frame).await;
    drop(out);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    exit
}

async fn publisher_pump(opts: &TcpTunnelOptions, shared: &Arc<PubShared>, out: &mpsc::Sender<Message>, src: &mut Source, stop: &mut watch::Receiver<bool>, heartbeat: Duration, max_frame: usize) -> Exit {
    let mut streams: HashMap<u32, PubStream> = HashMap::new();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<u32>();
    let mut hb = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    let mut last_seen = Instant::now();
    let mut goaway: Option<GoAway> = None;
    let exit = loop {
        tokio::select! {
            _ = stop.changed() => {
                if *stop.borrow() {
                    let _ = out.send(Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "stopped".into() }))).await;
                    break Exit::Stopped;
                }
            }
            _ = hb.tick() => {
                if last_seen.elapsed() > heartbeat * 3 {
                    break Exit::Closed { reconnect: true, delay: None, reason: "the relay stopped answering".into() };
                }
                let _ = out.try_send(Message::Text(HEARTBEAT_PING.into()));
            }
            Some(id) = done_rx.recv() => {
                if streams.remove(&id).is_some() {
                    let n = streams.len() as u32;
                    shared.status.send_modify(|s| s.connections = n);
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
                    let f = match Frame::decode_bytes(b) {
                        Ok(f) => f,
                        Err(e) => {
                            let _ = out.send(binary(Frame::json(ft::ERROR, 0, &ErrorPayload { code: "protocol_error".into(), message: e.to_string(), fatal: true }))).await;
                            break Exit::Closed { reconnect: true, delay: None, reason: e.to_string() };
                        }
                    };
                    match f.kind {
                        ft::TCP_OPEN => {
                            let open: TcpOpen = match f.parse() {
                                Ok(o) => o,
                                Err(_) => TcpOpen { client_ip: None, via: "owner".into() },
                            };
                            let (tx, rx) = mpsc::unbounded_channel::<Frame>();
                            let credit = Arc::new(Semaphore::new(DEFAULT_WINDOW as usize));
                            let task = tokio::spawn(publisher_stream(opts.local.clone(), opts.connect_timeout, f.stream, open, out.clone(), rx, credit.clone(), max_frame, shared.clone(), done_tx.clone()));
                            if let Some(old) = streams.insert(f.stream, PubStream { tx, credit, task: task.abort_handle() }) {
                                old.task.abort();
                            }
                            let n = streams.len() as u32;
                            shared.status.send_modify(|s| {
                                s.connections = n;
                                s.total_connections += 1;
                            });
                        }
                        ft::TCP_DATA | ft::TCP_CLOSE => {
                            if let Some(st) = streams.get(&f.stream) {
                                let _ = st.tx.send(f);
                            }
                        }
                        ft::WINDOW => {
                            if let (Ok(n), Some(st)) = (f.parse_u32(), streams.get(&f.stream)) {
                                st.credit.add_permits((n as usize).min(64 * 1024 * 1024));
                            }
                        }
                        ft::QUOTA => {
                            if let Ok(q) = f.parse::<QuotaPayload>() {
                                shared.update(|s| s.quota = Some(q));
                            }
                        }
                        ft::USAGE => {
                            if let Ok(u) = f.parse::<Usage>() {
                                shared.update(|s| s.usage = Some(u));
                            }
                        }
                        ft::GOAWAY => goaway = f.parse().ok(),
                        ft::ERROR => {
                            if let Ok(e) = f.parse::<ErrorPayload>() {
                                shared.update(|s| s.last_error = Some(format!("{} ({})", e.message, e.code)));
                            }
                        }
                        ft::PING => {
                            let _ = out.send(binary(Frame::new(ft::PONG, 0, f.payload))).await;
                        }
                        _ => {}
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
    for (_, st) in streams.drain() {
        st.task.abort();
    }
    exit
}

#[allow(clippy::too_many_arguments)]
async fn publisher_stream(
    local: String,
    connect_timeout: Duration,
    id: u32,
    open: TcpOpen,
    out: mpsc::Sender<Message>,
    rx: mpsc::UnboundedReceiver<Frame>,
    credit: Arc<Semaphore>,
    max_frame: usize,
    shared: Arc<PubShared>,
    done: mpsc::UnboundedSender<u32>,
) {
    let started = Instant::now();
    let at_ms = now_ms();
    let counters = Arc::new(Counters::default());
    let error = match tokio::time::timeout(connect_timeout, TcpStream::connect(local.as_str())).await {
        Ok(Ok(sock)) => {
            let _ = sock.set_nodelay(true);
            let _ = out.send(binary(Frame::empty(ft::TCP_ACCEPT, id))).await;
            bridge(sock, id, out, rx, credit, max_frame, counters.clone()).await
        }
        Ok(Err(e)) => {
            let msg = format!("could not connect to {local}: {e}");
            let _ = out.send(binary(Frame::json(ft::TCP_CLOSE, id, &TcpClose { reason: "local_unreachable".into(), message: Some(msg.clone()) }))).await;
            Some(msg)
        }
        Err(_) => {
            let msg = format!("connecting to {local} timed out");
            let _ = out.send(binary(Frame::json(ft::TCP_CLOSE, id, &TcpClose { reason: "local_unreachable".into(), message: Some(msg.clone()) }))).await;
            Some(msg)
        }
    };
    let log = TcpConnLog {
        stream: id,
        at_ms,
        client_ip: open.client_ip,
        via: open.via,
        duration_ms: started.elapsed().as_millis() as u64,
        bytes_in: counters.bytes_in.load(Ordering::Relaxed),
        bytes_out: counters.bytes_out.load(Ordering::Relaxed),
        error,
    };
    shared.status.send_modify(|s| {
        s.bytes_in += log.bytes_in;
        s.bytes_out += log.bytes_out;
    });
    let _ = shared.events.send(TcpTunnelEvent::Connection(log));
    let _ = done.send(id);
}

// ---------- consumer ----------

/// How a consumer authenticates to the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TcpAccess {
    /// The signed-in account (must own the tunnel).
    Account,
    /// The tunnel's `dtcp_…` access secret.
    Secret(String),
}

#[derive(Debug, Clone)]
pub struct TcpConnectOptions {
    /// The TCP tunnel's name (its subdomain).
    pub name: String,
    /// Where to listen, e.g. `127.0.0.1:15432` (port 0 = any free port).
    pub bind: SocketAddr,
    pub access: TcpAccess,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpConnectStatus {
    pub name: String,
    /// The local address clients connect to.
    pub listen: String,
    pub running: bool,
    pub connections: u32,
    pub total_connections: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TcpConnectEvent {
    Status(Box<TcpConnectStatus>),
    Connection(TcpConnLog),
}

/// A local listener forwarding every connection through the relay.
pub struct TcpConnectHandle {
    local_addr: SocketAddr,
    status: watch::Receiver<TcpConnectStatus>,
    events: broadcast::Sender<TcpConnectEvent>,
    stop: watch::Sender<bool>,
    task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl TcpConnectHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn status(&self) -> TcpConnectStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TcpConnectEvent> {
        self.events.subscribe()
    }

    pub async fn stop(&self) {
        let _ = self.stop.send(true);
        let task = self.task.lock().expect("task lock").take();
        if let Some(t) = task {
            let abort = t.abort_handle();
            if tokio::time::timeout(Duration::from_secs(5), t).await.is_err() {
                abort.abort();
            }
        }
    }
}

struct ConShared {
    status: watch::Sender<TcpConnectStatus>,
    events: broadcast::Sender<TcpConnectEvent>,
}

impl ConShared {
    fn update(&self, f: impl FnOnce(&mut TcpConnectStatus)) {
        self.status.send_modify(f);
        let s = self.status.borrow().clone();
        let _ = self.events.send(TcpConnectEvent::Status(Box::new(s)));
    }
}

pub(crate) async fn spawn_consumer(connector: Arc<Connector>, opts: TcpConnectOptions) -> Result<TcpConnectHandle> {
    let listener = TcpListener::bind(opts.bind).await.map_err(|e| RelayError::Config(format!("can't listen on {}: {e}", opts.bind)))?;
    let local_addr = listener.local_addr()?;
    let (status_tx, status_rx) = watch::channel(TcpConnectStatus {
        name: opts.name.clone(),
        listen: local_addr.to_string(),
        running: true,
        connections: 0,
        total_connections: 0,
        bytes_in: 0,
        bytes_out: 0,
        last_error: None,
    });
    let (events, _) = broadcast::channel(1024);
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let shared = Arc::new(ConShared { status: status_tx, events: events.clone() });
    let task = tokio::spawn(async move {
        let mut conns: Vec<JoinHandle<()>> = Vec::new();
        loop {
            tokio::select! {
                _ = stop_rx.changed() => break,
                acc = listener.accept() => {
                    let Ok((sock, peer)) = acc else { continue };
                    let _ = sock.set_nodelay(true);
                    conns.retain(|c| !c.is_finished());
                    shared.update(|s| {
                        s.connections += 1;
                        s.total_connections += 1;
                    });
                    conns.push(tokio::spawn(consumer_conn(connector.clone(), opts.clone(), sock, peer, shared.clone())));
                }
            }
        }
        for c in conns {
            c.abort();
        }
        shared.update(|s| {
            s.running = false;
            s.connections = 0;
        });
    });
    Ok(TcpConnectHandle { local_addr, status: status_rx, events, stop: stop_tx, task: std::sync::Mutex::new(Some(task)) })
}

async fn consumer_conn(connector: Arc<Connector>, opts: TcpConnectOptions, sock: TcpStream, peer: SocketAddr, shared: Arc<ConShared>) {
    let started = Instant::now();
    let at_ms = now_ms();
    let counters = Arc::new(Counters::default());
    let query = [("name", opts.name.clone())];
    let ws = match &opts.access {
        TcpAccess::Account => connector.open("/tcp", &query).await,
        TcpAccess::Secret(s) => connector.open_with_bearer("/tcp", &query, s).await,
    };
    let error = match ws {
        Err(e) => Some(e.to_string()),
        Ok(ws) => {
            let (sink, mut src) = ws.split();
            let (out, out_rx) = mpsc::channel::<Message>(256);
            let writer = spawn_writer(sink, out_rx);
            let (tx, rx) = mpsc::unbounded_channel::<Frame>();
            let credit = Arc::new(Semaphore::new(DEFAULT_WINDOW as usize));
            let credit_in = credit.clone();
            let out_hb = out.clone();
            // Relay → local: route the consumer socket's frames into the bridge.
            let pump = async move {
                let mut hb = tokio::time::interval(Duration::from_secs(25));
                hb.tick().await;
                loop {
                    tokio::select! {
                        _ = hb.tick() => { let _ = out_hb.try_send(Message::Text(HEARTBEAT_PING.into())); }
                        m = src.next() => match m {
                            Some(Ok(Message::Binary(b))) => {
                                let Ok(f) = Frame::decode_bytes(b) else { break };
                                if f.stream != TCP_CONSUMER_STREAM {
                                    continue;
                                }
                                match f.kind {
                                    ft::TCP_DATA | ft::TCP_CLOSE => {
                                        let close = f.kind == ft::TCP_CLOSE;
                                        let _ = tx.send(f);
                                        if close {
                                            break;
                                        }
                                    }
                                    ft::WINDOW => {
                                        if let Ok(n) = f.parse_u32() {
                                            credit_in.add_permits((n as usize).min(64 * 1024 * 1024));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            Some(Ok(_)) => {}
                        }
                    }
                }
                // Dropping `tx` ends the bridge's writer half.
            };
            let pump = tokio::spawn(pump);
            let err = bridge(sock, TCP_CONSUMER_STREAM, out.clone(), rx, credit, DEFAULT_MAX_FRAME, counters.clone()).await;
            let _ = out.send(Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "closed".into() }))).await;
            drop(out);
            pump.abort();
            let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
            err.filter(|e| e != "relay connection ended")
        }
    };
    let log = TcpConnLog {
        stream: 0,
        at_ms,
        client_ip: Some(peer.to_string()),
        via: "local".into(),
        duration_ms: started.elapsed().as_millis() as u64,
        bytes_in: counters.bytes_in.load(Ordering::Relaxed),
        bytes_out: counters.bytes_out.load(Ordering::Relaxed),
        error: error.clone(),
    };
    shared.update(|s| {
        s.connections = s.connections.saturating_sub(1);
        s.bytes_in += log.bytes_in;
        s.bytes_out += log.bytes_out;
        if error.is_some() {
            s.last_error.clone_from(&error);
        }
    });
    let _ = shared.events.send(TcpConnectEvent::Connection(log));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_random_and_hashed() {
        let a = generate_access_secret();
        let b = generate_access_secret();
        assert!(a.starts_with(SECRET_PREFIX) && a.len() == 45 && a != b);
        assert_eq!(secret_sha256("dtcp_x"), hex::encode(Sha256::digest(b"dtcp_x")));
        assert_eq!(secret_sha256("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
