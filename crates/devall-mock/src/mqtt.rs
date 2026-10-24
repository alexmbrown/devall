//! An embedded MQTT 3.1.1 / 5 broker with scripted behaviours, built on rumqttc's packet codec
//! (rumqttd would pull in axum, config, metrics and tracing-subscriber for little gain).
//!
//! It routes PUBLISH (QoS 0/1/2, `+`/`#` filters, retained messages) between clients like a normal
//! broker, and plays the behaviours in `mocks/mqtt/*.json` (one object or an array):
//!
//! ```json
//! {
//!   "name": "Sensors",
//!   "retained": [ { "topic": "devices/1/state", "payload": "on", "qos": 1 } ],
//!   "onSubscribe": [ { "topic": "devices/+/config", "publish": [ { "topic": "devices/1/config", "json": { "rate": 5 } } ] } ],
//!   "periodic": [ { "intervalMs": 1000, "count": 0, "publish": { "topic": "sensors/temp", "payload": "{{randomInt lower=18 upper=25}}" } } ],
//!   "rules": [ { "topic": "rpc/request/+", "payload": { "matchesJsonPath": "$.id" },
//!                "publish": [ { "topic": "rpc/response/{{topicSegments.[2]}}", "json": { "ok": true, "id": "{{jsonPath message.payload '$.id'}}" }, "delayMs": 50 } ] } ]
//! }
//! ```
//!
//! `onSubscribe` fires when a client subscribes with a filter equal to `topic` (or one that
//! `topic` falls under) and delivers to that client only; `periodic` publishes to everyone;
//! `rules` answer a client's PUBLISH whose topic matches the `topic` filter (and `payload`
//! matcher). Payloads (`payload` text or `json`) are response templates with `topic`,
//! `topicSegments`, `message.payload` / `message.json` and `clientId`. Not supported: sessions
//! that survive a reconnect (clean session only), wills, authentication (any credentials are
//! accepted), shared subscriptions, MQTT over WebSocket.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use bytes::BytesMut;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::error::MockError;
use crate::files::{LoadError, load_json_defs};
use crate::matchers::ValuePattern;
use crate::messages::{Direction, Protocol, body_of};
use crate::server::ServerState;

pub const MQTT_DIR: &str = "mqtt";
const MAX_PACKET: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Msg {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

/// A publish in a behaviour (payload template, delay).
#[derive(Debug, Clone)]
struct PublishDef {
    topic: String,
    payload: String,
    qos: u8,
    retain: bool,
    delay_ms: u64,
}

impl PublishDef {
    fn from_json(v: &Value, what: &str) -> Result<Self, String> {
        let topic = v.get("topic").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or(format!("{what}.topic is required"))?.to_string();
        if topic.contains(['+', '#']) {
            return Err(format!("{what}.topic {topic} must not contain wildcards"));
        }
        let payload = match (v.get("payload"), v.get("json")) {
            (Some(Value::String(s)), _) => s.clone(),
            (Some(other), _) => other.to_string(),
            (None, Some(j)) => j.to_string(),
            (None, None) => String::new(),
        };
        let qos = v.get("qos").and_then(Value::as_u64).unwrap_or(0);
        if qos > 2 {
            return Err(format!("{what}.qos must be 0, 1 or 2"));
        }
        Ok(Self { topic, payload, qos: qos as u8, retain: v.get("retain").and_then(Value::as_bool).unwrap_or(false), delay_ms: v.get("delayMs").and_then(Value::as_u64).unwrap_or(0) })
    }

    fn render(&self, st: &ServerState, model: &Value) -> Msg {
        let r = |s: &str| if s.contains("{{") { st.templates.render(s, model).unwrap_or_else(|e| e) } else { s.to_string() };
        Msg { topic: r(&self.topic), payload: r(&self.payload).into_bytes(), qos: self.qos, retain: self.retain }
    }
}

fn publish_list(v: Option<&Value>, what: &str) -> Result<Vec<PublishDef>, String> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().enumerate().map(|(i, p)| PublishDef::from_json(p, &format!("{what}[{i}]"))).collect(),
        Some(one) => Ok(vec![PublishDef::from_json(one, what)?]),
    }
}

