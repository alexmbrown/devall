use std::path::Path;

use devall_core::vars::NoSecrets;
use devall_core::{Auth, Body, HttpRequestFile, KeyValue, Resolver};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

/// Minimal HTTP/1.1 server: echoes the raw request back as the body. `/redirect` answers 302.
async fn echo_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                // Read headers, then Content-Length bytes of body.
                loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        while buf.len() < pos + 4 + len {
                            let n = sock.read(&mut tmp).await.unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            buf.extend_from_slice(&tmp[..n]);
                        }
                        break;
                    }
                }
                let req = String::from_utf8_lossy(&buf).to_string();
                let resp = if req.starts_with("GET /redirect") {
                    "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                } else {
                    format!(
                        "HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\nX-Test: a\r\nX-Test: b\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        req.len(),
                        req
                    )
                };
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

fn prep(req: &HttpRequestFile) -> PreparedRequest {
    let mut r = Resolver::new(&NoSecrets);
    r.push_layer([("name".to_string(), "world".to_string())].into_iter().collect());
    prepare(req, &mut r, Path::new(".")).unwrap()
}

#[tokio::test]
async fn sends_params_headers_json_and_measures() {
    let base = echo_server().await;
    let mut req = HttpRequestFile::new("t");
    req.method = "post".into();
    req.url = format!("{base}/hello");
    req.params = vec![KeyValue::new("q", "{{name}} & co"), KeyValue { enabled: false, ..KeyValue::new("off", "1") }];
    req.headers = vec![KeyValue::new("X-Name", "{{name}}")];
    req.auth = Auth::Bearer { token: "abc".into(), prefix: None };
    req.body = Body::Json { content: "{\"hi\":\"{{name}}\"}".into() };
    let p = prep(&req);
    assert_eq!(p.method, "POST");
    assert!(p.url.ends_with("/hello?q=world+%26+co"), "{}", p.url);

    let resp = ClientPool::new().execute(&p).await.unwrap();
    assert_eq!(resp.status, 201);
    assert_eq!(resp.status_text, "Created");
    let echoed = String::from_utf8(resp.body.clone()).unwrap();
    assert!(echoed.starts_with("POST /hello?q=world+%26+co HTTP/1.1"), "{echoed}");
    let lower = echoed.to_lowercase();
    assert!(lower.contains("x-name: world"));
    assert!(lower.contains("authorization: bearer abc"));
    assert!(lower.contains("content-type: application/json"));
    assert!(echoed.ends_with("{\"hi\":\"world\"}"));
    // Duplicate headers preserved in order.
    let xs: Vec<_> = resp.headers.iter().filter(|(k, _)| k == "x-test").map(|(_, v)| v.as_str()).collect();
    assert_eq!(xs, ["a", "b"]);
    assert!(resp.timings.total_ms >= resp.timings.ttfb_ms);
    let view = resp.into_view();
    assert!(view.body_text.is_some() && view.body_base64.is_none());
}

#[tokio::test]
async fn form_basic_auth_and_redirects() {
    let base = echo_server().await;
    let mut req = HttpRequestFile::new("t");
    req.method = "POST".into();
    req.url = format!("{base}/form");
    req.auth = Auth::Basic { username: "u".into(), password: "p".into() };
    req.body = Body::Form { fields: vec![KeyValue::new("a", "1 2"), KeyValue::new("b", "{{name}}")] };
    let resp = ClientPool::new().execute(&prep(&req)).await.unwrap();
    let echoed = String::from_utf8(resp.body).unwrap();
    assert!(echoed.to_lowercase().contains("authorization: basic dtpw"), "{echoed}");
    assert!(echoed.ends_with("a=1+2&b=world"), "{echoed}");

    let mut req = HttpRequestFile::new("r");
    req.url = format!("{base}/redirect");
    let pool = ClientPool::new();
    let resp = pool.execute(&prep(&req)).await.unwrap();
    assert!(resp.redirected);
    assert!(resp.url.ends_with("/final"));
    req.settings.follow_redirects = false;
    let resp = pool.execute(&prep(&req)).await.unwrap();
    assert_eq!(resp.status, 302);
}

