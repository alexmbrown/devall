//! Packet codec unit tests plus end-to-end tests against a real Socket.IO server implementation
//! (`socketioxide`, served with hyper) on 127.0.0.1:0.

use serde_json::json;
use socketioxide::SocketIo;
use socketioxide::extract::{AckSender, Data, SocketRef, TryData};
use socketioxide::handler::ConnectHandler;
use tokio::net::TcpListener;

use super::*;

#[test]
fn packets_round_trip() {
    let cases = [
        ("0", SioPacket { kind: CONNECT, namespace: "/".into(), id: None, attachments: 0, data: None }),
        ("0/admin,{\"token\":\"x\"}", SioPacket { kind: CONNECT, namespace: "/admin".into(), id: None, attachments: 0, data: Some(json!({ "token": "x" })) }),
        ("2[\"hello\",1]", SioPacket { kind: EVENT, namespace: "/".into(), id: None, attachments: 0, data: Some(json!(["hello", 1])) }),
        ("2/chat,12[\"m\"]", SioPacket { kind: EVENT, namespace: "/chat".into(), id: Some(12), attachments: 0, data: Some(json!(["m"])) }),
        ("312[\"ok\"]", SioPacket { kind: ACK, namespace: "/".into(), id: Some(12), attachments: 0, data: Some(json!(["ok"])) }),
        ("1/admin,", SioPacket { kind: DISCONNECT, namespace: "/admin".into(), id: None, attachments: 0, data: None }),
        (
            "51-[\"f\",{\"_placeholder\":true,\"num\":0}]",
            SioPacket { kind: BINARY_EVENT, namespace: "/".into(), id: None, attachments: 1, data: Some(json!(["f", { "_placeholder": true, "num": 0 }])) },
        ),
    ];
    for (text, packet) in cases {
        assert_eq!(decode_packet(text).unwrap(), packet, "{text}");
        if !text.ends_with(',') {
            assert_eq!(encode_packet(&packet), text);
        }
    }
    assert!(decode_packet("9").is_err());
    assert!(decode_packet("2[oops").is_err());
    assert!(decode_packet("").is_err());
}

#[test]
fn urls_and_args() {
    let (u, ns) = engine_url("http://localhost:3000", "", &[]).unwrap();
    assert_eq!((u.as_str(), ns.as_str()), ("ws://localhost:3000/socket.io/?EIO=4&transport=websocket", "/"));
    let (u, ns) = engine_url("https://api.test/admin?room=a", "/rt", &[("token".into(), "t".into())]).unwrap();
    assert_eq!((u.as_str(), ns.as_str()), ("wss://api.test/rt/?EIO=4&transport=websocket&room=a&token=t", "/admin"));
    assert!(engine_url("ftp://x", "", &[]).is_err());
    assert_eq!(parse_args("").unwrap(), Vec::<Value>::new());
    assert_eq!(parse_args("[1, \"a\"]").unwrap(), vec![json!(1), json!("a")]);
    assert_eq!(parse_args("{\"a\":1}").unwrap(), vec![json!({ "a": 1 })]);
    assert!(parse_args("{").is_err());
    let mut v = json!(["x", { "_placeholder": true, "num": 0 }]);
    fill_placeholders(&mut v, &[vec![1, 2]]);
    assert_eq!(v[1], json!({ "$binary": "AQI=", "size": 2 }));
}

#[test]
fn resolves_file() {
    let mut f = SioFile::new("s");
    f.url = "{{base}}/admin".into();
    f.auth = "{\"token\":\"{{secret.T}}\"}".into();
    f.listen = vec!["a".into(), " ".into()];
    struct S;
    impl devall_core::SecretLookup for S {
        fn lookup(&self, _: &str) -> Option<String> {
            Some("tok-9999".into())
        }
    }
    let mut r = Resolver::new(&S);
    r.push_layer([("base".to_string(), "http://h:1".to_string())].into_iter().collect());
    let o = SioOptions::resolve(&f, &mut r).unwrap();
    assert_eq!(o.namespace, "/admin");
    assert_eq!(o.auth, Some(json!({ "token": "tok-9999" })));
    assert_eq!(o.listen, vec!["a"]);
    f.auth = "[1]".into();
    assert!(SioOptions::resolve(&f, &mut r).is_err());
}

#[derive(Debug)]
struct Denied;
impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid token")
    }
}

async fn admin_guard(TryData(auth): TryData<Value>) -> std::result::Result<(), Denied> {
    match auth {
        Ok(a) if a.get("token").and_then(Value::as_str) == Some("ok") => Ok(()),
        _ => Err(Denied),
    }
}

async fn start_server() -> String {
    let (svc, io) = SocketIo::new_svc();
    io.ns("/", async |s: SocketRef, TryData(auth): TryData<Value>| {
        s.emit("welcome", &json!({ "auth": auth.unwrap_or(Value::Null) })).ok();
        s.on("echo", async |s: SocketRef, Data(data): Data<Value>, ack: AckSender| {
            ack.send(&json!({ "got": data })).ok();
            s.emit("echoed", &data).ok();
        });
        s.on("file", async |s: SocketRef| {
            s.emit("file", &bytes::Bytes::from_static(&[0, 1, 2])).ok();
        });
        s.on("bye", async |s: SocketRef| {
            s.disconnect().ok();
        });
    });
    io.ns("/admin", (async |s: SocketRef| {
        s.emit("hi-admin", &1).ok();
    })
    .with(admin_guard));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let svc = svc.clone();
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(stream), svc).with_upgrades().await;
            });
        }
    });
    format!("http://{addr}")
}

