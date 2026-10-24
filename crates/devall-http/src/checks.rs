//! Assertions (`tests:`) and captures (`captures:`) evaluated against a response.

use std::path::Path;

use devall_core::{AssertOp, AssertTarget, Assertion, Capture, CaptureSource, Resolver, Variables};
use serde::Serialize;
use serde_json::Value;

use crate::HttpResponseData;
use crate::snapshot::{self, Outcome};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssertionResult {
    /// Human-readable description, e.g. `status eq 200`.
    pub label: String,
    pub passed: bool,
    /// The value that was checked (truncated), when there was one.
    pub actual: Option<String>,
    pub message: Option<String>,
}

/// `pm.test(...)` results from pre-request / post-response scripts show up as assertion rows.
impl From<devall_script::ScriptTestResult> for AssertionResult {
    fn from(t: devall_script::ScriptTestResult) -> Self {
        Self { label: t.label, passed: t.passed, actual: t.actual, message: t.message }
    }
}

fn json_body(resp: &HttpResponseData) -> Option<Value> {
    serde_json::from_slice(&resp.body).ok()
}

/// Evaluates an RFC 9535 JSONPath; returns every node found.
pub fn json_path(body: &Value, path: &str) -> Result<Vec<Value>, String> {
    let p = serde_json_path::JsonPath::parse(path.trim()).map_err(|e| format!("invalid JSONPath: {e}"))?;
    Ok(p.query(body).all().into_iter().cloned().collect())
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn target_label(t: &AssertTarget) -> String {
    match t {
        AssertTarget::Status => "status".into(),
        AssertTarget::Header { name } => format!("header {name}"),
        AssertTarget::JsonPath { path } => format!("jsonPath {path}"),
        AssertTarget::Body => "body".into(),
        AssertTarget::ResponseTime => "responseTime".into(),
        AssertTarget::Size => "size".into(),
        AssertTarget::ContentType => "contentType".into(),
        AssertTarget::Snapshot { file, .. } => format!("snapshot {file}"),
    }
}

fn op_label(op: AssertOp) -> &'static str {
    match op {
        AssertOp::Eq => "eq",
        AssertOp::Ne => "ne",
        AssertOp::Lt => "lt",
        AssertOp::Lte => "lte",
        AssertOp::Gt => "gt",
        AssertOp::Gte => "gte",
        AssertOp::Contains => "contains",
        AssertOp::NotContains => "notContains",
        AssertOp::StartsWith => "startsWith",
        AssertOp::EndsWith => "endsWith",
        AssertOp::Matches => "matches",
        AssertOp::Exists => "exists",
        AssertOp::NotExists => "notExists",
        AssertOp::IsEmpty => "isEmpty",
        AssertOp::IsNotEmpty => "isNotEmpty",
        AssertOp::IsType => "isType",
        AssertOp::OneOf => "oneOf",
    }
}

/// The value(s) an assertion inspects. `None` = absent (missing header, no JSON match).
fn actual(t: &AssertTarget, resp: &HttpResponseData, json: &Option<Value>) -> Result<Option<Value>, String> {
    Ok(match t {
        AssertTarget::Status => Some(Value::from(resp.status)),
        AssertTarget::Header { name } => resp.header(name).map(|v| Value::String(v.to_string())),
        AssertTarget::JsonPath { path } => {
            let Some(body) = json else { return Err("response body is not JSON".into()) };
            let nodes = json_path(body, path)?;
            match nodes.len() {
                0 => None,
                1 => nodes.into_iter().next(),
                _ => Some(Value::Array(nodes)),
            }
        }
        AssertTarget::Body => Some(Value::String(String::from_utf8_lossy(&resp.body).into_owned())),
        AssertTarget::ResponseTime => Some(Value::from(resp.timings.total_ms)),
        AssertTarget::Size => Some(Value::from(resp.body_size)),
        AssertTarget::ContentType => resp.header("content-type").map(|v| Value::String(v.to_string())),
        AssertTarget::Snapshot { .. } => return Err("snapshot assertions are evaluated separately".into()),
    })
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// Equality that tolerates `"200"` vs `200` and JSON-encoded expectations (`{"a":1}`, `true`).
fn loosely_equal(actual: &Value, expected: &str) -> bool {
    if value_to_string(actual) == expected {
        return true;
    }
    if let (Some(a), Ok(e)) = (as_f64(actual), expected.trim().parse::<f64>()) {
        return (a - e).abs() < f64::EPSILON * a.abs().max(1.0) * 4.0;
    }
    serde_json::from_str::<Value>(expected).is_ok_and(|e| &e == actual)
}

fn check(op: AssertOp, actual: Option<&Value>, expected: &str) -> Result<bool, String> {
    use AssertOp::*;
    let need = || actual.ok_or_else(|| "value not found".to_string());
    Ok(match op {
        Exists => actual.is_some(),
        NotExists => actual.is_none(),
        Eq => loosely_equal(need()?, expected),
        Ne => !loosely_equal(need()?, expected),
        Lt | Lte | Gt | Gte => {
            let a = as_f64(need()?).ok_or("not a number")?;
            let e: f64 = expected.trim().parse().map_err(|_| format!("expected value `{expected}` is not a number"))?;
            match op {
                Lt => a < e,
                Lte => a <= e,
                Gt => a > e,
                _ => a >= e,
            }
        }
        Contains | NotContains => {
            let v = need()?;
            let found = match v {
                Value::Array(items) => items.iter().any(|i| loosely_equal(i, expected)),
                other => value_to_string(other).contains(expected),
            };
            if op == Contains { found } else { !found }
        }
        StartsWith => value_to_string(need()?).starts_with(expected),
        EndsWith => value_to_string(need()?).ends_with(expected),
        Matches => {
            let re = regex::Regex::new(expected).map_err(|e| format!("invalid regex: {e}"))?;
            re.is_match(&value_to_string(need()?))
        }
        IsEmpty => actual.is_none_or(is_empty),
        IsNotEmpty => actual.is_some_and(|v| !is_empty(v)),
        IsType => type_name(need()?) == expected.trim(),
        OneOf => {
            let v = need()?;
            expected.split(',').map(str::trim).any(|e| loosely_equal(v, e))
        }
    })
}

/// Where file-backed assertions (snapshots) live, and whether to (re)write them.
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckContext<'a> {
    /// Project root; snapshot assertions fail without it.
    pub project_root: Option<&'a Path>,
    /// Request path (relative to `http/`), recorded in new snapshot files.
    pub request_path: Option<&'a str>,
    /// Write missing or changed snapshots instead of failing (`devall run --update-snapshots`).
    pub update_snapshots: bool,
}

