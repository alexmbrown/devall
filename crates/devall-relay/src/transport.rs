//! TCP/TLS + WebSocket handshake for the relay socket (and local pass-through WebSockets).
//!
//! TLS is rustls with the aws-lc-rs provider and the OS trust store
//! (`rustls-platform-verifier`, so corporate CAs work). `HTTPS_PROXY` is honoured with an HTTP
//! CONNECT tunnel, because the whole point of WebSocket-over-443 is surviving corporate networks.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use url::Url;

use crate::error::{RelayError, Result, chain};

pub type WsStream = WebSocketStream<NetStream>;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// rustls config; `verify = false` accepts any certificate (local dev servers only).
pub fn tls_config(verify: bool) -> Result<Arc<ClientConfig>> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().map_err(|e| RelayError::Tls(e.to_string()))?;
    let verifier: Arc<dyn ServerCertVerifier> = if verify {
        Arc::new(rustls_platform_verifier::Verifier::new(provider).map_err(|e| RelayError::Tls(format!("OS certificate store: {e}")))?)
    } else {
        Arc::new(AcceptAny(provider))
    };
    Ok(Arc::new(builder.dangerous().with_custom_certificate_verifier(verifier).with_no_client_auth()))
}

#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Plain TCP or TLS over TCP.
pub enum NetStream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for NetStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for NetStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_flush(cx),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// `HTTPS_PROXY` / `https_proxy` unless the host matches `NO_PROXY`.
fn proxy_for(host: &str, secure: bool) -> Option<Url> {
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return None;
    }
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let raw = if secure { get("HTTPS_PROXY").or_else(|| get("https_proxy")) } else { get("HTTP_PROXY").or_else(|| get("http_proxy")) }.or_else(|| get("ALL_PROXY").or_else(|| get("all_proxy")))?;
    if let Some(no) = get("NO_PROXY").or_else(|| get("no_proxy")) {
        let h = host.to_ascii_lowercase();
        for entry in no.split(',').map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase()).filter(|s| !s.is_empty()) {
            if entry == "*" || h == entry || h.ends_with(&format!(".{entry}")) {
                return None;
            }
        }
    }
    let with_scheme = if raw.contains("://") { raw } else { format!("http://{raw}") };
    Url::parse(&with_scheme).ok().filter(|u| u.scheme() == "http")
}

