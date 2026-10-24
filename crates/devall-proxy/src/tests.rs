//! End-to-end tests: a real proxy in front of local upstream servers (plain HTTP, HTTPS with a
//! separate test CA, WebSocket), driven by reqwest configured with the proxy and trusting only
//! the generated CA. Nothing is installed into any OS trust store.

use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::export::har::{HarOptions, from_har, to_har};
use crate::export::openapi::{OpenApiOptions, infer_yaml};
use crate::export::wiremock::{StubOptions, write_stubs};
use crate::rules::{BlockMode, HeaderMatch, HeaderTarget, NameValue};

// ---------- upstream test servers ----------

struct Upstream {
    http: SocketAddr,
    https: SocketAddr,
    ws: SocketAddr,
    wss: SocketAddr,
    ca_pem: String,
    _dir: tempfile::TempDir,
}

fn tls_server_config(ca: &CertAuthority, alpn: &[&[u8]]) -> Arc<rustls::ServerConfig> {
    let (chain, key) = ca.issue_leaf_pem(&["localhost", "127.0.0.1"]).unwrap();
    let certs = CertificateDer::pem_slice_iter(chain.as_bytes()).collect::<std::result::Result<Vec<_>, _>>().unwrap();
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).unwrap();
    let mut cfg = rustls::ServerConfig::builder_with_provider(ca::provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    cfg.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    Arc::new(cfg)
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn brotli_enc(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
        w.write_all(data).unwrap();
    }
    out
}

async fn upstream_svc(req: Request<Incoming>) -> std::result::Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let (parts, body) = req.into_parts();
    let body = body.collect().await.map(|c| c.to_bytes()).unwrap_or_default();
    let path = parts.uri.path().to_string();
    let compressed = |enc: &str, data: Vec<u8>| {
        Response::builder().header("content-type", "application/json").header("content-encoding", enc).body(Full::new(Bytes::from(data))).unwrap()
    };
    let payload = json!({ "items": [1, 2, 3], "name": "compressed" }).to_string();
    let resp = match path.as_str() {
        "/gzip" => compressed("gzip", gzip(payload.as_bytes())),
        "/br" => compressed("br", brotli_enc(payload.as_bytes())),
        "/zstd" => compressed("zstd", zstd::stream::encode_all(payload.as_bytes(), 3).unwrap()),
        p if p.starts_with("/status/") => {
            let code: u16 = p.trim_start_matches("/status/").parse().unwrap_or(500);
            Response::builder().status(code).body(Full::new(Bytes::new())).unwrap()
        }
        _ => {
            let headers: Vec<Value> = parts.headers.iter().map(|(k, v)| json!([k.as_str(), v.to_str().unwrap_or("")])).collect();
            let echo = json!({
                "method": parts.method.as_str(),
                "path": parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/"),
                "version": format!("{:?}", parts.version),
                "headers": headers,
                "body": String::from_utf8_lossy(&body),
            });
            Response::builder()
                .header("content-type", "application/json")
                .header("x-remove-me", "1")
                .header("x-upstream", "yes")
                .body(Full::new(Bytes::from(echo.to_string())))
                .unwrap()
        }
    };
    Ok(resp)
}

async fn serve_http(listener: TcpListener, tls: Option<Arc<rustls::ServerConfig>>) {
    loop {
        let Ok((s, _)) = listener.accept().await else {
            return;
        };
        let tls = tls.clone();
        tokio::spawn(async move {
            let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            match tls {
                Some(cfg) => {
                    let Ok(t) = tokio_rustls::TlsAcceptor::from(cfg).accept(s).await else {
                        return;
                    };
                    let _ = builder.serve_connection(TokioIo::new(t), service_fn(upstream_svc)).await;
                }
                None => {
                    let _ = builder.serve_connection(TokioIo::new(s), service_fn(upstream_svc)).await;
                }
            }
        });
    }
}

async fn ws_echo<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(s: S) {
    let Ok(mut ws) = tokio_tungstenite::accept_async(s).await else {
        return;
    };
    while let Some(Ok(m)) = ws.next().await {
        if m.is_text() || m.is_binary() {
            if ws.send(m).await.is_err() {
                return;
            }
        } else if m.is_close() {
            return;
        }
    }
}

async fn serve_ws(listener: TcpListener, tls: Option<Arc<rustls::ServerConfig>>) {
    loop {
        let Ok((s, _)) = listener.accept().await else {
            return;
        };
        let tls = tls.clone();
        tokio::spawn(async move {
            match tls {
                Some(cfg) => {
                    if let Ok(t) = tokio_rustls::TlsAcceptor::from(cfg).accept(s).await {
                        ws_echo(t).await;
                    }
                }
                None => ws_echo(s).await,
            }
        });
    }
}

async fn start_upstream() -> Upstream {
    let dir = tempfile::tempdir().unwrap();
    let ca = CertAuthority::create(dir.path(), "upstream test CA").unwrap();
    let bind = || async { TcpListener::bind("127.0.0.1:0").await.unwrap() };
    let (l1, l2, l3, l4) = (bind().await, bind().await, bind().await, bind().await);
    let addrs = (l1.local_addr().unwrap(), l2.local_addr().unwrap(), l3.local_addr().unwrap(), l4.local_addr().unwrap());
    tokio::spawn(serve_http(l1, None));
    tokio::spawn(serve_http(l2, Some(tls_server_config(&ca, &[b"h2", b"http/1.1"]))));
    tokio::spawn(serve_ws(l3, None));
    tokio::spawn(serve_ws(l4, Some(tls_server_config(&ca, &[b"http/1.1"]))));
    Upstream { http: addrs.0, https: addrs.1, ws: addrs.2, wss: addrs.3, ca_pem: ca.cert_pem().to_string(), _dir: dir }
}

// ---------- proxy under test ----------

struct Ctx {
    engine: ProxyEngine,
    port: u16,
    up: Upstream,
    _dir: tempfile::TempDir,
}

impl Ctx {
    fn client(&self) -> reqwest::Client {
        self.client_trusting(self.engine.ca().cert_pem())
    }

    fn client_trusting(&self, pem: &str) -> reqwest::Client {
        reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{}", self.port)).unwrap())
            .tls_certs_only([reqwest::Certificate::from_pem(pem.as_bytes()).unwrap()])
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap()
    }