/// Runs every enabled assertion. Expected values may contain `{{variables}}`.
pub fn evaluate(assertions: &[Assertion], resp: &HttpResponseData, r: &mut Resolver) -> Vec<AssertionResult> {
    evaluate_in(assertions, resp, r, &CheckContext::default())
}

/// [`evaluate`] with a project context, needed by `snapshot` assertions.
pub fn evaluate_in(assertions: &[Assertion], resp: &HttpResponseData, r: &mut Resolver, cx: &CheckContext) -> Vec<AssertionResult> {
    let json = json_body(resp);
    assertions
        .iter()
        .filter(|a| a.enabled)
        .map(|a| {
            if let AssertTarget::Snapshot { file, ignore } = &a.target {
                return snapshot_result(file, ignore, a.op, resp, cx);
            }
            let expected = a.value.as_deref().map(|v| r.resolve(v)).unwrap_or_default();
            let mut label = format!("{} {}", target_label(&a.target), op_label(a.op));
            if a.value.is_some() {
                label.push(' ');
                label.push_str(&expected);
            }
            match actual(&a.target, resp, &json) {
                Err(message) => AssertionResult { label, passed: false, actual: None, message: Some(message) },
                Ok(value) => {
                    let shown = value.as_ref().map(|v| truncate(&value_to_string(v), 200));
                    match check(a.op, value.as_ref(), &expected) {
                        Ok(passed) => AssertionResult { label, passed, actual: shown, message: None },
                        Err(message) => AssertionResult { label, passed: false, actual: shown, message: Some(message) },
                    }
                }
            }
        })
        .collect()
}

