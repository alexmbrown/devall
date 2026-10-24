//! Sends captured events on: forwarding to the local app and replays.
//!
//! The body goes out byte for byte (signatures cover the raw bytes). Headers are the received
//! ones minus hop-by-hop headers and `Content-Length`; `Host` is replaced by the target's unless
//! `preserveHost` is set, and `X-Forwarded-For/Host/Proto` are added when absent so frameworks can
//! rebuild the public URL (Twilio validation). The HTTP client is dedicated to forwarding: no
//! cookie jar, no redirects followed, no proxy, no automatic decompression or `Accept-Encoding`.

use std::time::{Duration, Instant};

use crate::event::{ForwardResult, WebhookEvent, header, now_ms, set_header, without_hop_by_hop};

/// Response bodies kept on the event (as text).
pub const MAX_EXCERPT: usize = 64 * 1024;
/// Upstream bodies relayed to the sender in pass-through mode.
pub const MAX_PASS_THROUGH: usize = crate::event::MAX_BODY_BYTES;

#[derive(Debug, Clone, PartialEq)]
pub struct OutboundRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout_ms: u64,
}

/// The local app's full response (for pass-through).
#[derive(Debug, Clone)]
pub struct UpstreamResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone)]
pub struct Forwarder {
    strict: reqwest::Client,
    /// For loopback targets: local dev servers often use self-signed certificates.
    loopback: reqwest::Client,
}

impl Default for Forwarder {
    fn default() -> Self {
        Self::new()
    }
}

fn client(accept_invalid_certs: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .tls_danger_accept_invalid_certs(accept_invalid_certs)
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .expect("static reqwest configuration")
}

fn is_loopback(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost") || d.to_ascii_lowercase().ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

impl Forwarder {
    pub fn new() -> Self {
        Self { strict: client(false), loopback: client(true) }
    }

    /// Sends `req`. Never fails: errors are described in the result. With `keep_body`, the full
    /// upstream response (up to 10 MB) is returned too.
    pub async fn send(&self, req: &OutboundRequest, keep_body: bool) -> (ForwardResult, Option<UpstreamResponse>) {
        let at = now_ms();
        let started = Instant::now();
        let fail = |e: String| (ForwardResult { url: req.url.clone(), at, error: Some(e), duration_ms: ms(started.elapsed()), ..Default::default() }, None);
        let url = match reqwest::Url::parse(&req.url) {
            Ok(u) if matches!(u.scheme(), "http" | "https") => u,
            Ok(u) => return fail(format!("unsupported URL scheme {}:", u.scheme())),
            Err(e) => return fail(format!("invalid target URL {}: {e}", req.url)),
        };
        let Ok(method) = reqwest::Method::from_bytes(req.method.as_bytes()) else { return fail(format!("invalid method {}", req.method)) };
        let c = if is_loopback(&url) { &self.loopback } else { &self.strict };
        let mut rb = c.request(method, url.clone()).timeout(Duration::from_millis(req.timeout_ms.max(1)));
        for (k, v) in &req.headers {
            match (reqwest::header::HeaderName::from_bytes(k.as_bytes()), reqwest::header::HeaderValue::from_bytes(v.as_bytes())) {
                (Ok(n), Ok(v)) => rb = rb.header(n, v),
                _ => continue,
            }
        }
        if !req.body.is_empty() || !matches!(req.method.as_str(), "GET" | "HEAD") {
            rb = rb.body(req.body.clone());
        }
        let mut resp = match rb.send().await {
            Ok(r) => r,
            Err(e) => return fail(describe_error(&e, &url, req.timeout_ms)),
        };
        let status = resp.status();
        let headers: Vec<(String, String)> =
            resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
        let cap = if keep_body { MAX_PASS_THROUGH } else { MAX_EXCERPT };
        let mut body = Vec::new();
        let mut size = 0usize;
        let mut read_error = None;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    size += chunk.len();
                    if body.len() < cap {
                        let take = chunk.len().min(cap - body.len());
                        body.extend_from_slice(&chunk[..take]);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    read_error = Some(format!("reading the response body failed: {}", devall_http::error_chain(&e)));
                    break;
                }
            }
        }
        let excerpt = (!body.is_empty()).then(|| String::from_utf8_lossy(&body[..body.len().min(MAX_EXCERPT)]).into_owned());
        let result = ForwardResult {
            url: req.url.clone(),
            at,
            status: Some(status.as_u16()),
            status_text: status.canonical_reason().unwrap_or("").to_string(),
            headers: headers.clone(),
            body_excerpt: excerpt,
            body_size: size,
            duration_ms: ms(started.elapsed()),
            error: read_error,
        };
        let upstream = keep_body.then_some(UpstreamResponse { status: status.as_u16(), headers, body });
        (result, upstream)
    }
}

fn describe_error(e: &reqwest::Error, url: &reqwest::Url, timeout_ms: u64) -> String {
    let target = format!("{}://{}", url.scheme(), url.authority());
    if e.is_timeout() {
        format!("timed out after {timeout_ms} ms waiting for {target}")
    } else if e.is_connect() {
        let port = url.port_or_known_default().map(|p| format!(" on port {p}")).unwrap_or_default();
        if is_loopback(url) {
            format!("could not connect to {target} — is your app running{port}? ({})", devall_http::error_chain(e))
        } else {
            format!("could not connect to {target}: {}", devall_http::error_chain(e))
        }
    } else {
        format!("request to {target} failed: {}", devall_http::error_chain(e))
    }
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1000.0 * 100.0).round() / 100.0
}

