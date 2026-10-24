//! Spec ↔ mock ↔ traffic drift: do the project's mock stubs and the traffic actually seen
//! (inspector captures, HAR files, request history) agree with the OpenAPI spec?
//!
//! For every stub: its method and URL must map to a documented operation, its status must be
//! documented, and a JSON body must validate against the response schema. For every request
//! seen: path, method and status must be documented, and JSON request/response bodies must
//! validate. Traffic findings are grouped (one row per problem, with a count).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::lint::Severity;
use crate::schema::{Direction, validate};
use crate::conform::{Violation, match_operation, request_violations, response_violations, split_url, summarize};
use crate::spec::{Operation, PathMatch, Spec, is_json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DriftKind {
    /// No path template matches.
    UndocumentedPath,
    /// The path exists, the method doesn't.
    UndocumentedMethod,
    /// The status code isn't in the operation's responses.
    UndocumentedStatus,
    /// The response body doesn't match the response schema.
    InvalidResponseBody,
    /// The request body doesn't match the request body schema (or a required body is missing).
    InvalidRequestBody,
    /// The body's content type isn't documented for that response.
    UndocumentedContentType,
    /// Could not be checked (templated body, catch-all URL pattern, …).
    Unverifiable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Mock,
    Traffic,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftFinding {
    pub kind: DriftKind,
    pub severity: Severity,
    pub source_kind: SourceKind,
    /// Stub file (`mocks/mappings/x.json`) or traffic origin (`inspector`, `history`, a HAR file).
    pub source: String,
    /// Stub id or a sample id (exchange id, history id, HAR entry index).
    pub ref_id: Option<String>,
    /// Stub name or request URL.
    pub label: String,
    pub method: String,
    pub path: String,
    /// `GET /users/{id}` when the request mapped to an operation.
    pub operation: Option<String>,
    pub status: Option<u16>,
    pub message: String,
    /// Schema errors and similar specifics.
    pub details: Vec<String>,
    /// How many requests showed this problem (traffic findings are grouped).
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationCoverage {
    pub operation: String,
    pub operation_id: Option<String>,
    /// Stubs mapped to the operation.
    pub mocks: usize,
    /// Requests seen for the operation.
    pub traffic: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftReport {
    pub title: String,
    pub operations: usize,
    pub mocks_checked: usize,
    pub traffic_checked: usize,
    /// Requests skipped because their host is not one of the spec's servers.
    pub traffic_skipped: usize,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub findings: Vec<DriftFinding>,
    pub coverage: Vec<OperationCoverage>,
}

impl DriftReport {
    pub fn passed(&self) -> bool {
        self.errors == 0
    }
}

/// One stub mapping (WireMock JSON) and where it came from.
#[derive(Debug, Clone)]
pub struct MockInput {
    pub id: String,
    pub name: Option<String>,
    /// Relative file path, for messages.
    pub file: String,
    pub mapping: Value,
}

/// One request/response pair seen on the wire.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrafficSample {
    pub id: String,
    /// Absolute URL (or a path).
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub request_content_type: Option<String>,
    pub request_body: Option<String>,
    pub response_content_type: Option<String>,
    pub response_body: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TrafficOptions {
    /// Check every host, not only the hosts in the spec's `servers`.
    pub all_hosts: bool,
}

/// Collects findings from several sources into one report.
pub struct Drift<'a> {
    spec: &'a Spec,
    findings: Vec<DriftFinding>,
    coverage: BTreeMap<String, (Option<String>, usize, usize, usize)>,
    mocks_checked: usize,
    traffic_checked: usize,
    traffic_skipped: usize,
}

fn header<'v>(headers: Option<&'v Value>, name: &str) -> Option<&'v str> {
    headers?.as_object()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).and_then(|(_, v)| v.as_str().or_else(|| v.as_array().and_then(|a| a.first()).and_then(Value::as_str)))
}

impl<'a> Drift<'a> {
    pub fn new(spec: &'a Spec) -> Self {
        let mut coverage = BTreeMap::new();
        for (i, op) in spec.operations().iter().enumerate() {
            coverage.insert(op.key(), (op.operation_id().map(str::to_string), 0, 0, i));
        }
        Self { spec, findings: Vec::new(), coverage, mocks_checked: 0, traffic_checked: 0, traffic_skipped: 0 }
    }

