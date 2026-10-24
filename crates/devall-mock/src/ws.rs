//! WebSocket mocks on the HTTP port (`ws://127.0.0.1:<port>/<path>`).
//!
//! Stubs are JSON files in `mocks/websocket/` (one object, an array, or `{"stubs": [...]}`):
//!
//! ```json
//! {
//!   "name": "Chat",
//!   "request": { "urlPath": "/ws/chat", "queryParameters": { "room": { "equalTo": "general" } } },
//!   "onConnect": [ { "text": "welcome {{request.query.user}}" } ],
//!   "messages": [
//!     { "match": { "equalTo": "ping" }, "reply": [ { "text": "pong" } ] },
//!     { "match": { "matchesJsonPath": "$[?(@.type == 'say')]" }, "reply": [ { "json": { "echo": "{{jsonPath message.text '$.text'}}" }, "delayMs": 100 } ], "broadcast": true },
//!     { "match": { "equalTo": "bye" }, "close": { "code": 1000, "reason": "bye" } }
//!   ],
//!   "periodic": [ { "intervalMs": 1000, "count": 5, "message": { "json": { "tick": "{{now}}" } } } ],
//!   "close": { "afterMs": 60000, "code": 1000, "reason": "session over" },
//!   "echo": false
//! }
//! ```
//!
//! `request` is a WireMock request pattern evaluated against the upgrade request. Messages are
//! `{"text"}`, `{"json"}` or `{"binary": "<base64>"}` with an optional `delayMs`; text and JSON are
//! response templates (model: `request`, `message.text`/`message.json`, `connection.id`,
//! `connection.messageCount`). The first matching `messages` rule answers (its `match` is a
//! WireMock value matcher on the frame); `broadcast` sends the replies to every connection of the
//! stub. Compared with WireMock 4's message stubs: no channel/topic abstraction or HTTP-triggered
//! message actions (use `POST /__admin/devall/websocket/send`), and stubs are per endpoint rather
//! than global.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use http::{HeaderMap, Response};
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role};

use crate::files::{LoadError, load_json_defs};
use crate::matchers::ValuePattern;
use crate::messages::{Direction, Protocol, body_of};
use crate::pattern::RequestPattern;
use crate::request::MockRequest;
use crate::server::{RespBody, ServerState, build_response, full};
use crate::template;
use crate::util::{b64_decode, b64_encode, stable_uuid};

pub const WS_DIR: &str = "websocket";

/// A message to send (`text`, `json` or `binary`), optionally delayed.
#[derive(Debug, Clone)]
pub(crate) struct MessageDef {
    text: Option<String>,
    json: Option<Value>,
    binary: Option<Vec<u8>>,
    pub delay_ms: u64,
}

