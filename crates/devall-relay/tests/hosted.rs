//! Batch D client features against in-process fakes: raw TCP tunnels (publisher + consumer), the
//! hosted-extras JSON API and the Firebase callables for custom domains.

mod common;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use common::*;
use devall_core::{MemorySecretStore, SecretStore};
use devall_relay::auth::store_refresh_token;
use devall_relay::protocol::{Frame, Hello, TCP_CONSUMER_STREAM, TcpClose, TcpOpen, TunnelConfig, ft};
use devall_relay::tcp::{generate_access_secret, secret_sha256};
use devall_relay::{ConnState, MockPayload, MockSettings, ReconnectPolicy, RelayAuth, RelayClient, RelayConfig, RelayError, TcpAccess, TcpConnectOptions, TcpTunnelEvent, TcpTunnelOptions};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "static-token-0123456789";

fn client(endpoint: String, auth: RelayAuth, functions_url: Option<String>, secrets: Arc<dyn SecretStore>) -> RelayClient {
    RelayClient::new(RelayConfig { endpoint, auth, verify_tls: true, connect_timeout: Duration::from_secs(5), functions_url }, secrets).unwrap()
}

fn static_client(relay: &MockRelay) -> RelayClient {
    client(relay.endpoint(), RelayAuth::StaticToken(Some(TOKEN.into())), None, Arc::new(MemorySecretStore::default()))
}

fn tcp_hello(sub: &str) -> Hello {
    let mut h = hello("tunnel", sub);
    h.kind = "tcp".into();
    h.plan = "supporter".into();
    h
}

/// A local TCP server that echoes everything back in upper case.
async fn upper_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else { continue };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let up: Vec<u8> = buf[..n].iter().map(u8::to_ascii_uppercase).collect();
                    if s.write_all(&up).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

fn fast() -> ReconnectPolicy {
    ReconnectPolicy { initial: Duration::from_millis(30), max: Duration::from_millis(200) }
}