#[derive(Debug, Clone)]
struct OnSubscribe {
    topic: String,
    publish: Vec<PublishDef>,
    source: String,
}

#[derive(Debug, Clone)]
struct Periodic {
    interval_ms: u64,
    initial_delay_ms: u64,
    count: u64,
    publish: PublishDef,
    source: String,
}

#[derive(Debug, Clone)]
struct Rule {
    topic: String,
    payload: Option<ValuePattern>,
    publish: Vec<PublishDef>,
    source: String,
}

#[derive(Debug, Default)]
struct Behaviours {
    retained: Vec<PublishDef>,
    on_subscribe: Vec<OnSubscribe>,
    periodic: Vec<Periodic>,
    rules: Vec<Rule>,
    raw: Vec<Value>,
}

fn valid_filter(f: &str) -> bool {
    !f.is_empty() && f.split('/').enumerate().all(|(i, level)| match level {
        "#" => i == f.split('/').count() - 1,
        "+" => true,
        l => !l.contains(['+', '#']),
    })
}

impl Behaviours {
    fn add(&mut self, v: &Value, file: &str) -> Result<(), String> {
        let o = v.as_object().ok_or("an MQTT behaviour must be an object")?;
        let name = o.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| file.to_string());
        self.retained.extend(publish_list(o.get("retained"), "retained")?);
        for (i, s) in o.get("onSubscribe").and_then(Value::as_array).into_iter().flatten().enumerate() {
            let topic = s.get("topic").and_then(Value::as_str).filter(|t| valid_filter(t)).ok_or(format!("onSubscribe[{i}].topic must be a topic filter"))?.to_string();
            self.on_subscribe.push(OnSubscribe { topic, publish: publish_list(s.get("publish"), &format!("onSubscribe[{i}].publish"))?, source: name.clone() });
        }
        for (i, p) in o.get("periodic").and_then(Value::as_array).into_iter().flatten().enumerate() {
            let interval_ms = p.get("intervalMs").and_then(Value::as_u64).filter(|n| *n >= 10).ok_or(format!("periodic[{i}].intervalMs must be ≥ 10"))?;
            self.periodic.push(Periodic {
                interval_ms,
                initial_delay_ms: p.get("initialDelayMs").and_then(Value::as_u64).unwrap_or(interval_ms),
                count: p.get("count").and_then(Value::as_u64).unwrap_or(0),
                publish: PublishDef::from_json(p.get("publish").ok_or(format!("periodic[{i}].publish is required"))?, &format!("periodic[{i}].publish"))?,
                source: name.clone(),
            });
        }
        for (i, r) in o.get("rules").and_then(Value::as_array).into_iter().flatten().enumerate() {
            let topic = r.get("topic").and_then(Value::as_str).filter(|t| valid_filter(t)).ok_or(format!("rules[{i}].topic must be a topic filter"))?.to_string();
            let payload = match r.get("payload") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(ValuePattern::from_json(&json!({ "equalTo": s }))?),
                Some(p) => Some(ValuePattern::from_json(p).map_err(|e| format!("rules[{i}].payload: {e}"))?),
            };
            self.rules.push(Rule { topic, payload, publish: publish_list(r.get("publish"), &format!("rules[{i}].publish"))?, source: name.clone() });
        }
        let mut raw = v.clone();
        if let Value::Object(m) = &mut raw {
            m.insert("file".into(), json!(file));
        }
        self.raw.push(raw);
        Ok(())
    }
}

struct Client {
    client_id: String,
    tx: mpsc::UnboundedSender<Msg>,
    subs: Vec<(String, u8)>,
    connected_at: i64,
    peer: String,
}