impl MessageDef {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let o = match v {
            Value::String(s) => return Ok(Self { text: Some(s.clone()), json: None, binary: None, delay_ms: 0 }),
            Value::Object(o) => o,
            _ => return Err("a message is a string or {\"text\" | \"json\" | \"binary\", \"delayMs\"}".into()),
        };
        let binary = match o.get("binary") {
            Some(Value::String(b)) => Some(b64_decode(b).ok_or("binary must be base64")?),
            Some(_) => return Err("binary must be a base64 string".into()),
            None => None,
        };
        let m = Self { text: o.get("text").map(|t| t.as_str().map(str::to_string).unwrap_or_else(|| t.to_string())), json: o.get("json").cloned(), binary, delay_ms: o.get("delayMs").and_then(Value::as_u64).unwrap_or(0) };
        if m.text.is_none() && m.json.is_none() && m.binary.is_none() {
            return Err("a message needs text, json or binary".into());
        }
        Ok(m)
    }

    /// The frame to send, templates rendered with `model`.
    pub fn render(&self, st: &ServerState, model: &Value) -> Out {
        if let Some(b) = &self.binary {
            return Out::Binary(b.clone());
        }
        let raw = match (&self.text, &self.json) {
            (Some(t), _) => t.clone(),
            (None, Some(Value::String(s))) => s.clone(),
            (None, Some(j)) => j.to_string(),
            _ => String::new(),
        };
        Out::Text(st.templates.render(&raw, model).unwrap_or_else(|e| e))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CloseDef {
    code: u16,
    reason: String,
    after_ms: Option<u64>,
}

impl CloseDef {
    fn from_json(v: &Value) -> Self {
        Self {
            code: v.get("code").and_then(Value::as_u64).map(|c| c as u16).unwrap_or(1000),
            reason: v.get("reason").and_then(Value::as_str).unwrap_or_default().to_string(),
            after_ms: v.get("afterMs").and_then(Value::as_u64),
        }
    }
}

#[derive(Debug, Clone)]
struct Rule {
    matcher: Option<ValuePattern>,
    reply: Vec<MessageDef>,
    broadcast: bool,
    close: Option<CloseDef>,
}

#[derive(Debug, Clone)]
struct Periodic {
    interval_ms: u64,
    initial_delay_ms: u64,
    count: u64,
    message: MessageDef,
}

#[derive(Debug)]
pub(crate) struct WsStub {
    pub id: String,
    pub name: Option<String>,
    pub file: String,
    request: RequestPattern,
    priority: i64,
    subprotocol: Option<String>,
    on_connect: Vec<MessageDef>,
    rules: Vec<Rule>,
    periodic: Vec<Periodic>,
    close: Option<CloseDef>,
    echo: bool,
    pub raw: Value,
}

fn messages(v: Option<&Value>, what: &str) -> Result<Vec<MessageDef>, String> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().enumerate().map(|(i, m)| MessageDef::from_json(m).map_err(|e| format!("{what}[{i}]: {e}"))).collect(),
        Some(single) => Ok(vec![MessageDef::from_json(single).map_err(|e| format!("{what}: {e}"))?]),
    }
}

impl WsStub {
    pub fn from_json(v: &Value, location: &str, file: &str) -> Result<Self, String> {
        let o = v.as_object().ok_or("a WebSocket stub must be an object")?;
        let mut req = o.get("request").cloned().ok_or("request is required (e.g. {\"urlPath\": \"/ws\"})")?;
        if let Value::Object(r) = &mut req
            && !r.contains_key("method")
        {
            r.insert("method".into(), json!("GET"));
        }
        let rules = match o.get("messages") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .enumerate()
                .map(|(i, r)| -> Result<Rule, String> {
                    Ok(Rule {
                        matcher: match r.get("match") {
                            None | Some(Value::Null) => None,
                            Some(Value::String(s)) => Some(ValuePattern::from_json(&json!({ "equalTo": s }))?),
                            Some(m) => Some(ValuePattern::from_json(m).map_err(|e| format!("messages[{i}].match: {e}"))?),
                        },
                        reply: messages(r.get("reply"), &format!("messages[{i}].reply"))?,
                        broadcast: r.get("broadcast").and_then(Value::as_bool).unwrap_or(false),
                        close: r.get("close").filter(|c| c.is_object()).map(CloseDef::from_json),
                    })
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("messages must be an array of rules".into()),
        };
        let periodic = match o.get("periodic") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .enumerate()
                .map(|(i, p)| -> Result<Periodic, String> {
                    let interval_ms = p.get("intervalMs").and_then(Value::as_u64).filter(|n| *n >= 10).ok_or(format!("periodic[{i}].intervalMs must be ≥ 10"))?;
                    Ok(Periodic {
                        interval_ms,
                        initial_delay_ms: p.get("initialDelayMs").and_then(Value::as_u64).unwrap_or(interval_ms),
                        count: p.get("count").and_then(Value::as_u64).unwrap_or(0),
                        message: MessageDef::from_json(p.get("message").ok_or(format!("periodic[{i}].message is required"))?).map_err(|e| format!("periodic[{i}].message: {e}"))?,
                    })
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("periodic must be an array".into()),
        };
        Ok(Self {
            id: o.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| stable_uuid(location).to_string()),
            name: o.get("name").and_then(Value::as_str).map(str::to_string),
            file: file.to_string(),
            request: RequestPattern::from_json(&req).map_err(|e| format!("request: {e}"))?,
            priority: o.get("priority").and_then(Value::as_i64).unwrap_or(crate::stub::DEFAULT_PRIORITY),
            subprotocol: o.get("subprotocol").and_then(Value::as_str).map(str::to_string),
            on_connect: messages(o.get("onConnect"), "onConnect")?,
            rules,
            periodic,
            close: o.get("close").filter(|c| c.is_object()).map(CloseDef::from_json),
            echo: o.get("echo").and_then(Value::as_bool).unwrap_or(false),
            raw: v.clone(),
        })
    }

    fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.clone())
    }
}

