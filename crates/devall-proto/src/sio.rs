//! Socket.IO client (Socket.IO protocol v5 over Engine.IO v4, i.e. Socket.IO 3.x/4.x servers),
//! WebSocket transport only, on our own tokio-tungstenite + rustls plumbing.
//!
//! Implemented here rather than with `rust_socketio`: that crate hard-depends on `native-tls`
//! (OpenSSL on Linux), a second `reqwest` and an old `tokio-tungstenite`, and cannot use our TLS
//! settings ("verify TLS" off, OS trust store). The wire protocol is small:
//! - Engine.IO: `0{open json}` from the server, `2` ping → `3` pong, `1` close, `4<socket.io packet>`;
//! - Socket.IO: `<type>[<attachments>-][/<namespace>,][<ack id>][json]` with types CONNECT(0),
//!   DISCONNECT(1), EVENT(2), ACK(3), CONNECT_ERROR(4), BINARY_EVENT(5), BINARY_ACK(6); binary
//!   attachments follow as separate binary frames.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine;
use devall_core::Resolver;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{self, Message};

use crate::error::{ProtoError, Result, chain};
use crate::event::{Direction, EventSink, ProtoEvent};
use crate::model::{SioFile, resolve_pairs};
use crate::net::timeout_ms;
use crate::now_ms;
use crate::ws::{WsStream, handshake};

/// Concrete connection settings (every template resolved).
#[derive(Debug, Clone, PartialEq)]
pub struct SioOptions {
    /// Engine.IO WebSocket endpoint, e.g. `ws://host:3000/socket.io/?EIO=4&transport=websocket`.
    pub ws_url: String,
    /// `/` or `/admin`.
    pub namespace: String,
    pub auth: Option<Value>,
    pub headers: Vec<(String, String)>,
    pub verify_tls: bool,
    pub connect_timeout: Duration,
    /// Event names to report; empty = all.
    pub listen: Vec<String>,
}

impl SioOptions {
    pub fn resolve(file: &SioFile, r: &mut Resolver) -> Result<Self> {
        let auth_text = r.resolve(file.auth.trim());
        let auth = if auth_text.is_empty() {
            None
        } else {
            let v: Value = serde_json::from_str(&auth_text).map_err(|e| ProtoError::Invalid(format!("auth is not valid JSON: {e}")))?;
            if !v.is_object() {
                return Err(ProtoError::Invalid("auth must be a JSON object".into()));
            }
            Some(v)
        };
        let query = resolve_pairs(&file.query, r);
        let (ws_url, namespace) = engine_url(&r.resolve(file.url.trim()), &r.resolve(file.path.trim()), &query)?;
        Ok(Self {
            ws_url,
            namespace,
            auth,
            headers: resolve_pairs(&file.headers, r),
            verify_tls: file.settings.verify_tls,
            connect_timeout: timeout_ms(file.settings.connect_timeout_ms),
            listen: file.listen.iter().map(|e| r.resolve(e.trim())).filter(|e| !e.is_empty()).collect(),
        })
    }
}

/// Server URL (its path = namespace, like the JS client) + Engine.IO path → WebSocket URL and
/// namespace.
pub fn engine_url(server: &str, path: &str, query: &[(String, String)]) -> Result<(String, String)> {
    let server = server.trim();
    if server.is_empty() {
        return Err(ProtoError::InvalidUrl("URL is empty".into()));
    }
    let with_scheme = if server.contains("://") { server.to_string() } else { format!("http://{server}") };
    let (scheme, rest) = with_scheme.split_once("://").unwrap_or(("http", &with_scheme));
    let ws_scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => return Err(ProtoError::InvalidUrl(format!("{server}: unsupported scheme {other}://"))),
    };
    let mut url = url::Url::parse(&format!("{ws_scheme}://{rest}")).map_err(|e| ProtoError::InvalidUrl(format!("{server} ({e})")))?;
    if url.host().is_none() {
        return Err(ProtoError::InvalidUrl(format!("{server} has no host")));
    }
    let ns = url.path().trim_end_matches('/');
    let namespace = if ns.is_empty() { "/".to_string() } else { ns.to_string() };
    let mut p = if path.is_empty() { crate::model::DEFAULT_SIO_PATH.to_string() } else { path.to_string() };
    if !p.starts_with('/') {
        p.insert(0, '/');
    }
    if !p.ends_with('/') {
        p.push('/');
    }
    let existing: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    url.set_path(&p);
    url.set_query(None);
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("EIO", "4").append_pair("transport", "websocket");
        for (k, v) in existing.iter().chain(query.iter()) {
            q.append_pair(k, v);
        }
    }
    Ok((url.to_string(), namespace))
}