    fn cover(&mut self, op: &Operation<'_>, mock: bool) {
        if let Some(c) = self.coverage.get_mut(&op.key()) {
            if mock {
                c.1 += 1;
            } else {
                c.2 += 1;
            }
        }
    }

    // ---------- mocks ----------

    /// Checks stubs. `read_body_file` returns the text of a `bodyFileName` (relative to
    /// `mocks/__files/`).
    pub fn add_mocks(&mut self, mocks: &[MockInput], read_body_file: &dyn Fn(&str) -> Option<String>) {
        for m in mocks {
            self.mocks_checked += 1;
            self.check_mock(m, read_body_file);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn mock_finding(&mut self, m: &MockInput, kind: DriftKind, severity: Severity, method: &str, path: &str, op: Option<String>, status: Option<u16>, message: String, details: Vec<String>) {
        self.findings.push(DriftFinding {
            kind,
            severity,
            source_kind: SourceKind::Mock,
            source: m.file.clone(),
            ref_id: Some(m.id.clone()),
            label: m.name.clone().unwrap_or_else(|| format!("{method} {path}")),
            method: method.to_string(),
            path: path.to_string(),
            operation: op,
            status,
            message,
            details,
            count: 1,
        });
    }

    fn check_mock(&mut self, m: &MockInput, read_body_file: &dyn Fn(&str) -> Option<String>) {
        let spec = self.spec;
        let req = m.mapping.get("request").cloned().unwrap_or(Value::Null);
        let resp = m.mapping.get("response").cloned().unwrap_or(Value::Null);
        let method = req.get("method").and_then(Value::as_str).unwrap_or("ANY").to_ascii_uppercase();
        let s = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_string);
        // Concrete path for exact matchers, a sample for templates; regexes are tried against
        // every documented path.
        let (shown, concrete): (String, Option<String>) = if let Some(u) = s("url") {
            (u.clone(), Some(u.split('?').next().unwrap_or("").to_string()))
        } else if let Some(u) = s("urlPath") {
            (u.clone(), Some(u))
        } else if let Some(t) = s("urlPathTemplate") {
            (t.clone(), Some(sample_template(&t)))
        } else if let Some(p) = s("urlPathPattern").or_else(|| s("urlPattern")) {
            (p, None)
        } else {
            self.mock_finding(m, DriftKind::Unverifiable, Severity::Info, &method, "*", None, None, "Matches any URL; not checked against the spec".into(), Vec::new());
            return;
        };
        let template_path: Option<String> = match concrete {
            Some(p) => Some(p),
            None => {
                let pattern = s("urlPathPattern").or_else(|| s("urlPattern")).unwrap_or_default();
                match self.paths_matching_regex(&pattern) {
                    Ok(found) if found.len() == 1 => Some(sample_template(&found[0])),
                    Ok(found) if found.is_empty() => {
                        self.mock_finding(m, DriftKind::UndocumentedPath, Severity::Error, &method, &shown, None, None, format!("URL pattern `{shown}` matches no documented path"), Vec::new());
                        return;
                    }
                    Ok(found) => {
                        let list: Vec<String> = found.iter().take(5).cloned().collect();
                        self.mock_finding(m, DriftKind::Unverifiable, Severity::Info, &method, &shown, None, None, format!("URL pattern matches {} documented paths; not checked", found.len()), list);
                        return;
                    }
                    Err(e) => {
                        self.mock_finding(m, DriftKind::Unverifiable, Severity::Info, &method, &shown, None, None, format!("URL pattern can't be checked: {e}"), Vec::new());
                        return;
                    }
                }
            }
        };
        let path = template_path.unwrap_or_default();
        let status = resp.get("status").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(200) as u16;

        // ANY: the first operation on the path that documents the status (else the first one).
        let op = if method == "ANY" {
            let ops: Vec<Operation<'_>> = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
                .iter()
                .filter_map(|mm| match spec.match_path(mm, &path) {
                    PathMatch::Operation(o) => Some(o),
                    _ => None,
                })
                .collect();
            match ops.iter().find(|o| spec.response_for(o.op, status).is_some()).or(ops.first()) {
                Some(o) => o.clone(),
                None => {
                    self.mock_finding(m, DriftKind::UndocumentedPath, Severity::Error, &method, &shown, None, Some(status), format!("`{shown}` is not a documented path"), Vec::new());
                    return;
                }
            }
        } else {
            match spec.match_path(&method, &path) {
                PathMatch::Operation(o) => o,
                PathMatch::MethodNotDocumented { path: p, methods } => {
                    self.mock_finding(m, DriftKind::UndocumentedMethod, Severity::Error, &method, &shown, None, Some(status), format!("{method} is not documented for `{p}` (documented: {})", methods.join(", ")), Vec::new());
                    return;
                }
                PathMatch::NoPath => {
                    self.mock_finding(m, DriftKind::UndocumentedPath, Severity::Error, &method, &shown, None, Some(status), format!("`{shown}` is not a documented path"), Vec::new());
                    return;
                }
            }
        };
        self.cover(&op, true);
        let op_key = Some(op.key());

        if resp.get("fault").is_some_and(|f| !f.is_null()) || resp.get("proxyBaseUrl").is_some_and(|p| p.as_str().is_some_and(|s| !s.is_empty())) {
            return; // faults and proxies have no body of their own
        }
        let Some((_, response)) = spec.response_for(op.op, status) else {
            let documented: Vec<String> = op.op.get("responses").and_then(Value::as_object).map(|r| r.keys().cloned().collect()).unwrap_or_default();
            self.mock_finding(m, DriftKind::UndocumentedStatus, Severity::Warning, &method, &shown, op_key, Some(status), format!("Status {status} is not documented for {} (documented: {})", op.key(), documented.join(", ")), Vec::new());
            return;
        };

        // Body.
        let templated = resp.get("transformers").is_some_and(|t| t.to_string().contains("response-template")) || resp.pointer("/transformerParameters/dataSource").is_some();
        let headers = resp.get("headers");
        let ct = header(headers, "content-type").map(str::to_string);
        let (body, ct): (Option<String>, Option<String>) = if let Some(j) = resp.get("jsonBody").filter(|v| !v.is_null()) {
            (Some(j.to_string()), ct.or(Some("application/json".into())))
        } else if let Some(b) = resp.get("body").and_then(Value::as_str) {
            (Some(b.to_string()), ct)
        } else if let Some(f) = resp.get("bodyFileName").and_then(Value::as_str) {
            match read_body_file(f) {
                Some(t) => (Some(t), ct),
                None => {
                    self.mock_finding(m, DriftKind::Unverifiable, Severity::Info, &method, &shown, op_key, Some(status), format!("Body file `{f}` can't be read (binary or missing); not checked"), Vec::new());
                    return;
                }
            }
        } else {
            (None, ct)
        };
        let Some(body) = body.filter(|b| !b.trim().is_empty()) else { return };
        let content = response.get("content");
        if content.and_then(Value::as_object).is_none_or(|c| c.is_empty()) {
            self.mock_finding(m, DriftKind::UndocumentedContentType, Severity::Info, &method, &shown, op_key, Some(status), format!("The stub returns a body but {} → {status} documents none", op.key()), Vec::new());
            return;
        }
        let ct = ct.or_else(|| serde_json::from_str::<Value>(&body).ok().map(|_| "application/json".to_string()));
        let Some((media, schema)) = spec.media_schema(content.unwrap_or(&Value::Null), ct.as_deref()) else {
            self.mock_finding(
                m,
                DriftKind::UndocumentedContentType,
                Severity::Warning,
                &method,
                &shown,
                op_key,
                Some(status),
                format!("Content type `{}` is not documented for {} → {status}", ct.as_deref().unwrap_or("?"), op.key()),
                Vec::new(),
            );
            return;
        };
        let Some(schema) = schema else { return };
        if !is_json(media) && !ct.as_deref().is_some_and(is_json) {
            return;
        }
        let value = match serde_json::from_str::<Value>(&body) {
            Ok(v) => v,
            Err(_) if templated || body.contains("{{") => {
                self.mock_finding(m, DriftKind::Unverifiable, Severity::Info, &method, &shown, op_key, Some(status), "Templated body; validated only when it renders to JSON at runtime (check served traffic instead)".into(), Vec::new());
                return;
            }
            Err(e) => {
                self.mock_finding(m, DriftKind::InvalidResponseBody, Severity::Error, &method, &shown, op_key, Some(status), format!("The body is not valid JSON: {e}"), Vec::new());
                return;
            }
        };
        if templated && value.to_string().contains("{{") {
            self.mock_finding(m, DriftKind::Unverifiable, Severity::Info, &method, &shown, op_key, Some(status), "Templated body; placeholders are not validated".into(), Vec::new());
            return;
        }
        let errs = validate(spec, schema, &value, Direction::Response);
        if !errs.is_empty() {
            self.mock_finding(
                m,
                DriftKind::InvalidResponseBody,
                Severity::Error,
                &method,
                &shown,
                op_key.clone(),
                Some(status),
                format!("The body doesn't match the {status} response schema of {} ({} problem{})", op.key(), errs.len(), if errs.len() == 1 { "" } else { "s" }),
                summarize(&errs),
            );
        }
    }

    /// Documented path templates whose sample paths fully match a WireMock URL regex.
    fn paths_matching_regex(&self, pattern: &str) -> Result<Vec<String>, String> {
        let re = regex::Regex::new(&format!("^(?:{pattern})$")).map_err(|e| e.to_string())?;
        let Some(paths) = self.spec.doc.get("paths").and_then(Value::as_object) else { return Ok(Vec::new()) };
        let bases = self.spec.base_paths();
        let mut out = Vec::new();
        for t in paths.keys() {
            let hit = ["1", "abc", "123e4567-e89b-42d3-a456-426614174000"].iter().any(|fill| {
                let sample = fill_template(t, fill);
                bases.iter().any(|b| {
                    let full = format!("{b}{sample}");
                    re.is_match(&full) || re.is_match(&format!("{full}?x=1"))
                })
            });
            if hit {
                out.push(t.clone());
            }
        }
        Ok(out)
    }

    // ---------- traffic ----------

    /// Checks requests seen on the wire; `origin` names where they came from (for messages).
    pub fn add_traffic(&mut self, origin: &str, samples: &[TrafficSample], opts: &TrafficOptions) {
        let hosts = self.spec.server_hosts();
        let mut grouped: BTreeMap<(DriftKind, String, Option<u16>, String), DriftFinding> = BTreeMap::new();
        for t in samples {
            let (host, path) = split_url(&t.url);
            if !opts.all_hosts
                && !hosts.is_empty()
                && let Some(h) = &host
                && !hosts.iter().any(|x| x.eq_ignore_ascii_case(h) || x.split(':').next() == Some(h.as_str()) || h.split(':').next() == Some(x.as_str()))
            {
                self.traffic_skipped += 1;
                continue;
            }
            self.traffic_checked += 1;
            for f in self.check_sample(origin, t, &path) {
                let key = (f.kind, f.operation.clone().unwrap_or_else(|| format!("{} {}", f.method, f.path)), f.status, f.message.clone());
                grouped
                    .entry(key)
                    .and_modify(|g| {
                        g.count += 1;
                        if g.details.len() < 3 && !g.details.contains(&t.url) && f.details.is_empty() {
                            g.details.push(t.url.clone());
                        }
                    })
                    .or_insert(f);
            }
        }
        self.findings.extend(grouped.into_values());
    }

    fn check_sample(&mut self, origin: &str, t: &TrafficSample, path: &str) -> Vec<DriftFinding> {
        let spec = self.spec;
        let method = t.method.to_ascii_uppercase();
        let finding = |x: Violation| DriftFinding {
            kind: x.kind,
            severity: x.severity,
            source_kind: SourceKind::Traffic,
            source: origin.to_string(),
            ref_id: Some(t.id.clone()),
            label: t.url.clone(),
            method: method.clone(),
            path: path.to_string(),
            operation: x.operation,
            status: t.status,
            message: x.message,
            details: x.details,
            count: 1,
        };
        let op = match match_operation(spec, &method, path) {
            Ok(o) => o,
            Err(v) => return vec![finding(v)],
        };
        self.cover(&op, false);
        let mut out: Vec<DriftFinding> = request_violations(spec, &op, t.request_content_type.as_deref(), t.request_body.as_deref()).into_iter().map(finding).collect();
        if let Some(status) = t.status {
            out.extend(response_violations(spec, &op, status, t.response_content_type.as_deref(), t.response_body.as_deref()).into_iter().map(finding));
        }
        out
    }

    pub fn finish(self) -> DriftReport {
        let mut findings = self.findings;
        findings.sort_by(|a, b| a.severity.cmp(&b.severity).then_with(|| a.kind.cmp(&b.kind)).then_with(|| a.source.cmp(&b.source)).then_with(|| a.path.cmp(&b.path)));
        let count = |s: Severity| findings.iter().filter(|f| f.severity == s).count();
        let mut coverage: Vec<(usize, OperationCoverage)> =
            self.coverage.into_iter().map(|(operation, (operation_id, mocks, traffic, order))| (order, OperationCoverage { operation, operation_id, mocks, traffic })).collect();
        coverage.sort_by_key(|(o, _)| *o);
        DriftReport {
            title: self.spec.title(),
            operations: coverage.len(),
            mocks_checked: self.mocks_checked,
            traffic_checked: self.traffic_checked,
            traffic_skipped: self.traffic_skipped,
            errors: count(Severity::Error),
            warnings: count(Severity::Warning),
            infos: count(Severity::Info),
            findings,
            coverage: coverage.into_iter().map(|(_, c)| c).collect(),
        }
    }
}


/// `/users/{id}` → `/users/1`.
fn sample_template(t: &str) -> String {
    fill_template(t, "1")
}

fn fill_template(t: &str, fill: &str) -> String {
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(j) => {
                out.push_str(fill);
                rest = &rest[i + j + 1..];
            }
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SPEC: &str = r##"
openapi: 3.0.3
info: { title: Shop, version: "1" }
servers: [{ url: https://api.example.com/v1 }]
paths:
  /users:
    get:
      operationId: listUsers
      responses:
        "200": { description: ok, content: { application/json: { schema: { type: array, items: { $ref: '#/components/schemas/User' } } } } }
    post:
      operationId: createUser
      requestBody:
        required: true
        content: { application/json: { schema: { $ref: '#/components/schemas/User' } } }
      responses:
        "201": { description: created, content: { application/json: { schema: { $ref: '#/components/schemas/User' } } } }
  /users/{id}:
    get:
      operationId: getUser
      parameters: [{ name: id, in: path, required: true, schema: { type: integer } }]
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: '#/components/schemas/User' } } } }
        "404": { description: missing }
