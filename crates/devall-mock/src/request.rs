//! The request model shared by matching, templating, the journal and near-miss reports.

use std::borrow::Cow;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::util::{b64_decode, b64_encode, canonical_header_name};

#[derive(Debug, Clone)]
pub struct MockRequest {
    pub id: Uuid,
    pub method: String,
    /// Path and query exactly as received (still percent-encoded).
    pub url: String,
    pub path: String,
    /// Header names are lower-case (hyper normalises them).
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub client_ip: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub protocol: String,
    pub logged_at: DateTime<Utc>,
    query: Vec<(String, String)>,
    cookies: Vec<(String, String)>,
}

fn split_url(url: &str) -> (String, Option<String>) {
    let url = url.split('#').next().unwrap_or(url);
    match url.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (url.to_string(), None),
    }
}

pub(crate) fn parse_form(text: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(text.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

fn parse_cookies(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("cookie") {
            for part in v.split(';') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let (name, value) = part.split_once('=').unwrap_or((part, ""));
                out.push((name.trim().to_string(), value.trim().trim_matches('"').to_string()));
            }
        }
    }
    out
}

impl MockRequest {
    pub fn new(method: &str, url: &str, headers: Vec<(String, String)>, body: impl Into<Bytes>) -> Self {
        let (path, query) = split_url(url);
        let path = if path.is_empty() { "/".to_string() } else { path };
        let headers: Vec<(String, String)> = headers.into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
        let host_header = headers.iter().find(|(k, _)| k == "host").map(|(_, v)| v.clone());
        let (host, port) = match host_header.as_deref().map(split_host_port) {
            Some((h, p)) => (h, p.unwrap_or(80)),
            None => ("localhost".to_string(), 80),
        };
        let cookies = parse_cookies(&headers);
        Self {
            id: Uuid::new_v4(),
            method: method.to_ascii_uppercase(),
            url: if url.is_empty() { "/".into() } else { url.to_string() },
            path,
            query: query.as_deref().map(parse_form).unwrap_or_default(),
            cookies,
            headers,
            body: body.into(),
            client_ip: "127.0.0.1".into(),
            scheme: "http".into(),
            host,
            port,
            protocol: "HTTP/1.1".into(),
            logged_at: Utc::now(),
        }
    }

    pub fn query_params(&self) -> &[(String, String)] {
        &self.query
    }