/// A decoded Socket.IO packet.
#[derive(Debug, Clone, PartialEq)]
pub struct SioPacket {
    pub kind: u8,
    pub namespace: String,
    pub id: Option<u64>,
    pub attachments: usize,
    pub data: Option<Value>,
}

pub const CONNECT: u8 = 0;
pub const DISCONNECT: u8 = 1;
pub const EVENT: u8 = 2;
pub const ACK: u8 = 3;
pub const CONNECT_ERROR: u8 = 4;
pub const BINARY_EVENT: u8 = 5;
pub const BINARY_ACK: u8 = 6;

pub fn encode_packet(p: &SioPacket) -> String {
    let mut s = p.kind.to_string();
    if p.kind == BINARY_EVENT || p.kind == BINARY_ACK {
        s.push_str(&format!("{}-", p.attachments));
    }
    if p.namespace != "/" && !p.namespace.is_empty() {
        s.push_str(&p.namespace);
        s.push(',');
    }
    if let Some(id) = p.id {
        s.push_str(&id.to_string());
    }
    if let Some(d) = &p.data {
        s.push_str(&d.to_string());
    }
    s
}

pub fn decode_packet(s: &str) -> Result<SioPacket> {
    let bad = |why: &str| ProtoError::Protocol(format!("invalid Socket.IO packet ({why}): {}", s.chars().take(120).collect::<String>()));
    let mut chars = s.char_indices().peekable();
    let kind = chars.next().and_then(|(_, c)| c.to_digit(10)).ok_or_else(|| bad("type"))? as u8;
    if kind > BINARY_ACK {
        return Err(bad("type"));
    }
    let mut rest = &s[1..];
    let mut attachments = 0;
    if kind == BINARY_EVENT || kind == BINARY_ACK {
        let dash = rest.find('-').ok_or_else(|| bad("attachments"))?;
        attachments = rest[..dash].parse().map_err(|_| bad("attachments"))?;
        rest = &rest[dash + 1..];
    }
    let mut namespace = "/".to_string();
    if rest.starts_with('/') {
        let end = rest.find(',').unwrap_or(rest.len());
        namespace = rest[..end].to_string();
        rest = if end < rest.len() { &rest[end + 1..] } else { "" };
    }
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    let id = if digits > 0 { Some(rest[..digits].parse().map_err(|_| bad("ack id"))?) } else { None };
    rest = &rest[digits..];
    let data = if rest.is_empty() { None } else { Some(serde_json::from_str(rest).map_err(|_| bad("payload"))?) };
    Ok(SioPacket { kind, namespace, id, attachments, data })
}

/// Event arguments from JSON text: an array is spread, anything else is one argument, empty =
/// none.
pub fn parse_args(text: &str) -> Result<Vec<Value>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<Value>(text).map_err(|e| ProtoError::Invalid(format!("arguments are not valid JSON: {e}")))? {
        Value::Array(items) => Ok(items),
        other => Ok(vec![other]),
    }
}

/// Replaces `{"_placeholder":true,"num":n}` with `{"$binary": base64, "size": len}`.
fn fill_placeholders(v: &mut Value, bins: &[Vec<u8>]) {
    match v {
        Value::Object(o) if o.get("_placeholder") == Some(&Value::Bool(true)) => {
            let n = o.get("num").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(b) = bins.get(n) {
                *v = json!({ "$binary": base64::engine::general_purpose::STANDARD.encode(b), "size": b.len() });
            }
        }
        Value::Object(o) => o.values_mut().for_each(|x| fill_placeholders(x, bins)),
        Value::Array(a) => a.iter_mut().for_each(|x| fill_placeholders(x, bins)),
        _ => {}
    }
}

enum Cmd {
    Emit { event: String, args: Vec<Value>, ack: Option<u64> },
    Close,
}

/// A live Socket.IO connection to one namespace. Dropping it disconnects.
pub struct SioSession {
    tx: mpsc::UnboundedSender<Cmd>,
    open: Arc<AtomicBool>,
    done: watch::Receiver<bool>,
    next_ack: AtomicU64,
    pub namespace: String,
    pub sid: Option<String>,
}

impl std::fmt::Debug for SioSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SioSession").field("namespace", &self.namespace).field("open", &self.is_open()).finish()
    }
}

async fn send_text(ws: &mut WsStream, text: String) -> Result<()> {
    ws.send(Message::text(text)).await.map_err(|e| ProtoError::Connect(chain(&e)))
}