/// An outgoing frame for a connection.
#[derive(Debug, Clone)]
pub(crate) enum Out {
    Text(String),
    Binary(Vec<u8>),
    Close(u16, String),
}

struct Conn {
    tx: mpsc::UnboundedSender<Out>,
    stub: String,
    stub_name: Option<String>,
    path: String,
    opened: i64,
    client: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WsConnectionInfo {
    pub id: String,
    pub path: String,
    pub stub_id: String,
    pub stub_name: Option<String>,
    /// Epoch ms.
    pub opened: i64,
    pub client_ip: String,
}

#[derive(Default)]
pub(crate) struct WsState {
    stubs: RwLock<Vec<Arc<WsStub>>>,
    pub errors: RwLock<Vec<LoadError>>,
    conns: Mutex<HashMap<String, Conn>>,
    next: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl WsState {
    /// Replaces the stubs with `mocks/websocket/**/*.json`.
    pub fn reload(&self, root: &std::path::Path) -> Vec<LoadError> {
        let (defs, mut errors) = load_json_defs(root, WS_DIR, "stubs", &[]);
        let mut stubs = Vec::new();
        for d in defs {
            match WsStub::from_json(&d.value, &d.location(), &d.file) {
                Ok(s) => stubs.push(Arc::new(s)),
                Err(e) => errors.push(LoadError { file: d.file.clone(), error: if d.index.is_some() { format!("stub #{}: {e}", d.index.unwrap_or(0) + 1) } else { e } }),
            }
        }
        // Lower priority number first; file order breaks ties (stable sort).
        stubs.sort_by_key(|s| s.priority);
        *self.stubs.write().unwrap_or_else(|e| e.into_inner()) = stubs;
        *self.errors.write().unwrap_or_else(|e| e.into_inner()) = errors.clone();
        errors
    }

    fn find(&self, req: &MockRequest) -> Option<Arc<WsStub>> {
        self.stubs.read().unwrap_or_else(|e| e.into_inner()).iter().find(|s| s.request.is_match(req)).cloned()
    }

    pub fn stubs_json(&self) -> Vec<Value> {
        self.stubs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|s| json!({ "id": s.id, "name": s.name, "file": s.file, "definition": s.raw }))
            .collect()
    }

    pub fn connections(&self) -> Vec<WsConnectionInfo> {
        let mut out: Vec<WsConnectionInfo> = lock(&self.conns)
            .iter()
            .map(|(id, c)| WsConnectionInfo { id: id.clone(), path: c.path.clone(), stub_id: c.stub.clone(), stub_name: c.stub_name.clone(), opened: c.opened, client_ip: c.client.clone() })
            .collect();
        out.sort_by_key(|c| c.opened);
        out
    }

    /// Sends to one connection (`connection`), to every connection on `path`, or to all.
    pub fn send(&self, connection: Option<&str>, path: Option<&str>, out: &Out) -> usize {
        let conns = lock(&self.conns);
        conns
            .iter()
            .filter(|(id, c)| connection.is_none_or(|x| x == id.as_str()) && path.is_none_or(|p| p == c.path))
            .filter(|(_, c)| c.tx.send(out.clone()).is_ok())
            .count()
    }

    fn broadcast(&self, stub: &str, out: &Out) {
        for c in lock(&self.conns).values().filter(|c| c.stub == stub) {
            let _ = c.tx.send(out.clone());
        }
    }
}

pub(crate) fn is_upgrade(headers: &HeaderMap) -> bool {
    let has = |name: &str, token: &str| headers.get_all(name).iter().any(|v| v.to_str().is_ok_and(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))));
    has("connection", "upgrade") && has("upgrade", "websocket") && headers.contains_key("sec-websocket-key")
}

