//! Correlation ids from HTTP headers (W3C `traceparent`, `x-request-id`, `x-correlation-id`, B3,
//! Jaeger, AWS X-Ray, Google Cloud) and from bodies (JSON keys or `request_id=…` text).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

/// Header names in priority order.
pub const HEADERS: &[&str] = &[
    "traceparent",
    "x-correlation-id",
    "correlation-id",
    "x-request-id",
    "request-id",
    "x-trace-id",
    "trace-id",
    "x-amzn-trace-id",
    "x-b3-traceid",
    "b3",
    "uber-trace-id",
    "x-cloud-trace-context",
];

/// Body keys (any case, `-`/`_` insensitive), searched up to three levels deep.
const BODY_KEYS: &[&str] = &["traceid", "correlationid", "requestid", "xrequestid", "reqid"];

/// W3C trace context: `00-<32 hex trace id>-<16 hex span id>-<flags>` → trace id.
pub fn trace_id_from_traceparent(v: &str) -> Option<&str> {
    let mut parts = v.trim().split('-');
    let _version = parts.next()?;
    let trace = parts.next()?;
    (trace.len() == 32 && trace.bytes().all(|c| c.is_ascii_hexdigit()) && trace.bytes().any(|c| c != b'0')).then_some(trace)
}

fn normalize_header(name: &str, value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let id = match name {
        "traceparent" => trace_id_from_traceparent(v)?.to_string(),
        "x-amzn-trace-id" => v.split(';').find_map(|p| p.trim().strip_prefix("Root=")).unwrap_or(v).to_string(),
        "b3" => v.split('-').next()?.to_string(),
        "uber-trace-id" => v.split(':').next()?.to_string(),
        "x-cloud-trace-context" => v.split('/').next()?.to_string(),
        _ => v.to_string(),
    };
    (!id.is_empty() && id.len() <= 256).then_some(id)
}

/// The best correlation id among `headers` (name matching is case-insensitive).
pub fn from_headers<I, K, V>(headers: I) -> Option<String>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut best: Option<(usize, String)> = None;
    for (k, v) in headers {
        let k = k.as_ref().to_ascii_lowercase();
        if let Some(rank) = HEADERS.iter().position(|h| *h == k)
            && best.as_ref().is_none_or(|(r, _)| rank < *r)
            && let Some(id) = normalize_header(&k, v.as_ref())
        {
            best = Some((rank, id));
        }
    }
    best.map(|b| b.1)
}

fn norm_key(k: &str) -> String {
    k.chars().filter(|c| *c != '-' && *c != '_' && *c != '.').flat_map(char::to_lowercase).collect()
}

fn find_json(v: &Value, depth: u8) -> Option<String> {
    let Value::Object(o) = v else { return None };
    let mut best: Option<(usize, String)> = None;
    for (k, val) in o {
        if let Some(rank) = BODY_KEYS.iter().position(|b| *b == norm_key(k)) {
            let s = match val {
                Value::String(s) if !s.is_empty() && s.len() <= 256 => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => continue,
            };
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, s));
            }
        }
    }
    if let Some(b) = best {
        return Some(b.1);
    }
    if depth == 0 {
        return None;
    }
    o.values().find_map(|x| find_json(x, depth - 1))
}

fn text_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:trace[_-]?id|correlation[_-]?id|x-request-id|request[_-]?id)["']?\s*[:=]\s*["']?([A-Za-z0-9][A-Za-z0-9._:-]{5,127})"#).unwrap()
    })
}

/// A correlation id in a body: JSON keys (`traceId`, `correlation_id`, `requestId`…) or
/// `request_id=…`-style text. Only the first 64 KiB are examined.
pub fn from_body(body: &str) -> Option<String> {
    let body = &body[..body.floor_char_boundary(64 * 1024)];
    let t = body.trim_start();
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return match &v {
            Value::Array(a) => a.iter().take(5).find_map(|x| find_json(x, 2)),
            _ => find_json(&v, 2),
        };
    }
    text_re().captures(body).map(|c| c[1].to_string())
}

/// Headers first, then the body.
pub fn extract<I, K, V>(headers: I, body: Option<&str>) -> Option<String>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    from_headers(headers).or_else(|| body.and_then(from_body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert_eq!(from_headers([("TraceParent", tp), ("X-Request-Id", "r1")]).as_deref(), Some("4bf92f3577b34da6a3ce929d0e0e4736"));
        assert_eq!(from_headers([("x-request-id", "r1"), ("x-correlation-id", "c1")]).as_deref(), Some("c1"));
        assert_eq!(from_headers(vec![("X-Amzn-Trace-Id".to_string(), "Root=1-5759e988-bd862e3fe1be46a994272793;Sampled=1".to_string())]).as_deref(), Some("1-5759e988-bd862e3fe1be46a994272793"));
        assert_eq!(from_headers([("b3", "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1")]).as_deref(), Some("80f198ee56343ba864fe8b2a57d3eff7"));
        assert_eq!(from_headers([("content-type", "x")]), None);
        assert_eq!(from_headers([("traceparent", "00-00000000000000000000000000000000-00f067aa0ba902b7-01")]), None);
    }

    #[test]
    fn bodies() {
        assert_eq!(from_body(r#"{"error":{"message":"x","requestId":"req-123456"}}"#).as_deref(), Some("req-123456"));
        assert_eq!(from_body(r#"{"trace_id":"abc","correlation-id":"zzz"}"#).as_deref(), Some("abc"));
        assert_eq!(from_body("failed request_id=r-7f3a9b done").as_deref(), Some("r-7f3a9b"));
        assert_eq!(from_body(r#"[{"traceId": 1234567}]"#).as_deref(), Some("1234567"));
        assert_eq!(from_body("nothing here"), None);
        assert_eq!(extract([("x", "y")], Some("traceId: abcdef12")).as_deref(), Some("abcdef12"));
    }
}
