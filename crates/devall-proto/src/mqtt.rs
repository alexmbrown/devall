//! MQTT 3.1.1 and 5 client on `rumqttc`: TCP, TLS, WebSocket and secure WebSocket transports,
//! QoS 0/1/2, retained messages, last will. TLS uses our rustls config (OS trust store, or no
//! verification when asked).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devall_core::Resolver;
use rumqttc::{Outgoing, TlsConfiguration, Transport};
use serde_json::json;
use tokio::sync::watch;

use crate::error::{ProtoError, Result, chain};
use crate::event::{Direction, EventSink, ProtoEvent};
use crate::model::{MqttFile, MqttVersion};
use crate::net::{self, timeout_ms};
use crate::now_ms;

/// Largest packet we send or accept.
pub const MAX_PACKET_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttTransport {
    Tcp,
    Tls,
    Ws,
    Wss,
}

/// A parsed broker URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerUrl {
    pub transport: MqttTransport,
    pub host: String,
    pub port: u16,
    /// Full URL with explicit port (WebSocket transports connect to this).
    pub url: String,
}

/// `mqtt://` / `tcp://` (1883), `mqtts://` / `ssl://` / `tls://` (8883), `ws://` (80, path
/// `/mqtt`), `wss://` (443, path `/mqtt`). A bare `host:port` is plain TCP.
pub fn parse_broker_url(raw: &str) -> Result<BrokerUrl> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ProtoError::InvalidUrl("broker URL is empty".into()));
    }
    let (scheme, rest) = raw.split_once("://").unwrap_or(("mqtt", raw));
    let (transport, default_port) = match scheme.to_ascii_lowercase().as_str() {
        "mqtt" | "tcp" => (MqttTransport::Tcp, 1883),
        "mqtts" | "ssl" | "tls" => (MqttTransport::Tls, 8883),
        "ws" => (MqttTransport::Ws, 80),
        "wss" => (MqttTransport::Wss, 443),
        other => return Err(ProtoError::InvalidUrl(format!("{raw}: unsupported scheme {other}:// (use mqtt://, mqtts://, ws:// or wss://)"))),
    };
    let u = url::Url::parse(&format!("http://{rest}")).map_err(|e| ProtoError::InvalidUrl(format!("{raw} ({e})")))?;
    let host = net::host_of(&u)?;
    let port = u.port().unwrap_or(default_port);
    let host_part = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    let url = match transport {
        MqttTransport::Ws | MqttTransport::Wss => {
            let path = if u.path() == "/" { "/mqtt".to_string() } else { u.path().to_string() };
            let query = u.query().map(|q| format!("?{q}")).unwrap_or_default();
            format!("{}://{host_part}:{port}{path}{query}", if transport == MqttTransport::Ws { "ws" } else { "wss" })
        }
        MqttTransport::Tcp => format!("mqtt://{host_part}:{port}"),
        MqttTransport::Tls => format!("mqtts://{host_part}:{port}"),
    };
    Ok(BrokerUrl { transport, host, port, url })
}

