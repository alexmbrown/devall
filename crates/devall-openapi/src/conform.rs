//! Per-exchange conformance checks: does this request / response match the spec? Shared by the
//! drift report (captured traffic, HAR, history) and the inspector's live validation rule.
//!
//! [`Conformance`] owns a parsed spec (compiled once) and checks one exchange at a time:
//! undocumented path / method / status, request bodies (required, schema) and JSON response
//! bodies against the response schema.

use serde::Serialize;
use serde_json::Value;

use crate::drift::DriftKind;
use crate::lint::Severity;
use crate::schema::{Direction, validate};
use crate::spec::{Operation, PathMatch, Spec, is_json};

/// One problem with an exchange.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Violation {
    pub kind: DriftKind,
    pub severity: Severity,
    /// `GET /users/{id}` when the request mapped to an operation.
    pub operation: Option<String>,
    pub message: String,
    /// Schema errors and similar specifics.
    pub details: Vec<String>,
}

fn v(kind: DriftKind, severity: Severity, operation: Option<String>, message: String, details: Vec<String>) -> Violation {
    Violation { kind, severity, operation, message, details }
}

pub(crate) fn summarize(errs: &[crate::schema::SchemaError]) -> Vec<String> {
    errs.iter().take(10).map(|e| e.to_string()).collect()
}