fn opts(server: &str, ns: &str, auth: Option<Value>, listen: &[&str]) -> SioOptions {
    let (ws_url, namespace) = engine_url(&format!("{server}{ns}"), "", &[]).unwrap();
    SioOptions {
        ws_url,
        namespace,
        auth,
        headers: vec![("X-Test".into(), "1".into())],
        verify_tls: true,
        connect_timeout: Duration::from_secs(5),
        listen: listen.iter().map(|s| s.to_string()).collect(),
    }
}

async fn next_in(rx: &mut mpsc::UnboundedReceiver<ProtoEvent>) -> (String, Value, Option<Value>) {
    loop {
        let e = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("timed out").expect("sink closed");
        if let ProtoEvent::Message { direction: Direction::In, label, body, meta, .. } = e {
            return (label.unwrap_or_default(), serde_json::from_str(&body).unwrap(), meta);
        }
    }
}

#[tokio::test]
async fn events_acks_binary_and_server_disconnect() {
    let server = start_server().await;
    let (sink, mut rx) = EventSink::channel();
    let s = SioSession::connect(opts(&server, "", Some(json!({ "token": "abc" })), &[]), sink).await.unwrap();
    assert!(s.is_open());
    assert!(s.sid.is_some());
    let (event, args, _) = next_in(&mut rx).await;
    assert_eq!(event, "welcome");
    assert_eq!(args, json!([{ "auth": { "token": "abc" } }]));

    let ack = s.emit("echo", vec![json!({ "a": 1 })], true).unwrap();
    assert_eq!(ack, Some(1));
    let mut got_ack = false;
    let mut got_echo = false;
    while !(got_ack && got_echo) {
        let (label, args, meta) = next_in(&mut rx).await;
        match label.as_str() {
            "ack #1" => {
                assert_eq!(args, json!([{ "got": { "a": 1 } }]));
                assert_eq!(meta.unwrap()["ackId"], 1);
                got_ack = true;
            }
            "echoed" => {
                assert_eq!(args, json!([{ "a": 1 }]));
                got_echo = true;
            }
            other => panic!("unexpected {other}"),
        }
    }

    s.emit("file", vec![], false).unwrap();
    let (label, args, _) = next_in(&mut rx).await;
    assert_eq!(label, "file");
    assert_eq!(args, json!([{ "$binary": "AAEC", "size": 3 }]));

    assert!(s.emit("  ", vec![], false).is_err());
    s.emit("bye", vec![], false).unwrap();
    tokio::time::timeout(Duration::from_secs(5), s.wait_closed()).await.unwrap();
    assert!(!s.is_open());
    let mut reason = None;
    while let Ok(e) = rx.try_recv() {
        if let ProtoEvent::Closed { reason: r, .. } = e {
            reason = r;
        }
    }
    assert_eq!(reason.as_deref(), Some("server disconnected the namespace"));
    assert!(matches!(s.emit("x", vec![], false), Err(ProtoError::Closed)));
}

#[tokio::test]
async fn namespaces_auth_listen_filter_and_client_close() {
    let server = start_server().await;
    let e = SioSession::connect(opts(&server, "/admin", Some(json!({ "token": "nope" })), &[]), EventSink::noop()).await.unwrap_err();
    assert!(e.to_string().contains("invalid token"), "{e}");

    let (sink, mut rx) = EventSink::channel();
    let s = SioSession::connect(opts(&server, "/admin", Some(json!({ "token": "ok" })), &[]), sink).await.unwrap();
    assert_eq!(s.namespace, "/admin");
    let (label, args, _) = next_in(&mut rx).await;
    assert_eq!((label.as_str(), args), ("hi-admin", json!([1])));
    s.close();
    tokio::time::timeout(Duration::from_secs(5), s.wait_closed()).await.unwrap();

    // Only listened events are shown; others are mentioned once.
    let (sink, mut rx) = EventSink::channel();
    let s = SioSession::connect(opts(&server, "", None, &["echoed"]), sink).await.unwrap();
    s.emit("echo", vec![json!("x")], false).unwrap();
    let mut infos = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap() {
            ProtoEvent::Info { message, .. } => infos.push(message),
            ProtoEvent::Message { direction: Direction::In, label, .. } => {
                assert_eq!(label.as_deref(), Some("echoed"), "only listened events are shown");
                break;
            }
            _ => {}
        }
    }
    // The server's "welcome" (sent right after connecting) was hidden and mentioned instead.
    assert!(infos.iter().any(|m| m.contains("\"welcome\"")), "{infos:?}");
    s.close();
    tokio::time::timeout(Duration::from_secs(5), s.wait_closed()).await.unwrap();
}

#[tokio::test]
async fn not_a_socketio_server() {
    // A plain WebSocket echo server answers the upgrade but never sends an Engine.IO open packet.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(Message::text("hello")).await.unwrap();
        while ws.next().await.is_some() {}
    });
    let mut o = opts(&format!("http://{addr}"), "", None, &[]);
    o.connect_timeout = Duration::from_secs(3);
    let e = SioSession::connect(o, EventSink::noop()).await.unwrap_err();
    assert!(e.to_string().contains("Engine.IO open packet"), "{e}");
}