components:
  schemas:
    User:
      type: object
      required: [id, name]
      properties: { id: { type: integer }, name: { type: string } }
"##;

    fn mock(id: &str, mapping: Value) -> MockInput {
        MockInput { id: id.into(), name: None, file: format!("mocks/mappings/{id}.json"), mapping }
    }

    #[test]
    fn mocks_against_spec() {
        let spec = Spec::parse(SPEC).unwrap();
        let mut d = Drift::new(&spec);
        let mocks = vec![
            mock("ok", json!({ "request": { "method": "GET", "urlPathTemplate": "/v1/users/{id}" }, "response": { "status": 200, "jsonBody": { "id": 1, "name": "Ann" } } })),
            mock("bad-body", json!({ "request": { "method": "GET", "urlPath": "/v1/users" }, "response": { "status": 200, "jsonBody": [{ "id": "x" }] } })),
            mock("bad-status", json!({ "request": { "method": "GET", "urlPath": "/users/7" }, "response": { "status": 500 } })),
            mock("bad-method", json!({ "request": { "method": "DELETE", "urlPath": "/users/7" }, "response": { "status": 204 } })),
            mock("bad-path", json!({ "request": { "method": "GET", "url": "/orders?x=1" }, "response": { "status": 200 } })),
            mock("regex", json!({ "request": { "method": "GET", "urlPathPattern": "/v1/users/[0-9]+" }, "response": { "status": 200, "bodyFileName": "user.json", "headers": { "Content-Type": "application/json" } } })),
            mock("catch-all", json!({ "request": { "urlPattern": ".*" }, "response": { "status": 200 } })),
            mock("templated", json!({ "request": { "method": "POST", "urlPath": "/users" }, "response": { "status": 201, "body": "{\"id\": {{randomInt}}, \"name\": \"x\"}", "transformers": ["response-template"] } })),
            mock("html", json!({ "request": { "method": "GET", "urlPath": "/users/1" }, "response": { "status": 200, "body": "<p>hi</p>", "headers": { "Content-Type": "text/html" } } })),
        ];
        d.add_mocks(&mocks, &|f| (f == "user.json").then(|| r#"{"id": 2}"#.to_string()));
        let r = d.finish();
        let by = |id: &str| r.findings.iter().filter(|f| f.ref_id.as_deref() == Some(id)).map(|f| f.kind).collect::<Vec<_>>();
        assert!(by("ok").is_empty(), "{:#?}", r.findings);
        assert_eq!(by("bad-body"), vec![DriftKind::InvalidResponseBody]);
        assert_eq!(by("bad-status"), vec![DriftKind::UndocumentedStatus]);
        assert_eq!(by("bad-method"), vec![DriftKind::UndocumentedMethod]);
        assert_eq!(by("bad-path"), vec![DriftKind::UndocumentedPath]);
        assert_eq!(by("regex"), vec![DriftKind::InvalidResponseBody], "body file lacks name");
        assert_eq!(by("catch-all"), vec![DriftKind::Unverifiable]);
        assert_eq!(by("templated"), vec![DriftKind::Unverifiable]);
        assert_eq!(by("html"), vec![DriftKind::UndocumentedContentType]);
        let bad = r.findings.iter().find(|f| f.ref_id.as_deref() == Some("bad-body")).unwrap();
        assert!(bad.details.iter().any(|d| d.contains("$[0].id: expected integer")), "{:?}", bad.details);
        assert_eq!(r.coverage.iter().map(|c| (c.operation.as_str(), c.mocks)).collect::<Vec<_>>(), vec![("GET /users", 1), ("POST /users", 1), ("GET /users/{id}", 4)]);
        assert_eq!(r.mocks_checked, 9);
        assert!(!r.passed());
    }

    #[test]
    fn traffic_against_spec() {
        let spec = Spec::parse(SPEC).unwrap();
        let mut d = Drift::new(&spec);
        let s = |id: &str, method: &str, url: &str, status: u16, body: &str| TrafficSample {
            id: id.into(),
            url: url.into(),
            method: method.into(),
            status: Some(status),
            response_content_type: Some("application/json".into()),
            response_body: Some(body.into()),
            ..Default::default()
        };
        let samples = vec![
            s("1", "GET", "https://api.example.com/v1/users/1", 200, r#"{"id":1,"name":"Ann"}"#),
            s("2", "GET", "https://api.example.com/v1/users/2", 200, r#"{"id":2}"#),
            s("3", "GET", "https://api.example.com/v1/users/3", 200, r#"{"id":3}"#),
            s("4", "GET", "https://api.example.com/v1/orders/9", 200, "{}"),
            s("5", "GET", "https://api.example.com/v1/users/4", 418, ""),
            s("6", "GET", "https://cdn.other.com/app.js", 200, ""),
            TrafficSample { id: "7".into(), url: "https://api.example.com/v1/users".into(), method: "POST".into(), status: Some(201), request_content_type: Some("application/json".into()), request_body: Some(r#"{"name": 5}"#.into()), response_content_type: Some("application/json".into()), response_body: Some(r#"{"id":5,"name":"x"}"#.into()) },
        ];
        d.add_traffic("inspector", &samples, &TrafficOptions::default());
        let r = d.finish();
        assert_eq!((r.traffic_checked, r.traffic_skipped), (6, 1));
        let invalid = r.findings.iter().find(|f| f.kind == DriftKind::InvalidResponseBody).unwrap();
        assert_eq!(invalid.count, 2, "grouped");
        assert_eq!(invalid.operation.as_deref(), Some("GET /users/{id}"));
        let path = r.findings.iter().find(|f| f.kind == DriftKind::UndocumentedPath).unwrap();
        assert!(path.message.contains("/v1/orders/{id}"), "{}", path.message);
        assert!(r.findings.iter().any(|f| f.kind == DriftKind::UndocumentedStatus && f.status == Some(418)));
        assert!(r.findings.iter().any(|f| f.kind == DriftKind::InvalidRequestBody));
        let get_user = r.coverage.iter().find(|c| c.operation == "GET /users/{id}").unwrap();
        assert_eq!(get_user.traffic, 4);

        let mut all = Drift::new(&spec);
        all.add_traffic("har", &samples[5..6], &TrafficOptions { all_hosts: true });
        assert_eq!(all.finish().findings[0].kind, DriftKind::UndocumentedPath);
    }
}