#[tokio::test]
async fn tcp_publisher_bridges_streams_with_credit() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let echo = upper_echo().await;
    let secret = generate_access_secret();
    let c = static_client(&relay);
    let t = c.start_tcp_tunnel(TcpTunnelOptions { local: echo.to_string(), access_secret: Some(secret.clone()), reconnect: fast(), ..TcpTunnelOptions::for_port(1) });
    let mut events = t.subscribe();

    let mut conn = relay.next_conn().await;
    assert_eq!(conn.info.path, "/connect");
    assert!(conn.info.query.contains("kind=tcp"));
    conn.send(Frame::json(ft::HELLO, 0, &tcp_hello("pg-box"))).await;
    let st = t.wait_connected(T).await.unwrap();
    assert_eq!(st.name.as_deref(), Some("pg-box"));
    assert!(st.has_secret);

    // Only the secret's SHA-256 travels to the relay.
    let cfg: TunnelConfig = conn.expect(ft::TUNNEL_CONFIG, Some(0)).await.parse().unwrap();
    assert_eq!(cfg.tcp.unwrap().secret_sha256.as_deref(), Some(secret_sha256(&secret).as_str()));

    conn.send(Frame::json(ft::TCP_OPEN, 9, &TcpOpen { client_ip: Some("203.0.113.5".into()), via: "secret".into() })).await;
    conn.expect(ft::TCP_ACCEPT, Some(9)).await;
    conn.send(Frame::new(ft::TCP_DATA, 9, b"select 1;".to_vec())).await;
    let back = conn.expect(ft::TCP_DATA, Some(9)).await;
    assert_eq!(&back.payload[..], b"SELECT 1;");

    // 300 KiB in: the publisher returns credit (WINDOW) as it writes to the local socket, and its
    // replies stop at the initial 256 KiB window until the relay grants more.
    let big = vec![b'a'; 300 * 1024];
    for chunk in big.chunks(60 * 1024) {
        conn.send(Frame::new(ft::TCP_DATA, 9, chunk.to_vec())).await;
    }
    let w = conn.expect(ft::WINDOW, Some(9)).await.parse_u32().unwrap();
    assert!(w >= 128 * 1024, "credit returned in batches of at least window/2 ({w})");
    let mut got = 0usize;
    while conn.sees(ft::TCP_DATA, Some(9), Duration::from_millis(300)).await {
        got += conn.expect(ft::TCP_DATA, Some(9)).await.payload.len();
    }
    assert!(got > 0 && got <= 256 * 1024 - 9, "never more than the window without credit ({got})");
    conn.send(Frame::u32(ft::WINDOW, 9, 1024 * 1024)).await;
    while got < 300 * 1024 {
        got += conn.expect(ft::TCP_DATA, Some(9)).await.payload.len();
    }
    assert_eq!(got, 300 * 1024);

    // The relay closes the stream; the log records the connection.
    conn.send(Frame::json(ft::TCP_CLOSE, 9, &TcpClose { reason: "eof".into(), message: None })).await;
    let log = tokio::time::timeout(T, async {
        loop {
            if let Ok(TcpTunnelEvent::Connection(l)) = events.recv().await {
                return l;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((log.via.as_str(), log.client_ip.as_deref()), ("secret", Some("203.0.113.5")));
    assert_eq!(log.bytes_in, 9 + 300 * 1024);

    // Nothing listening → TCP_CLOSE local_unreachable instead of TCP_ACCEPT.
    let dead = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    t.stop().await;
    let t2 = c.start_tcp_tunnel(TcpTunnelOptions { local: dead.to_string(), reconnect: fast(), ..TcpTunnelOptions::for_port(1) });
    let mut conn2 = relay.next_conn().await;
    conn2.send(Frame::json(ft::HELLO, 0, &tcp_hello("pg-two"))).await;
    t2.wait_connected(T).await.unwrap();
    conn2.send(Frame::json(ft::TCP_OPEN, 3, &TcpOpen { client_ip: None, via: "owner".into() })).await;
    let closed: TcpClose = conn2.expect(ft::TCP_CLOSE, Some(3)).await.parse().unwrap();
    assert_eq!(closed.reason, "local_unreachable");
    t2.stop().await;
}

#[tokio::test]
async fn tcp_publisher_stops_on_relays_without_tcp_support() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let t = static_client(&relay).start_tcp_tunnel(TcpTunnelOptions { reconnect: fast(), ..TcpTunnelOptions::for_port(5432) });
    let mut conn = relay.next_conn().await;
    conn.send_hello("tunnel", "old-relay-1234").await; // an old relay ignores kind=tcp
    let err = t.wait_connected(T).await.unwrap_err();
    assert!(err.to_string().contains("raw TCP"), "{err}");
    assert_eq!(t.status().state, ConnState::Failed);
}

#[tokio::test]
async fn tcp_consumer_forwards_local_connections() {
    let secret = "dtcp_shared-secret-for-teammates-0123456789";
    let relay = MockRelay::start(&[TOKEN, secret]).await;
    let c = static_client(&relay);

    // With the access secret.
    let h = c.tcp_connect(TcpConnectOptions { name: "pg-box".into(), bind: "127.0.0.1:0".parse().unwrap(), access: TcpAccess::Secret(secret.into()) }).await.unwrap();
    let mut local = TcpStream::connect(h.local_addr()).await.unwrap();
    let mut conn = relay.next_conn().await;
    assert_eq!(conn.info.path, "/tcp");
    assert_eq!(conn.info.query, "name=pg-box");
    assert_eq!(conn.info.authorization.as_deref(), Some(&*format!("Bearer {secret}")));

    // Server greeting first (relay → local), then the client's query (local → relay).
    conn.send(Frame::new(ft::TCP_DATA, TCP_CONSUMER_STREAM, b"220 ready\r\n".to_vec())).await;
    let mut buf = [0u8; 64];
    let n = local.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"220 ready\r\n");
    local.write_all(b"PING\r\n").await.unwrap();
    let f = conn.expect(ft::TCP_DATA, Some(TCP_CONSUMER_STREAM)).await;
    assert_eq!(&f.payload[..], b"PING\r\n");

    // The tunnel side closes → the local socket sees EOF.
    conn.send(Frame::json(ft::TCP_CLOSE, TCP_CONSUMER_STREAM, &TcpClose { reason: "eof".into(), message: None })).await;
    let n = tokio::time::timeout(T, local.read(&mut buf)).await.unwrap().unwrap();
    assert_eq!(n, 0);

    // The local client hangs up → the relay hears TCP_CLOSE eof.
    let mut local2 = TcpStream::connect(h.local_addr()).await.unwrap();
    let mut conn2 = relay.next_conn().await;
    local2.write_all(b"x").await.unwrap();
    conn2.expect(ft::TCP_DATA, Some(TCP_CONSUMER_STREAM)).await;
    drop(local2);
    let closed: TcpClose = conn2.expect(ft::TCP_CLOSE, Some(TCP_CONSUMER_STREAM)).await.parse().unwrap();
    assert_eq!(closed.reason, "eof");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let st = h.status();
    assert_eq!(st.total_connections, 2);
    h.stop().await;

    // With the account token; a refused upgrade closes the local socket and is reported.
    *relay.reject_next.lock().unwrap() = Some((403, "forbidden".into()));
    let h2 = c.tcp_connect(TcpConnectOptions { name: "pg-box".into(), bind: "127.0.0.1:0".parse().unwrap(), access: TcpAccess::Account }).await.unwrap();
    let mut local3 = TcpStream::connect(h2.local_addr()).await.unwrap();
    let n = tokio::time::timeout(T, local3.read(&mut buf)).await.unwrap().unwrap_or(0);
    assert_eq!(n, 0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(h2.status().last_error.unwrap_or_default().contains("forbidden"));
    h2.stop().await;
}

// ---------- JSON API + callables ----------

type Seen = Arc<Mutex<Vec<(String, String, String, String)>>>;

/// Records (method, path+query, authorization, body) and answers like the relay / Cloud Functions.
async fn fake_api() -> (SocketAddr, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else { continue };
            let seen = s2.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let seen = seen.clone();
                    async move {
                        let method = req.method().to_string();
                        let path = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
                        let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                        let body = String::from_utf8(req.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
                        seen.lock().unwrap().push((method.clone(), path.clone(), auth.clone(), body.clone()));
                        let (status, out) = match (method.as_str(), path.as_str()) {
                            (_, _) if !(auth == format!("Bearer {TOKEN}") || auth == "Bearer ID-2") => (401, r#"{"error":{"code":"unauthorized","message":"bad token"}}"#.to_string()),
                            ("PUT", "/v1/mocks/pets-api") => {
                                let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                                let stubs = v["mappings"].as_array().unwrap().len();
                                (200, format!(r#"{{"name":"pets-api","url":"https://pets-api.mock.devall.test","pathUrl":"https://mock.devall.test/pets-api","stubs":{stubs},"bytes":10,"files":1,"warnings":[{{"stub":"x","feature":"request","message":"nope"}}]}}"#))
                            }
                            ("PUT", "/v1/mocks/taken") => (409, r#"{"error":{"code":"name_taken","message":"\"taken\" is published by another account."}}"#.to_string()),
                            ("GET", "/v1/shares") => (200, r#"{"shares":[],"limits":{"shares":20}}"#.to_string()),
                            ("POST", "/addCustomDomain") => (200, r#"{"result":{"hostname":"api.example.com","status":"pending","records":[]}}"#.to_string()),
                            ("POST", "/removeCustomDomain") => (400, r#"{"error":{"message":"api.example.com isn't one of your custom domains.","status":"NOT_FOUND"}}"#.to_string()),
                            _ => (404, r#"{"error":{"code":"not_found","message":"Not found"}}"#.to_string()),
                        };
                        Ok::<_, Infallible>(Response::builder().status(status).header("content-type", "application/json").body(Full::new(Bytes::from(out))).unwrap())
                    }
                });
                let _ = http1::Builder::new().serve_connection(TokioIo::new(s), svc).await;
            });
        }
    });
    (addr, seen)
}

#[tokio::test]
async fn hosted_api_publishes_mocks_and_maps_errors() {
    let (addr, seen) = fake_api().await;
    let c = client(format!("ws://{addr}"), RelayAuth::StaticToken(Some(TOKEN.into())), None, Arc::new(MemorySecretStore::default()));
    let payload = MockPayload { mappings: vec![serde_json::json!({"request": {"urlPath": "/pets"}, "response": {"status": 200}})], files: Default::default(), settings: MockSettings::default() };
    let r = c.publish_mock("pets-api", &payload).await.unwrap();
    assert_eq!((r.stubs, r.url.as_str()), (1, "https://pets-api.mock.devall.test"));
    assert_eq!(r.warnings.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0].3).unwrap();
    assert_eq!(body["settings"], serde_json::json!({"cors": true, "globalTemplating": false}));

    let err = c.publish_mock("taken", &payload).await.unwrap_err();
    assert!(matches!(&err, RelayError::Rejected { status: 409, code, .. } if code == "name_taken"), "{err:?}");
    assert!(c.publish_mock("Bad Name", &payload).await.is_err(), "names are checked locally first");
    let shares = c.api(reqwest::Method::GET, "/v1/shares", None).await.unwrap();
    assert_eq!(shares["limits"]["shares"], 20);
    // Callables are hosted-only.
    assert!(matches!(c.function("addCustomDomain", &serde_json::json!({})).await, Err(RelayError::Config(_))));
}

#[tokio::test]
async fn custom_domain_callables_use_the_firebase_id_token() {
    let (addr, seen) = fake_api().await;
    let st = FakeSecureToken::start().await;
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    store_refresh_token(secrets.as_ref(), "R1").unwrap();
    let c = client(format!("ws://{addr}"), RelayAuth::FirebaseRefreshToken { api_key: "test-key".into(), token_url: st.url() }, Some(format!("http://{addr}")), secrets);
    let v = c.function("addCustomDomain", &serde_json::json!({"hostname": "api.example.com", "subdomain": "my-api"})).await.unwrap();
    assert_eq!(v["status"], "pending");
    let (method, path, auth, body) = seen.lock().unwrap()[0].clone();
    assert_eq!((method.as_str(), path.as_str(), auth.as_str()), ("POST", "/addCustomDomain", "Bearer ID-2"));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap(), serde_json::json!({"data": {"hostname": "api.example.com", "subdomain": "my-api"}}));
    let err = c.function("removeCustomDomain", &serde_json::json!({"hostname": "api.example.com"})).await.unwrap_err();
    assert!(matches!(&err, RelayError::Rejected { code, message, .. } if code == "not_found" && message.contains("isn't one of")), "{err:?}");
}