#[tokio::test]
async fn connection_errors_are_classified() {
    let mut req = HttpRequestFile::new("t");
    req.url = "http://127.0.0.1:9".into();
    req.settings.timeout_ms = 3000;
    match ClientPool::new().execute(&prep(&req)).await {
        Err(HttpError::Connect(_)) | Err(HttpError::Timeout(_)) => {}
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn prepare_defaults_and_errors() {
    let mut req = HttpRequestFile::new("t");
    req.url = "localhost:3000/x".into();
    req.auth = Auth::ApiKey { key: "api_key".into(), value: "k".into(), location: devall_core::ApiKeyLocation::Query };
    let p = prep(&req);
    assert_eq!(p.url, "http://localhost:3000/x?api_key=k");
    assert_eq!(p.header("accept"), Some("*/*"));
    assert!(p.header("user-agent").unwrap().starts_with("devall/"));

    let mut bad = HttpRequestFile::new("t");
    bad.url = "".into();
    let mut r = Resolver::new(&NoSecrets);
    assert!(matches!(prepare(&bad, &mut r, Path::new(".")), Err(HttpError::InvalidUrl(_))));

    let mut gql = HttpRequestFile::new("g");
    gql.url = "http://x".into();
    gql.body = Body::Graphql { query: "{ a }".into(), variables: "{nope".into(), operation_name: None };
    assert!(matches!(prepare(&gql, &mut r, Path::new(".")), Err(HttpError::InvalidRequest(_))));
}

#[test]
fn curl_round_trip() {
    let mut req = HttpRequestFile::new("t");
    req.method = "PUT".into();
    req.url = "https://api.example.com/items/1".into();
    req.headers = vec![KeyValue::new("X-It's", "o'k")];
    req.body = Body::Json { content: "{\"a\":1}".into() };
    let cmd = to_curl(&prep(&req));
    assert!(cmd.contains("-X PUT"));
    assert!(cmd.contains("'X-It'\\''s: o'\\''k'"), "{cmd}");
    let back = devall_core::curl::parse_curl(&cmd).unwrap();
    assert_eq!(back.method, "PUT");
    assert_eq!(back.url, req.url);
    assert!(back.headers.iter().any(|h| h.name == "X-It's" && h.value == "o'k"));
    assert!(matches!(back.body, Body::Json { .. }));
}

/// An NTLM server: the challenge is only honoured on the connection that negotiated it.
async fn ntlm_server(password: &'static str) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = std::sync::Arc::new(AtomicUsize::new(0));
    let conns = connections.clone();
    let server_challenge = [7u8, 6, 5, 4, 3, 2, 1, 0];
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            conns.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut negotiated = false;
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    // One request per iteration (GET, no body).
                    let end = loop {
                        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break p + 4;
                        }
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).to_string();
                    buf.drain(..end);
                    let auth = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim().to_string()));
                    let msg = auth.as_deref().and_then(|a| a.strip_prefix("NTLM ")).and_then(|t| base64::engine::general_purpose::STANDARD.decode(t).ok()).unwrap_or_default();
                    let kind = msg.get(8..12).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                    let (status, extra, body) = match kind {
                        Some(1) => {
                            negotiated = true;
                            let info = hex::decode("02000800430030005200500000000000").unwrap();
                            let mut c = b"NTLMSSP\0".to_vec();
                            c.extend_from_slice(&2u32.to_le_bytes());
                            c.extend_from_slice(&[0, 0, 0, 0, 48, 0, 0, 0]);
                            c.extend_from_slice(&0xe28a_8235u32.to_le_bytes());
                            c.extend_from_slice(&server_challenge);
                            c.extend_from_slice(&[0; 8]);
                            c.extend_from_slice(&(info.len() as u16).to_le_bytes());
                            c.extend_from_slice(&(info.len() as u16).to_le_bytes());
                            c.extend_from_slice(&48u32.to_le_bytes());
                            c.extend_from_slice(&info);
                            (401, format!("WWW-Authenticate: {}\r\n", ntlm::header("NTLM", &c)), String::new())
                        }
                        Some(3) if negotiated => {
                            let at = |i: usize| (u16::from_le_bytes([msg[i], msg[i + 1]]) as usize, u32::from_le_bytes(msg[i + 4..i + 8].try_into().unwrap()) as usize);
                            let (nl, no) = at(20);
                            let nt = &msg[no..no + nl];
                            let (ul, uo) = at(36);
                            let (dl, d_o) = at(28);
                            let utf16 = |b: &[u8]| String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>());
                            let (user, domain) = (utf16(&msg[uo..uo + ul]), utf16(&msg[d_o..d_o + dl]));
                            use hmac::{KeyInit, Mac};
                            let key = ntlm::nt_owf_v2(&user, password, &domain);
                            let mut m = hmac::Hmac::<md5::Md5>::new_from_slice(&key).unwrap();
                            m.update(&server_challenge);
                            m.update(&nt[16..]);
                            if m.finalize().into_bytes().as_slice() == &nt[..16] { (200, String::new(), format!("hello {domain}\\{user}")) } else { (401, String::new(), "bad proof".into()) }
                        }
                        _ => (401, "WWW-Authenticate: NTLM\r\n".to_string(), String::new()),
                    };
                    let resp = format!("HTTP/1.1 {status} X\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len());
                    if sock.write_all(resp.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (format!("http://{addr}"), connections)
}

#[tokio::test]
async fn ntlm_handshake_on_one_connection() {
    let (base, connections) = ntlm_server("S3cret!").await;
    let mut req = HttpRequestFile::new("n");
    req.url = format!("{base}/protected");
    req.auth = Auth::Ntlm { username: "CORP\\ada".into(), password: "S3cret!".into(), domain: None, workstation: Some("LAPTOP".into()) };
    let p = prep(&req);
    let resp = ClientPool::new().execute(&p).await.unwrap();
    assert_eq!(resp.status, 200, "{}", String::from_utf8_lossy(&resp.body));
    assert_eq!(String::from_utf8_lossy(&resp.body), "hello CORP\\ada");
    assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 1, "negotiate and authenticate share a connection");

    // A wrong password gets the server's 401 back.
    req.auth = Auth::Ntlm { username: "ada".into(), password: "nope".into(), domain: Some("CORP".into()), workstation: None };
    assert_eq!(ClientPool::new().execute(&prep(&req)).await.unwrap().status, 401);

    let cmd = to_curl(&p);
    assert!(cmd.contains("--ntlm -u 'CORP\\ada:S3cret!'"), "{cmd}");
}