#[derive(Debug, Clone, PartialEq)]
pub struct LastWill {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

/// Concrete connection settings (every template resolved).
#[derive(Debug, Clone, PartialEq)]
pub struct MqttOptions {
    pub broker: BrokerUrl,
    pub client_id: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub keep_alive: Duration,
    pub clean_session: bool,
    pub version: MqttVersion,
    pub last_will: Option<LastWill>,
    pub verify_tls: bool,
    pub connect_timeout: Duration,
    /// Subscribed right after connecting.
    pub subscriptions: Vec<(String, u8)>,
}

impl MqttOptions {
    pub fn resolve(file: &MqttFile, r: &mut Resolver) -> Result<Self> {
        let client_id = r.resolve(file.client_id.trim());
        let username = r.resolve(&file.username);
        let password = r.resolve(&file.password);
        Ok(Self {
            broker: parse_broker_url(&r.resolve(&file.broker_url))?,
            client_id: if client_id.is_empty() { format!("devall-{:08x}", fastrand::u32(..)) } else { client_id },
            username: (!username.is_empty()).then_some(username),
            password: (!password.is_empty()).then_some(password),
            keep_alive: Duration::from_secs(file.keep_alive),
            clean_session: file.clean_session,
            version: file.mqtt_version,
            last_will: file
                .last_will
                .as_ref()
                .filter(|w| !w.topic.trim().is_empty())
                .map(|w| LastWill { topic: r.resolve(w.topic.trim()), payload: r.resolve(&w.payload).into_bytes(), qos: w.qos.min(2), retain: w.retain }),
            verify_tls: file.verify_tls,
            connect_timeout: timeout_ms(file.connect_timeout_ms),
            subscriptions: file.subscriptions.iter().filter(|s| s.enabled && !s.topic.trim().is_empty()).map(|s| (r.resolve(s.topic.trim()), s.qos.min(2))).collect(),
        })
    }
}

fn check_qos(qos: u8) -> Result<u8> {
    if qos > 2 { Err(ProtoError::Invalid(format!("QoS must be 0, 1 or 2 (got {qos})"))) } else { Ok(qos) }
}

fn qos4(q: u8) -> rumqttc::QoS {
    match q {
        0 => rumqttc::QoS::AtMostOnce,
        1 => rumqttc::QoS::AtLeastOnce,
        _ => rumqttc::QoS::ExactlyOnce,
    }
}

fn qos5(q: u8) -> rumqttc::v5::mqttbytes::QoS {
    use rumqttc::v5::mqttbytes::QoS;
    match q {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        _ => QoS::ExactlyOnce,
    }
}

fn transport(opts: &MqttOptions) -> Result<Transport> {
    let tls = || -> Result<TlsConfiguration> { Ok(TlsConfiguration::Rustls(net::tls_config(opts.verify_tls, &[])?)) };
    Ok(match opts.broker.transport {
        MqttTransport::Tcp => Transport::Tcp,
        MqttTransport::Tls => Transport::Tls(tls()?),
        MqttTransport::Ws => Transport::Ws,
        MqttTransport::Wss => Transport::Wss(tls()?),
    })
}

/// Host argument for rumqttc: WebSocket transports take the whole URL.
fn broker_addr(b: &BrokerUrl) -> String {
    match b.transport {
        MqttTransport::Ws | MqttTransport::Wss => b.url.clone(),
        _ => b.host.clone(),
    }
}

fn keep_alive(d: Duration) -> Duration {
    if d.is_zero() { d } else { d.max(Duration::from_secs(1)) }
}

/// Topic filters of subscribe requests in flight, matched to their SUBACK by packet id.
#[derive(Default)]
struct PendingSubs {
    queued: VecDeque<String>,
    by_pkid: HashMap<u16, String>,
}

enum Client {
    V4(rumqttc::AsyncClient),
    V5(rumqttc::v5::AsyncClient),
}

/// A live MQTT connection. Dropping it stops the event loop.
pub struct MqttSession {
    client: Client,
    open: Arc<AtomicBool>,
    done: watch::Receiver<bool>,
    pending: Arc<Mutex<PendingSubs>>,
    sink: EventSink,
    task: tokio::task::AbortHandle,
}

impl std::fmt::Debug for MqttSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MqttSession").field("open", &self.is_open()).finish()
    }
}

impl Drop for MqttSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn connect_error(e: &dyn std::error::Error) -> ProtoError {
    let msg = chain(e);
    let lower = msg.to_ascii_lowercase();
    if lower.contains("refused, return code") || lower.contains("connectionrefused") {
        ProtoError::Connect(format!("broker refused the connection ({msg})"))
    } else if lower.contains("tls") || lower.contains("certificate") {
        ProtoError::Tls(msg)
    } else {
        ProtoError::Connect(msg)
    }
}