async fn connect_via_proxy(proxy: &Url, host: &str, port: u16) -> Result<TcpStream> {
    let phost = proxy.host_str().ok_or_else(|| RelayError::Config("proxy URL has no host".into()))?;
    let pport = proxy.port_or_known_default().unwrap_or(8080);
    let mut s = TcpStream::connect((phost, pport)).await.map_err(|e| RelayError::Connect(format!("proxy {phost}:{pport}: {e}")))?;
    let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if !proxy.username().is_empty() {
        let user = percent_decode(proxy.username());
        let pass = percent_decode(proxy.password().unwrap_or(""));
        let cred = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        req.push_str(&format!("Proxy-Authorization: Basic {cred}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await?;
    let mut buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if s.read(&mut byte).await? == 0 || buf.len() > 16 * 1024 {
            return Err(RelayError::Connect("proxy closed the CONNECT request".into()));
        }
        buf.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&buf);
    let status = head.split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        return Err(RelayError::Connect(format!("proxy refused CONNECT: {}", head.lines().next().unwrap_or(""))));
    }
    Ok(s)
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(s.replace('+', "%2B").as_bytes()).map(|(k, _)| k.into_owned()).next().unwrap_or_default()
}

/// Host suitable for connecting and SNI (IPv6 brackets removed).
pub fn host_of(url: &Url) -> Result<String> {
    match url.host() {
        Some(url::Host::Domain(d)) => Ok(d.to_string()),
        Some(url::Host::Ipv4(a)) => Ok(a.to_string()),
        Some(url::Host::Ipv6(a)) => Ok(a.to_string()),
        None => Err(RelayError::Config(format!("{url} has no host"))),
    }
}

/// Opens TCP (through a proxy when configured) and TLS for `wss`/`https`.
pub async fn connect_stream(url: &Url, verify_tls: bool, timeout: Duration) -> Result<NetStream> {
    let secure = matches!(url.scheme(), "wss" | "https");
    let host = host_of(url)?;
    let port = url.port_or_known_default().unwrap_or(if secure { 443 } else { 80 });
    let fut = async {
        let tcp = match proxy_for(&host, secure) {
            Some(p) => connect_via_proxy(&p, &host, port).await?,
            None => TcpStream::connect((host.as_str(), port)).await.map_err(|e| RelayError::Connect(format!("{host}:{port}: {e}")))?,
        };
        let _ = tcp.set_nodelay(true);
        if !secure {
            return Ok(NetStream::Plain(tcp));
        }
        let name = ServerName::try_from(host.clone()).map_err(|e| RelayError::Tls(format!("invalid server name {host}: {e}")))?;
        let tls = TlsConnector::from(tls_config(verify_tls)?).connect(name, tcp).await.map_err(|e| RelayError::Tls(chain(&e)))?;
        Ok(NetStream::Tls(Box::new(tls)))
    };
    tokio::time::timeout(timeout, fut).await.map_err(|_| RelayError::Timeout(format!("connecting to {host}:{port}")))?
}

/// The outcome of a WebSocket handshake.
pub struct Handshake {
    pub ws: WsStream,
    pub protocol: Option<String>,
}

#[derive(serde::Deserialize)]
struct RejectBody {
    error: RejectError,
}

#[derive(serde::Deserialize)]
struct RejectError {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// WebSocket handshake with extra headers and offered subprotocols. HTTP refusals become
/// [`RelayError::Rejected`] carrying the relay's JSON error code and message.
///
/// The upgrade request is written and the response head read here (not by tungstenite), so a
/// refusal's body is read completely: tungstenite only keeps the bytes that happened to arrive
/// with the head, and the edge often sends the JSON error separately — users then saw "HTTP 429"
/// instead of "Your plan allows 1 tunnel at a time".
pub async fn websocket(url: &Url, headers: &[(String, String)], protocols: &[String], verify_tls: bool, timeout: Duration, max_message: usize) -> Result<Handshake> {
    let mut stream = connect_stream(url, verify_tls, timeout).await?;
    let key = tokio_tungstenite::tungstenite::handshake::client::generate_key();
    let request = upgrade_request(url, headers, protocols, &key)?;
    let head = tokio::time::timeout(timeout, async {
        stream.write_all(request.as_bytes()).await?;
        stream.flush().await?;
        read_head(&mut stream).await
    })
    .await
    .map_err(|_| RelayError::Timeout("WebSocket handshake".into()))??;
    let (status, resp_headers, leftover) = head;
    let header = |name: &str| resp_headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
    if status != 101 {
        let retry_after = header("retry-after").and_then(|v| v.trim().parse::<u64>().ok()).map(Duration::from_secs);
        let body = tokio::time::timeout(Duration::from_secs(5), read_body(&mut stream, leftover, header("content-length"), header("transfer-encoding")))
            .await
            .unwrap_or_default();
        let (code, message) = match serde_json::from_slice::<RejectBody>(&body) {
            Ok(b) => (b.error.code, b.error.message),
            Err(_) => (String::new(), String::from_utf8_lossy(&body).chars().take(200).collect::<String>()),
        };
        let message = if message.trim().is_empty() { format!("the server answered HTTP {status}") } else { message.trim().to_string() };
        return Err(RelayError::Rejected { status, code, message, retry_after });
    }
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    if header("sec-websocket-accept").map(str::trim) != Some(accept.as_str()) || !header("upgrade").is_some_and(|u| u.trim().eq_ignore_ascii_case("websocket")) {
        return Err(RelayError::WebSocket("the server's upgrade response isn't a valid WebSocket handshake".into()));
    }
    let protocol = header("sec-websocket-protocol").map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let cfg = WebSocketConfig::default().max_message_size(Some(max_message.max(1024 * 1024) * 4)).max_frame_size(Some(max_message.max(1024 * 1024) * 4));
    let ws = WebSocketStream::from_partially_read(stream, leftover, Role::Client, Some(cfg)).await;
    Ok(Handshake { ws, protocol })
}

/// `Host` defaults to the URL's authority; a `host` entry in `headers` replaces it.
fn upgrade_request(url: &Url, headers: &[(String, String)], protocols: &[String], key: &str) -> Result<String> {
    let host = url.host_str().ok_or_else(|| RelayError::Config(format!("{url} has no host")))?;
    let authority = match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    let target = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    };
    let clean = |s: &str, what: &str| -> Result<()> {
        if s.contains(['\r', '\n']) { Err(RelayError::WebSocket(format!("{what} contains a line break"))) } else { Ok(()) }
    };
    clean(&target, "request path")?;
    let custom_host = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("host")).map(|(_, v)| v.clone());
    let mut req = format!(
        "GET {target} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n",
        custom_host.unwrap_or(authority)
    );
    for (k, v) in headers {
        let lk = k.to_ascii_lowercase();
        if lk == "host" || lk == "connection" || lk == "upgrade" || lk.starts_with("sec-websocket-") {
            continue;
        }
        if tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes()).is_err() {
            return Err(RelayError::WebSocket(format!("invalid header name {k}")));
        }
        clean(v, k)?;
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !protocols.is_empty() {
        let joined = protocols.join(", ");
        clean(&joined, "subprotocols")?;
        req.push_str(&format!("Sec-WebSocket-Protocol: {joined}\r\n"));
    }
    req.push_str("\r\n");
    Ok(req)
}

const MAX_HEAD: usize = 64 * 1024;
const MAX_ERROR_BODY: usize = 64 * 1024;