/// `snapshot <file>`: `eq` (the default) passes when the body matches, `ne` when it differs.
fn snapshot_result(file: &str, ignore: &[String], op: AssertOp, resp: &HttpResponseData, cx: &CheckContext) -> AssertionResult {
    let label = format!("snapshot {file}");
    let fail = |message: String| AssertionResult { label: label.clone(), passed: false, actual: None, message: Some(message) };
    let Some(root) = cx.project_root else { return fail("snapshot assertions need a project".into()) };
    if !matches!(op, AssertOp::Eq | AssertOp::Ne) {
        return fail("snapshot assertions support eq (matches) and ne (differs)".into());
    }
    let update = cx.update_snapshots && op == AssertOp::Eq;
    match snapshot::check(root, file, &resp.body, resp.header("content-type"), cx.request_path, ignore, update) {
        Err(e) => fail(e),
        Ok(Outcome::Match) => AssertionResult { label, passed: op == AssertOp::Eq, actual: None, message: (op == AssertOp::Ne).then(|| "the body matches the snapshot".into()) },
        Ok(Outcome::Written { created }) => AssertionResult { label, passed: true, actual: None, message: Some(if created { "snapshot written".into() } else { "snapshot updated".into() }) },
        Ok(Outcome::Missing) => fail(format!("{file} doesn't exist yet — record it with `devall run --update-snapshots` or “Promote to snapshot test” in History")),
        Ok(Outcome::Mismatch(d)) if op == AssertOp::Ne => AssertionResult { label, passed: true, actual: d.first().cloned(), message: None },
        Ok(Outcome::Mismatch(d)) => AssertionResult {
            label,
            passed: false,
            actual: d.first().cloned(),
            message: Some(format!("body differs from the snapshot: {} (accept with `devall run --update-snapshots` or “Update snapshot”)", d.join("; "))),
        },
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureResult {
    pub name: String,
    pub value: Option<String>,
    pub error: Option<String>,
}

/// Extracts every enabled capture. Successful values are returned in `vars` too.
pub fn capture(captures: &[Capture], resp: &HttpResponseData) -> (Variables, Vec<CaptureResult>) {
    let json = json_body(resp);
    let body = String::from_utf8_lossy(&resp.body).into_owned();
    let mut vars = Variables::new();
    let mut results = Vec::new();
    for c in captures.iter().filter(|c| c.enabled && !c.name.trim().is_empty()) {
        let got: Result<Option<String>, String> = match &c.source {
            CaptureSource::Status => Ok(Some(resp.status.to_string())),
            CaptureSource::Header { header } => Ok(resp.header(header).map(str::to_string)),
            CaptureSource::Body => Ok(Some(body.clone())),
            CaptureSource::Cookie { cookie } => Ok(resp
                .headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
                .filter_map(|(_, v)| v.split(';').next()?.split_once('='))
                .find(|(k, _)| k.trim() == cookie)
                .map(|(_, v)| v.trim().to_string())),
            CaptureSource::JsonPath { path } => match &json {
                None => Err("response body is not JSON".into()),
                Some(j) => json_path(j, path).map(|nodes| match nodes.len() {
                    0 => None,
                    1 => Some(value_to_string(&nodes[0])),
                    _ => Some(Value::Array(nodes).to_string()),
                }),
            },
            CaptureSource::Regex { pattern, group } => regex::Regex::new(pattern)
                .map_err(|e| format!("invalid regex: {e}"))
                .map(|re| re.captures(&body).and_then(|caps| caps.get(*group)).map(|m| m.as_str().to_string())),
        };
        let (value, error) = match got {
            Ok(Some(v)) => {
                vars.insert(c.name.trim().to_string(), v.clone());
                (Some(v), None)
            }
            Ok(None) => (None, Some("not found".into())),
            Err(e) => (None, Some(e)),
        };
        results.push(CaptureResult { name: c.name.trim().to_string(), value, error });
    }
    (vars, results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timings;
    use devall_core::vars::NoSecrets;

    fn resp(status: u16, body: &str) -> HttpResponseData {
        HttpResponseData {
            status,
            status_text: String::new(),
            http_version: "HTTP/1.1".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("set-cookie".into(), "sid=abc123; Path=/; HttpOnly".into()),
                ("x-request-id".into(), "r-1".into()),
            ],
            body: body.as_bytes().to_vec(),
            body_truncated: false,
            url: "http://x".into(),
            redirected: false,
            remote_addr: None,
            timings: Timings { ttfb_ms: 10.0, download_ms: 2.0, total_ms: 12.0 },
            body_size: body.len(),
        }
    }

    fn a(target: AssertTarget, op: AssertOp, value: Option<&str>) -> Assertion {
        Assertion { target, op, value: value.map(str::to_string), enabled: true }
    }

    #[test]
    fn assertions() {
        let r = resp(201, r#"{"data":{"id":7,"tags":["a","b"],"name":"Ada","empty":[]},"ok":true}"#);
        let mut res = Resolver::new(&NoSecrets);
        res.push_layer([("expectedId".to_string(), "7".to_string())].into_iter().collect());
        let checks = vec![
            a(AssertTarget::Status, AssertOp::Eq, Some("201")),
            a(AssertTarget::Status, AssertOp::OneOf, Some("200, 201")),
            a(AssertTarget::JsonPath { path: "$.data.id".into() }, AssertOp::Eq, Some("{{expectedId}}")),
            a(AssertTarget::JsonPath { path: "$.data.tags".into() }, AssertOp::Contains, Some("b")),
            a(AssertTarget::JsonPath { path: "$.data.tags".into() }, AssertOp::IsType, Some("array")),
            a(AssertTarget::JsonPath { path: "$.data.empty".into() }, AssertOp::IsEmpty, None),
            a(AssertTarget::JsonPath { path: "$.ok".into() }, AssertOp::Eq, Some("true")),
            a(AssertTarget::JsonPath { path: "$.missing".into() }, AssertOp::NotExists, None),
            a(AssertTarget::Header { name: "X-Request-Id".into() }, AssertOp::StartsWith, Some("r-")),
            a(AssertTarget::ContentType, AssertOp::Contains, Some("json")),
            a(AssertTarget::ResponseTime, AssertOp::Lt, Some("500")),
            a(AssertTarget::Body, AssertOp::Matches, Some(r#""name":"A\w+""#)),
        ];
        let out = evaluate(&checks, &r, &mut res);
        for o in &out {
            assert!(o.passed, "{o:?}");
        }
        let fails = evaluate(
            &[
                a(AssertTarget::Status, AssertOp::Eq, Some("200")),
                a(AssertTarget::JsonPath { path: "$.nope".into() }, AssertOp::Eq, Some("1")),
                a(AssertTarget::JsonPath { path: "$[".into() }, AssertOp::Exists, None),
            ],
            &r,
            &mut res,
        );
        assert!(fails.iter().all(|f| !f.passed));
        assert_eq!(fails[0].actual.as_deref(), Some("201"));
        assert!(fails[2].message.as_deref().unwrap().contains("JSONPath"));
    }

    #[test]
    fn snapshot_assertions() {
        let dir = tempfile::tempdir().unwrap();
        let file = "http/__snapshots__/x.snap.json".to_string();
        let snap = Assertion { target: AssertTarget::Snapshot { file: file.clone(), ignore: vec!["$.at".into()] }, op: AssertOp::Eq, value: None, enabled: true };
        let mut res = Resolver::new(&NoSecrets);
        let r1 = resp(200, r#"{"a":1,"at":"t1"}"#);
        let no_project = evaluate(std::slice::from_ref(&snap), &r1, &mut res);
        assert!(!no_project[0].passed && no_project[0].message.as_deref().unwrap().contains("project"));
        let cx = CheckContext { project_root: Some(dir.path()), request_path: Some("x.req.yaml"), update_snapshots: false };
        let missing = evaluate_in(std::slice::from_ref(&snap), &r1, &mut res, &cx);
        assert!(!missing[0].passed && missing[0].message.as_deref().unwrap().contains("--update-snapshots"), "{missing:?}");
        let written = evaluate_in(std::slice::from_ref(&snap), &r1, &mut res, &CheckContext { update_snapshots: true, ..cx });
        assert!(written[0].passed && written[0].message.as_deref() == Some("snapshot written"), "{written:?}");
        assert!(evaluate_in(std::slice::from_ref(&snap), &resp(200, r#"{"at":"t2","a":1}"#), &mut res, &cx)[0].passed);
        let diff = evaluate_in(std::slice::from_ref(&snap), &resp(200, r#"{"a":2,"at":"t3"}"#), &mut res, &cx);
        assert!(!diff[0].passed);
        assert_eq!(diff[0].actual.as_deref(), Some("$.a: expected 1, got 2"));
        assert_eq!(diff[0].label, format!("snapshot {file}"));
    }

    #[test]
    fn captures() {
        let r = resp(200, r#"{"token":"t-1","items":[{"id":1},{"id":2}]}"#);
        let caps = vec![
            Capture { name: "token".into(), source: CaptureSource::JsonPath { path: "$.token".into() }, enabled: true },
            Capture { name: "ids".into(), source: CaptureSource::JsonPath { path: "$.items[*].id".into() }, enabled: true },
            Capture { name: "sid".into(), source: CaptureSource::Cookie { cookie: "sid".into() }, enabled: true },
            Capture { name: "rid".into(), source: CaptureSource::Header { header: "x-request-id".into() }, enabled: true },
            Capture { name: "num".into(), source: CaptureSource::Regex { pattern: r#""id":(\d+)"#.into(), group: 1 }, enabled: true },
            Capture { name: "nope".into(), source: CaptureSource::JsonPath { path: "$.x".into() }, enabled: true },
        ];
        let (vars, results) = capture(&caps, &r);
        assert_eq!(vars["token"], "t-1");
        assert_eq!(vars["ids"], "[1,2]");
        assert_eq!(vars["sid"], "abc123");
        assert_eq!(vars["rid"], "r-1");
        assert_eq!(vars["num"], "1");
        assert_eq!(results.last().unwrap().error.as_deref(), Some("not found"));
    }

    #[test]
    fn yaml_shape() {
        let yaml = "tests:\n- assert: status\n  op: eq\n  value: '200'\n- assert: jsonPath\n  path: $.id\n  op: exists\ncaptures:\n- name: token\n  from: jsonPath\n  path: $.token\nname: x\n";
        let req: devall_core::HttpRequestFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(req.tests.len(), 2);
        assert_eq!(req.captures[0].source, CaptureSource::JsonPath { path: "$.token".into() });
        let back = serde_yaml_ng::to_string(&req).unwrap();
        assert!(back.contains("assert: jsonPath"), "{back}");
        assert!(back.contains("from: jsonPath"), "{back}");
    }
}
