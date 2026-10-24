//! Small shared helpers: time, headers, globs.

use http::{HeaderMap, HeaderName, HeaderValue, Version};
use regex::Regex;

use crate::model::HeaderList;

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

pub(crate) fn header_value<'a>(headers: &'a HeaderList, name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// `application/json; charset=utf-8` → `application/json`.
pub(crate) fn mime_essence(ct: &str) -> String {
    ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

pub(crate) fn is_json_type(ct: &str) -> bool {
    let m = mime_essence(ct);
    m == "application/json" || m.ends_with("+json") || m == "text/json" || m.ends_with("/json")
}

pub(crate) fn is_xml_type(ct: &str) -> bool {
    let m = mime_essence(ct);
    m.ends_with("/xml") || m.ends_with("+xml")
}

/// Text-ish content (worth inlining as text).
pub(crate) fn is_text_type(ct: &str) -> bool {
    let m = mime_essence(ct);
    m.starts_with("text/")
        || is_json_type(&m)
        || is_xml_type(&m)
        || m.contains("javascript")
        || m == "application/x-www-form-urlencoded"
        || m == "application/graphql"
}

pub(crate) fn version_str(v: Version) -> &'static str {
    match v {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/1.1",
    }
}

pub(crate) fn header_list(map: &HeaderMap) -> HeaderList {
    map.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect()
}

/// Builds a header map from name/value pairs, skipping invalid ones.
pub(crate) fn header_map(list: &HeaderList) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in list {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(k.trim().as_bytes()), HeaderValue::from_str(v)) {
            map.append(name, value);
        }
    }
    map
}

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus the legacy `Proxy-Connection`.
pub(crate) const HOP_BY_HOP: &[&str] =
    &["connection", "proxy-connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"];

/// Removes hop-by-hop headers and anything listed in `Connection`.
pub(crate) fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<String> = headers
        .get_all(http::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    for name in HOP_BY_HOP.iter().copied().chain(listed.iter().map(String::as_str)) {
        headers.remove(name);
    }
}

pub(crate) fn is_hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP.iter().any(|h| h.eq_ignore_ascii_case(name))
}

/// Converts a glob (`*` any run of characters, `?` one character) into an anchored regex.
pub(crate) fn glob_regex(glob: &str, case_insensitive: bool) -> Result<Regex, regex::Error> {
    let mut re = String::with_capacity(glob.len() + 8);
    if case_insensitive {
        re.push_str("(?i)");
    }
    re.push('^');
    let mut buf = [0u8; 4];
    for ch in glob.chars() {
        match ch {
            '*' => {
                if !re.ends_with(".*") {
                    re.push_str(".*");
                }
            }
            '?' => re.push('.'),
            c => re.push_str(&regex::escape(c.encode_utf8(&mut buf))),
        }
    }
    re.push('$');
    Regex::new(&re)
}

/// Host patterns for the bypass and ignore-TLS lists. A pattern without wildcards matches the
/// host and its subdomains (`example.com` matches `api.example.com`, like `NO_PROXY`); with `*`
/// or `?` it is a glob over the host. Patterns containing `:` also match the port.
#[derive(Debug, Clone, Default)]
pub(crate) struct HostMatcher {
    patterns: Vec<(Regex, bool)>,
}

impl HostMatcher {
    pub fn new(patterns: &[String]) -> Self {
        let mut out = Vec::new();
        for p in patterns {
            let p = p.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_ascii_lowercase();
            if p.is_empty() {
                continue;
            }
            let with_port = p.contains(':') && !p.starts_with('[');
            let re = if p.contains('*') || p.contains('?') {
                glob_regex(&p, true)
            } else {
                let base = regex::escape(p.trim_start_matches('.'));
                Regex::new(&format!("(?i)^(?:.*\\.)?{base}$"))
            };
            if let Ok(re) = re {
                out.push((re, with_port));
            }
        }
        Self { patterns: out }
    }

    pub fn matches(&self, host: &str, port: u16) -> bool {
        let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let with_port = format!("{host}:{port}");
        self.patterns.iter().any(|(re, p)| if *p { re.is_match(&with_port) } else { re.is_match(&host) })
    }
}

/// Splits `host:port` (IPv6 in brackets) with a default port.
pub(crate) fn split_host_port(authority: &str, default_port: u16) -> (String, u16) {
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(rest) = authority.strip_prefix('[')
        && let Some((h, tail)) = rest.split_once(']')
    {
        let port = tail.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(default_port);
        return (h.to_string(), port);
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => match p.parse() {
            Ok(port) => (h.to_string(), port),
            Err(_) => (authority.to_string(), default_port),
        },
        _ => (authority.to_string(), default_port),
    }
}

/// `host[:port]` for a URL authority, omitting the scheme's default port; IPv6 gets brackets.
pub(crate) fn authority_for(scheme: &str, host: &str, port: u16) -> String {
    let h = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
    let default = if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") { 443 } else { 80 };
    if port == default { h } else { format!("{h}:{port}") }
}

/// Short human-readable error chain (`a: b: c`), without repeated messages.
pub(crate) fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        let s = err.to_string();
        if !s.is_empty() && !parts.iter().any(|p| p.contains(&s)) {
            parts.push(s);
        }
        cur = err.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        let re = glob_regex("/api/*/items?", false).unwrap();
        assert!(re.is_match("/api/v1/items1"));
        assert!(re.is_match("/api/v1/x/items2"));
        assert!(!re.is_match("/api/v1/items"));
        assert!(glob_regex("*.Example.com", true).unwrap().is_match("a.example.COM"));
    }

    #[test]
    fn hosts() {
        let m = HostMatcher::new(&["example.com".into(), "*.apple.com".into(), "localhost:3000".into()]);
        assert!(m.matches("example.com", 443));
        assert!(m.matches("api.example.com", 443));
        assert!(!m.matches("notexample.com", 443));
        assert!(m.matches("x.apple.com", 443));
        assert!(!m.matches("apple.com", 443));
        assert!(m.matches("localhost", 3000));
        assert!(!m.matches("localhost", 3001));
    }

    #[test]
    fn host_port() {
        assert_eq!(split_host_port("a.com:8443", 443), ("a.com".into(), 8443));
        assert_eq!(split_host_port("a.com", 443), ("a.com".into(), 443));
        assert_eq!(split_host_port("[::1]:80", 443), ("::1".into(), 80));
        assert_eq!(authority_for("https", "a.com", 443), "a.com");
        assert_eq!(authority_for("http", "::1", 8080), "[::1]:8080");
    }
}