/// Reads the response head; returns (status, headers, bytes read past the head).
async fn read_head(stream: &mut NetStream) -> Result<(u16, Vec<(String, String)>, Vec<u8>)> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(RelayError::WebSocket("response head too large".into()));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(RelayError::WebSocket("connection closed during the WebSocket handshake".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let leftover = buf[end + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|_| status_line.starts_with("HTTP/"))
        .ok_or_else(|| RelayError::WebSocket(format!("not an HTTP response: {}", status_line.chars().take(80).collect::<String>())))?;
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    Ok((status, headers, leftover))
}

/// An error response's body (Content-Length, chunked, or until the server closes), capped.
async fn read_body(stream: &mut NetStream, mut buf: Vec<u8>, content_length: Option<&str>, transfer_encoding: Option<&str>) -> Vec<u8> {
    let chunked = transfer_encoding.is_some_and(|t| t.to_ascii_lowercase().contains("chunked"));
    let want = content_length.and_then(|v| v.trim().parse::<usize>().ok()).map(|n| n.min(MAX_ERROR_BODY));
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(n) = want
            && buf.len() >= n
        {
            buf.truncate(n);
            return buf;
        }
        if chunked && let Some(body) = dechunk(&buf) {
            return body;
        }
        if buf.len() >= MAX_ERROR_BODY {
            break;
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    if chunked { dechunk(&buf).unwrap_or_default() } else { buf }
}

/// The decoded body once the terminating zero-size chunk arrived.
fn dechunk(raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let line_end = raw[i..].windows(2).position(|w| w == b"\r\n")? + i;
        let size_hex = String::from_utf8_lossy(&raw[i..line_end]);
        let size = usize::from_str_radix(size_hex.split(';').next().unwrap_or("").trim(), 16).ok()?;
        let start = line_end + 2;
        if size == 0 {
            return Some(out);
        }
        if raw.len() < start + size + 2 {
            return None;
        }
        out.extend_from_slice(&raw[start..start + size]);
        i = start + size + 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_both_tls_modes() {
        assert!(tls_config(true).is_ok());
        assert!(tls_config(false).is_ok());
    }

    #[test]
    fn hosts_without_brackets() {
        assert_eq!(host_of(&Url::parse("ws://[::1]:80/x").unwrap()).unwrap(), "::1");
        assert_eq!(host_of(&Url::parse("wss://relay.devall.dev/connect").unwrap()).unwrap(), "relay.devall.dev");
    }

    #[test]
    fn loopback_never_uses_a_proxy() {
        assert!(proxy_for("127.0.0.1", true).is_none());
        assert!(proxy_for("localhost", false).is_none());
    }

    #[test]
    fn builds_upgrade_requests() {
        let u = Url::parse("ws://127.0.0.1:8787/connect?name=a%20b").unwrap();
        let r = upgrade_request(&u, &[("authorization".into(), "Bearer t".into())], &["devall-relay.1".into()], "k==").unwrap();
        assert!(r.starts_with("GET /connect?name=a%20b HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n"), "{r}");
        assert!(r.contains("Sec-WebSocket-Key: k==\r\n") && r.contains("authorization: Bearer t\r\n") && r.contains("Sec-WebSocket-Protocol: devall-relay.1\r\n"));
        assert!(r.ends_with("\r\n\r\n"));
        let h = upgrade_request(&Url::parse("wss://relay.devall.dev/x").unwrap(), &[("Host".into(), "my-app.devall.dev".into())], &[], "k").unwrap();
        assert!(h.contains("Host: my-app.devall.dev\r\n") && !h.contains("Host: relay.devall.dev"));
        assert!(upgrade_request(&u, &[("x".into(), "a\r\nb".into())], &[], "k").is_err());
    }

    #[test]
    fn dechunks_error_bodies() {
        assert_eq!(dechunk(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n").unwrap(), b"hello world");
        assert!(dechunk(b"5\r\nhel").is_none());
    }

    #[tokio::test]
    async fn refusals_keep_the_json_body_even_when_it_arrives_late() {
        // The head and the body in separate writes, like workerd does.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            let body = br#"{"error":{"code":"too_many_tunnels","message":"Your plan allows 1 tunnel at a time."}}"#;
            s.write_all(format!("HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: 7\r\ncontent-length: {}\r\n\r\n", body.len()).as_bytes()).await.unwrap();
            s.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            s.write_all(body).await.unwrap();
        });
        let url = Url::parse(&format!("ws://{addr}/connect")).unwrap();
        let err = websocket(&url, &[], &[], true, Duration::from_secs(5), 1 << 20).await.err().unwrap();
        match err {
            RelayError::Rejected { status, code, message, retry_after } => {
                assert_eq!((status, code.as_str(), message.as_str()), (429, "too_many_tunnels", "Your plan allows 1 tunnel at a time."));
                assert_eq!(retry_after, Some(Duration::from_secs(7)));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn decodes_proxy_credentials() {
        assert_eq!(percent_decode("a%40b"), "a@b");
        assert_eq!(percent_decode("p+q"), "p+q");
    }
}