    fn http(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.up.http.port())
    }

    fn https(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.up.https.port())
    }

    async fn wait(&self, what: &str, pred: impl Fn(&Exchange) -> bool) -> Exchange {
        let start = Instant::now();
        loop {
            if let Some(e) = self.engine.store().all().into_iter().rev().find(|e| pred(e)) {
                return e;
            }
            if start.elapsed() > Duration::from_secs(10) {
                let have: Vec<String> =
                    self.engine.store().all().iter().map(|e| format!("{} {} {:?} {:?}", e.request.method, e.request.url, e.state, e.error)).collect();
                panic!("timed out waiting for {what}; captured: {have:#?}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn done(&self, url_part: &str) -> Exchange {
        self.wait(url_part, |e| {
            e.request.url.contains(url_part) && !matches!(e.state, ExchangeState::Pending | ExchangeState::Receiving | ExchangeState::Paused)
        })
        .await
    }
}

async fn setup(adjust: impl FnOnce(&mut ProxyConfig, &Upstream)) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let ca = CertAuthority::create(dir.path().join("ca"), "devall test CA").unwrap();
    let up = start_upstream().await;
    let mut cfg = ProxyConfig { system_roots: false, extra_roots_pem: vec![up.ca_pem.clone()], ..Default::default() };
    adjust(&mut cfg, &up);
    let engine = ProxyEngine::new(ca, cfg).unwrap();
    let st = engine.start(Some(0)).await.unwrap();
    assert!(st.running);
    Ctx { port: st.port.unwrap(), engine, up, _dir: dir }
}

async fn json_of(resp: reqwest::Response) -> Value {
    serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap()
}

fn rule(v: Value) -> Rule {
    serde_json::from_value(v).unwrap()
}

// ---------- capture ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn captures_plain_http_with_header_order_and_bodies() {
    let ctx = setup(|_, _| {}).await;
    let resp = ctx
        .client()
        .post(ctx.http("/echo?q=1"))
        .header("x-zeta", "1")
        .header("x-alpha", "2")
        .header("x-mid", "3")
        .header("content-type", "application/json")
        .body(r#"{"hello":"world"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let echo: Value = json_of(resp).await;
    assert_eq!(echo["body"], r#"{"hello":"world"}"#);
    assert_eq!(echo["path"], "/echo?q=1");
    // Hop-by-hop proxy headers never reach the server.
    assert!(!echo["headers"].as_array().unwrap().iter().any(|h| h[0] == "proxy-connection"));

    let ex = ctx.done("/echo?q=1").await;
    assert_eq!(ex.state, ExchangeState::Complete);
    assert_eq!(ex.request.method, "POST");
    assert_eq!(ex.request.url, ctx.http("/echo?q=1"));
    let names: Vec<&str> = ex.request.headers.iter().map(|(k, _)| k.as_str()).filter(|k| k.starts_with("x-")).collect();
    assert_eq!(names, ["x-zeta", "x-alpha", "x-mid"]);
    assert_eq!(&ex.request.body.raw[..], br#"{"hello":"world"}"#);
    let r = ex.response.as_ref().unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(crate::util::header_value(&r.headers, "x-upstream"), Some("yes"));
    let body: Value = serde_json::from_slice(&r.body.raw).unwrap();
    assert_eq!(body["method"], "POST");
    assert!(ex.timing.total_ms.unwrap() > 0.0);
    assert!(ex.timing.connect_ms.is_some());
    assert!(ex.server_addr.is_some());

    // Filtering and summaries.
    let list = ctx.engine.list(&ExchangeFilter { text: "echo".into(), methods: vec!["post".into()], ..Default::default() });
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].status, Some(200));
    assert_eq!(list[0].content_type.as_deref(), Some("application/json"));
    let detail = ctx.engine.get(ex.id).unwrap();
    assert!(detail.response.unwrap().body.text.unwrap().contains("\"POST\""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decrypts_https_over_http2_and_decodes_compression() {
    let ctx = setup(|_, _| {}).await;
    let client = ctx.client();
    for (path, enc) in [("/gzip", "gzip"), ("/br", "br"), ("/zstd", "zstd")] {
        let resp = client.get(ctx.https(path)).send().await.unwrap();
        assert_eq!(resp.status(), 200, "{path}");
        let v: Value = json_of(resp).await;
        assert_eq!(v["items"], json!([1, 2, 3]));
        let ex = ctx.done(path).await;
        assert_eq!(ex.state, ExchangeState::Complete);
        assert_eq!(ex.request.http_version, "HTTP/2", "client ↔ proxy negotiated h2");
        assert!(ex.request.url.starts_with("https://localhost:"));
        let r = ex.response.as_ref().unwrap();
        assert_eq!(crate::util::header_value(&r.headers, "content-encoding"), Some(enc));
        // Raw bytes are kept compressed; the view decodes them.
        assert!(serde_json::from_slice::<Value>(&r.body.raw).is_err());
        let view = ctx.engine.get(ex.id).unwrap().response.unwrap().body;
        assert!(view.decoded, "{path}");
        assert_eq!(serde_json::from_str::<Value>(view.text.as_deref().unwrap()).unwrap()["name"], "compressed");
    }
    // The first exchange paid for the TLS handshake to the upstream; later ones reused the pool.
    let all = ctx.engine.store().all();
    assert!(all.iter().any(|e| e.timing.tls_ms.is_some()));
    assert!(all.iter().any(|e| e.timing.connection_reused == Some(true)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upstream_tls_is_verified_unless_ignored() {
    // The proxy does not trust the upstream test CA here.
    let other = tempfile::tempdir().unwrap();
    let unrelated = CertAuthority::create(other.path(), "unrelated").unwrap();
    let ctx = setup(|cfg, _| cfg.extra_roots_pem = vec![unrelated.cert_pem().to_string()]).await;
    let resp = ctx.client().get(ctx.https("/echo")).send().await.unwrap();
    assert_eq!(resp.status(), 502);
    let ex = ctx.done("/echo").await;
    assert_eq!(ex.state, ExchangeState::Failed);
    assert!(ex.error.as_deref().unwrap().contains("certificate"), "{:?}", ex.error);

    let mut cfg = ctx.engine.config();
    cfg.ignore_tls_errors = vec!["localhost".into()];
    ctx.engine.set_config(cfg).unwrap();
    let resp = ctx.client().get(ctx.https("/echo2")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bypassed_hosts_are_tunnelled_without_decryption() {
    let ctx = setup(|cfg, _| cfg.bypass = vec!["localhost".into()]).await;
    // The client trusts only the upstream's CA: this works only if the proxy does not intercept.
    // No pooling, so the tunnel closes when the response is read.
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{}", ctx.port)).unwrap())
        .tls_certs_only([reqwest::Certificate::from_pem(ctx.up.ca_pem.as_bytes()).unwrap()])
        .pool_max_idle_per_host(0)
        .http1_only()
        .build()
        .unwrap();
    let resp = client.get(ctx.https("/echo")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    drop(client);
    let ex = ctx.wait("closed tunnel", |e| e.state == ExchangeState::Tunnel && e.timing.total_ms.is_some()).await;
    assert_eq!(ex.request.method, "CONNECT");
    assert!(ex.tags.contains(&"bypassed".to_string()));
    assert!(ex.request.body.size > 0 && ex.response.as_ref().unwrap().body.size > 0);
    assert!(!ctx.engine.store().all().iter().any(|e| e.request.url.contains("/echo")), "nothing was decrypted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serves_info_page_and_ca_download() {
    let ctx = setup(|_, _| {}).await;
    let direct = reqwest::Client::builder().no_proxy().build().unwrap();
    let pem = direct.get(format!("http://127.0.0.1:{}/ca.pem", ctx.port)).send().await.unwrap().text().await.unwrap();
    assert_eq!(pem, ctx.engine.ca().cert_pem());
    let page = ctx.client().get("http://devall.proxy/").send().await.unwrap().text().await.unwrap();
    assert!(page.contains("traffic inspector"));
    assert!(ctx.engine.store().is_empty(), "the proxy's own pages are not captured");

    // Stop releases the port; start again on the same port works.
    ctx.engine.stop();
    assert!(!ctx.engine.status().running);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(TcpStream::connect(("127.0.0.1", ctx.port)).await.is_err());
    let st = ctx.engine.start(Some(ctx.port)).await.unwrap();
    assert_eq!(st.port, Some(ctx.port));
    assert!(TcpStream::connect(("127.0.0.1", ctx.port)).await.is_ok());
}

// ---------- rules ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rules_map_remote_mock_headers_local_throttle_and_block() {
    let ctx = setup(|_, _| {}).await;
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("data.json"), r#"{"from":"disk"}"#).unwrap();
    let up = ctx.up.http.port();
    ctx.engine
        .set_rules(vec![
            rule(json!({"name": "prod to local", "match": {"host": "api.example.invalid"}, "actions": [{"type": "mapRemote", "scheme": "http", "host": "127.0.0.1", "port": up}]})),
            rule(json!({"name": "headers", "match": {"path": "/echo*"}, "actions": [
                {"type": "setHeader", "name": "x-injected", "value": "yes"},
                {"type": "removeHeader", "target": "response", "name": "x-remove-me"}
            ]})),
            rule(json!({"name": "mock", "match": {"methods": ["GET"], "path": "/mocked"}, "actions": [{"type": "mockResponse", "status": 201, "headers": [{"name": "x-mock", "value": "1"}], "body": "{\"ok\":true}"}]})),
            rule(json!({"name": "local", "match": {"path": "/local/*"}, "actions": [{"type": "mapLocal", "file": local.path().display().to_string()}]})),
            rule(json!({"name": "slow", "match": {"path": "/slow"}, "actions": [{"type": "throttle", "latencyMs": 200}]})),
            rule(json!({"name": "forbid", "match": {"path": "/forbidden"}, "actions": [{"type": "block"}]})),
            rule(json!({"name": "drop", "match": {"path": "/dropped"}, "actions": [{"type": "block", "mode": "close"}]})),
            rule(json!({"name": "disabled", "enabled": false, "match": {}, "actions": [{"type": "block"}]})),
        ])
        .unwrap();
    let c = ctx.client();

    let resp = c.get("http://api.example.invalid/echo").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("x-remove-me").is_none());
    assert_eq!(resp.headers().get("x-upstream").unwrap(), "yes");
    let echo: Value = json_of(resp).await;
    assert!(echo["headers"].as_array().unwrap().iter().any(|h| h[0] == "x-injected" && h[1] == "yes"));
    assert!(echo["headers"].as_array().unwrap().iter().any(|h| h[0] == "host" && h[1] == format!("127.0.0.1:{up}")));
    let ex = ctx.done("api.example.invalid/echo").await;
    assert_eq!(ex.mapped_url.as_deref(), Some(format!("http://127.0.0.1:{up}/echo").as_str()));
    assert_eq!(ex.rules, ["prod to local", "headers"]);
    assert!(ex.tags.contains(&"mapRemote".to_string()));

    let resp = c.get(ctx.http("/mocked")).send().await.unwrap();
    assert_eq!(resp.status(), 201);
    assert_eq!(resp.headers().get("x-mock").unwrap(), "1");
    assert_eq!(resp.headers().get("content-type").unwrap(), "application/json");
    assert_eq!(resp.text().await.unwrap(), r#"{"ok":true}"#);
    assert!(ctx.done("/mocked").await.tags.contains(&"mocked".to_string()));

    let resp = c.get(ctx.http("/local/data.json")).send().await.unwrap();
    assert_eq!(resp.headers().get("content-type").unwrap(), "application/json");
    assert_eq!(resp.text().await.unwrap(), r#"{"from":"disk"}"#);
    assert_eq!(c.get(ctx.http("/local/missing.json")).send().await.unwrap().status(), 404);
    assert_eq!(c.get(ctx.http("/local/..%2fsecret")).send().await.unwrap().status(), 403);

    let t = Instant::now();
    assert_eq!(c.get(ctx.http("/slow")).send().await.unwrap().status(), 200);
    assert!(t.elapsed() >= Duration::from_millis(200));
    assert!(ctx.done("/slow").await.timing.blocked_ms.unwrap() >= 200.0);

    assert_eq!(c.get(ctx.http("/forbidden")).send().await.unwrap().status(), 403);
    assert_eq!(ctx.done("/forbidden").await.state, ExchangeState::Blocked);
    assert!(c.get(ctx.http("/dropped")).send().await.is_err());
    assert_eq!(ctx.done("/dropped").await.state, ExchangeState::Blocked);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bandwidth_throttle_slows_downloads() {
    let ctx = setup(|_, _| {}).await;
    ctx.engine
        .set_rules(vec![rule(
            json!({"name": "3g", "match": {"path": "/big"}, "actions": [{"type": "mockResponse", "body": "x".repeat(6000)}, {"type": "throttle", "kbps": 20}]}),
        )])
        .unwrap();
    let t = Instant::now();
    let body = ctx.client().get(ctx.http("/big")).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(body.len(), 6000);
    // 6000 bytes at 20 KiB/s ≈ 290 ms.
    assert!(t.elapsed() >= Duration::from_millis(200), "{:?}", t.elapsed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn breakpoints_pause_edit_resume_and_abort() {
    let ctx = setup(|_, _| {}).await;
    ctx.engine
        .set_rules(vec![
            rule(json!({"name": "bp", "match": {"path": "/echo", "header": {"name": "x-bp"}}, "actions": [{"type": "breakpoint"}]})),
            rule(json!({"name": "bp-resp", "match": {"path": "/status/*"}, "actions": [{"type": "breakpoint", "request": false, "response": true}]})),
        ])
        .unwrap();
    let mut events = ctx.engine.subscribe();
    let c = ctx.client();

    // Request breakpoint: edit headers and body, then resume.
    let url = ctx.http("/echo");
    let task = tokio::spawn({
        let c = c.clone();
        async move { json_of(c.post(url).header("x-bp", "1").body("original").send().await.unwrap()).await }
    });
    let paused = loop {
        match tokio::time::timeout(Duration::from_secs(10), events.recv()).await.expect("paused event").unwrap() {
            ProxyEvent::Paused { paused } => break paused,
            _ => continue,
        }
    };
    assert_eq!(paused.phase, Phase::Request);
    assert_eq!(paused.body.text.as_deref(), Some("original"));
    assert_eq!(ctx.engine.paused().len(), 1);
    assert_eq!(ctx.engine.status().paused, 1);
    let mut headers = paused.headers.clone();
    headers.push(("x-edited".into(), "true".into()));
    ctx.engine.resume(paused.id, Some(BreakpointEdit { headers: Some(headers), body: Some("changed body".into()), ..Default::default() })).unwrap();
    let echo = task.await.unwrap();
    assert_eq!(echo["body"], "changed body");
    assert!(echo["headers"].as_array().unwrap().iter().any(|h| h[0] == "x-edited"));
    let ex = ctx.done("/echo").await;
    assert!(ex.tags.contains(&"edited".to_string()));
    assert_eq!(&ex.request.body.raw[..], b"changed body");
    assert!(ex.timing.blocked_ms.is_some());
    assert!(ctx.engine.resume(paused.id, None).is_err(), "no longer paused");

    // Abort closes the client connection.
    let url = ctx.http("/echo");
    let task = tokio::spawn({
        let c = c.clone();
        async move { c.get(url).header("x-bp", "1").send().await }
    });
    let paused = loop {
        if let Some(p) = ctx.engine.paused().into_iter().next() {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    ctx.engine.abort(paused.id).unwrap();
    assert!(task.await.unwrap().is_err());
    assert_eq!(ctx.engine.store().get(paused.id).unwrap().state, ExchangeState::Aborted);

    // Response breakpoint: change the status.
    let url = ctx.http("/status/200");
    let task = tokio::spawn({
        let c = c.clone();
        async move { c.get(url).send().await.unwrap().status() }
    });
    let paused = loop {
        if let Some(p) = ctx.engine.paused().into_iter().next() {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(paused.phase, Phase::Response);
    assert_eq!(paused.status, Some(200));
    ctx.engine.resume(paused.id, Some(BreakpointEdit { status: Some(418), ..Default::default() })).unwrap();
    assert_eq!(task.await.unwrap(), 418);
    let ex = ctx.done("/status/200").await;
    assert_eq!(ex.response.as_ref().unwrap().status, 418);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn breakpoints_time_out_and_resume_unchanged() {
    let ctx = setup(|cfg, _| cfg.breakpoint_timeout_secs = 5).await;
    ctx.engine.set_rules(vec![rule(json!({"name": "bp", "match": {"path": "/echo"}, "actions": [{"type": "breakpoint"}]}))]).unwrap();
    let t = Instant::now();
    let resp = ctx.client().get(ctx.http("/echo")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(t.elapsed() >= Duration::from_secs(5));
    assert!(ctx.done("/echo").await.tags.contains(&"breakpointTimeout".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replays_with_edits() {
    let ctx = setup(|_, _| {}).await;
    ctx.client().post(ctx.http("/echo")).body("first").send().await.unwrap().text().await.unwrap();
    let ex = ctx.done("/echo").await;
    let id = ctx.engine.replay(ex.id, Some(BreakpointEdit { body: Some("second".into()), ..Default::default() })).await.unwrap();
    assert_ne!(id, ex.id);
    let replayed = ctx.engine.store().get(id).unwrap();
    assert_eq!(replayed.state, ExchangeState::Complete);
    assert!(replayed.tags.contains(&"replay".to_string()));
    let body: Value = serde_json::from_slice(&replayed.response.unwrap().body.raw).unwrap();
    assert_eq!(body["body"], "second");
}

// ---------- WebSocket ----------

async fn connect_through(proxy: u16, target: &str) -> TcpStream {
    let mut tcp = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    tcp.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        tcp.read_exact(&mut byte).await.unwrap();
        buf.push(byte[0]);
    }
    assert!(String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 200"), "{}", String::from_utf8_lossy(&buf));
    tcp
}

async fn ws_round_trip<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(stream: S, url: &str) {
    let (mut ws, resp) = tokio_tungstenite::client_async(url, stream).await.unwrap();
    assert_eq!(resp.status(), 101);
    ws.send(Message::text("hello")).await.unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap(), Message::text("hello"));
    ws.send(Message::binary(vec![1u8, 2, 3])).await.unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap(), Message::binary(vec![1u8, 2, 3]));
    ws.close(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_frames_pass_through_and_are_captured() {
    let ctx = setup(|_, _| {}).await;
    // ws:// through a plain CONNECT tunnel.
    let target = format!("127.0.0.1:{}", ctx.up.ws.port());
    let tcp = connect_through(ctx.port, &target).await;
    ws_round_trip(tcp, &format!("ws://{target}/socket")).await;
    let ex = ctx.wait("ws frames", |e| e.request.url.ends_with("/socket") && e.ws_frames.len() >= 4).await;
    assert_eq!(ex.state, ExchangeState::WebSocket);
    assert_eq!(ex.response.as_ref().unwrap().status, 101);
    let f = &ex.ws_frames;
    assert_eq!((f[0].direction, f[0].opcode.as_str(), f[0].text.as_deref()), (WsDirection::Send, "text", Some("hello")));
    assert_eq!((f[1].direction, f[1].opcode.as_str()), (WsDirection::Receive, "text"));
    assert_eq!((f[2].opcode.as_str(), f[2].base64.as_deref()), ("binary", Some("AQID")));

    // wss:// through the TLS interception (HTTP/1.1 ALPN, as browsers do for WebSockets).
    let target = format!("localhost:{}", ctx.up.wss.port());
    let tcp = connect_through(ctx.port, &target).await;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ctx.engine.ca().cert_der().clone()).unwrap();
    let mut cfg = rustls::ClientConfig::builder_with_provider(ca::provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(ServerName::try_from("localhost").unwrap(), tcp).await.unwrap();
    ws_round_trip(tls, &format!("wss://{target}/secure")).await;
    let ex = ctx.wait("wss frames", |e| e.request.url.ends_with("/secure") && e.ws_frames.len() >= 4).await;
    assert!(ex.request.url.starts_with("https://localhost:"));
    assert_eq!(ex.ws_frame_count, ex.ws_frames.len());
}

// ---------- exports ----------

fn synthetic(method: &str, url: &str, status: u16, req_body: Option<&str>, resp_body: &str, req_headers: &[(&str, &str)]) -> Exchange {
    let mut headers: HeaderList = req_headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    if req_body.is_some() {
        headers.push(("content-type".into(), "application/json".into()));
    }
    let mut e = Exchange::new(method, url, "HTTP/1.1", headers);
    if let Some(b) = req_body {
        e.request.body = CapturedBody::new(Bytes::from(b.to_string()), usize::MAX);
    }
    let ct = if resp_body.starts_with('{') || resp_body.starts_with('[') { "application/json" } else { "text/plain" };
    e.response = Some(CapturedResponse {
        status,
        reason: String::new(),
        http_version: "HTTP/1.1".into(),
        headers: vec![("content-type".into(), ct.into()), ("date".into(), "Tue, 01 Oct 2024 00:00:00 GMT".into()), ("set-cookie".into(), "sid=abc".into())],
        body: CapturedBody::new(Bytes::from(resp_body.to_string()), usize::MAX),
    });
    e.state = ExchangeState::Complete;
    e.started_at = 1_700_000_000_000;
    e.timing = Timing {
        dns_ms: Some(1.0),
        connect_ms: Some(2.0),
        tls_ms: Some(3.0),
        wait_ms: Some(10.0),
        receive_ms: Some(1.0),
        total_ms: Some(17.0),
        ..Default::default()
    };
    e
}

fn traffic() -> Vec<Exchange> {
    let auth = [("authorization", "Bearer secret-token"), ("cookie", "sid=abc; theme=dark")];
    vec![
        synthetic(
            "GET",
            "https://api.example.com/users?page=1&limit=20",
            200,
            None,
            r#"[{"id":1,"name":"Ann","email":"ann@example.com","createdAt":"2024-01-01T10:00:00Z"}]"#,
            &auth,
        ),
        synthetic("GET", "https://api.example.com/users?page=2", 200, None, r#"[]"#, &auth),
        synthetic(
            "GET",
            "https://api.example.com/users/42",
            200,
            None,
            r#"{"id":42,"name":"Bob","email":"bob@example.com","manager":null,"tags":["a"]}"#,
            &auth,
        ),
        synthetic("GET", "https://api.example.com/users/7", 200, None, r#"{"id":7,"name":"Cy","email":"cy@example.com","manager":{"id":42},"tags":[]}"#, &auth),
        synthetic("GET", "https://api.example.com/users/9999", 404, None, r#"{"error":"not found"}"#, &auth),
        synthetic("POST", "https://api.example.com/users", 201, Some(r#"{"name":"Dee","email":"dee@example.com"}"#), r#"{"id":43,"name":"Dee"}"#, &auth),
        synthetic(
            "POST",
            "https://api.example.com/users",
            201,
            Some(r#"{"name":"Eve","email":"eve@example.com","admin":true}"#),
            r#"{"id":44,"name":"Eve"}"#,
            &auth,
        ),
        synthetic("GET", "https://api.example.com/orders/3fa85f64-5717-4562-b3fc-2c963f66afa6", 200, None, r#"{"total":12.5,"items":2}"#, &[]),
        synthetic("GET", "https://api.example.com/repos/alice/issues", 200, None, r#"[{"n":1}]"#, &[]),
        synthetic("GET", "https://api.example.com/repos/bob/issues", 200, None, r#"[]"#, &[]),
        synthetic("GET", "https://api.example.com/repos/carol/issues", 200, None, r#"[{"n":2}]"#, &[]),
        synthetic("DELETE", "https://api.example.com/users/42", 204, None, "", &auth),
        synthetic("GET", "https://api.example.com/health", 200, None, "ok", &[]),
    ]
}

#[test]
fn har_export_is_valid_and_redacts_secrets() {
    let ex = traffic();
    let har = to_har(&ex, &HarOptions::default());
    assert_eq!(har["log"]["version"], "1.2");
    assert_eq!(har["log"]["creator"]["name"], "devall");
    let entries = har["log"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), ex.len());
    let first = &entries[0];
    for key in ["startedDateTime", "time", "request", "response", "cache", "timings"] {
        assert!(first.get(key).is_some(), "entry.{key}");
    }
    assert_eq!(first["startedDateTime"], "2023-11-14T22:13:20.000Z");
    assert_eq!(first["request"]["queryString"], json!([{"name": "page", "value": "1"}, {"name": "limit", "value": "20"}]));
    assert_eq!(first["timings"]["connect"], 5.0);
    assert_eq!(first["timings"]["ssl"], 3.0);
    let text = har.to_string();
    assert!(!text.contains("secret-token") && !text.contains("sid=abc") && !text.contains("\"abc\""), "secrets redacted");
    assert!(first["request"]["headers"].as_array().unwrap().iter().any(|h| h["name"] == "authorization" && h["value"] == "REDACTED"));
    assert_eq!(first["request"]["cookies"][0], json!({"name": "sid", "value": "REDACTED"}));
    let post = &entries[5];
    assert_eq!(post["request"]["postData"]["mimeType"], "application/json");
    assert!(post["request"]["postData"]["text"].as_str().unwrap().contains("Dee"));

    let full = to_har(&ex, &HarOptions { include_sensitive: true }).to_string();
    assert!(full.contains("Bearer secret-token"));

    // Round trip through the importer.
    let back = from_har(&full).unwrap();
    assert_eq!(back.len(), ex.len());
    assert_eq!(back[2].request.url, "https://api.example.com/users/42");
    assert_eq!(back[4].response.as_ref().unwrap().status, 404);
    assert_eq!(&back[5].request.body.raw[..], br#"{"name":"Dee","email":"dee@example.com"}"#);
    assert!(back[0].tags.contains(&"imported".to_string()));
    assert_eq!(back[0].timing.tls_ms, Some(3.0));
    assert_eq!(back[0].timing.connect_ms, Some(2.0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wiremock_stubs_generalize_paths_and_load_in_devall_mock() {
    let dir = tempfile::tempdir().unwrap();
    devall_core::Project::init(dir.path(), "stubs").unwrap();
    let mut ex = traffic();
    let png: Vec<u8> = std::iter::repeat_n(0u8, 70 * 1024).chain([0x89, b'P', b'N', b'G']).collect();
    let mut img = synthetic("GET", "https://cdn.example.com/img/logo.png", 200, None, "", &[]);
    img.response.as_mut().unwrap().headers = vec![("content-type".into(), "image/png".into())];
    img.response.as_mut().unwrap().body = CapturedBody::new(Bytes::from(png.clone()), usize::MAX);
    ex.push(img);
    let res = write_stubs(dir.path(), &ex, &StubOptions::default()).unwrap();
    let files: Vec<&str> = res.written.iter().map(|w| w.file.as_str()).collect();
    assert!(files.contains(&"mocks/mappings/get-users-id.json"), "{files:?}");
    assert!(files.contains(&"mocks/mappings/get-users-2.json"), "another query → its own stub: {files:?}");
    assert!(files.contains(&"mocks/mappings/post-users-2.json"), "another body → its own stub: {files:?}");
    // /users/7 and /users/9999 generalize to the same matcher as /users/42.
    assert_eq!(res.skipped.iter().filter(|s| s.contains("same request matcher")).count(), 2, "{:?}", res.skipped);
    assert_eq!(res.written.len(), ex.len() - 2);

    let stub: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("mocks/mappings/get-users-id.json")).unwrap()).unwrap();
    let keys: Vec<&String> = stub.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["name", "request", "response"]);
    assert_eq!(stub["name"], "GET /users/{id}");
    assert_eq!(stub["request"], json!({"method": "GET", "urlPathPattern": "/users/[0-9]+"}));
    assert_eq!(stub["response"]["status"], 200);
    assert_eq!(stub["response"]["headers"], json!({"Content-Type": "application/json"}), "date and set-cookie are dropped");
    assert_eq!(stub["response"]["jsonBody"]["name"], "Bob");
    let list: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("mocks/mappings/get-users.json")).unwrap()).unwrap();
    assert_eq!(list["request"]["queryParameters"], json!({"page": {"equalTo": "1"}, "limit": {"equalTo": "20"}}));
    let post: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("mocks/mappings/post-users.json")).unwrap()).unwrap();
    assert_eq!(post["request"]["bodyPatterns"][0], json!({"equalToJson": {"name": "Dee", "email": "dee@example.com"}, "ignoreExtraElements": true}));
    let uuid: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("mocks/mappings/get-orders-id.json")).unwrap()).unwrap();
    assert!(uuid["request"]["urlPathPattern"].as_str().unwrap().starts_with("/orders/[0-9a-fA-F]{8}-"));
    let img: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("mocks/mappings/get-img-logo-png.json")).unwrap()).unwrap();
    assert_eq!(img["response"]["bodyFileName"], "get-img-logo-png.png");
    assert_eq!(std::fs::read(dir.path().join("mocks/__files/get-img-logo-png.png")).unwrap(), png);

    // The mock engine loads every file and answers with the captured responses.
    let handle = devall_mock::MockServer::start(devall_mock::MockConfig::new(dir.path())).await.unwrap();
    assert!(handle.load_errors().is_empty(), "{:?}", handle.load_errors());
    let c = reqwest::Client::builder().no_proxy().build().unwrap();
    let r = c.get(format!("{}/users/12345", handle.url())).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(json_of(r).await["name"], "Bob");
    let r = c
        .post(format!("{}/users", handle.url()))
        .header("content-type", "application/json")
        .body(json!({"email": "dee@example.com", "name": "Dee", "x": 1}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = c.get(format!("{}/orders/0b6f4a70-0000-4000-8000-000000000001", handle.url())).send().await.unwrap();
    assert_eq!(json_of(r).await["total"], 12.5);
    assert_eq!(c.get(format!("{}/img/logo.png", handle.url())).send().await.unwrap().bytes().await.unwrap().len(), png.len());
    handle.stop().await;

    // Writing again never overwrites.
    let again = write_stubs(dir.path(), &ex[..1], &StubOptions { generalize: false, ..Default::default() }).unwrap();
    assert_eq!(again.written[0].file, "mocks/mappings/get-users-3.json");
}

const OPENAPI_SNAPSHOT: &str = include_str!("../tests/fixtures/openapi_snapshot.yaml");

#[test]
fn openapi_inference_is_stable() {
    let yaml = infer_yaml(&traffic(), &OpenApiOptions::default());
    assert_eq!(yaml, infer_yaml(&traffic(), &OpenApiOptions::default()), "deterministic");
    if std::env::var_os("DEVALL_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/openapi_snapshot.yaml"), &yaml).unwrap();
        return;
    }
    assert_eq!(yaml.replace("\r\n", "\n"), OPENAPI_SNAPSHOT.replace("\r\n", "\n"));
    let doc: Value = serde_yaml_ng::from_str(&yaml).unwrap();
    assert_eq!(doc["openapi"], "3.1.0");
    let paths: Vec<&String> = doc["paths"].as_object().unwrap().keys().collect();
    assert_eq!(paths, ["/health", "/orders/{orderId}", "/repos/{repo}/issues", "/users", "/users/{userId}"]);
    let user = &doc["paths"]["/users/{userId}"]["get"];
    assert_eq!(user["parameters"][0], json!({"name": "userId", "in": "path", "required": true, "schema": {"type": "integer"}}));
    assert_eq!(user["responses"]["200"]["content"]["application/json"]["schema"]["required"], json!(["id", "name", "email", "manager", "tags"]));
    assert_eq!(user["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["manager"]["type"], json!(["object", "null"]));
    assert!(user["responses"]["404"].is_object());
    let list = &doc["paths"]["/users"]["get"];
    assert_eq!(list["parameters"][0]["required"], true);
    assert!(list["parameters"][1].get("required").is_none(), "limit is optional");
    let create = &doc["paths"]["/users"]["post"];
    assert_eq!(create["requestBody"]["content"]["application/json"]["schema"]["required"], json!(["name", "email"]));
    assert_eq!(doc["components"]["securitySchemes"]["bearerAuth"]["scheme"], "bearer");
    assert_eq!(doc["paths"]["/orders/{orderId}"]["get"]["parameters"][0]["schema"]["format"], "uuid");
    assert!(!yaml.contains("secret-token") && !yaml.contains("ann@example.com"), "no captured values without examples");
}

#[test]
fn saved_requests_use_secret_references_and_load_scenarios_validate() {
    let dir = tempfile::tempdir().unwrap();
    let project = devall_core::Project::init(dir.path(), "reqs").unwrap();
    let ex = traffic();
    let paths = crate::export::requests::save_requests(dir.path(), &ex[..6], "captured").unwrap();
    assert_eq!(paths.len(), 6);
    assert!(paths[0].starts_with("captured/"));
    let first = project.read_request(&paths[0]).unwrap();
    assert_eq!(first.method, "GET");
    assert_eq!(first.url, "https://api.example.com/users");
    assert_eq!(first.params.iter().map(|p| (p.name.as_str(), p.value.as_str())).collect::<Vec<_>>(), [("page", "1"), ("limit", "20")]);
    assert!(matches!(&first.auth, devall_core::Auth::Bearer { token, .. } if token == "{{secret.BEARER_TOKEN}}"));
    assert!(!first.headers.iter().any(|h| h.name.eq_ignore_ascii_case("cookie") || h.name.eq_ignore_ascii_case("authorization")));
    let raw = std::fs::read_to_string(project.http_root().join(&paths[0])).unwrap();
    assert!(!raw.contains("secret-token") && !raw.contains("sid=abc"));
    let post = project.read_request(&paths[5]).unwrap();
    assert!(matches!(&post.body, devall_core::Body::Json { content } if content.contains("\"Dee\"")));

    let mut flow = ex.clone();
    flow.push(ex[0].clone());
    let load = crate::export::load::save_load_test(dir.path(), &flow, "Users smoke").unwrap();
    assert_eq!(load.load_file, "users-smoke.load.yaml");
    assert_eq!(load.requests.len(), ex.len(), "identical requests are saved once");
    let test = devall_load::store::read_test(dir.path(), &load.load_file).unwrap();
    assert_eq!(test.scenario.flow.len(), flow.len());
    assert_eq!(test.scenario.flow.first(), test.scenario.flow.last());
    assert!(devall_load::validate(&test).is_empty(), "{:?}", devall_load::validate(&test));
}

#[test]
fn curl_export_redacts_by_default() {
    let ex = &traffic()[5];
    let curl = crate::export::curl::to_curl(ex, false);
    assert!(curl.starts_with("curl \\\n  -X POST \\\n  https://api.example.com/users"), "{curl}");
    assert!(curl.contains("authorization: REDACTED"));
    assert!(curl.contains("--data-raw"));
    assert!(crate::export::curl::to_curl(ex, true).contains("Bearer secret-token"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sessions_save_and_load_back() {
    devall_core::local::isolate_for_tests();
    let dir = tempfile::tempdir().unwrap();
    devall_core::Project::init(dir.path(), "sessions").unwrap();
    let file = crate::session::save_session(dir.path(), Some("Login flow"), &traffic(), false).unwrap();
    assert_eq!(file, "login-flow.har");
    assert!(devall_core::local::project_dir(dir.path()).join("captures/login-flow.har").is_file());
    assert!(!dir.path().join(".devall").exists());
    assert_eq!(crate::session::list_sessions(dir.path()).unwrap()[0].file, "login-flow.har");
    let loaded = crate::session::read_session(dir.path(), &file).unwrap();
    assert_eq!(loaded.len(), traffic().len());
    assert!(crate::session::read_session(dir.path(), "../devall.yaml").is_err());

    let ca_dir = tempfile::tempdir().unwrap();
    let engine = ProxyEngine::new(
        CertAuthority::create(ca_dir.path(), "x").unwrap(),
        ProxyConfig {
            system_roots: false,
            extra_roots_pem: vec![CertAuthority::create(ca_dir.path().join("u"), "u").unwrap().cert_pem().to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    let ids = engine.import(loaded);
    assert_eq!(engine.list(&ExchangeFilter::default()).len(), ids.len());
    assert_eq!(engine.get(ids[0]).unwrap().summary.tags, ["imported"]);
}

#[test]
fn rule_header_match_types_compile() {
    // Typed construction (as the Tauri layer does) round-trips through JSON.
    let r = Rule {
        id: String::new(),
        name: "typed".into(),
        enabled: true,
        matcher: RuleMatch {
            methods: vec!["POST".into()],
            host: Some("*.example.com".into()),
            path: None,
            path_regex: Some("^/v[0-9]+/".into()),
            header: Some(HeaderMatch { name: "x-a".into(), value: None }),
        },
        actions: vec![
            RuleAction::SetHeader { target: HeaderTarget::Response, name: "x-b".into(), value: "1".into() },
            RuleAction::Block { mode: BlockMode::Close },
            RuleAction::MockResponse { status: 200, headers: vec![NameValue { name: "a".into(), value: "b".into() }], body: String::new(), delay_ms: 0 },
        ],
    };
    let text = crate::rules::rules_json(std::slice::from_ref(&r)).unwrap();
    let back = crate::rules::parse_rules(&text).unwrap();
    assert_eq!(back[0].matcher, r.matcher);
    assert_eq!(back[0].actions, r.actions);
}

// ---------- OpenAPI validation rule ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validation_rule_flags_and_blocks_spec_violations() {
    let ctx = setup(|_, _| {}).await;
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("api.yaml");
    std::fs::write(
        &spec,
        format!(
            r#"openapi: 3.1.0
info: {{ title: T, version: '1' }}
servers: [{{ url: 'http://127.0.0.1:{port}' }}]
paths:
  /echo:
    post:
      requestBody: {{ required: true, content: {{ application/json: {{ schema: {{ type: object, required: [sku], properties: {{ sku: {{ type: string }} }} }} }} }} }}
      responses: {{ '200': {{ description: ok, content: {{ application/json: {{ schema: {{ type: object, required: [method, path] }} }} }} }} }}
  /status/{{code}}:
    get:
      parameters: [{{ name: code, in: path, required: true, schema: {{ type: integer }} }}]
      responses: {{ '200': {{ description: ok }} }}
  /strict/echo:
    post:
      requestBody: {{ required: true, content: {{ application/json: {{ schema: {{ type: object, required: [sku] }} }} }} }}
      responses: {{ '200': {{ description: ok, content: {{ application/json: {{ schema: {{ type: object, required: [total] }} }} }} }} }}
"#,
            port = ctx.up.http.port()
        ),
    )
    .unwrap();
    let spec_path = spec.display().to_string();
    ctx.engine
        .set_rules(vec![
            rule(json!({"name": "strict", "match": {"path": "/strict/*"}, "actions": [{"type": "validateSpec", "spec": spec_path, "mode": "block"}]})),
            rule(json!({"name": "spec", "actions": [{"type": "validateSpec", "spec": spec_path}]})),
        ])
        .unwrap();
    assert!(crate::rules::validate(&[rule(json!({"name": "bad", "actions": [{"type": "validateSpec", "spec": "relative.yaml"}]}))]).is_err());
    assert!(crate::rules::validate(&[rule(json!({"name": "missing", "actions": [{"type": "validateSpec", "spec": dir.path().join("nope.yaml").display().to_string()}]}))]).is_err());
    let c = ctx.client();
    let checked = |url: &'static str| move |e: &Exchange| e.request.url.ends_with(url) && e.validation.as_ref().is_some_and(|v| v.response_checked);

    // Conforming: nothing flagged (the response is checked after it streamed).
    assert_eq!(c.post(ctx.http("/echo")).header("content-type", "application/json").body(r#"{"sku":"A1"}"#).send().await.unwrap().status(), 200);
    let ok = ctx.wait("ok", checked("/echo")).await;
    assert_eq!(ok.validation.as_ref().unwrap().count(), 0, "{:?}", ok.validation);
    assert!(!ok.tags.contains(&"specViolation".to_string()));

    // Flag mode forwards but records the problems.
    assert_eq!(c.post(ctx.http("/echo?x=1")).header("content-type", "application/json").body(r#"{"qty":1}"#).send().await.unwrap().status(), 200);
    let bad = ctx.wait("bad body", checked("/echo?x=1")).await;
    let v = bad.validation.as_ref().unwrap();
    assert_eq!(v.request[0].kind, devall_openapi::drift::DriftKind::InvalidRequestBody);
    assert!(bad.tags.contains(&"specViolation".to_string()) && !v.blocked);
    assert_eq!(bad.summary().violations, 1);
    assert_eq!(c.get(ctx.http("/status/500")).send().await.unwrap().status(), 500);
    let status = ctx.wait("status", checked("/status/500")).await;
    assert_eq!(status.validation.unwrap().response[0].kind, devall_openapi::drift::DriftKind::UndocumentedStatus);
    c.get(ctx.http("/nope")).send().await.unwrap();
    let nope = ctx.done("/nope").await;
    assert_eq!(nope.validation.unwrap().request[0].kind, devall_openapi::drift::DriftKind::UndocumentedPath);

    // Block mode: a bad request is answered with 422 and never forwarded…
    let r = c.post(ctx.http("/strict/echo")).header("content-type", "application/json").body(r#"{"qty":1}"#).send().await.unwrap();
    assert_eq!(r.status(), 422);
    assert_eq!(r.headers().get("content-type").unwrap(), "application/problem+json");
    let problem: Value = json_of(r).await;
    assert_eq!(problem["violations"][0]["kind"], "invalidRequestBody");
    let blocked = ctx.done("/strict/echo").await;
    assert_eq!(blocked.state, ExchangeState::Blocked);
    assert!(blocked.server_addr.is_none(), "not forwarded");
    // …and a non-conforming upstream response is replaced with 502.
    let r = c.post(ctx.http("/strict/echo?ok=1")).header("content-type", "application/json").body(r#"{"sku":"x"}"#).send().await.unwrap();
    assert_eq!(r.status(), 502);
    let problem: Value = json_of(r).await;
    assert_eq!(problem["title"], "The response does not match the OpenAPI spec");
    let replaced = ctx.done("/strict/echo?ok=1").await;
    assert!(replaced.validation.as_ref().unwrap().blocked);

    // Hosts outside the spec's servers are not checked.
    c.get(format!("http://localhost:{}/nope", ctx.up.http.port())).send().await.unwrap();
    assert!(ctx.done("localhost").await.validation.is_none());
}