/// `/users/123` → `/users/{id}` for messages about undocumented paths.
pub(crate) fn undocumented_label(path: &str) -> String {
    path.split('/')
        .map(|s| {
            let looks_id = (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())) || (s.len() >= 32 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            if looks_id { "{id}".to_string() } else { s.to_string() }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// (`host[:port]`, path) of an absolute URL; `(None, path)` for a bare path.
pub(crate) fn split_url(url: &str) -> (Option<String>, String) {
    if let Ok(u) = url::Url::parse(url) {
        let host = u.host_str().map(|h| match u.port() {
            Some(p) => format!("{h}:{p}"),
            None => h.to_string(),
        });
        return (host.map(|h| h.to_ascii_lowercase()), u.path().to_string());
    }
    (None, url.split(['?', '#']).next().unwrap_or("").to_string())
}

/// The operation for `method` + `path`, or the violation explaining why there is none.
pub fn match_operation<'s>(spec: &'s Spec, method: &str, path: &str) -> Result<Operation<'s>, Violation> {
    let method = method.to_ascii_uppercase();
    match spec.match_path(&method, path) {
        PathMatch::Operation(o) => Ok(o),
        PathMatch::MethodNotDocumented { path: p, methods } => {
            Err(v(DriftKind::UndocumentedMethod, Severity::Error, None, format!("{method} is not documented for `{p}` (documented: {})", methods.join(", ")), Vec::new()))
        }
        PathMatch::NoPath => Err(v(DriftKind::UndocumentedPath, Severity::Error, None, format!("`{}` is not a documented path", undocumented_label(path)), Vec::new())),
    }
}

/// Request-body problems for a matched operation.
pub fn request_violations(spec: &Spec, op: &Operation<'_>, content_type: Option<&str>, body: Option<&str>) -> Vec<Violation> {
    let mut out = Vec::new();
    let key = op.key();
    let Some(rb) = op.op.get("requestBody").map(|r| spec.resolve(r)) else { return out };
    match body.filter(|b| !b.trim().is_empty()) {
        None if rb.get("required").and_then(Value::as_bool) == Some(true) && !["GET", "HEAD"].contains(&op.method.as_str()) => {
            out.push(v(DriftKind::InvalidRequestBody, Severity::Warning, Some(key.clone()), format!("{key} requires a request body but none was sent"), Vec::new()));
        }
        Some(b) => {
            if let Some(content) = rb.get("content")
                && let Some((media, Some(schema))) = spec.media_schema(content, content_type)
                && is_json(media)
                && let Ok(val) = serde_json::from_str::<Value>(b)
            {
                let errs = validate(spec, schema, &val, Direction::Request);
                if !errs.is_empty() {
                    out.push(v(DriftKind::InvalidRequestBody, Severity::Warning, Some(key.clone()), format!("The request body doesn't match the schema of {key}"), summarize(&errs)));
                }
            }
        }
        None => {}
    }
    out
}

/// Status / content-type / body problems of a response to a matched operation. `body = None`
/// skips the body check (not captured or not decodable).
pub fn response_violations(spec: &Spec, op: &Operation<'_>, status: u16, content_type: Option<&str>, body: Option<&str>) -> Vec<Violation> {
    let key = op.key();
    let Some((_, response)) = spec.response_for(op.op, status) else {
        return vec![v(DriftKind::UndocumentedStatus, Severity::Warning, Some(key.clone()), format!("Status {status} is not documented for {key}"), Vec::new())];
    };
    let Some(body) = body.filter(|b| !b.trim().is_empty()) else { return Vec::new() };
    let Some(content) = response.get("content").filter(|c| c.as_object().is_some_and(|m| !m.is_empty())) else { return Vec::new() };
    match spec.media_schema(content, content_type) {
        None => vec![v(
            DriftKind::UndocumentedContentType,
            Severity::Warning,
            Some(key.clone()),
            format!("Content type `{}` is not documented for {key} → {status}", content_type.map(|c| c.split(';').next().unwrap_or(c)).unwrap_or("?")),
            Vec::new(),
        )],
        Some((media, Some(schema))) if is_json(media) => match serde_json::from_str::<Value>(body) {
            Ok(val) => {
                let errs = validate(spec, schema, &val, Direction::Response);
                if errs.is_empty() {
                    Vec::new()
                } else {
                    vec![v(DriftKind::InvalidResponseBody, Severity::Error, Some(key.clone()), format!("The {status} response doesn't match the schema of {key}"), summarize(&errs))]
                }
            }
            Err(_) => vec![v(DriftKind::InvalidResponseBody, Severity::Error, Some(key.clone()), format!("The {status} response of {key} is not valid JSON"), Vec::new())],
        },
        _ => Vec::new(),
    }
}

/// A compiled spec for checking live traffic.
#[derive(Debug, Clone)]
pub struct Conformance {
    spec: Spec,
    hosts: Vec<String>,
    all_hosts: bool,
}

impl Conformance {
    /// `all_hosts`: check every request, not only those to the spec's `servers` hosts.
    pub fn new(spec: Spec, all_hosts: bool) -> Self {
        let hosts = spec.server_hosts();
        Self { spec, hosts, all_hosts }
    }

    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    /// Whether the URL is one this spec describes (host in `servers`, unless `all_hosts`).
    pub fn covers(&self, url: &str) -> bool {
        if self.all_hosts || self.hosts.is_empty() {
            return true;
        }
        let (host, _) = split_url(url);
        match host {
            Some(h) => self.hosts.iter().any(|x| x.eq_ignore_ascii_case(&h) || x.split(':').next() == Some(h.as_str()) || h.split(':').next() == Some(x.as_str())),
            None => true,
        }
    }

    /// Path, method and request-body problems.
    pub fn check_request(&self, method: &str, url: &str, content_type: Option<&str>, body: Option<&str>) -> Vec<Violation> {
        let (_, path) = split_url(url);
        match match_operation(&self.spec, method, &path) {
            Ok(op) => request_violations(&self.spec, &op, content_type, body),
            Err(v) => vec![v],
        }
    }

    /// Status, content-type and body problems of the response. Undocumented paths/methods are
    /// reported by [`check_request`](Self::check_request), so they yield nothing here.
    pub fn check_response(&self, method: &str, url: &str, status: u16, content_type: Option<&str>, body: Option<&str>) -> Vec<Violation> {
        let (_, path) = split_url(url);
        match match_operation(&self.spec, method, &path) {
            Ok(op) => response_violations(&self.spec, &op, status, content_type, body),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r#"
openapi: 3.1.0
info: { title: Shop, version: '1' }
servers: [{ url: https://api.shop.test/v1 }]
paths:
  /orders:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: object, required: [sku], properties: { sku: { type: string }, qty: { type: integer } } }
      responses:
        '201':
          description: Created
          content:
            application/json:
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"#;

    #[test]
    fn checks_requests_and_responses() {
        let c = Conformance::new(Spec::parse(SPEC).unwrap(), false);
        let url = "https://api.shop.test/v1/orders";
        assert!(c.covers(url) && !c.covers("https://other.test/v1/orders"));
        assert!(c.check_request("POST", url, Some("application/json"), Some(r#"{"sku":"A1","qty":2}"#)).is_empty());
        let bad = c.check_request("POST", url, Some("application/json"), Some(r#"{"qty":"two"}"#));
        assert_eq!(bad[0].kind, DriftKind::InvalidRequestBody);
        assert!(bad[0].details.iter().any(|d| d.contains("sku")), "{:?}", bad[0].details);
        assert_eq!(c.check_request("POST", url, None, None)[0].message, "POST /orders requires a request body but none was sent");
        assert_eq!(c.check_request("GET", url, None, None)[0].kind, DriftKind::UndocumentedMethod);
        assert_eq!(c.check_request("GET", "https://api.shop.test/v1/orders/123", None, None)[0].message, "`/v1/orders/{id}` is not a documented path");
        assert!(c.check_response("POST", url, 201, Some("application/json"), Some(r#"{"id":7}"#)).is_empty());
        assert_eq!(c.check_response("POST", url, 201, Some("application/json"), Some(r#"{"id":"x"}"#))[0].kind, DriftKind::InvalidResponseBody);
        assert_eq!(c.check_response("POST", url, 500, None, None)[0].kind, DriftKind::UndocumentedStatus);
        assert!(c.check_response("GET", "https://api.shop.test/v1/nope", 200, None, None).is_empty());
    }
}
