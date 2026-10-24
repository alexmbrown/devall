//! Tests against a minimal in-process MQTT broker (3.1.1 and 5) built on rumqttc's own packet
//! codec: CONNECT/CONNACK (username `bad` is refused), SUBSCRIBE with `+`/`#`, PUBLISH QoS 0/1/2
//! routing, retained messages, UNSUBSCRIBE, PING. (`rumqttd` was not used: it drags in axum,
//! clap, config, metrics/prometheus and tracing-subscriber just for tests.)

use std::collections::HashMap;

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::*;
use crate::model::{MqttLastWill, MqttSubscription};

const MAX: usize = 1024 * 1024;

#[derive(Clone)]
struct Fwd {
    topic: String,
    payload: Vec<u8>,
    qos: u8,
    retain: bool,
    content_type: Option<String>,
}

#[derive(Default)]
struct Broker {
    next_id: usize,
    subs: Vec<(usize, String, u8)>,
    retained: HashMap<String, Fwd>,
    conns: HashMap<usize, mpsc::UnboundedSender<Fwd>>,
}

type Shared = Arc<Mutex<Broker>>;

impl Broker {
    fn register(b: &Shared, tx: mpsc::UnboundedSender<Fwd>) -> usize {
        let mut g = b.lock().unwrap();
        g.next_id += 1;
        let id = g.next_id;
        g.conns.insert(id, tx);
        id
    }

    fn subscribe(b: &Shared, id: usize, filter: &str, qos: u8) {
        let mut g = b.lock().unwrap();
        g.subs.push((id, filter.to_string(), qos));
        let retained: Vec<Fwd> = g.retained.values().filter(|f| rumqttc::matches(&f.topic, filter)).cloned().collect();
        if let Some(tx) = g.conns.get(&id) {
            for mut f in retained {
                f.qos = f.qos.min(qos);
                let _ = tx.send(f);
            }
        }
    }

    fn unsubscribe(b: &Shared, id: usize, filter: &str) {
        b.lock().unwrap().subs.retain(|(i, f, _)| !(*i == id && f == filter));
    }

    fn publish(b: &Shared, msg: Fwd) {
        let mut g = b.lock().unwrap();
        if msg.retain {
            g.retained.insert(msg.topic.clone(), msg.clone());
        }
        for (id, filter, qos) in &g.subs {
            if rumqttc::matches(&msg.topic, filter)
                && let Some(tx) = g.conns.get(id)
            {
                let _ = tx.send(Fwd { qos: msg.qos.min(*qos), retain: false, ..msg.clone() });
            }
        }
    }

    fn drop_conn(b: &Shared, id: usize) {
        let mut g = b.lock().unwrap();
        g.conns.remove(&id);
        g.subs.retain(|(i, _, _)| *i != id);
    }
}

async fn start_broker() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let broker: Shared = Arc::default();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle(stream, broker.clone()));
        }
    });
    port
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

