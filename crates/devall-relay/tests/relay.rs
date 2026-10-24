//! End-to-end tests of the relay client against an in-process mock relay (tests/common).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use devall_core::{MemorySecretStore, SecretStore};
use devall_relay::auth::{load_refresh_token, store_refresh_token};
use devall_relay::protocol::{Frame, GoAway, InboxAck, InboxConfig, InboxEvent, QuotaPayload, RequestHead, ResponseHead, TunnelConfig, WsAccept, WsClose, WsOpen, WsReject, ft};
use devall_relay::{ConnState, InboxOptions, InboxResponse, OfflineInbox, ReconnectPolicy, RelayAuth, RelayClient, RelayConfig, RelayError, TunnelEvent, TunnelOptions, handler};

const TOKEN: &str = "static-token-0123456789";

/// (event id, body) pairs the inbox handler accepted.
type Handled = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

fn fast() -> ReconnectPolicy {
    ReconnectPolicy { initial: Duration::from_millis(30), max: Duration::from_millis(200) }
}

fn client(relay: &MockRelay) -> RelayClient {
    let cfg = RelayConfig { endpoint: relay.endpoint(), auth: RelayAuth::StaticToken(Some(TOKEN.into())), verify_tls: true, connect_timeout: Duration::from_secs(5), functions_url: None };
    RelayClient::new(cfg, Arc::new(MemorySecretStore::default())).unwrap()
}

fn tunnel_opts(local: url::Url) -> TunnelOptions {
    TunnelOptions { reconnect: fast(), ..TunnelOptions::new(local) }
}

fn header<'a>(h: &'a ResponseHead, name: &str) -> Option<&'a str> {
    h.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

fn open(method: &str, path: &str, headers: Vec<(&str, &str)>, body_length: Option<u64>) -> RequestHead {
    RequestHead {
        method: method.into(),
        url: format!("https://brave-otter-1234.devall.test{path}"),
        path: path.into(),
        headers: headers.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        client_ip: Some("203.0.113.9".into()),
        body_length,
    }
}