#[tokio::test]
async fn runs_inline_requests_without_project_files() {
    use crate::runner::{InlineRequest, NoHooks, RunContext, RunOptions, run_requests};
    use devall_core::{AssertOp, AssertTarget, Assertion, Capture, CaptureSource, Project};
    let base = echo_server().await;
    let dir = tempfile::tempdir().unwrap();
    let project = Project::detached(dir.path(), "inline", [("base".to_string(), base.clone())].into_iter().collect());
    let mut first = HttpRequestFile::new("first");
    first.url = "{{base}}/one".into();
    first.tests.push(Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some("201".into()), enabled: true });
    first.captures.push(Capture { name: "st".into(), source: CaptureSource::Status, enabled: true });
    let mut second = HttpRequestFile::new("second");
    second.url = "{{base}}/two/{{st}}".into();
    second.tests.push(Assertion { target: AssertTarget::Body, op: AssertOp::Contains, value: Some("GET /two/201".into()), enabled: true });
    let reqs = vec![InlineRequest { label: "x.hurl#1".into(), request: first }, InlineRequest { label: "x.hurl#2".into(), request: second }];
    let tokens = crate::oauth2::MemoryTokenStore::default();
    let pool = ClientPool::new();
    let ctx = RunContext { project: &project, environment: None, secrets: &NoSecrets, tokens: &tokens, browser: None, pool: &pool };
    let mut runtime = devall_core::Variables::new();
    let s = run_requests(&ctx, &reqs, &mut runtime, &RunOptions::default(), &NoHooks).await;
    assert_eq!((s.total, s.passed, s.failed), (2, 2, 0), "{:#?}", s.items);
    assert_eq!(s.items[1].path, "x.hurl#2");
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none(), "nothing written");
}