async fn handle(mut stream: TcpStream, broker: Shared) {
    let mut buf = BytesMut::new();
    let level = loop {
        if let Some(l) = protocol_level(&buf) {
            break l;
        }
        if stream.read_buf(&mut buf).await.unwrap_or(0) == 0 {
            return;
        }
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<Fwd>();
    let id = Broker::register(&broker, tx);
    if level == 5 { serve_v5(&mut stream, &mut buf, &mut rx, &broker, id).await } else { serve_v4(&mut stream, &mut buf, &mut rx, &broker, id).await }
    Broker::drop_conn(&broker, id);
}

async fn serve_v4(stream: &mut TcpStream, buf: &mut BytesMut, rx: &mut mpsc::UnboundedReceiver<Fwd>, broker: &Shared, id: usize) {
    use rumqttc::mqttbytes::v4::*;
    use rumqttc::mqttbytes::QoS;
    let mut next_pkid = 0u16;
    loop {
        let mut out = BytesMut::new();
        match Packet::read(buf, MAX) {
            Ok(p) => {
                let reply = match p {
                    Packet::Connect(c) => {
                        if c.login.as_ref().is_some_and(|l| l.username == "bad") {
                            let _ = Packet::ConnAck(ConnAck::new(ConnectReturnCode::BadUserNamePassword, false)).write(&mut out, MAX);
                            let _ = stream.write_all(&out).await;
                            return;
                        }
                        vec![Packet::ConnAck(ConnAck::new(ConnectReturnCode::Success, false))]
                    }
                    Packet::Subscribe(s) => {
                        let codes = s.filters.iter().map(|f| SubscribeReasonCode::Success(f.qos)).collect();
                        let mut r = vec![Packet::SubAck(SubAck::new(s.pkid, codes))];
                        // Write the SUBACK before retained messages are forwarded.
                        for pk in r.drain(..) {
                            let _ = pk.write(&mut out, MAX);
                        }
                        let _ = stream.write_all(&out).await;
                        out.clear();
                        for f in &s.filters {
                            Broker::subscribe(broker, id, &f.path, f.qos as u8);
                        }
                        r
                    }
                    Packet::Publish(p) => {
                        Broker::publish(broker, Fwd { topic: p.topic.clone(), payload: p.payload.to_vec(), qos: p.qos as u8, retain: p.retain, content_type: None });
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
                        for t in &u.topics {
                            Broker::unsubscribe(broker, id, t);
                        }
                        vec![Packet::UnsubAck(UnsubAck::new(u.pkid))]
                    }
                    Packet::Disconnect => return,
                    _ => vec![],
                };
                for pk in reply {
                    let _ = pk.write(&mut out, MAX);
                }
                if !out.is_empty() && stream.write_all(&out).await.is_err() {
                    return;
                }
            }
            Err(e) if insufficient(&e) => {
                tokio::select! {
                    n = stream.read_buf(buf) => if n.unwrap_or(0) == 0 { return },
                    f = rx.recv() => {
                        let Some(f) = f else { return };
                        let qos = qos4(f.qos);
                        let mut p = Publish::new(f.topic, qos, f.payload);
                        p.retain = f.retain;
                        if f.qos > 0 {
                            next_pkid += 1;
                            p.pkid = next_pkid;
                        }
                        let _ = Packet::Publish(p).write(&mut out, MAX);
                        if stream.write_all(&out).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(_) => return,
        }
    }
}

async fn serve_v5(stream: &mut TcpStream, buf: &mut BytesMut, rx: &mut mpsc::UnboundedReceiver<Fwd>, broker: &Shared, id: usize) {
    use rumqttc::v5::mqttbytes::QoS;
    use rumqttc::v5::mqttbytes::v5::*;
    let max = Some(MAX as u32);
    let mut next_pkid = 0u16;
    loop {
        let mut out = BytesMut::new();
        match Packet::read(buf, max) {
            Ok(p) => {
                let reply = match p {
                    Packet::Connect(..) => vec![Packet::ConnAck(ConnAck { session_present: false, code: ConnectReturnCode::Success, properties: None })],
                    Packet::Subscribe(s) => {
                        let codes = s.filters.iter().map(|f| SubscribeReasonCode::Success(f.qos)).collect();
                        let _ = Packet::SubAck(SubAck { pkid: s.pkid, return_codes: codes, properties: None }).write(&mut out, max);
                        let _ = stream.write_all(&out).await;
                        out.clear();
                        for f in &s.filters {
                            Broker::subscribe(broker, id, &f.path, f.qos as u8);
                        }
                        vec![]
                    }
                    Packet::Publish(p) => {
                        let content_type = p.properties.as_ref().and_then(|pr| pr.content_type.clone());
                        Broker::publish(
                            broker,
                            Fwd { topic: String::from_utf8_lossy(&p.topic).into_owned(), payload: p.payload.to_vec(), qos: p.qos as u8, retain: p.retain, content_type },
                        );
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
                        for t in &u.filters {
                            Broker::unsubscribe(broker, id, t);
                        }
                        vec![Packet::UnsubAck(UnsubAck { pkid: u.pkid, reasons: vec![UnsubAckReason::Success; u.filters.len()], properties: None })]
                    }
                    Packet::Disconnect(_) => return,
                    _ => vec![],
                };
                for pk in reply {
                    let _ = pk.write(&mut out, max);
                }
                if !out.is_empty() && stream.write_all(&out).await.is_err() {
                    return;
                }
            }
            Err(e) if insufficient(&e) => {
                tokio::select! {
                    n = stream.read_buf(buf) => if n.unwrap_or(0) == 0 { return },
                    f = rx.recv() => {
                        let Some(f) = f else { return };
                        let props = f.content_type.map(|ct| PublishProperties { content_type: Some(ct), ..Default::default() });
                        let mut p = Publish::new(f.topic, qos5(f.qos), f.payload, props);
                        p.retain = f.retain;
                        if f.qos > 0 {
                            next_pkid += 1;
                            p.pkid = next_pkid;
                        }
                        let _ = Packet::Publish(p).write(&mut out, max);
                        if stream.write_all(&out).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(_) => return,
        }
    }
}

fn opts(port: u16, version: MqttVersion, subs: &[(&str, u8)]) -> MqttOptions {
    MqttOptions {
        broker: parse_broker_url(&format!("mqtt://127.0.0.1:{port}")).unwrap(),
        client_id: format!("t-{:08x}", fastrand::u32(..)),
        username: None,
        password: None,
        keep_alive: Duration::from_secs(30),
        clean_session: true,
        version,
        last_will: None,
        verify_tls: true,
        connect_timeout: Duration::from_secs(5),
        subscriptions: subs.iter().map(|(t, q)| (t.to_string(), *q)).collect(),
    }
}

/// Next incoming message (skipping info/out events).
async fn next_in(rx: &mut mpsc::UnboundedReceiver<ProtoEvent>) -> (String, String, serde_json::Value) {
    loop {
        let e = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("timed out waiting for a message").expect("sink closed");
        if let ProtoEvent::Message { direction: Direction::In, label, body, meta, .. } = e {
            return (label.unwrap_or_default(), body, meta.unwrap_or_default());
        }
    }
}

async fn wait_info(rx: &mut mpsc::UnboundedReceiver<ProtoEvent>, needle: &str) {
    loop {
        let e = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("timed out waiting for info").expect("sink closed");
        if let ProtoEvent::Info { message, .. } = &e
            && message.contains(needle)
        {
            return;
        }
    }
}

#[test]
fn broker_urls() {
    let b = parse_broker_url("mqtt://broker.local").unwrap();
    assert_eq!((b.transport, b.host.as_str(), b.port), (MqttTransport::Tcp, "broker.local", 1883));
    let b = parse_broker_url("mqtts://b:9999").unwrap();
    assert_eq!((b.transport, b.port), (MqttTransport::Tls, 9999));
    let b = parse_broker_url("ws://b").unwrap();
    assert_eq!((b.transport, b.url.as_str()), (MqttTransport::Ws, "ws://b:80/mqtt"));
    let b = parse_broker_url("wss://b/custom?x=1").unwrap();
    assert_eq!(b.url, "wss://b:443/custom?x=1");
    assert_eq!(broker_addr(&b), "wss://b:443/custom?x=1");
    let b = parse_broker_url("10.0.0.1:1884").unwrap();
    assert_eq!((b.transport, b.port), (MqttTransport::Tcp, 1884));
    assert!(parse_broker_url("http://x").is_err());
    assert!(parse_broker_url("").is_err());
    assert!(matches!(transport(&MqttOptions { broker: parse_broker_url("wss://b").unwrap(), ..opts(1, MqttVersion::V311, &[]) }).unwrap(), Transport::Wss(_)));
}

#[test]
fn resolves_file() {
    let mut f = MqttFile::new("m");
    f.broker_url = "{{broker}}".into();
    f.username = "u".into();
    f.password = "{{secret.P}}".into();
    f.subscriptions = vec![
        MqttSubscription { topic: "a/{{dev}}/#".into(), qos: 1, enabled: true },
        MqttSubscription { topic: "off".into(), qos: 0, enabled: false },
    ];
    f.last_will = Some(MqttLastWill { topic: "status/{{dev}}".into(), payload: "gone".into(), qos: 1, retain: true });
    struct S;
    impl devall_core::SecretLookup for S {
        fn lookup(&self, _: &str) -> Option<String> {
            Some("pa55word".into())
        }
    }
    let mut r = Resolver::new(&S);
    r.push_layer([("broker".to_string(), "mqtts://b".to_string()), ("dev".to_string(), "d1".to_string())].into_iter().collect());
    let o = MqttOptions::resolve(&f, &mut r).unwrap();
    assert!(o.client_id.starts_with("devall-"));
    assert_eq!(o.password.as_deref(), Some("pa55word"));
    assert_eq!(o.subscriptions, vec![("a/d1/#".to_string(), 1)]);
    assert_eq!(o.last_will.unwrap().topic, "status/d1");
    assert!(r.secret_values.contains("pa55word"));
}

#[tokio::test]
async fn v311_publish_subscribe_retain_qos_and_disconnect() {
    let port = start_broker().await;
    let (sink, mut rx) = EventSink::channel();
    let s = MqttSession::connect(opts(port, MqttVersion::V311, &[("devall/#", 2)]), sink).await.unwrap();
    assert!(s.is_open());
    wait_info(&mut rx, "subscribed to devall/# (QoS 2), granted QoS 2").await;

    for qos in [0u8, 1, 2] {
        s.publish("devall/hello", format!("{{\"q\":{qos}}}").into_bytes(), qos, false).await.unwrap();
        let (topic, body, meta) = next_in(&mut rx).await;
        assert_eq!((topic.as_str(), body.clone()), ("devall/hello", format!("{{\"q\":{qos}}}")));
        assert_eq!(meta["qos"], qos);
    }

    // Retained message reaches a later subscriber with the retain flag.
    s.publish("devall/state", b"on".to_vec(), 1, true).await.unwrap();
    let _ = next_in(&mut rx).await;
    let (sink2, mut rx2) = EventSink::channel();
    let s2 = MqttSession::connect(opts(port, MqttVersion::V311, &[]), sink2).await.unwrap();
    s2.subscribe("devall/+", 1).await.unwrap();
    let (topic, body, meta) = next_in(&mut rx2).await;
    assert_eq!((topic.as_str(), body.as_str(), meta["retain"].as_bool()), ("devall/state", "on", Some(true)));

    // Binary payloads are base64.
    s.publish("devall/bin", vec![0xff, 0x00], 0, false).await.unwrap();
    loop {
        if let ProtoEvent::Message { direction: Direction::In, label: Some(l), body, binary, .. } = rx.recv().await.unwrap()
            && l == "devall/bin"
        {
            assert!(binary);
            assert_eq!(body, "/wA=");
            break;
        }
    }

    s2.unsubscribe("devall/+").await.unwrap();
    wait_info(&mut rx2, "unsubscribed").await;

    // Validation.
    assert!(s.publish("devall/#", vec![], 0, false).await.is_err());
    assert!(s.publish("x", vec![], 3, false).await.is_err());
    assert!(s.subscribe("a/#/b", 0).await.is_err());

    s.disconnect().await;
    assert!(!s.is_open());
    let mut closed = false;
    while let Ok(e) = rx.try_recv() {
        closed |= e.is_closed();
    }
    assert!(closed);
    assert!(matches!(s.publish("devall/x", vec![], 0, false).await, Err(ProtoError::Closed)));
}

#[tokio::test]
async fn v5_round_trip_with_properties() {
    let port = start_broker().await;
    let (sink, mut rx) = EventSink::channel();
    let s = MqttSession::connect(opts(port, MqttVersion::V5, &[("v5/#", 1)]), sink).await.unwrap();
    let opened = loop {
        if let ProtoEvent::Opened { detail, .. } = rx.recv().await.unwrap() {
            break detail.unwrap();
        }
    };
    assert!(opened.starts_with("MQTT 5"), "{opened}");
    wait_info(&mut rx, "granted QoS 1").await;
    s.publish("v5/a", b"hi".to_vec(), 1, false).await.unwrap();
    let (topic, body, meta) = next_in(&mut rx).await;
    assert_eq!((topic.as_str(), body.as_str(), meta["qos"].as_u64()), ("v5/a", "hi", Some(1)));
    s.disconnect().await;
    assert!(!s.is_open());
}

#[tokio::test]
async fn refused_and_unreachable_brokers() {
    let port = start_broker().await;
    let mut o = opts(port, MqttVersion::V311, &[]);
    o.username = Some("bad".into());
    let e = MqttSession::connect(o, EventSink::noop()).await.unwrap_err();
    assert!(e.to_string().contains("refused"), "{e}");

    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let e = MqttSession::connect(opts(dead, MqttVersion::V311, &[]), EventSink::noop()).await.unwrap_err();
    assert!(matches!(e, ProtoError::Connect(_)), "{e}");
}