#[tokio::test]
async fn http_round_trip_preserves_body_headers_and_public_host() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let local = LocalHttp::start().await;
    let c = client(&relay);
    let t = c.start_tunnel(tunnel_opts(local.url()));
    let mut events = t.subscribe();

    let mut conn = relay.next_conn().await;
    assert_eq!(conn.info.path, "/connect");
    assert_eq!(conn.info.authorization.as_deref(), Some(&*format!("Bearer {TOKEN}")));
    assert_eq!(conn.info.protocol.as_deref(), Some("devall-relay.1"));
    conn.send_hello("tunnel", "brave-otter-1234").await;
    let st = t.wait_connected(T).await.unwrap();
    assert_eq!(st.public_url.as_deref(), Some("https://brave-otter-1234.devall.test"));
    assert_eq!(st.plan.as_deref(), Some("free"));

    let body = "Body=hello&From=%2B15550001111";
    let head = open("POST", "/webhooks/twilio?a=1", vec![("content-type", "application/x-www-form-urlencoded"), ("x-twilio-signature", "sig=="), ("connection", "keep-alive")], Some(body.len() as u64));
    conn.send(Frame::json(ft::OPEN, 7, &head)).await;
    conn.send(Frame::new(ft::DATA, 7, body.as_bytes().to_vec())).await;
    conn.send(Frame::empty(ft::END, 7)).await;

    let res: ResponseHead = conn.expect(ft::RESPONSE, Some(7)).await.parse().unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(header(&res, "x-echo-method"), Some("POST"));
    assert_eq!(header(&res, "x-echo-path"), Some("/webhooks/twilio?a=1"));
    assert_eq!(header(&res, "x-echo-host"), Some("brave-otter-1234.devall.test"), "public Host is kept by default");
    assert_eq!(header(&res, "x-echo-xfh"), Some("brave-otter-1234.devall.test"));
    assert_eq!(header(&res, "x-echo-xff"), Some("203.0.113.9"));
    assert_eq!(header(&res, "x-echo-xfp"), Some("https"));
    assert_eq!(header(&res, "x-echo-sig"), Some("sig=="));
    assert_eq!(header(&res, "x-echo-accept-encoding"), Some(""), "the client must not add Accept-Encoding");
    let got = conn.read_body(7, false).await;
    assert_eq!(String::from_utf8(got).unwrap(), body);

    // The request shows up in the live log.
    let log = tokio::time::timeout(T, async {
        loop {
            if let Ok(TunnelEvent::Request(r)) = events.recv().await {
                return r;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((log.method.as_str(), log.path.as_str(), log.status), ("POST", "/webhooks/twilio?a=1", Some(200)));
    assert_eq!(log.bytes_in, body.len() as u64);
    assert!(t.status().requests >= 1);

    // Repeated headers and error statuses pass through; compressed bodies stay compressed.
    conn.send(Frame::json(ft::OPEN, 9, &open("GET", "/status/404", vec![], None))).await;
    conn.send(Frame::empty(ft::END, 9)).await;
    let res: ResponseHead = conn.expect(ft::RESPONSE, Some(9)).await.parse().unwrap();
    assert_eq!(res.status, 404);
    assert_eq!(res.headers.iter().filter(|(k, _)| k == "set-cookie").count(), 2);
    assert_eq!(conn.read_body(9, false).await, b"nope");
    conn.send(Frame::json(ft::OPEN, 11, &open("GET", "/gzip", vec![("accept-encoding", "gzip")], None))).await;
    conn.send(Frame::empty(ft::END, 11)).await;
    let res: ResponseHead = conn.expect(ft::RESPONSE, Some(11)).await.parse().unwrap();
    assert_eq!(header(&res, "content-encoding"), Some("gzip"));
    assert_eq!(conn.read_body(11, false).await, vec![0x1f, 0x8b, 8, 0, 1, 2, 3]);

    t.stop().await;
    assert_eq!(t.status().state, ConnState::Stopped);
}

#[tokio::test]
async fn rewrite_host_option_targets_the_local_address() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let local = LocalHttp::start().await;
    let t = client(&relay).start_tunnel(TunnelOptions { rewrite_host: true, ..tunnel_opts(local.url()) });
    let mut conn = relay.next_conn().await;
    conn.send_hello("tunnel", "x-y-1234").await;
    t.wait_connected(T).await.unwrap();
    conn.send(Frame::json(ft::OPEN, 1, &open("GET", "/", vec![], Some(0)))).await;
    conn.send(Frame::empty(ft::END, 1)).await;
    let res: ResponseHead = conn.expect(ft::RESPONSE, Some(1)).await.parse().unwrap();
    assert_eq!(header(&res, "x-echo-host"), Some(&*local.addr.to_string()));
    assert_eq!(header(&res, "x-echo-xfh"), Some("brave-otter-1234.devall.test"));
    t.stop().await;
}

#[tokio::test]
async fn streams_bodies_both_ways_under_flow_control() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let local = LocalHttp::start().await;
    let t = client(&relay).start_tunnel(tunnel_opts(local.url()));
    let mut conn = relay.next_conn().await;
    conn.send_hello("tunnel", "flow-flow-1234").await;
    t.wait_connected(T).await.unwrap();

    // Response: 512 KiB but only 256 KiB of credit until the relay grants more.
    conn.send(Frame::json(ft::OPEN, 3, &open("GET", "/stream", vec![], None))).await;
    conn.send(Frame::empty(ft::END, 3)).await;
    let res: ResponseHead = conn.expect(ft::RESPONSE, Some(3)).await.parse().unwrap();
    assert_eq!(res.status, 200);
    let mut received = 0usize;
    while received < 256 * 1024 {
        let f = conn.expect(ft::RESPONSE_DATA, Some(3)).await;
        assert!(f.payload.len() <= 64 * 1024);
        received += f.payload.len();
    }
    assert_eq!(received, 256 * 1024, "client must stop at the window");
    assert!(!conn.sees(ft::RESPONSE_DATA, Some(3), Duration::from_millis(300)).await, "no data beyond the credit");
    conn.send(Frame::u32(ft::WINDOW, 3, 256 * 1024)).await;
    let rest = conn.read_body(3, true).await;
    assert_eq!(received + rest.len(), 512 * 1024);

    // Request: 600 KiB upload; the client returns credit as the local server consumes it.
    let total = 600 * 1024;
    let payload: Vec<u8> = (0..total).map(|i| (i % 251) as u8).collect();
    conn.send(Frame::json(ft::OPEN, 5, &open("PUT", "/upload", vec![("content-type", "application/octet-stream")], Some(total as u64)))).await;
    let mut credit: i64 = 256 * 1024;
    let mut sent = 0usize;
    let mut windows = 0;
    while sent < total {
        if credit <= 0 {
            let w = conn.expect(ft::WINDOW, Some(5)).await.parse_u32().unwrap();
            credit += i64::from(w);
            windows += 1;
            continue;
        }
        let n = (64 * 1024).min(credit as usize).min(total - sent);
        conn.send(Frame::new(ft::DATA, 5, payload[sent..sent + n].to_vec())).await;
        sent += n;
        credit -= n as i64;
    }
    conn.send(Frame::empty(ft::END, 5)).await;
    let res: ResponseHead = conn.expect(ft::RESPONSE, Some(5)).await.parse().unwrap();
    assert_eq!(header(&res, "x-echo-len"), Some(&*total.to_string()));
    assert!(windows >= 1);
    let echoed = conn.read_body(5, true).await;
    assert_eq!(echoed, payload);
    t.stop().await;
}

#[tokio::test]
async fn unreachable_local_target_answers_502() {
    let relay = MockRelay::start(&[TOKEN]).await;
    // A port nothing listens on.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let t = client(&relay).start_tunnel(tunnel_opts(url::Url::parse(&format!("http://{dead}")).unwrap()));
    let mut conn = relay.next_conn().await;
    conn.send_hello("tunnel", "dead-port-1234").await;
    t.wait_connected(T).await.unwrap();
    conn.send(Frame::json(ft::OPEN, 1, &open("GET", "/", vec![], Some(0)))).await;
    conn.send(Frame::empty(ft::END, 1)).await;
    let res: ResponseHead = conn.expect(ft::RESPONSE, Some(1)).await.parse().unwrap();
    assert_eq!(res.status, 502);
    let body = String::from_utf8(conn.read_body(1, false).await).unwrap();
    assert!(body.contains("could not reach"), "{body}");
    t.stop().await;
}

#[tokio::test]
async fn websocket_pass_through() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let ws_addr = start_ws_echo().await;
    let t = client(&relay).start_tunnel(tunnel_opts(url::Url::parse(&format!("http://{ws_addr}")).unwrap()));
    let mut conn = relay.next_conn().await;
    conn.send_hello("tunnel", "ws-echo-1234").await;
    t.wait_connected(T).await.unwrap();

    let ws_open = WsOpen { url: "https://ws-echo-1234.devall.test/chat".into(), path: "/chat".into(), headers: vec![("origin".into(), "https://example.com".into())], protocols: vec!["chat".into(), "v2".into()], client_ip: Some("1.2.3.4".into()) };
    conn.send(Frame::json(ft::WS_OPEN, 21, &ws_open)).await;
    let acc: WsAccept = conn.expect(ft::WS_ACCEPT, Some(21)).await.parse().unwrap();
    assert_eq!(acc.protocol.as_deref(), Some("chat"));
    conn.send(Frame::new(ft::WS_TEXT, 21, b"hello".to_vec())).await;
    assert_eq!(&conn.expect(ft::WS_TEXT, Some(21)).await.payload[..], b"hello");
    conn.send(Frame::new(ft::WS_BINARY, 21, vec![0u8, 1, 255])).await;
    assert_eq!(&conn.expect(ft::WS_BINARY, Some(21)).await.payload[..], &[0u8, 1, 255]);
    assert_eq!(t.status().websockets, 1);
    // The local server closes: the relay hears the code.
    conn.send(Frame::new(ft::WS_TEXT, 21, b"close-me".to_vec())).await;
    let c: WsClose = conn.expect(ft::WS_CLOSE, Some(21)).await.parse().unwrap();
    assert_eq!(c.code, 4002);

    // Refused upgrades are reported with their status.
    let refused = WsOpen { path: "/forbidden".into(), ..ws_open.clone() };
    conn.send(Frame::json(ft::WS_OPEN, 23, &refused)).await;
    let rej: WsReject = conn.expect(ft::WS_REJECT, Some(23)).await.parse().unwrap();
    assert_eq!(rej.status, 403);

    // A public-side close reaches the local server (stream ends cleanly).
    conn.send(Frame::json(ft::WS_OPEN, 25, &ws_open)).await;
    conn.expect(ft::WS_ACCEPT, Some(25)).await;
    conn.send(Frame::json(ft::WS_CLOSE, 25, &WsClose { code: 1000, reason: "bye".into() })).await;
    tokio::time::timeout(T, async {
        while t.status().websockets != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    t.stop().await;
}

#[tokio::test]
async fn reconnects_and_resumes_the_same_subdomain() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let local = LocalHttp::start().await;
    let t = client(&relay).start_tunnel(tunnel_opts(local.url()));
    let mut conn = relay.next_conn().await;
    assert_eq!(conn.info.query, "");
    conn.send_hello("tunnel", "lucky-lynx-4821").await;
    t.wait_connected(T).await.unwrap();
    // Server restart (deploy): close with 1012.
    conn.close(1012, "restart").await;
    drop(conn);

    let mut again = relay.next_conn().await;
    assert_eq!(again.info.query, "name=lucky-lynx-4821", "reconnect asks for the same random name");
    again.send_hello("tunnel", "lucky-lynx-4821").await;
    let st = t.wait_connected(T).await.unwrap();
    assert!(st.reconnects >= 1);

    // Abrupt drop (no close frame) also reconnects.
    drop(again);
    let mut third = relay.next_conn().await;
    third.send_hello("tunnel", "lucky-lynx-4821").await;
    t.wait_connected(T).await.unwrap();

    // Session expiry: GOAWAY + 4008 → immediate reconnect.
    third.send(Frame::json(ft::QUOTA, 0, &QuotaPayload { kind: "session".into(), used: 1, limit: 1, reset_at: None, message: "Session time limit reached.".into() })).await;
    third.send(Frame::json(ft::GOAWAY, 0, &GoAway { reason: "session_expired".into(), reconnect: true, retry_after_ms: Some(0), message: None })).await;
    third.close(4008, "session expired").await;
    let mut fourth = relay.next_conn().await;
    fourth.send_hello("tunnel", "other-name-1111").await;
    let st = t.wait_connected(T).await.unwrap();
    assert_eq!(st.subdomain.as_deref(), Some("other-name-1111"));

    // Replaced by another connection: GOAWAY reconnect=false → the tunnel stops.
    fourth.send(Frame::json(ft::GOAWAY, 0, &GoAway { reason: "replaced".into(), reconnect: false, retry_after_ms: None, message: Some("A newer connection took over this tunnel.".into()) })).await;
    fourth.close(4009, "replaced").await;
    tokio::time::timeout(T, async {
        while t.status().state != ConnState::Stopped {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(t.status().last_error.unwrap().contains("newer connection"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!t.is_running());
}

#[tokio::test]
async fn fatal_rejections_stop_retryable_ones_back_off() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let local = LocalHttp::start().await;
    *relay.reject_next.lock().unwrap() = Some((403, "subdomain_not_reserved".into()));
    let t = client(&relay).start_tunnel(TunnelOptions { subdomain: Some("my-api".into()), ..tunnel_opts(local.url()) });
    let err = t.wait_connected(T).await.unwrap_err();
    assert!(err.to_string().contains("subdomain_not_reserved"), "{err}");
    assert_eq!(t.status().state, ConnState::Failed);

    *relay.reject_next.lock().unwrap() = Some((429, "too_many_tunnels".into()));
    let before = relay.attempts.load(Ordering::SeqCst);
    let t2 = client(&relay).start_tunnel(tunnel_opts(local.url()));
    let mut conn = relay.next_conn().await;
    assert!(relay.attempts.load(Ordering::SeqCst) >= before + 2, "retried after 429");
    conn.send_hello("tunnel", "back-off-1234").await;
    t2.wait_connected(T).await.unwrap();
    t2.stop().await;

    // Wrong static token: 401 is fatal for static tokens.
    let bad = RelayClient::new(
        RelayConfig { endpoint: relay.endpoint(), auth: RelayAuth::StaticToken(Some("wrong".into())), verify_tls: true, connect_timeout: Duration::from_secs(5), functions_url: None },
        Arc::new(MemorySecretStore::default()),
    )
    .unwrap();
    let t3 = bad.start_tunnel(tunnel_opts(local.url()));
    assert!(t3.wait_connected(T).await.is_err());
    assert_eq!(t3.status().state, ConnState::Failed);
}

#[tokio::test]
async fn sends_offline_inbox_config_and_tracks_quota_and_usage() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let local = LocalHttp::start().await;
    let oi = OfflineInbox { inbox_id: "abcdefghijklmnopqrst".into(), path_prefixes: vec!["/webhooks/".into()] };
    let t = client(&relay).start_tunnel(TunnelOptions { offline_inbox: Some(oi.clone()), ..tunnel_opts(local.url()) });
    let mut conn = relay.next_conn().await;
    conn.send_hello("tunnel", "cfg-test-1234").await;
    let cfg: TunnelConfig = conn.expect(ft::TUNNEL_CONFIG, Some(0)).await.parse().unwrap();
    assert_eq!(cfg.offline_inbox, Some(oi));
    conn.send(Frame::json(ft::QUOTA, 0, &QuotaPayload { kind: "bandwidth".into(), used: 2, limit: 1, reset_at: None, message: "Monthly bandwidth used up".into() })).await;
    conn.send(Frame::json(ft::USAGE, 0, &devall_relay::Usage { bandwidth_bytes: 99, requests: 3, period_start: "a".into(), period_end: "b".into() })).await;
    conn.send(Frame::new(ft::PING, 0, b"rtt".to_vec())).await;
    assert_eq!(&conn.expect(ft::PONG, Some(0)).await.payload[..], b"rtt");
    tokio::time::timeout(T, async {
        while t.status().usage.as_ref().map(|u| u.bandwidth_bytes) != Some(99) || t.status().quota.is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(t.status().quota.unwrap().kind, "bandwidth");
    t.stop().await;
}

#[tokio::test]
async fn refreshes_firebase_tokens_and_retries_after_401() {
    let relay = MockRelay::start(&["ID-2"]).await;
    let local = LocalHttp::start().await;
    let st = FakeSecureToken::start().await;
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    store_refresh_token(secrets.as_ref(), "R1").unwrap();
    let cfg = RelayConfig {
        endpoint: relay.endpoint(),
        auth: RelayAuth::FirebaseRefreshToken { api_key: "test-key".into(), token_url: st.url() },
        verify_tls: true,
        connect_timeout: Duration::from_secs(5),
        functions_url: None,
    };
    let c = RelayClient::new(cfg, secrets.clone()).unwrap();
    assert!(c.is_signed_in());
    assert_eq!(c.bearer().await.unwrap(), "ID-2");
    assert_eq!(load_refresh_token(secrets.as_ref()).unwrap().as_deref(), Some("R2"), "rotated refresh token is stored in the keychain");
    assert_eq!(c.bearer().await.unwrap(), "ID-2", "cached until close to expiry");
    assert_eq!(st.calls.load(Ordering::SeqCst), 1);

    // The relay starts rejecting ID-2 (e.g. revoked): 401 → refresh → retry with ID-3.
    *relay.accept_tokens.lock().unwrap() = vec!["ID-3".into()];
    let t = c.start_tunnel(tunnel_opts(local.url()));
    let mut conn = relay.next_conn().await;
    assert_eq!(conn.info.authorization.as_deref(), Some("Bearer ID-3"));
    assert_eq!(st.calls.load(Ordering::SeqCst), 2);
    assert_eq!(load_refresh_token(secrets.as_ref()).unwrap().as_deref(), Some("R3"));
    conn.send_hello("tunnel", "token-test-1234").await;
    t.wait_connected(T).await.unwrap();
    t.stop().await;

    // A revoked refresh token is fatal ("sign in again").
    store_refresh_token(secrets.as_ref(), "revoked").unwrap();
    c.sign_out().await.unwrap();
    assert!(!c.is_signed_in());
    assert!(matches!(c.bearer().await, Err(RelayError::NotSignedIn)));
    store_refresh_token(secrets.as_ref(), "revoked").unwrap();
    match c.bearer().await {
        Err(RelayError::Auth(m)) => assert!(m.contains("INVALID_REFRESH_TOKEN"), "{m}"),
        other => panic!("expected auth error, got {other:?}"),
    }
}

fn event(id: &str, seq: u64, body: &[u8]) -> InboxEvent {
    use base64::Engine as _;
    InboxEvent {
        id: id.into(),
        seq,
        inbox_id: "abcdefghijklmnopqrst".into(),
        method: "POST".into(),
        url: format!("https://hooks.devall.test/abcdefghijklmnopqrst/stripe?n={seq}"),
        path: format!("/stripe?n={seq}"),
        headers: vec![("stripe-signature".into(), format!("t=1,v1={seq}"))],
        body_base64: base64::engine::general_purpose::STANDARD.encode(body),
        body_size: body.len() as u64,
        received_at: "2026-10-21T16:13:24.517Z".into(),
        source_ip: Some("54.187.174.169".into()),
    }
}

#[tokio::test]
async fn inbox_delivers_backlog_in_order_acks_after_handler_and_dedupes() {
    let relay = MockRelay::start(&[TOKEN]).await;
    let c = client(&relay);
    let seen: Handled = Arc::new(Mutex::new(Vec::new()));
    let failures = Arc::new(AtomicUsize::new(0));
    let h = {
        let seen = seen.clone();
        let failures = failures.clone();
        handler(move |ev: InboxEvent| {
            let seen = seen.clone();
            let failures = failures.clone();
            async move {
                // The second event fails once (e.g. disk busy) and must be retried, not skipped.
                if ev.id == "evt_2" && failures.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("disk busy".to_string());
                }
                seen.lock().unwrap().push((ev.id.clone(), ev.body().unwrap()));
                Ok(())
            }
        })
    };
    let response = InboxResponse { status: 202, headers: vec![("content-type".into(), "text/plain".into())], body: "thanks".into() };
    let opts = InboxOptions { project_key: Some("proj-1".into()), response: Some(response.clone()), reconnect: fast(), retry: ReconnectPolicy { initial: Duration::from_millis(20), max: Duration::from_millis(50) } };
    let inbox = c.connect_inbox(opts, h);

    let mut conn = relay.next_conn().await;
    assert_eq!(conn.info.path, "/inbox");
    assert_eq!(conn.info.query, "project=proj-1");
    conn.send_hello("inbox", "abcdefghijklmnopqrst").await;
    let cfg: InboxConfig = conn.expect(ft::INBOX_CONFIG, Some(0)).await.parse().unwrap();
    assert_eq!(cfg.response, Some(response));
    let st = inbox.wait_connected(T).await.unwrap();
    assert_eq!(st.url.as_deref(), Some("https://hooks.devall.test/abcdefghijklmnopqrst/"));

    // Backlog of three, raw bytes included.
    for (i, body) in [b"{\"a\":1}".to_vec(), vec![0, 255, 13, 10], b"third".to_vec()].into_iter().enumerate() {
        conn.send(Frame::json(ft::INBOX_EVENT, 0, &event(&format!("evt_{}", i + 1), i as u64 + 1, &body))).await;
    }
    let mut acked = Vec::new();
    while acked.len() < 3 {
        let ack: InboxAck = conn.expect(ft::INBOX_ACK, Some(0)).await.parse().unwrap();
        acked.extend(ack.ids);
    }
    assert_eq!(acked, vec!["evt_1", "evt_2", "evt_3"], "acks in order, only after success");
    assert_eq!(seen.lock().unwrap().iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["evt_1", "evt_2", "evt_3"]);
    assert_eq!(seen.lock().unwrap()[1].1, vec![0, 255, 13, 10]);
    assert_eq!(failures.load(Ordering::SeqCst), 2);

    // Reconnect: the relay redelivers evt_3 (its ack was "lost") plus a new one.
    conn.close(1012, "restart").await;
    drop(conn);
    let mut conn = relay.next_conn().await;
    conn.send_hello("inbox", "abcdefghijklmnopqrst").await;
    conn.send(Frame::json(ft::INBOX_EVENT, 0, &event("evt_3", 3, b"third"))).await;
    conn.send(Frame::json(ft::INBOX_EVENT, 0, &event("evt_4", 4, b"fourth"))).await;
    let mut acked = Vec::new();
    while acked.len() < 2 {
        let ack: InboxAck = conn.expect(ft::INBOX_ACK, Some(0)).await.parse().unwrap();
        acked.extend(ack.ids);
    }
    assert_eq!(acked, vec!["evt_3", "evt_4"]);
    assert_eq!(seen.lock().unwrap().len(), 4, "evt_3 was not handled twice");
    let st = inbox.status();
    assert_eq!(st.duplicates, 1);
    assert_eq!(st.delivered, 4);
    inbox.stop().await;
}

#[tokio::test]
async fn account_api_uses_the_bearer_token() {
    let addr = start_account_api(TOKEN).await;
    let c = RelayClient::new(RelayConfig::self_hosted(format!("ws://{addr}"), Some(TOKEN.into())), Arc::new(MemorySecretStore::default())).unwrap();
    let a = c.account().await.unwrap();
    assert_eq!(a.plan, "supporter");
    assert_eq!(a.reserved_subdomains, vec!["my-api"]);
    assert_eq!(a.usage.bandwidth_bytes, 1234);
    assert_eq!(a.inboxes[0].inbox_id, "abcdefghijklmnopqrst");
    let bad = RelayClient::new(RelayConfig::self_hosted(format!("ws://{addr}"), Some("nope".into())), Arc::new(MemorySecretStore::default())).unwrap();
    assert!(matches!(bad.account().await, Err(RelayError::Auth(_))));
}

#[tokio::test]
async fn loopback_sign_in_checks_state_and_stores_the_refresh_token() {
    let secrets = MemorySecretStore::default();
    let opened = Arc::new(Mutex::new(String::new()));
    let opened2 = opened.clone();
    let open = move |url: &str| {
        *opened2.lock().unwrap() = url.to_string();
        let u = url::Url::parse(url).unwrap();
        let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
        let port = q["port"].clone();
        let state = q["state"].clone();
        // Simulate the hosted page: a forged callback first, then the real one (form POST).
        tokio::spawn(async move {
            let http = reqwest::Client::new();
            let cb = format!("http://127.0.0.1:{port}/callback");
            let forged = http.get(format!("{cb}?state=wrong&refresh_token=evil")).send().await.unwrap();
            assert_eq!(forged.status(), 400);
            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("state", &state)
                .append_pair("refresh_token", "R-from-browser")
                .append_pair("uid", "uid-9")
                .append_pair("email", "dev@example.com")
                .finish();
            let ok = http.post(&cb).header("content-type", "application/x-www-form-urlencoded").body(form).send().await.unwrap();
            assert_eq!(ok.status(), 200);
            assert!(ok.text().await.unwrap().contains("signed in"));
        });
        Ok(())
    };
    let res = devall_relay::signin::sign_in_with_browser("https://auth.devall.test/signin.html", &secrets, open, T).await.unwrap();
    assert_eq!(res.uid.as_deref(), Some("uid-9"));
    assert_eq!(res.email.as_deref(), Some("dev@example.com"));
    assert_eq!(load_refresh_token(&secrets).unwrap().as_deref(), Some("R-from-browser"));
    let url = opened.lock().unwrap().clone();
    assert!(url.starts_with("https://auth.devall.test/signin.html?port="), "{url}");

    // Nobody completes the flow: timeout, nothing stored.
    let empty = MemorySecretStore::default();
    let err = devall_relay::signin::sign_in_with_browser("https://auth.devall.test/signin.html", &empty, |_| Ok(()), Duration::from_millis(200)).await.unwrap_err();
    assert!(matches!(err, RelayError::Timeout(_)));
    assert_eq!(load_refresh_token(&empty).unwrap(), None);
}
