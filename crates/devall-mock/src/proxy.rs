//! Forwarding requests to an upstream (`proxyBaseUrl` stubs and recording).

use std::time::Duration;

use bytes::Bytes;

use crate::request::MockRequest;

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus the ones the client recomputes.
const HOP_BY_HOP: &[&str] = &["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade", "host", "content-length"];

#[derive(Debug, Clone)]
pub(crate) struct ProxyResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

pub(crate) fn client(trust_all: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .tls_danger_accept_invalid_certs(trust_all)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Upstream URL: base + request URL (minus `prefix_to_remove`).
pub(crate) fn target_url(base: &str, url: &str, prefix_to_remove: Option<&str>) -> String {
    let mut path = url;
    if let Some(prefix) = prefix_to_remove.filter(|p| !p.is_empty())
        && let Some(rest) = path.strip_prefix(prefix)
    {
        path = rest;
    }
    let base = base.trim_end_matches('/');
    if path.is_empty() || path.starts_with('?') {
        format!("{base}/{path}")
    } else if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// Adds a header, tolerating non-ASCII values (sent as raw bytes) and skipping invalid names.
fn with_header(builder: reqwest::RequestBuilder, name: &str, value: &str) -> reqwest::RequestBuilder {
    match (reqwest::header::HeaderName::from_bytes(name.as_bytes()), reqwest::header::HeaderValue::from_bytes(value.as_bytes())) {
        (Ok(n), Ok(v)) => builder.header(n, v),
        _ => builder,
    }
}

pub(crate) async fn forward(
    client: &reqwest::Client,
    req: &MockRequest,
    base_url: &str,
    prefix_to_remove: Option<&str>,
    add_headers: &[(String, String)],
    remove_headers: &[String],
) -> Result<ProxyResponse, String> {
    let url = target_url(base_url, &req.url, prefix_to_remove);
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut builder = client.request(method, &url);
    for (k, v) in &req.headers {
        let lower = k.to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str()) || remove_headers.iter().any(|r| r.eq_ignore_ascii_case(k)) {
            continue;
        }
        if add_headers.iter().any(|(a, _)| a.eq_ignore_ascii_case(k)) {
            continue;
        }
        builder = with_header(builder, k, v);
    }
    for (k, v) in add_headers {
        builder = with_header(builder, k, v);
    }
    if !req.body.is_empty() {
        builder = builder.body(req.body.clone());
    }
    let resp = builder.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter(|(k, _)| !HOP_BY_HOP.contains(&k.as_str()))
        .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    Ok(ProxyResponse { status, headers, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(target_url("http://up:1/", "/a?b=1", None), "http://up:1/a?b=1");
        assert_eq!(target_url("http://up:1/api", "/proxy/a", Some("/proxy")), "http://up:1/api/a");
        assert_eq!(target_url("http://up:1", "/proxy", Some("/proxy")), "http://up:1/");
        assert_eq!(target_url("http://up:1", "/x", Some("/nope")), "http://up:1/x");
    }
}