#[derive(Default)]
struct Broker {
    next_id: usize,
    clients: HashMap<usize, Client>,
    retained: BTreeMap<String, Msg>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MqttClientInfo {
    pub client_id: String,
    pub subscriptions: Vec<String>,
    pub connected_at: i64,
    pub peer: String,
}

pub(crate) struct Listener {
    pub addr: SocketAddr,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
    periodic: JoinSet<()>,
}

#[derive(Default)]
pub(crate) struct MqttState {
    behaviours: RwLock<Arc<Behaviours>>,
    pub errors: RwLock<Vec<LoadError>>,
    broker: Mutex<Broker>,
    pub listener: Mutex<Option<Listener>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl MqttState {
    pub fn reload(&self, root: &std::path::Path) -> Vec<LoadError> {
        let (defs, mut errors) = load_json_defs(root, MQTT_DIR, "behaviours", &[]);
        let mut b = Behaviours::default();
        for d in defs {
            if let Err(e) = b.add(&d.value, &d.file) {
                errors.push(LoadError { file: d.file.clone(), error: if let Some(i) = d.index { format!("behaviour #{}: {e}", i + 1) } else { e } });
            }
        }
        *self.behaviours.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(b);
        *self.errors.write().unwrap_or_else(|e| e.into_inner()) = errors.clone();
        errors
    }

    pub fn has_behaviours(&self) -> bool {
        let b = self.behaviours.read().unwrap_or_else(|e| e.into_inner());
        !b.raw.is_empty()
    }

    pub fn port(&self) -> Option<u16> {
        lock(&self.listener).as_ref().map(|l| l.addr.port())
    }

    pub fn behaviours_json(&self) -> Vec<Value> {
        self.behaviours.read().unwrap_or_else(|e| e.into_inner()).raw.clone()
    }

    pub fn clients(&self) -> Vec<MqttClientInfo> {
        let b = lock(&self.broker);
        let mut out: Vec<MqttClientInfo> = b
            .clients
            .values()
            .map(|c| MqttClientInfo { client_id: c.client_id.clone(), subscriptions: c.subs.iter().map(|(f, q)| format!("{f} (QoS {q})")).collect(), connected_at: c.connected_at, peer: c.peer.clone() })
            .collect();
        out.sort_by_key(|c| c.connected_at);
        out
    }

    pub fn retained(&self) -> Vec<Value> {
        lock(&self.broker)
            .retained
            .values()
            .map(|m| {
                let (body, binary) = body_of(&m.payload);
                json!({ "topic": m.topic, "payload": body, "binary": binary, "qos": m.qos })
            })
            .collect()
    }

    fn register(&self, client_id: String, peer: String, tx: mpsc::UnboundedSender<Msg>) -> usize {
        let mut b = lock(&self.broker);
        b.next_id += 1;
        let id = b.next_id;
        b.clients.insert(id, Client { client_id, tx, subs: Vec::new(), connected_at: crate::messages::now_ms(), peer });
        id
    }

    /// Adds a subscription and delivers matching retained messages.
    fn subscribe(&self, id: usize, filter: &str, qos: u8) {
        let mut b = lock(&self.broker);
        let retained: Vec<Msg> = b.retained.values().filter(|m| rumqttc::matches(&m.topic, filter)).cloned().collect();
        if let Some(c) = b.clients.get_mut(&id) {
            c.subs.retain(|(f, _)| f != filter);
            c.subs.push((filter.to_string(), qos));
            for mut m in retained {
                m.qos = m.qos.min(qos);
                m.retain = true;
                let _ = c.tx.send(m);
            }
        }
    }

    fn unsubscribe(&self, id: usize, filter: &str) {
        if let Some(c) = lock(&self.broker).clients.get_mut(&id) {
            c.subs.retain(|(f, _)| f != filter);
        }
    }

    /// Routes a message to every matching subscription (and stores it when retained).
    pub(crate) fn route(&self, msg: &Msg) -> usize {
        let mut b = lock(&self.broker);
        if msg.retain {
            if msg.payload.is_empty() {
                b.retained.remove(&msg.topic);
            } else {
                b.retained.insert(msg.topic.clone(), msg.clone());
            }
        }
        let mut n = 0;
        for c in b.clients.values() {
            if let Some(q) = c.subs.iter().filter(|(f, _)| rumqttc::matches(&msg.topic, f)).map(|(_, q)| *q).max() {
                let _ = c.tx.send(Msg { qos: msg.qos.min(q), retain: false, ..msg.clone() });
                n += 1;
            }
        }
        n
    }

    fn deliver_to(&self, id: usize, msg: Msg) {
        if let Some(c) = lock(&self.broker).clients.get(&id) {
            let _ = c.tx.send(msg);
        }
    }

    fn drop_client(&self, id: usize) {
        lock(&self.broker).clients.remove(&id);
    }
}

fn topic_model(topic: &str, payload: &[u8], client_id: &str) -> Value {
    let (text, _) = body_of(payload);
    let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
    json!({
        "topic": topic,
        "topicSegments": topic.split('/').collect::<Vec<_>>(),
        "message": { "payload": text, "json": parsed },
        "clientId": client_id,
    })
}

fn journal_msg(st: &ServerState, client: &str, dir: Direction, m: &Msg, source: Option<&str>, extra: Value) {
    let (body, binary) = body_of(&m.payload);
    let mut meta = json!({ "qos": m.qos, "retain": m.retain });
    if let (Value::Object(a), Value::Object(b)) = (&mut meta, extra) {
        a.extend(b);
    }
    st.messages.record(Protocol::Mqtt, client, dir, &m.topic, body, binary, source, meta);
}

/// Publishes a broker-generated message (behaviours, admin API) and journals it.
pub(crate) fn publish(st: &ServerState, m: Msg, source: Option<&str>) -> usize {
    let n = st.mqtt.route(&m);
    journal_msg(st, "broker", Direction::Out, &m, source, json!({ "delivered": n }));
    n
}

fn schedule_publish(st: &Arc<ServerState>, m: Msg, delay_ms: u64, source: String) {
    let st = st.clone();
    tokio::spawn(async move {
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        publish(&st, m, Some(&source));
    });
}

fn start_periodic(st: &Arc<ServerState>, set: &mut JoinSet<()>) {
    let b = st.mqtt.behaviours.read().unwrap_or_else(|e| e.into_inner()).clone();
    for r in &b.retained {
        let m = r.render(st, &json!({}));
        st.mqtt.route(&Msg { retain: true, ..m });
    }
    for p in b.periodic.clone() {
        let st = st.clone();
        set.spawn(async move {
            tokio::time::sleep(Duration::from_millis(p.initial_delay_ms)).await;
            let mut sent = 0u64;
            loop {
                let model = json!({ "topic": p.publish.topic, "count": sent });
                publish(&st, p.publish.render(&st, &model), Some(&p.source));
                sent += 1;
                if p.count > 0 && sent >= p.count {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(p.interval_ms)).await;
            }
        });
    }
}

/// Re-applies retained messages and restarts periodic publishes after a reload.
pub(crate) fn restart_behaviours(st: &Arc<ServerState>) {
    let mut guard = lock(&st.mqtt.listener);
    if let Some(l) = guard.as_mut() {
        l.periodic.abort_all();
        start_periodic(st, &mut l.periodic);
    }
}

pub(crate) async fn start(st: &Arc<ServerState>, port: u16) -> Result<u16, MockError> {
    if let Some(p) = st.mqtt.port()
        && (port == 0 || port == p)
    {
        return Ok(p);
    }
    stop(st).await;
    let ip = if st.config.bind_all { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let addr = SocketAddr::new(ip, port);
    let listener = TcpListener::bind(addr).await.map_err(|source| MockError::Bind { addr: addr.to_string(), source })?;
    let local = listener.local_addr().map_err(|e| MockError::Invalid(e.to_string()))?;
    let (tx, rx) = watch::channel(false);
    *lock(&st.mqtt.broker) = Broker::default();
    let mut periodic = JoinSet::new();
    start_periodic(st, &mut periodic);
    let task = tokio::spawn(accept_loop(listener, st.clone(), rx));
    *lock(&st.mqtt.listener) = Some(Listener { addr: local, shutdown: tx, task, periodic });
    st.messages.record(Protocol::Mqtt, "broker", Direction::Event, "", String::new(), false, None, json!({ "event": "listening", "port": local.port() }));
    Ok(local.port())
}

pub(crate) async fn stop(st: &ServerState) {
    let l = lock(&st.mqtt.listener).take();
    if let Some(mut l) = l {
        l.periodic.abort_all();
        let _ = l.shutdown.send(true);
        let _ = l.task.await;
    }
}

async fn accept_loop(listener: TcpListener, st: Arc<ServerState>, mut shutdown: watch::Receiver<bool>) {
    let mut conns = JoinSet::new();
    loop {
        tokio::select! {
            res = listener.accept() => match res {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    conns.spawn(handle(stream, peer, st.clone(), shutdown.clone()));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            },
            _ = shutdown.changed() => break,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
    drop(listener);
    conns.abort_all();
    while conns.join_next().await.is_some() {}
}

fn insufficient(e: &dyn std::fmt::Debug) -> bool {
    format!("{e:?}").starts_with("InsufficientBytes")
}

/// Protocol level byte of the CONNECT packet at the start of `buf`, once enough bytes arrived.
fn protocol_level(buf: &[u8]) -> Option<u8> {
    let mut i = 1;
    while i < buf.len() && buf[i] & 0x80 != 0 {
        i += 1;
    }
    buf.get(i + 1 + 6).copied()
}

/// What the connection loop needs to know about a client.
struct Session {
    id: usize,
    client_id: String,
}

async fn handle(mut stream: TcpStream, peer: SocketAddr, st: Arc<ServerState>, mut shutdown: watch::Receiver<bool>) {
    let mut buf = BytesMut::new();
    let level = loop {
        if let Some(l) = protocol_level(&buf) {
            break l;
        }
        tokio::select! {
            n = stream.read_buf(&mut buf) => if n.unwrap_or(0) == 0 { return },
            _ = tokio::time::sleep(Duration::from_secs(10)) => return,
            _ = shutdown.changed() => return,
        }
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut session: Option<Session> = None;
    let result = if level == 5 {
        serve_v5(&mut stream, &mut buf, &mut rx, &tx, &st, &mut session, peer, &mut shutdown).await
    } else {
        serve_v4(&mut stream, &mut buf, &mut rx, &tx, &st, &mut session, peer, &mut shutdown).await
    };
    if let Some(s) = session {
        st.mqtt.drop_client(s.id);
        st.messages.record(Protocol::Mqtt, &s.client_id, Direction::Event, "", String::new(), false, None, json!({ "event": "disconnect", "reason": result }));
    }
}

/// Behaviour reactions to a client's PUBLISH.
fn on_client_publish(st: &Arc<ServerState>, client_id: &str, m: &Msg) {
    journal_msg(st, client_id, Direction::In, m, None, Value::Null);
    st.mqtt.route(m);
    let b = st.mqtt.behaviours.read().unwrap_or_else(|e| e.into_inner()).clone();
    for r in b.rules.iter().filter(|r| rumqttc::matches(&m.topic, &r.topic)) {
        if r.payload.as_ref().is_some_and(|p| !p.eval_bytes(&m.payload, false).matched) {
            continue;
        }
        let model = topic_model(&m.topic, &m.payload, client_id);
        for p in &r.publish {
            schedule_publish(st, p.render(st, &model), p.delay_ms, r.source.clone());
        }
    }
}

fn on_subscribe(st: &Arc<ServerState>, s: &Session, filter: &str, qos: u8) {
    st.mqtt.subscribe(s.id, filter, qos);
    st.messages.record(Protocol::Mqtt, &s.client_id, Direction::Event, filter, String::new(), false, None, json!({ "event": "subscribe", "qos": qos }));
    let b = st.mqtt.behaviours.read().unwrap_or_else(|e| e.into_inner()).clone();
    for os in b.on_subscribe.iter().filter(|o| o.topic == filter || (!o.topic.contains(['+', '#']) && rumqttc::matches(&o.topic, filter))) {
        let model = topic_model(filter, b"", &s.client_id);
        for p in &os.publish {
            let m = p.render(st, &model);
            let (st2, id, delay, source, client) = (st.clone(), s.id, p.delay_ms, os.source.clone(), s.client_id.clone());
            tokio::spawn(async move {
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                journal_msg(&st2, &client, Direction::Out, &m, Some(&source), json!({ "trigger": "subscribe" }));
                if m.retain {
                    st2.mqtt.route(&m);
                }
                st2.mqtt.deliver_to(id, m);
            });
        }
    }
}

fn qos4(q: u8) -> rumqttc::mqttbytes::QoS {
    match q {
        0 => rumqttc::mqttbytes::QoS::AtMostOnce,
        1 => rumqttc::mqttbytes::QoS::AtLeastOnce,
        _ => rumqttc::mqttbytes::QoS::ExactlyOnce,
    }
}

fn qos5(q: u8) -> rumqttc::v5::mqttbytes::QoS {
    match q {
        0 => rumqttc::v5::mqttbytes::QoS::AtMostOnce,
        1 => rumqttc::v5::mqttbytes::QoS::AtLeastOnce,
        _ => rumqttc::v5::mqttbytes::QoS::ExactlyOnce,
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_v4(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
    tx: &mpsc::UnboundedSender<Msg>,
    st: &Arc<ServerState>,
    session: &mut Option<Session>,
    peer: SocketAddr,
    shutdown: &mut watch::Receiver<bool>,
) -> &'static str {
    use rumqttc::mqttbytes::QoS;
    use rumqttc::mqttbytes::v4::*;
    let mut next_pkid = 0u16;
    loop {
        let mut out = BytesMut::new();
        match Packet::read(buf, MAX_PACKET) {
            Ok(p) => {
                let reply = match p {
                    Packet::Connect(c) => {
                        let client_id = if c.client_id.is_empty() { format!("anon-{}", peer.port()) } else { c.client_id.clone() };
                        let id = st.mqtt.register(client_id.clone(), peer.to_string(), tx.clone());
                        st.messages.record(Protocol::Mqtt, &client_id, Direction::Event, "", String::new(), false, None, json!({ "event": "connect", "version": "3.1.1", "peer": peer.to_string() }));
                        *session = Some(Session { id, client_id });
                        vec![Packet::ConnAck(ConnAck::new(ConnectReturnCode::Success, false))]
                    }
                    _ if session.is_none() => return "protocol error: first packet was not CONNECT",
                    Packet::Subscribe(s) => {
                        let codes = s.filters.iter().map(|f| if valid_filter(&f.path) { SubscribeReasonCode::Success(f.qos) } else { SubscribeReasonCode::Failure }).collect();
                        let _ = Packet::SubAck(SubAck::new(s.pkid, codes)).write(&mut out, MAX_PACKET);
                        if stream.write_all(&out).await.is_err() {
                            return "write failed";
                        }
                        out.clear();
                        if let Some(sess) = session.as_ref() {
                            for f in s.filters.iter().filter(|f| valid_filter(&f.path)) {
                                on_subscribe(st, sess, &f.path, f.qos as u8);
                            }
                        }
                        vec![]
                    }
                    Packet::Publish(p) => {
                        let m = Msg { topic: p.topic.clone(), payload: p.payload.to_vec(), qos: p.qos as u8, retain: p.retain };
                        if let Some(sess) = session.as_ref() {
                            on_client_publish(st, &sess.client_id, &m);
                        }
                        match p.qos {
                            QoS::AtMostOnce => vec![],
                            QoS::AtLeastOnce => vec![Packet::PubAck(PubAck::new(p.pkid))],
                            QoS::ExactlyOnce => vec![Packet::PubRec(PubRec::new(p.pkid))],
                        }
                    }
                    Packet::PubRel(r) => vec![Packet::PubComp(PubComp::new(r.pkid))],
                    Packet::PubRec(r) => vec![Packet::PubRel(PubRel::new(r.pkid))],
                    Packet::PingReq => vec![Packet::PingResp],
                    Packet::Unsubscribe(u) => {
                        if let Some(sess) = session.as_ref() {
                            for t in &u.topics {
                                st.mqtt.unsubscribe(sess.id, t);
                                st.messages.record(Protocol::Mqtt, &sess.client_id, Direction::Event, t, String::new(), false, None, json!({ "event": "unsubscribe" }));
                            }
                        }
                        vec![Packet::UnsubAck(UnsubAck::new(u.pkid))]
                    }
                    Packet::Disconnect => return "client disconnected",
                    _ => vec![],
                };
                for pk in reply {
                    let _ = pk.write(&mut out, MAX_PACKET);
                }
                if !out.is_empty() && stream.write_all(&out).await.is_err() {
                    return "write failed";
                }
            }
            Err(e) if insufficient(&e) => {
                tokio::select! {
                    n = stream.read_buf(buf) => if n.unwrap_or(0) == 0 { return "connection closed" },
                    f = rx.recv() => {
                        let Some(f) = f else { return "broker closed" };
                        let mut p = Publish::new(f.topic, qos4(f.qos), f.payload);
                        p.retain = f.retain;
                        if f.qos > 0 {
                            next_pkid = next_pkid.wrapping_add(1).max(1);
                            p.pkid = next_pkid;
                        }
                        let _ = Packet::Publish(p).write(&mut out, MAX_PACKET);
                        if stream.write_all(&out).await.is_err() {
                            return "write failed";
                        }
                    }
                    _ = shutdown.changed() => return "broker stopped",
                }
            }
            Err(_) => return "malformed packet",
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_v5(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
    tx: &mpsc::UnboundedSender<Msg>,
    st: &Arc<ServerState>,
    session: &mut Option<Session>,
    peer: SocketAddr,
    shutdown: &mut watch::Receiver<bool>,
) -> &'static str {
    use rumqttc::v5::mqttbytes::QoS;
    use rumqttc::v5::mqttbytes::v5::*;
    let max = Some(MAX_PACKET as u32);
    let mut next_pkid = 0u16;
    loop {
        let mut out = BytesMut::new();
        match Packet::read(buf, max) {
            Ok(p) => {
                let reply = match p {
                    Packet::Connect(c, ..) => {
                        let client_id = if c.client_id.is_empty() { format!("anon-{}", peer.port()) } else { c.client_id.clone() };
                        let id = st.mqtt.register(client_id.clone(), peer.to_string(), tx.clone());
                        st.messages.record(Protocol::Mqtt, &client_id, Direction::Event, "", String::new(), false, None, json!({ "event": "connect", "version": "5", "peer": peer.to_string() }));
                        *session = Some(Session { id, client_id });
                        vec![Packet::ConnAck(ConnAck { session_present: false, code: ConnectReturnCode::Success, properties: None })]
                    }
                    _ if session.is_none() => return "protocol error: first packet was not CONNECT",
                    Packet::Subscribe(s) => {
                        let codes = s.filters.iter().map(|f| if valid_filter(&f.path) { SubscribeReasonCode::Success(f.qos) } else { SubscribeReasonCode::TopicFilterInvalid }).collect();
                        let _ = Packet::SubAck(SubAck { pkid: s.pkid, return_codes: codes, properties: None }).write(&mut out, max);
                        if stream.write_all(&out).await.is_err() {
                            return "write failed";
                        }
                        out.clear();
                        if let Some(sess) = session.as_ref() {
                            for f in s.filters.iter().filter(|f| valid_filter(&f.path)) {
                                on_subscribe(st, sess, &f.path, f.qos as u8);
                            }
                        }
                        vec![]
                    }
                    Packet::Publish(p) => {
                        let m = Msg { topic: String::from_utf8_lossy(&p.topic).into_owned(), payload: p.payload.to_vec(), qos: p.qos as u8, retain: p.retain };
                        if let Some(sess) = session.as_ref() {
                            on_client_publish(st, &sess.client_id, &m);
                        }
                        match p.qos {
                            QoS::AtMostOnce => vec![],
                            QoS::AtLeastOnce => vec![Packet::PubAck(PubAck::new(p.pkid, None))],
                            QoS::ExactlyOnce => vec![Packet::PubRec(PubRec::new(p.pkid, None))],
                        }
                    }
                    Packet::PubRel(r) => vec![Packet::PubComp(PubComp::new(r.pkid, None))],
                    Packet::PubRec(r) => vec![Packet::PubRel(PubRel::new(r.pkid, None))],
                    Packet::PingReq(_) => vec![Packet::PingResp(PingResp)],
                    Packet::Unsubscribe(u) => {
                        if let Some(sess) = session.as_ref() {
                            for t in &u.filters {
                                st.mqtt.unsubscribe(sess.id, t);
                                st.messages.record(Protocol::Mqtt, &sess.client_id, Direction::Event, t, String::new(), false, None, json!({ "event": "unsubscribe" }));
                            }
                        }
                        vec![Packet::UnsubAck(UnsubAck { pkid: u.pkid, reasons: vec![UnsubAckReason::Success; u.filters.len()], properties: None })]
                    }
                    Packet::Disconnect(_) => return "client disconnected",
                    _ => vec![],
                };
                for pk in reply {
                    let _ = pk.write(&mut out, max);
                }
                if !out.is_empty() && stream.write_all(&out).await.is_err() {
                    return "write failed";
                }
            }
            Err(e) if insufficient(&e) => {
                tokio::select! {
                    n = stream.read_buf(buf) => if n.unwrap_or(0) == 0 { return "connection closed" },
                    f = rx.recv() => {
                        let Some(f) = f else { return "broker closed" };
                        let mut p = Publish::new(f.topic, qos5(f.qos), f.payload, None);
                        p.retain = f.retain;
                        if f.qos > 0 {
                            next_pkid = next_pkid.wrapping_add(1).max(1);
                            p.pkid = next_pkid;
                        }
                        let _ = Packet::Publish(p).write(&mut out, max);
                        if stream.write_all(&out).await.is_err() {
                            return "write failed";
                        }
                    }
                    _ = shutdown.changed() => return "broker stopped",
                }
            }
            Err(_) => return "malformed packet",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_and_definitions() {
        assert!(valid_filter("a/+/c") && valid_filter("a/#") && valid_filter("#"));
        assert!(!valid_filter("a/#/c") && !valid_filter("a+/b") && !valid_filter(""));
        let mut b = Behaviours::default();
        b.add(&json!({ "rules": [{ "topic": "rpc/+", "publish": [{ "topic": "out", "json": { "a": 1 } }] }], "retained": [{ "topic": "s", "payload": "on", "qos": 1 }] }), "f").unwrap();
        assert_eq!(b.rules[0].publish[0].payload, "{\"a\":1}");
        assert_eq!(b.retained[0].qos, 1);
        assert!(Behaviours::default().add(&json!({ "rules": [{ "topic": "a/#/b" }] }), "f").is_err());
        assert!(Behaviours::default().add(&json!({ "retained": [{ "topic": "a/+", "payload": "x" }] }), "f").is_err());
        assert!(Behaviours::default().add(&json!({ "periodic": [{ "intervalMs": 1, "publish": { "topic": "a" } }] }), "f").is_err());
    }
}