/// Appends the received query string when `target` has none.
pub fn with_query(target: &str, query: &str) -> String {
    if query.is_empty() || target.contains('?') {
        return target.to_string();
    }
    match target.split_once('#') {
        Some((base, frag)) => format!("{base}?{query}#{frag}"),
        None => format!("{target}?{query}"),
    }
}

/// The request that forwards `event` to `target` (already resolved).
pub fn forward_request(event: &WebhookEvent, target: &str, preserve_host: bool, timeout_ms: u64) -> OutboundRequest {
    let mut headers = without_hop_by_hop(&event.headers);
    let original_host = header(&event.headers, "host").map(str::to_string);
    if !preserve_host {
        crate::event::remove_header(&mut headers, "host");
    }
    if let Some(ip) = event.remote_addr.as_deref().map(peer_ip) {
        match header(&headers, "x-forwarded-for").map(str::to_string) {
            Some(prev) => set_header(&mut headers, "X-Forwarded-For", &format!("{prev}, {ip}")),
            None => headers.push(("X-Forwarded-For".into(), ip.to_string())),
        }
    }
    let public = event.public_url.as_deref().and_then(|u| url::Url::parse(u).ok());
    if header(&headers, "x-forwarded-host").is_none() {
        let host = public.as_ref().map(|u| u.authority().to_string()).or(original_host);
        if let Some(h) = host.filter(|h| !h.is_empty()) {
            headers.push(("X-Forwarded-Host".into(), h));
        }
    }
    if header(&headers, "x-forwarded-proto").is_none() {
        let proto = public.as_ref().map(|u| u.scheme().to_string()).unwrap_or_else(|| "http".into());
        headers.push(("X-Forwarded-Proto".into(), proto));
    }
    OutboundRequest { method: event.method.clone(), url: with_query(target, &event.query), headers, body: event.body.clone(), timeout_ms }
}

/// `1.2.3.4:5678` → `1.2.3.4`; `[::1]:80` → `::1`.
fn peer_ip(addr: &str) -> &str {
    if let Some(rest) = addr.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match addr.rsplit_once(':') {
        Some((ip, port)) if !ip.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => ip,
        _ => addr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> WebhookEvent {
        WebhookEvent {
            method: "POST".into(),
            url: "http://127.0.0.1:4590/stripe?a=1".into(),
            path: "/stripe".into(),
            query: "a=1".into(),
            headers: vec![
                ("host".into(), "127.0.0.1:4590".into()),
                ("content-length".into(), "2".into()),
                ("connection".into(), "keep-alive".into()),
                ("stripe-signature".into(), "t=1,v1=x".into()),
            ],
            body: b"{}".to_vec(),
            remote_addr: Some("[::1]:5555".into()),
            ..Default::default()
        }
    }

    #[test]
    fn builds_forward_request() {
        let r = forward_request(&event(), "http://localhost:3000/hook", false, 1000);
        assert_eq!(r.url, "http://localhost:3000/hook?a=1");
        let names: Vec<&str> = r.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["stripe-signature", "X-Forwarded-For", "X-Forwarded-Host", "X-Forwarded-Proto"]);
        assert_eq!(header(&r.headers, "x-forwarded-for"), Some("::1"));
        assert_eq!(header(&r.headers, "x-forwarded-host"), Some("127.0.0.1:4590"));

        let mut e = event();
        e.public_url = Some("https://abc.trycloudflare.com/stripe".into());
        e.headers.push(("x-forwarded-for".into(), "203.0.113.9".into()));
        let r = forward_request(&e, "http://localhost:3000/hook?b=2", true, 1000);
        assert_eq!(r.url, "http://localhost:3000/hook?b=2");
        assert_eq!(header(&r.headers, "host"), Some("127.0.0.1:4590"));
        assert_eq!(header(&r.headers, "x-forwarded-for"), Some("203.0.113.9, ::1"));
        assert_eq!(header(&r.headers, "x-forwarded-host"), Some("abc.trycloudflare.com"));
        assert_eq!(header(&r.headers, "x-forwarded-proto"), Some("https"));
    }

    #[test]
    fn query_and_peer_helpers() {
        assert_eq!(with_query("http://a/x#f", "q=1"), "http://a/x?q=1#f");
        assert_eq!(with_query("http://a/x", ""), "http://a/x");
        assert_eq!(peer_ip("10.0.0.2:8080"), "10.0.0.2");
        assert_eq!(peer_ip("[fe80::1]:80"), "fe80::1");
    }

    #[tokio::test]
    async fn connection_refused_is_explained() {
        // Bind then drop to get a port with nothing listening.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let f = Forwarder::new();
        let req = OutboundRequest { method: "POST".into(), url: format!("http://127.0.0.1:{port}/x"), headers: vec![], body: b"{}".to_vec(), timeout_ms: 15_000 };
        let (r, up) = f.send(&req, true).await;
        assert!(up.is_none());
        assert!(r.status.is_none());
        assert!(r.error.as_deref().unwrap().contains("is your app running"), "{:?}", r.error);
        let (r, _) = f.send(&OutboundRequest { url: "ftp://x".into(), ..req }, false).await;
        assert!(r.error.unwrap().contains("unsupported"));
    }
}