impl MqttSession {
    /// Connects, waits for CONNACK, subscribes to `opts.subscriptions`, then reports everything
    /// to `sink` until disconnected. No automatic reconnects.
    pub async fn connect(opts: MqttOptions, sink: EventSink) -> Result<Self> {
        sink.emit(ProtoEvent::Connecting { at: now_ms(), target: opts.broker.url.clone() });
        let open = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = watch::channel(false);
        let pending = Arc::new(Mutex::new(PendingSubs::default()));
        let transport = transport(&opts)?;
        let timeout = opts.connect_timeout;

        let (client, task) = match opts.version {
            MqttVersion::V311 => {
                let mut o = rumqttc::MqttOptions::new(opts.client_id.clone(), broker_addr(&opts.broker), opts.broker.port);
                o.set_keep_alive(keep_alive(opts.keep_alive))
                    .set_clean_session(opts.clean_session)
                    .set_max_packet_size(MAX_PACKET_BYTES, MAX_PACKET_BYTES)
                    .set_transport(transport);
                if let Some(u) = &opts.username {
                    o.set_credentials(u.clone(), opts.password.clone().unwrap_or_default());
                }
                if let Some(w) = &opts.last_will {
                    o.set_last_will(rumqttc::LastWill::new(w.topic.clone(), w.payload.clone(), qos4(w.qos), w.retain));
                }
                let (client, mut el) = rumqttc::AsyncClient::new(o, 64);
                let connack = tokio::time::timeout(timeout, async {
                    loop {
                        match el.poll().await {
                            Ok(rumqttc::Event::Incoming(rumqttc::Packet::ConnAck(a))) => return Ok(a.session_present),
                            Ok(_) => {}
                            Err(e) => return Err(connect_error(&e)),
                        }
                    }
                })
                .await
                .map_err(|_| ProtoError::Timeout(format!("connecting to {}", opts.broker.url)))??;
                sink.emit(ProtoEvent::Opened {
                    at: now_ms(),
                    detail: Some(format!("MQTT 3.1.1 · client id {} · session present: {connack}", opts.client_id)),
                    headers: Vec::new(),
                });
                open.store(true, Ordering::SeqCst);
                let (s, p, f) = (sink.clone(), pending.clone(), open.clone());
                let task = tokio::spawn(async move {
                    run_v4(el, s, p).await;
                    f.store(false, Ordering::SeqCst);
                    let _ = done_tx.send(true);
                });
                (Client::V4(client), task)
            }
            MqttVersion::V5 => {
                let mut o = rumqttc::v5::MqttOptions::new(opts.client_id.clone(), broker_addr(&opts.broker), opts.broker.port);
                o.set_keep_alive(keep_alive(opts.keep_alive))
                    .set_clean_start(opts.clean_session)
                    .set_max_packet_size(Some(MAX_PACKET_BYTES as u32))
                    .set_transport(transport);
                if let Some(u) = &opts.username {
                    o.set_credentials(u.clone(), opts.password.clone().unwrap_or_default());
                }
                if let Some(w) = &opts.last_will {
                    o.set_last_will(rumqttc::v5::mqttbytes::v5::LastWill::new(w.topic.clone(), w.payload.clone(), qos5(w.qos), w.retain, None));
                }
                let (client, mut el) = rumqttc::v5::AsyncClient::new(o, 64);
                let connack = tokio::time::timeout(timeout, async {
                    loop {
                        match el.poll().await {
                            Ok(rumqttc::v5::Event::Incoming(rumqttc::v5::mqttbytes::v5::Packet::ConnAck(a))) => return Ok(a.session_present),
                            Ok(_) => {}
                            Err(e) => return Err(connect_error(&e)),
                        }
                    }
                })
                .await
                .map_err(|_| ProtoError::Timeout(format!("connecting to {}", opts.broker.url)))??;
                sink.emit(ProtoEvent::Opened {
                    at: now_ms(),
                    detail: Some(format!("MQTT 5 · client id {} · session present: {connack}", opts.client_id)),
                    headers: Vec::new(),
                });
                open.store(true, Ordering::SeqCst);
                let (s, p, f) = (sink.clone(), pending.clone(), open.clone());
                let task = tokio::spawn(async move {
                    run_v5(el, s, p).await;
                    f.store(false, Ordering::SeqCst);
                    let _ = done_tx.send(true);
                });
                (Client::V5(client), task)
            }
        };
        let session = Self { client, open, done, pending, sink, task: task.abort_handle() };
        for (topic, qos) in &opts.subscriptions {
            session.subscribe(topic, *qos).await?;
        }
        Ok(session)
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    /// Queues a PUBLISH and reports it as an outgoing message.
    pub async fn publish(&self, topic: &str, payload: Vec<u8>, qos: u8, retain: bool) -> Result<()> {
        let qos = check_qos(qos)?;
        if topic.is_empty() || !rumqttc::valid_topic(topic) {
            return Err(ProtoError::Invalid(format!("invalid topic {topic:?} (no wildcards when publishing)")));
        }
        if !self.is_open() {
            return Err(ProtoError::Closed);
        }
        let event = ProtoEvent::bytes(Direction::Out, Some(topic.to_string()), &payload, Some(json!({ "qos": qos, "retain": retain })));
        let r = match &self.client {
            Client::V4(c) => c.publish(topic, qos4(qos), retain, payload).await.map_err(|e| chain(&e)),
            Client::V5(c) => c.publish(topic, qos5(qos), retain, payload).await.map_err(|e| chain(&e)),
        };
        r.map_err(|e| ProtoError::Protocol(format!("publish failed: {e}")))?;
        self.sink.emit(event);
        Ok(())
    }

    pub async fn subscribe(&self, filter: &str, qos: u8) -> Result<()> {
        let qos = check_qos(qos)?;
        if filter.is_empty() || !rumqttc::valid_filter(filter) {
            return Err(ProtoError::Invalid(format!("invalid topic filter {filter:?}")));
        }
        if !self.is_open() {
            return Err(ProtoError::Closed);
        }
        self.pending.lock().unwrap().queued.push_back(format!("{filter} (QoS {qos})"));
        let r = match &self.client {
            Client::V4(c) => c.subscribe(filter, qos4(qos)).await.map_err(|e| chain(&e)),
            Client::V5(c) => c.subscribe(filter, qos5(qos)).await.map_err(|e| chain(&e)),
        };
        r.map_err(|e| ProtoError::Protocol(format!("subscribe failed: {e}")))
    }

    pub async fn unsubscribe(&self, filter: &str) -> Result<()> {
        if !self.is_open() {
            return Err(ProtoError::Closed);
        }
        let r = match &self.client {
            Client::V4(c) => c.unsubscribe(filter).await.map_err(|e| chain(&e)),
            Client::V5(c) => c.unsubscribe(filter).await.map_err(|e| chain(&e)),
        };
        r.map_err(|e| ProtoError::Protocol(format!("unsubscribe failed: {e}")))?;
        self.sink.info(format!("unsubscribing from {filter}"));
        Ok(())
    }

    /// Sends DISCONNECT and waits (briefly) for the event loop to stop.
    pub async fn disconnect(&self) {
        let _ = match &self.client {
            Client::V4(c) => c.disconnect().await.map_err(|e| chain(&e)),
            Client::V5(c) => c.disconnect().await.map_err(|e| chain(&e)),
        };
        if tokio::time::timeout(Duration::from_secs(3), self.wait_closed()).await.is_err() {
            self.task.abort();
            self.open.store(false, Ordering::SeqCst);
            self.sink.emit(ProtoEvent::closed(None, Some("disconnected".into())));
        }
    }

    pub async fn wait_closed(&self) {
        let mut rx = self.done.clone();
        let _ = rx.wait_for(|d| *d).await;
    }
}

fn on_outgoing(o: &Outgoing, pending: &Mutex<PendingSubs>) -> bool {
    match o {
        Outgoing::Subscribe(pkid) => {
            let mut p = pending.lock().unwrap();
            if let Some(t) = p.queued.pop_front() {
                p.by_pkid.insert(*pkid, t);
            }
            false
        }
        Outgoing::Disconnect => true,
        _ => false,
    }
}

fn suback_text(pending: &Mutex<PendingSubs>, pkid: u16, codes: Vec<String>, failed: bool) -> (bool, String) {
    let topic = pending.lock().unwrap().by_pkid.remove(&pkid).unwrap_or_else(|| format!("packet {pkid}"));
    if failed { (false, format!("subscription to {topic} rejected: {}", codes.join(", "))) } else { (true, format!("subscribed to {topic}, granted {}", codes.join(", "))) }
}

async fn run_v4(mut el: rumqttc::EventLoop, sink: EventSink, pending: Arc<Mutex<PendingSubs>>) {
    use rumqttc::{Event, Packet, SubscribeReasonCode};
    let reason = loop {
        match el.poll().await {
            Ok(Event::Incoming(Packet::Publish(p))) => sink.emit(ProtoEvent::bytes(
                Direction::In,
                Some(p.topic.clone()),
                &p.payload,
                Some(json!({ "qos": p.qos as u8, "retain": p.retain, "dup": p.dup })),
            )),
            Ok(Event::Incoming(Packet::SubAck(s))) => {
                let failed = s.return_codes.iter().any(|c| matches!(c, SubscribeReasonCode::Failure));
                let codes = s
                    .return_codes
                    .iter()
                    .map(|c| match c {
                        SubscribeReasonCode::Success(q) => format!("QoS {}", *q as u8),
                        SubscribeReasonCode::Failure => "failure".into(),
                    })
                    .collect();
                let (ok, text) = suback_text(&pending, s.pkid, codes, failed);
                if ok { sink.info(text) } else { sink.error(text) }
            }
            Ok(Event::Incoming(Packet::UnsubAck(_))) => sink.info("unsubscribed"),
            Ok(Event::Incoming(Packet::Disconnect)) => break "broker disconnected".to_string(),
            Ok(Event::Outgoing(o)) => {
                if on_outgoing(&o, &pending) {
                    break "disconnected".to_string();
                }
            }
            Ok(_) => {}
            Err(e) => {
                let msg = chain(&e);
                sink.error(msg.clone());
                break msg;
            }
        }
    };
    sink.emit(ProtoEvent::closed(None, Some(reason)));
}

async fn run_v5(mut el: rumqttc::v5::EventLoop, sink: EventSink, pending: Arc<Mutex<PendingSubs>>) {
    use rumqttc::v5::Event;
    use rumqttc::v5::mqttbytes::v5::{Packet, SubscribeReasonCode};
    let reason = loop {
        match el.poll().await {
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let topic = String::from_utf8_lossy(&p.topic).into_owned();
                let mut meta = json!({ "qos": p.qos as u8, "retain": p.retain, "dup": p.dup });
                if let Some(props) = &p.properties {
                    if let Some(ct) = &props.content_type {
                        meta["contentType"] = ct.clone().into();
                    }
                    if !props.user_properties.is_empty() {
                        meta["userProperties"] = props.user_properties.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>().into();
                    }
                    if let Some(rt) = &props.response_topic {
                        meta["responseTopic"] = rt.clone().into();
                    }
                }
                sink.emit(ProtoEvent::bytes(Direction::In, Some(topic), &p.payload, Some(meta)))
            }
            Ok(Event::Incoming(Packet::SubAck(s))) => {
                let failed = s.return_codes.iter().any(|c| !matches!(c, SubscribeReasonCode::Success(_)));
                let codes = s
                    .return_codes
                    .iter()
                    .map(|c| match c {
                        SubscribeReasonCode::Success(q) => format!("QoS {}", *q as u8),
                        other => format!("{other:?}"),
                    })
                    .collect();
                let (ok, text) = suback_text(&pending, s.pkid, codes, failed);
                if ok { sink.info(text) } else { sink.error(text) }
            }
            Ok(Event::Incoming(Packet::UnsubAck(_))) => sink.info("unsubscribed"),
            Ok(Event::Incoming(Packet::Disconnect(d))) => break format!("broker disconnected ({:?})", d.reason_code),
            Ok(Event::Outgoing(o)) => {
                if on_outgoing(&o, &pending) {
                    break "disconnected".to_string();
                }
            }
            Ok(_) => {}
            Err(e) => {
                let msg = chain(&e);
                sink.error(msg.clone());
                break msg;
            }
        }
    };
    sink.emit(ProtoEvent::closed(None, Some(reason)));
}

#[cfg(test)]
mod tests;