impl SioSession {
    /// Opens the Engine.IO session, connects to the namespace (with `auth`) and waits for the
    /// server's CONNECT (or CONNECT_ERROR).
    pub async fn connect(opts: SioOptions, sink: EventSink) -> Result<Self> {
        sink.emit(ProtoEvent::Connecting { at: now_ms(), target: opts.ws_url.clone() });
        let (mut ws, _) = handshake(&opts.ws_url, &opts.headers, &[], opts.verify_tls, opts.connect_timeout).await?;
        let deadline = Instant::now() + opts.connect_timeout;
        let next_text = async |ws: &mut WsStream| -> Result<String> {
            loop {
                let m = tokio::time::timeout_at(deadline, ws.next()).await.map_err(|_| ProtoError::Timeout("Socket.IO handshake".into()))?;
                match m {
                    Some(Ok(Message::Text(t))) => return Ok(t.to_string()),
                    Some(Ok(Message::Close(f))) => {
                        return Err(ProtoError::Protocol(format!("server closed the connection during the handshake{}", f.map(|f| format!(": {}", f.reason)).unwrap_or_default())));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(ProtoError::Connect(chain(&e))),
                    None => return Err(ProtoError::Protocol("connection closed during the handshake".into())),
                }
            }
        };

        // Engine.IO OPEN.
        let open_text = next_text(&mut ws).await?;
        let open: Value = open_text
            .strip_prefix('0')
            .and_then(|j| serde_json::from_str(j).ok())
            .ok_or_else(|| ProtoError::Protocol(format!("expected an Engine.IO open packet, got {:?} (is this a Socket.IO 3/4 server?)", open_text.chars().take(80).collect::<String>())))?;
        let ping_interval = Duration::from_millis(open.get("pingInterval").and_then(Value::as_u64).unwrap_or(25_000));
        let ping_timeout = Duration::from_millis(open.get("pingTimeout").and_then(Value::as_u64).unwrap_or(20_000));
        let eio_sid = open.get("sid").and_then(Value::as_str).unwrap_or_default().to_string();

        // Socket.IO CONNECT to the namespace.
        let connect = SioPacket { kind: CONNECT, namespace: opts.namespace.clone(), id: None, attachments: 0, data: opts.auth.clone() };
        send_text(&mut ws, format!("4{}", encode_packet(&connect))).await?;
        let sid = loop {
            let t = next_text(&mut ws).await?;
            match t.as_bytes().first() {
                Some(b'2') => send_text(&mut ws, format!("3{}", &t[1..])).await?,
                Some(b'4') => {
                    let p = decode_packet(&t[1..])?;
                    if p.namespace != opts.namespace {
                        continue;
                    }
                    match p.kind {
                        CONNECT => break p.data.as_ref().and_then(|d| d.get("sid")).and_then(Value::as_str).map(str::to_string),
                        CONNECT_ERROR => {
                            let msg = p.data.as_ref().map(|d| d.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| d.to_string())).unwrap_or_default();
                            let _ = ws.close(None).await;
                            return Err(ProtoError::Connect(format!("the server refused namespace {}: {msg}", opts.namespace)));
                        }
                        _ => {}
                    }
                }
                Some(b'1') => return Err(ProtoError::Protocol("server closed the session during the handshake".into())),
                _ => {}
            }
        };
        sink.emit(ProtoEvent::Opened {
            at: now_ms(),
            detail: Some(format!(
                "Engine.IO 4 · namespace {} · sid {}",
                opts.namespace,
                sid.as_deref().unwrap_or(eio_sid.as_str())
            )),
            headers: Vec::new(),
        });

        let (tx, rx) = mpsc::unbounded_channel();
        let open_flag = Arc::new(AtomicBool::new(true));
        let (done_tx, done) = watch::channel(false);
        let flag = open_flag.clone();
        let ns = opts.namespace.clone();
        let listen = opts.listen.clone();
        tokio::spawn(async move {
            run(ws, rx, sink, ns, listen, ping_interval + ping_timeout).await;
            flag.store(false, Ordering::SeqCst);
            let _ = done_tx.send(true);
        });
        Ok(Self { tx, open: open_flag, done, next_ack: AtomicU64::new(1), namespace: opts.namespace, sid })
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    /// Emits `event` with `args`; returns the ack id when an acknowledgement was requested.
    pub fn emit(&self, event: &str, args: Vec<Value>, want_ack: bool) -> Result<Option<u64>> {
        if event.trim().is_empty() {
            return Err(ProtoError::Invalid("event name is empty".into()));
        }
        if !self.is_open() {
            return Err(ProtoError::Closed);
        }
        let ack = want_ack.then(|| self.next_ack.fetch_add(1, Ordering::SeqCst));
        self.tx.send(Cmd::Emit { event: event.to_string(), args, ack }).map_err(|_| ProtoError::Closed)?;
        Ok(ack)
    }

    /// Leaves the namespace and closes the connection.
    pub fn close(&self) {
        let _ = self.tx.send(Cmd::Close);
    }

    pub async fn wait_closed(&self) {
        let mut rx = self.done.clone();
        let _ = rx.wait_for(|d| *d).await;
    }
}

/// A binary packet waiting for its attachments.
struct PendingBinary {
    packet: SioPacket,
    bins: Vec<Vec<u8>>,
}

fn report(p: SioPacket, bins: &[Vec<u8>], sink: &EventSink, listen: &[String], warned: &mut HashSet<String>) {
    let mut data = p.data.unwrap_or(Value::Null);
    fill_placeholders(&mut data, bins);
    match p.kind {
        EVENT | BINARY_EVENT => {
            let mut items = match data {
                Value::Array(a) => a,
                other => vec![other],
            };
            let event = if items.is_empty() { String::new() } else { items.remove(0).as_str().map(str::to_string).unwrap_or_default() };
            if !listen.is_empty() && !listen.contains(&event) {
                if warned.insert(event.clone()) {
                    sink.info(format!("received \"{event}\" events — add it to Listen to show them"));
                }
                return;
            }
            let meta = p.id.map(|id| json!({ "ackId": id, "ackRequested": true }));
            sink.emit(ProtoEvent::text(Direction::In, Some(event), Value::Array(items).to_string(), meta));
        }
        ACK | BINARY_ACK => {
            let id = p.id.unwrap_or_default();
            sink.emit(ProtoEvent::text(Direction::In, Some(format!("ack #{id}")), data.to_string(), Some(json!({ "ackId": id }))));
        }
        _ => {}
    }
}

async fn run(ws: WsStream, mut rx: mpsc::UnboundedReceiver<Cmd>, sink: EventSink, namespace: String, listen: Vec<String>, ping_deadline: Duration) {
    let (mut write, mut read) = ws.split();
    let mut last_ping = Instant::now();
    let mut pending: Option<PendingBinary> = None;
    let mut warned = HashSet::new();
    let reason: String = loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(Cmd::Emit { event, args, ack }) => {
                    let mut data = vec![Value::String(event.clone())];
                    data.extend(args.iter().cloned());
                    let p = SioPacket { kind: EVENT, namespace: namespace.clone(), id: ack, attachments: 0, data: Some(Value::Array(data)) };
                    if let Err(e) = write.send(Message::text(format!("4{}", encode_packet(&p)))).await {
                        sink.error(format!("send failed: {}", chain(&e)));
                        break "connection lost".into();
                    }
                    let meta = ack.map(|id| json!({ "ackId": id }));
                    sink.emit(ProtoEvent::text(Direction::Out, Some(event), Value::Array(args).to_string(), meta));
                }
                Some(Cmd::Close) | None => {
                    let p = SioPacket { kind: DISCONNECT, namespace: namespace.clone(), id: None, attachments: 0, data: None };
                    let _ = write.send(Message::text(format!("4{}", encode_packet(&p)))).await;
                    let _ = write.send(Message::Close(None)).await;
                    break "disconnected".into();
                }
            },
            msg = read.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let t = t.as_str();
                    match t.as_bytes().first() {
                        Some(b'2') => {
                            last_ping = Instant::now();
                            if write.send(Message::text(format!("3{}", &t[1..]))).await.is_err() {
                                break "connection lost".into();
                            }
                        }
                        Some(b'1') => break "server closed the session".into(),
                        Some(b'4') => match decode_packet(&t[1..]) {
                            Ok(p) if p.namespace != namespace => {}
                            Ok(p) if p.kind == DISCONNECT => break "server disconnected the namespace".into(),
                            Ok(p) if p.kind == CONNECT_ERROR => sink.error(format!("connect error: {}", p.data.map(|d| d.to_string()).unwrap_or_default())),
                            Ok(p) if p.attachments > 0 => pending = Some(PendingBinary { packet: p, bins: Vec::new() }),
                            Ok(p) => report(p, &[], &sink, &listen, &mut warned),
                            Err(e) => sink.error(e.to_string()),
                        },
                        _ => {}
                    }
                }
                Some(Ok(Message::Binary(b))) => {
                    if let Some(pb) = pending.as_mut() {
                        pb.bins.push(b.to_vec());
                        if pb.bins.len() >= pb.packet.attachments {
                            let pb = pending.take().unwrap();
                            report(pb.packet, &pb.bins, &sink, &listen, &mut warned);
                        }
                    }
                }
                Some(Ok(Message::Close(f))) => break f.map(|f| format!("closed by the server ({}{})", u16::from(f.code), if f.reason.is_empty() { String::new() } else { format!(": {}", f.reason) })).unwrap_or_else(|| "closed by the server".into()),
                Some(Ok(_)) => {}
                Some(Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed)) | None => break "connection closed".into(),
                Some(Err(e)) => {
                    sink.error(chain(&e));
                    break "connection lost".into();
                }
            },
            _ = tokio::time::sleep_until(last_ping + ping_deadline) => break "ping timeout (the server stopped sending pings)".into(),
        }
    };
    sink.emit(ProtoEvent::closed(None, Some(reason)));
}

#[cfg(test)]
mod tests;