/// Accepts the upgrade when a WebSocket stub matches (`Err` hands the request back otherwise).
pub(crate) fn accept(st: &Arc<ServerState>, headers: &HeaderMap, req: MockRequest, on_upgrade: OnUpgrade) -> Result<Response<RespBody>, Box<MockRequest>> {
    let Some(stub) = st.ws.find(&req) else { return Err(Box::new(req)) };
    let key = headers.get("sec-websocket-key").map(|v| v.as_bytes().to_vec()).unwrap_or_default();
    if headers.get("sec-websocket-version").and_then(|v| v.to_str().ok()) != Some("13") {
        return Ok(build_response(426, None, &[("Sec-WebSocket-Version".into(), "13".into())], full("WebSocket version 13 required")));
    }
    let mut out_headers = vec![
        ("Upgrade".to_string(), "websocket".to_string()),
        ("Connection".to_string(), "Upgrade".to_string()),
        ("Sec-WebSocket-Accept".to_string(), tokio_tungstenite::tungstenite::handshake::derive_accept_key(&key)),
    ];
    let offered: Vec<String> = headers.get_all("sec-websocket-protocol").iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(',').map(|s| s.trim().to_string())).collect();
    if let Some(p) = &stub.subprotocol
        && offered.iter().any(|o| o == p || p == "*")
    {
        let chosen = if p == "*" { offered[0].clone() } else { p.clone() };
        out_headers.push(("Sec-WebSocket-Protocol".into(), chosen));
    }
    let id = format!("ws-{}", st.ws.next.fetch_add(1, Ordering::SeqCst) + 1);
    tokio::spawn(run(st.clone(), stub, req, on_upgrade, id));
    Ok(build_response(101, None, &out_headers, full(bytes::Bytes::new())))
}

fn schedule(tasks: &mut JoinSet<()>, tx: &mpsc::UnboundedSender<Out>, out: Out, delay_ms: u64) {
    if delay_ms == 0 {
        let _ = tx.send(out);
        return;
    }
    let tx = tx.clone();
    tasks.spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let _ = tx.send(out);
    });
}

