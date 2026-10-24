//! The default host services: HTTP through reqwest on its own small runtime (callable from plain
//! threads and from inside a tokio runtime), redirects not followed, and plugin secrets.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use devall_plugins::contract::{Header, PluginRequest};
use devall_plugins::{DefaultServices, HostServices};

/// A one-shot HTTP/1.1 server answering `status` with `body` (and a Location header for 3xx).
fn serve_once(status: u16, body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = vec![0u8; 8192];
        let n = s.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]).into_owned();
        let extra = if (300..400).contains(&status) { "Location: http://127.0.0.1:1/elsewhere\r\n" } else { "" };
        let resp = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nX-Mock: yes\r\n{extra}Connection: close\r\n\r\n{body}", body.len());
        s.write_all(resp.as_bytes()).unwrap();
        req
    });
    (port, h)
}

fn request(port: u16) -> PluginRequest {
    PluginRequest { method: "POST".into(), url: format!("http://127.0.0.1:{port}/ping?x=1"), headers: vec![Header::new("X-Plugin", "1")], body: Some("hello".into()), body_base64: false }
}

#[test]
fn http_from_a_plain_thread() {
    let (port, h) = serve_once(200, "pong");
    let resp = DefaultServices::new(None).http(&request(port), Duration::from_secs(5)).unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.as_deref(), Some("pong"));
    assert!(resp.headers.iter().any(|h| h.name == "x-mock" && h.value == "yes"));
    let seen = h.join().unwrap();
    assert!(seen.starts_with("POST /ping?x=1 HTTP/1.1"), "{seen}");
    assert!(seen.to_ascii_lowercase().contains("x-plugin: 1"));
    assert!(seen.ends_with("hello"));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_from_inside_a_runtime_and_no_redirects() {
    let (port, h) = serve_once(302, "");
    // Plugin calls run on blocking threads inside the app's runtime.
    let resp = tokio::task::spawn_blocking(move || DefaultServices::new(None).http(&request(port), Duration::from_secs(5))).await.unwrap().unwrap();
    assert_eq!(resp.status, 302, "redirects are handed to the plugin, not followed");
    h.join().unwrap();
    // Unreachable host: an error, not a panic.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let err = tokio::task::spawn_blocking(move || DefaultServices::new(None).http(&request(dead), Duration::from_secs(2))).await.unwrap().unwrap_err();
    assert!(!err.is_empty());
    // Without a secret store every secret is unset.
    assert_eq!(DefaultServices::new(None).secret("p", "X"), None);
    let mem = devall_core::MemorySecretStore::default();
    devall_core::SecretStore::set(&mem, &devall_plugins::secret_scope("p"), "X", "v").unwrap();
    assert_eq!(DefaultServices::new(Some(Box::new(mem))).secret("p", "X").as_deref(), Some("v"));
}