    pub fn query_values(&self, name: &str) -> Vec<&str> {
        self.query.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).collect()
    }

    pub fn header_values(&self, name: &str) -> Vec<&str> {
        self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn cookies(&self) -> &[(String, String)] {
        &self.cookies
    }

    pub fn cookie_values(&self, name: &str) -> Vec<&str> {
        self.cookies.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).collect()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    /// `application/x-www-form-urlencoded` body fields (empty for other content types).
    pub fn form_params(&self) -> Vec<(String, String)> {
        let is_form = self.content_type().is_none_or(|ct| ct.to_ascii_lowercase().contains("x-www-form-urlencoded"));
        if !is_form {
            return Vec::new();
        }
        parse_form(&self.body_text())
    }

    pub fn body_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// `user:password` from a Basic `Authorization` header.
    pub fn basic_credentials(&self) -> Option<String> {
        let auth = self.header("authorization")?;
        let (scheme, value) = auth.trim().split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return None;
        }
        b64_decode(value.trim()).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    pub fn base_url(&self) -> String {
        let default_port = (self.scheme == "http" && self.port == 80) || (self.scheme == "https" && self.port == 443);
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if default_port { format!("{}://{host}", self.scheme) } else { format!("{}://{host}:{}", self.scheme, self.port) }
    }

    pub fn absolute_url(&self) -> String {
        format!("{}{}", self.base_url(), self.url)
    }

    fn multi_map(pairs: &[(String, String)], canonical: bool) -> Map<String, Value> {
        let mut map: Map<String, Value> = Map::new();
        for (k, v) in pairs {
            let key = if canonical { canonical_header_name(k) } else { k.clone() };
            match map.get_mut(&key) {
                None => {
                    map.insert(key, Value::String(v.clone()));
                }
                Some(Value::Array(a)) => a.push(Value::String(v.clone())),
                Some(existing) => {
                    let first = existing.take();
                    *existing = Value::Array(vec![first, Value::String(v.clone())]);
                }
            }
        }
        map
    }

    fn key_values(pairs: &[(String, String)]) -> Map<String, Value> {
        let mut map: Map<String, Value> = Map::new();
        for (k, v) in pairs {
            let entry = map.entry(k.clone()).or_insert_with(|| json!({ "key": k, "values": [] }));
            if let Some(a) = entry.get_mut("values").and_then(Value::as_array_mut) {
                a.push(Value::String(v.clone()));
            }
        }
        map
    }

    /// WireMock's `LoggedRequest` JSON.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("id".into(), json!(self.id.to_string()));
        m.insert("url".into(), json!(self.url));
        m.insert("absoluteUrl".into(), json!(self.absolute_url()));
        m.insert("method".into(), json!(self.method));
        m.insert("clientIp".into(), json!(self.client_ip));
        m.insert("headers".into(), Value::Object(Self::multi_map(&self.headers, true)));
        m.insert("cookies".into(), Value::Object(Self::multi_map(&self.cookies, false)));
        m.insert("browserProxyRequest".into(), json!(false));
        m.insert("loggedDate".into(), json!(self.logged_at.timestamp_millis()));
        m.insert("bodyAsBase64".into(), json!(b64_encode(&self.body)));
        m.insert("body".into(), json!(self.body_text()));
        m.insert("protocol".into(), json!(self.protocol));
        m.insert("scheme".into(), json!(self.scheme));
        m.insert("host".into(), json!(self.host));
        m.insert("port".into(), json!(self.port));
        m.insert("loggedDateString".into(), json!(self.logged_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)));
        m.insert("queryParams".into(), Value::Object(Self::key_values(&self.query)));
        m.insert("formParams".into(), Value::Object(Self::key_values(&self.form_params())));
        Value::Object(m)
    }

    /// Rebuilds a request from `LoggedRequest` JSON (`POST /__admin/near-misses/request`).
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let obj = v.as_object().ok_or("request must be a JSON object")?;
        let url = obj.get("url").and_then(Value::as_str).or_else(|| obj.get("absoluteUrl").and_then(Value::as_str)).ok_or("request.url is required")?;
        let url = match url.find("://") {
            Some(i) => {
                let rest = &url[i + 3..];
                rest.find('/').map_or("/", |j| &rest[j..])
            }
            None => url,
        };
        let method = obj.get("method").and_then(Value::as_str).unwrap_or("GET");
        let mut headers = Vec::new();
        if let Some(h) = obj.get("headers").and_then(Value::as_object) {
            for (k, v) in h {
                match v {
                    Value::Array(a) => headers.extend(a.iter().map(|x| (k.clone(), crate::util::json_text(x)))),
                    other => headers.push((k.clone(), crate::util::json_text(other))),
                }
            }
        }
        if let Some(c) = obj.get("cookies").and_then(Value::as_object)
            && !c.is_empty()
            && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        {
            let mut parts = Vec::new();
            for (k, v) in c {
                let values: Vec<String> = match v {
                    Value::Array(a) => a.iter().map(crate::util::json_text).collect(),
                    Value::Object(o) => o.get("values").and_then(Value::as_array).map(|a| a.iter().map(crate::util::json_text).collect()).unwrap_or_default(),
                    other => vec![crate::util::json_text(other)],
                };
                parts.extend(values.into_iter().map(|x| format!("{k}={x}")));
            }
            headers.push(("cookie".into(), parts.join("; ")));
        }
        let body: Vec<u8> = match obj.get("bodyAsBase64").and_then(Value::as_str).filter(|s| !s.is_empty()).and_then(b64_decode) {
            Some(b) => b,
            None => obj.get("body").and_then(Value::as_str).unwrap_or("").as_bytes().to_vec(),
        };
        let mut req = Self::new(method, url, headers, body);
        if let Some(id) = obj.get("id").and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()) {
            req.id = id;
        }
        if let Some(ms) = obj.get("loggedDate").and_then(Value::as_i64).and_then(DateTime::from_timestamp_millis) {
            req.logged_at = ms;
        }
        if let Some(ip) = obj.get("clientIp").and_then(Value::as_str) {
            req.client_ip = ip.to_string();
        }
        if let Some(s) = obj.get("scheme").and_then(Value::as_str) {
            req.scheme = s.to_string();
        }
        if let Some(h) = obj.get("host").and_then(Value::as_str) {
            req.host = h.to_string();
        }
        if let Some(p) = obj.get("port").and_then(Value::as_u64) {
            req.port = p as u16;
        }
        Ok(req)
    }
}

pub(crate) fn split_host_port(host: &str) -> (String, Option<u16>) {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').unwrap_or((rest, ""));
        return (h.to_string(), tail.strip_prefix(':').and_then(|p| p.parse().ok()));
    }
    match host.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h.to_string(), p.parse().ok()),
        _ => (host.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_parts() {
        let r = MockRequest::new(
            "post",
            "/a/b?x=1&x=2&name=J%C3%BCrgen+M",
            vec![
                ("Host".into(), "localhost:8080".into()),
                ("Cookie".into(), "session=abc; theme=dark".into()),
                ("Content-Type".into(), "application/x-www-form-urlencoded".into()),
                ("Authorization".into(), "Basic dXNlcjpwYXNz".into()),
            ],
            "a=1&b=two",
        );
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/a/b");
        assert_eq!(r.query_values("x"), vec!["1", "2"]);
        assert_eq!(r.query_values("name"), vec!["Jürgen M"]);
        assert_eq!(r.cookie_values("theme"), vec!["dark"]);
        assert_eq!(r.form_params(), vec![("a".into(), "1".into()), ("b".into(), "two".into())]);
        assert_eq!(r.basic_credentials().as_deref(), Some("user:pass"));
        assert_eq!(r.base_url(), "http://localhost:8080");
        let j = r.to_json();
        assert_eq!(j["headers"]["Content-Type"], "application/x-www-form-urlencoded");
        assert_eq!(j["queryParams"]["x"]["values"], json!(["1", "2"]));
        let back = MockRequest::from_json(&j).unwrap();
        assert_eq!(back.url, r.url);
        assert_eq!(back.body, r.body);
        assert_eq!(back.cookie_values("session"), vec!["abc"]);
        assert_eq!(split_host_port("[::1]:9000"), ("::1".to_string(), Some(9000)));
    }
}