async fn run(st: Arc<ServerState>, stub: Arc<WsStub>, req: MockRequest, on_upgrade: OnUpgrade, id: String) {
    let Ok(upgraded) = on_upgrade.await else { return };
    let ws = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let path = req.path.clone();
    lock(&st.ws.conns).insert(id.clone(), Conn { tx: tx.clone(), stub: stub.id.clone(), stub_name: stub.name.clone(), path: path.clone(), opened: crate::messages::now_ms(), client: req.client_ip.clone() });
    let label = stub.label();
    let journal = |dir: Direction, body: String, binary: bool, meta: Value| st.messages.record(Protocol::Websocket, &id, dir, &path, body, binary, Some(&label), meta);
    journal(Direction::Event, String::new(), false, json!({ "event": "open", "url": req.url }));

    let mut base = template::model(&req, &stub.request.path_params(&req), &Value::Null);
    let mut tasks = JoinSet::new();
    let mut count: u64 = 0;
    let model = |base: &mut Value, count: u64, msg: Option<(&str, bool)>| {
        if let Value::Object(o) = base {
            o.insert("connection".into(), json!({ "id": id, "path": path, "messageCount": count }));
            match msg {
                Some((text, binary)) => {
                    let parsed = if binary { Value::Null } else { serde_json::from_str::<Value>(text).unwrap_or(Value::Null) };
                    o.insert("message".into(), json!({ "text": text, "json": parsed, "binary": binary }));
                }
                None => {
                    o.remove("message");
                }
            }
        }
        base.clone()
    };
    let m0 = model(&mut base, 0, None);
    for m in &stub.on_connect {
        schedule(&mut tasks, &tx, m.render(&st, &m0), m.delay_ms);
    }
    for p in &stub.periodic {
        let (tx, st2, p, base2) = (tx.clone(), st.clone(), p.clone(), m0.clone());
        tasks.spawn(async move {
            tokio::time::sleep(Duration::from_millis(p.initial_delay_ms)).await;
            let mut sent = 0;
            loop {
                if tx.send(p.message.render(&st2, &base2)).is_err() {
                    break;
                }
                sent += 1;
                if p.count > 0 && sent >= p.count {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(p.interval_ms)).await;
            }
        });
    }
    if let Some(c) = stub.close.as_ref().filter(|c| c.after_ms.is_some()) {
        schedule(&mut tasks, &tx, Out::Close(c.code, c.reason.clone()), c.after_ms.unwrap_or(0));
    }

    let mut shutdown = st.shutdown.subscribe();
    let mut close_reason = json!({ "event": "close", "by": "client" });
    loop {
        tokio::select! {
            incoming = stream.next() => {
                let (text, binary) = match incoming {
                    Some(Ok(Message::Text(t))) => (t.to_string(), false),
                    Some(Ok(Message::Binary(b))) => (b64_encode(&b), true),
                    Some(Ok(Message::Ping(p))) => {
                        let _ = sink.send(Message::Pong(p)).await;
                        continue;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        close_reason = json!({ "event": "close", "by": "client", "code": frame.as_ref().map(|f| u16::from(f.code)), "reason": frame.as_ref().map(|f| f.reason.to_string()) });
                        let _ = sink.close().await;
                        break;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        close_reason = json!({ "event": "close", "by": "error", "error": e.to_string() });
                        break;
                    }
                    None => break,
                };
                count += 1;
                journal(Direction::In, text.clone(), binary, Value::Null);
                let raw: Vec<u8> = if binary { b64_decode(&text).unwrap_or_default() } else { text.clone().into_bytes() };
                let rule = stub.rules.iter().find(|r| r.matcher.as_ref().is_none_or(|m| m.eval_bytes(&raw, false).matched));
                let m = model(&mut base, count, Some((&text, binary)));
                match rule {
                    Some(r) => {
                        let mut last = 0;
                        for reply in &r.reply {
                            let out = reply.render(&st, &m);
                            if r.broadcast {
                                let (st2, sid, delay) = (st.clone(), stub.id.clone(), reply.delay_ms);
                                tasks.spawn(async move {
                                    tokio::time::sleep(Duration::from_millis(delay)).await;
                                    st2.ws.broadcast(&sid, &out);
                                });
                            } else {
                                schedule(&mut tasks, &tx, out, reply.delay_ms);
                            }
                            last = last.max(reply.delay_ms);
                        }
                        if let Some(c) = &r.close {
                            schedule(&mut tasks, &tx, Out::Close(c.code, c.reason.clone()), last + c.after_ms.unwrap_or(0));
                        }
                    }
                    None if stub.echo => {
                        let _ = tx.send(if binary { Out::Binary(raw) } else { Out::Text(text) });
                    }
                    None => {}
                }
            }
            out = rx.recv() => {
                let Some(out) = out else { break };
                let sent = match &out {
                    Out::Text(t) => {
                        journal(Direction::Out, t.clone(), false, Value::Null);
                        sink.send(Message::text(t.clone())).await
                    }
                    Out::Binary(b) => {
                        let (body, bin) = body_of(b);
                        journal(Direction::Out, body, bin, Value::Null);
                        sink.send(Message::binary(b.clone())).await
                    }
                    Out::Close(code, reason) => {
                        close_reason = json!({ "event": "close", "by": "mock", "code": code, "reason": reason });
                        let _ = sink.send(Message::Close(Some(CloseFrame { code: CloseCode::from(*code), reason: reason.clone().into() }))).await;
                        let _ = sink.close().await;
                        break;
                    }
                };
                if sent.is_err() {
                    break;
                }
            }
            _ = shutdown.changed() => {
                close_reason = json!({ "event": "close", "by": "mock", "code": 1001, "reason": "mock server stopped" });
                let _ = sink.send(Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "mock server stopped".into() }))).await;
                break;
            }
        }
    }
    tasks.abort_all();
    lock(&st.ws.conns).remove(&id);
    journal(Direction::Event, String::new(), false, close_reason);
}
