//! A built-in OpenAPI linter: no network, no plugins, a fixed rule set with severities.
//!
//! | Rule | Severity | Checks |
//! |---|---|---|
//! | `openapi-version` | error | `openapi` is 3.0.x / 3.1.x |
//! | `info-title` / `info-version` | error / warning | `info.title`, `info.version` |
//! | `servers-defined` | warning | at least one server |
//! | `paths-defined` | warning | at least one path |
//! | `path-leading-slash` | error | path keys start with `/` |
//! | `path-trailing-slash` | warning | no trailing `/` |
//! | `path-casing` | info | literal segments are kebab-case (or consistently cased) |
//! | `path-params` | error | every `{param}` is declared `in: path`, required, and vice versa |
//! | `path-duplicate` | warning | `/a/{x}` and `/a/{y}` are the same path |
//! | `operation-id` / `operation-id-unique` | warning / error | operationIds exist and are unique |
//! | `operation-summary` | warning | every operation has a summary |
//! | `operation-tags` | info | every operation has a tag (declared ones, if `tags` exists) |
//! | `operation-responses` | error | `responses` exists and is not empty |
//! | `operation-success-response` | warning | a 2xx (or 3xx / 101) response exists |
//! | `response-description` | error | responses have a description (required by the spec) |
//! | `response-schema` / `request-body-schema` | warning | JSON content declares a schema |
//! | `request-body-on-get` | warning | GET / HEAD / DELETE without a request body |
//! | `parameter-duplicate` | error | a parameter (name + in) is declared once per operation |
//! | `ref-resolves` | error | local `$ref`s point somewhere |
//! | `component-unused` | warning | every component is referenced |
//! | `security-defined` | error | security requirements name declared schemes |
//! | `example-valid` | warning | `example` / `examples` match their schema |

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::schema::{Direction, validate};
use crate::spec::{METHODS, Spec, template_params};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    pub message: String,
    /// Where: `paths./users/{id}.get.responses.200`, `components.schemas.User`, …
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LintReport {
    pub title: String,
    pub version: String,
    pub operations: usize,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    /// Sorted by severity, then location.
    pub findings: Vec<Finding>,
}

impl LintReport {
    pub fn passed(&self) -> bool {
        self.errors == 0
    }
}

/// Lints raw text (YAML or JSON). Unparseable or non-3.x documents produce a single error finding.
pub fn lint_text(text: &str) -> LintReport {
    match Spec::parse(text) {
        Ok(spec) => lint(&spec),
        Err(e) => {
            let version = crate::spec::parse_document(text).ok().and_then(|d| d.get("openapi").or_else(|| d.get("swagger")).and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
            finish("API".into(), version, 0, vec![Finding { rule: "openapi-version".into(), severity: Severity::Error, message: e.to_string(), location: String::new() }])
        }
    }
}

fn finish(title: String, version: String, operations: usize, mut findings: Vec<Finding>) -> LintReport {
    findings.sort_by(|a, b| a.severity.cmp(&b.severity).then_with(|| a.location.cmp(&b.location)).then_with(|| a.rule.cmp(&b.rule)));
    let count = |s: Severity| findings.iter().filter(|f| f.severity == s).count();
    LintReport { title, version, operations, errors: count(Severity::Error), warnings: count(Severity::Warning), infos: count(Severity::Info), findings }
}

struct Ctx<'a> {
    spec: &'a Spec,
    out: Vec<Finding>,
}

impl Ctx<'_> {
    fn add(&mut self, rule: &str, severity: Severity, location: impl Into<String>, message: impl Into<String>) {
        self.out.push(Finding { rule: rule.into(), severity, message: message.into(), location: location.into() });
    }
}

pub fn lint(spec: &Spec) -> LintReport {
    let mut cx = Ctx { spec, out: Vec::new() };
    let doc = &spec.doc;

    // ----- info / servers / paths -----
    let info = doc.get("info");
    if info.and_then(|i| i.get("title")).and_then(Value::as_str).is_none_or(|t| t.trim().is_empty()) {
        cx.add("info-title", Severity::Error, "info.title", "The API has no title (info.title)");
    }
    if info.and_then(|i| i.get("version")).is_none_or(|v| v.as_str().is_some_and(|s| s.trim().is_empty())) {
        cx.add("info-version", Severity::Warning, "info.version", "The API has no version (info.version)");
    }
    match doc.get("servers").and_then(Value::as_array) {
        Some(list) if !list.is_empty() => {
            for (i, s) in list.iter().enumerate() {
                if s.get("url").and_then(Value::as_str).is_none_or(|u| u.trim().is_empty()) {
                    cx.add("servers-defined", Severity::Warning, format!("servers[{i}]"), "Server has no url");
                }
            }
        }
        _ => cx.add("servers-defined", Severity::Warning, "servers", "No servers are defined; clients and mocks can't know the base URL"),
    }
    let paths = doc.get("paths").and_then(Value::as_object);
    if paths.is_none_or(|p| p.is_empty()) && doc.get("webhooks").is_none() {
        cx.add("paths-defined", Severity::Warning, "paths", "The document has no paths");
    }

    // ----- paths -----
    let mut normalized: HashMap<String, String> = HashMap::new();
    let mut casing: BTreeMap<&'static str, usize> = BTreeMap::new();
    if let Some(paths) = paths {
        for (path, item) in paths {
            let loc = format!("paths.{path}");
            if !path.starts_with('/') {
                cx.add("path-leading-slash", Severity::Error, &loc, format!("Path `{path}` must start with /"));
            }
            if path.len() > 1 && path.ends_with('/') {
                cx.add("path-trailing-slash", Severity::Warning, &loc, format!("Path `{path}` ends with /"));
            }
            let norm: String = path.split('/').map(|s| if s.contains('{') { "{}" } else { s }).collect::<Vec<_>>().join("/");
            if let Some(other) = normalized.insert(norm, path.clone()) {
                cx.add("path-duplicate", Severity::Warning, &loc, format!("`{path}` and `{other}` are the same path with different parameter names"));
            }
            for seg in path.split('/').filter(|s| !s.is_empty() && !s.contains('{')) {
                *casing.entry(segment_case(seg)).or_default() += 1;
            }
            let item = spec.resolve(item);
            lint_path_item(&mut cx, path, item);
        }
    }
    let styles: Vec<&str> = casing.keys().copied().filter(|k| *k != "plain").collect();
    if styles.len() > 1 {
        cx.add("path-casing", Severity::Info, "paths", format!("Path segments mix naming styles ({}); pick one, kebab-case is the most common", styles.join(", ")));
    } else if styles.first().is_some_and(|s| *s != "kebab-case") {
        cx.add("path-casing", Severity::Info, "paths", format!("Path segments use {}; kebab-case is the most common convention for URLs", styles[0]));
    }

    // ----- operation ids -----
    let mut ids: HashMap<String, Vec<String>> = HashMap::new();
    let ops = spec.operations();
    for op in &ops {
        match op.operation_id() {
            Some(id) if !id.trim().is_empty() => ids.entry(id.to_string()).or_default().push(op.location()),
            _ => cx.add("operation-id", Severity::Warning, op.location(), format!("{} has no operationId (code generators and SDKs name methods after it)", op.key())),
        }
    }
    for (id, locs) in &ids {
        if locs.len() > 1 {
            for l in locs {
                cx.add("operation-id-unique", Severity::Error, l, format!("operationId `{id}` is used by {} operations", locs.len()));
            }
        }
    }

    // ----- security -----
    let schemes: HashSet<String> = doc.pointer("/components/securitySchemes").and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default();
    let check_security = |cx: &mut Ctx, sec: &Value, loc: &str| {
        for req in sec.as_array().into_iter().flatten() {
            for name in req.as_object().into_iter().flat_map(|m| m.keys()) {
                if !schemes.contains(name) {
                    cx.add("security-defined", Severity::Error, loc, format!("Security scheme `{name}` is used but not defined in components.securitySchemes"));
                }
            }
        }
    };
    if let Some(sec) = doc.get("security") {
        check_security(&mut cx, sec, "security");
    }
    for op in &ops {
        if let Some(sec) = op.op.get("security") {
            check_security(&mut cx, sec, &format!("{}.security", op.location()));
        }
    }

    // ----- refs and components -----
    let mut refs: Vec<(String, String)> = Vec::new();
    collect_refs(doc, "", &mut refs);
    let mut used: HashSet<String> = HashSet::new();
    for (r, loc) in &refs {
        if let Some(local) = r.strip_prefix('#') {
            if spec.pointer(r).is_none() {
                cx.add("ref-resolves", Severity::Error, loc, format!("$ref `{r}` does not resolve"));
            }
            used.insert(percent_encoding::percent_decode_str(local).decode_utf8_lossy().to_string());
        }
    }
    // Components only reachable from unused components still count as used (simple model).
    if let Some(components) = doc.get("components").and_then(Value::as_object) {
        for (kind, items) in components {
            if kind == "securitySchemes" {
                for name in items.as_object().into_iter().flat_map(|m| m.keys()) {
                    let referenced = doc.get("security").is_some_and(|s| s.to_string().contains(&format!("\"{name}\""))) || ops.iter().any(|o| o.op.get("security").is_some_and(|s| s.to_string().contains(&format!("\"{name}\""))));
                    if !referenced {
                        cx.add("component-unused", Severity::Warning, format!("components.securitySchemes.{name}"), format!("Security scheme `{name}` is never required"));
                    }
                }
                continue;
            }
            if !["schemas", "responses", "parameters", "examples", "requestBodies", "headers", "links", "callbacks", "pathItems"].contains(&kind.as_str()) {
                continue;
            }
            for name in items.as_object().into_iter().flat_map(|m| m.keys()) {
                let ptr = format!("/components/{kind}/{}", name.replace('~', "~0").replace('/', "~1"));
                if !used.contains(&ptr) {
                    cx.add("component-unused", Severity::Warning, format!("components.{kind}.{name}"), format!("`{name}` in components.{kind} is never referenced"));
                }
            }
        }
    }

    // ----- examples on component schemas -----
    if let Some(schemas) = doc.pointer("/components/schemas").and_then(Value::as_object) {
        for (name, schema) in schemas {
            check_schema_example(&mut cx, schema, &format!("components.schemas.{name}"), Direction::Response);
        }
    }

    let version = doc.get("openapi").and_then(Value::as_str).unwrap_or_default().to_string();
    finish(spec.title(), version, ops.len(), cx.out)
}

fn segment_case(seg: &str) -> &'static str {
    let has_upper = seg.chars().any(|c| c.is_ascii_uppercase());
    if seg.contains('_') {
        "snake_case"
    } else if has_upper {
        "camelCase"
    } else if seg.contains('-') {
        "kebab-case"
    } else {
        "plain"
    }
}

fn collect_refs(v: &Value, loc: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(m) => {
            if let Some(r) = m.get("$ref").and_then(Value::as_str) {
                out.push((r.to_string(), loc.to_string()));
            }
            for (k, child) in m {
                // Examples hold arbitrary data; a `$ref` key in there isn't a reference.
                if k == "example" || (k == "examples" && child.is_array()) {
                    continue;
                }
                collect_refs(child, &if loc.is_empty() { k.clone() } else { format!("{loc}.{k}") }, out);
            }
        }
        Value::Array(a) => {
            for (i, child) in a.iter().enumerate() {
                collect_refs(child, &format!("{loc}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn param_key(spec: &Spec, p: &Value) -> Option<(String, String)> {
    let p = spec.resolve(p);
    Some((p.get("name")?.as_str()?.to_string(), p.get("in")?.as_str()?.to_string()))
}

fn lint_path_item(cx: &mut Ctx, path: &str, item: &Value) {
    let spec = cx.spec;
    let declared_in_template = template_params(path);
    let path_level: Vec<&Value> = item.get("parameters").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    for m in METHODS {
        let Some(op) = item.get(*m).filter(|v| v.is_object()) else { continue };
        let method = m.to_ascii_uppercase();
        let loc = format!("paths.{path}.{m}");
        let label = format!("{method} {path}");

        // Parameters: path-level, overridden by operation-level (name + in).
        let op_params: Vec<&Value> = op.get("parameters").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for p in &op_params {
            if let Some(k) = param_key(spec, p)
                && !seen.insert(k.clone())
            {
                cx.add("parameter-duplicate", Severity::Error, format!("{loc}.parameters"), format!("{label}: parameter `{}` in {} is declared twice", k.0, k.1));
            }
        }
        let mut path_params: HashMap<String, &Value> = HashMap::new();
        for p in path_level.iter().chain(op_params.iter()) {
            let resolved = spec.resolve(p);
            if resolved.get("in").and_then(Value::as_str) == Some("path")
                && let Some(n) = resolved.get("name").and_then(Value::as_str)
            {
                path_params.insert(n.to_string(), resolved);
            }
            check_param_example(cx, resolved, &format!("{loc}.parameters"));
        }
        for t in &declared_in_template {
            if !path_params.contains_key(t) {
                cx.add("path-params", Severity::Error, &loc, format!("{label}: path parameter `{{{t}}}` is not declared (in: path)"));
            }
        }
        for (n, p) in &path_params {
            if !declared_in_template.contains(n) {
                cx.add("path-params", Severity::Error, &loc, format!("{label}: parameter `{n}` is declared in: path but the path has no `{{{n}}}`"));
            } else if p.get("required").and_then(Value::as_bool) != Some(true) {
                cx.add("path-params", Severity::Error, &loc, format!("{label}: path parameter `{n}` must be `required: true`"));
            }
        }

        if op.get("summary").and_then(Value::as_str).is_none_or(|s| s.trim().is_empty()) {
            cx.add("operation-summary", Severity::Warning, &loc, format!("{label} has no summary"));
        }
        match op.get("tags").and_then(Value::as_array) {
            Some(tags) if !tags.is_empty() => {
                if let Some(declared) = spec.doc.get("tags").and_then(Value::as_array) {
                    let names: HashSet<&str> = declared.iter().filter_map(|t| t.get("name").and_then(Value::as_str)).collect();
                    for t in tags.iter().filter_map(Value::as_str) {
                        if !names.contains(t) {
                            cx.add("operation-tags", Severity::Info, &loc, format!("{label}: tag `{t}` is not declared in the top-level tags"));
                        }
                    }
                }
            }
            _ => cx.add("operation-tags", Severity::Info, &loc, format!("{label} has no tags")),
        }

        // Request body.
        if let Some(body) = op.get("requestBody") {
            let body = spec.resolve(body);
            if ["GET", "HEAD", "DELETE"].contains(&method.as_str()) {
                cx.add("request-body-on-get", Severity::Warning, format!("{loc}.requestBody"), format!("{label} has a request body; many clients and proxies drop bodies on {method}"));
            }
            check_content(cx, body.get("content"), &format!("{loc}.requestBody"), &label, "request-body-schema", Direction::Request);
        }

        // Responses.
        match op.get("responses").and_then(Value::as_object) {
            None => cx.add("operation-responses", Severity::Error, &loc, format!("{label} has no responses")),
            Some(r) if r.is_empty() => cx.add("operation-responses", Severity::Error, &loc, format!("{label} has no responses")),
            Some(responses) => {
                let success = responses.keys().any(|k| k.starts_with('2') || k.starts_with('3') || k == "101");
                if !success {
                    cx.add("operation-success-response", Severity::Warning, format!("{loc}.responses"), format!("{label} documents no 2xx response"));
                }
                for (code, resp) in responses {
                    let rloc = format!("{loc}.responses.{code}");
                    let resp = spec.resolve(resp);
                    if resp.get("description").and_then(Value::as_str).is_none() {
                        cx.add("response-description", Severity::Error, &rloc, format!("{label} → {code}: description is required"));
                    }
                    if code.starts_with('2') && code != "204" {
                        check_content(cx, resp.get("content"), &rloc, &format!("{label} → {code}"), "response-schema", Direction::Response);
                    }
                }
            }
        }
    }
}

fn check_content(cx: &mut Ctx, content: Option<&Value>, loc: &str, label: &str, rule: &str, dir: Direction) {
    let spec = cx.spec;
    let Some(map) = content.and_then(Value::as_object) else { return };
    for (mt, media) in map {
        let media = spec.resolve(media);
        let mloc = format!("{loc}.content.{mt}");
        match media.get("schema") {
            None if crate::spec::is_json(mt) => cx.add(rule, Severity::Warning, &mloc, format!("{label}: {mt} has no schema")),
            None => {}
            Some(schema) => {
                if let Some(ex) = media.get("example") {
                    report_example(cx, schema, ex, &format!("{mloc}.example"), label, dir);
                }
                if let Some(exs) = media.get("examples").and_then(Value::as_object) {
                    for (name, ex) in exs {
                        let ex = spec.resolve(ex);
                        if let Some(v) = ex.get("value") {
                            report_example(cx, schema, v, &format!("{mloc}.examples.{name}"), label, dir);
                        }
                    }
                }
            }
        }
    }
}

fn report_example(cx: &mut Ctx, schema: &Value, example: &Value, loc: &str, label: &str, dir: Direction) {
    let errs = validate(cx.spec, schema, example, dir);
    if let Some(first) = errs.first() {
        let more = if errs.len() > 1 { format!(" (+{} more)", errs.len() - 1) } else { String::new() };
        cx.add("example-valid", Severity::Warning, loc, format!("{label}: example doesn't match its schema: {first}{more}"));
    }
}

fn check_param_example(cx: &mut Ctx, p: &Value, loc: &str) {
    if let (Some(schema), Some(ex)) = (p.get("schema"), p.get("example")) {
        let name = p.get("name").and_then(Value::as_str).unwrap_or("?");
        report_example(cx, schema, ex, &format!("{loc}.{name}.example"), &format!("parameter `{name}`"), Direction::Request);
    }
}

fn check_schema_example(cx: &mut Ctx, schema: &Value, loc: &str, dir: Direction) {
    if let Some(ex) = schema.get("example") {
        // Validate against the schema minus its own example (identical, but keeps refs local).
        report_example(cx, schema, ex, &format!("{loc}.example"), loc, dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r##"
openapi: 3.1.0
info: { title: Shop, version: "1.0" }
servers: [{ url: https://api.example.com }]
tags: [{ name: users }]
security: [{ bearer: [] }]
paths:
  /users/{id}:
    parameters:
      - { name: id, in: path, required: true, schema: { type: integer }, example: 7 }
    get:
      operationId: getUser
      summary: Get a user
      tags: [users]
      responses:
        "200":
          description: The user
          content:
            application/json:
              schema: { $ref: '#/components/schemas/User' }
              example: { id: 7, name: Ann }
        "404": { $ref: '#/components/responses/NotFound' }
components:
  securitySchemes:
    bearer: { type: http, scheme: bearer }
  responses:
    NotFound: { description: Not found }
  schemas:
    User:
      type: object
      required: [id, name]
      properties: { id: { type: integer }, name: { type: string } }
      example: { id: 1, name: Bo }
"##;

    #[test]
    fn clean_spec_has_no_findings() {
        let r = lint_text(GOOD);
        assert_eq!(r.findings, vec![], "{:#?}", r.findings);
        assert_eq!((r.operations, r.title.as_str(), r.version.as_str()), (1, "Shop", "3.1.0"));
        assert!(r.passed());
    }

    #[test]
    fn finds_the_usual_problems() {
        let bad = r##"
openapi: 3.0.0
info: { title: "" }
paths:
  users/:
    get:
      operationId: list
      requestBody: { content: { application/json: { schema: { type: object } } } }
      responses:
        "500": { description: boom }
  /Users/{userId}/order_items:
    get:
      operationId: list
      security: [{ apiKey: [] }]
      parameters:
        - { name: q, in: query }
        - { name: q, in: query }
      responses:
        "200":
          content:
            application/json:
              example: { a: 1 }
  /things/{id}:
    post:
      parameters: [{ name: id, in: path, schema: { type: string } }]
      responses:
        "201":
          description: created
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Missing' }
              examples: { one: { value: { n: "x" } } }
  /things/{other}:
    get:
      parameters: [{ name: other, in: path, required: true }]
      summary: x
      responses: {}
components:
  schemas:
    Unused: { type: string, example: 5 }
"##;
        let r = lint_text(bad);
        let rules: Vec<&str> = r.findings.iter().map(|f| f.rule.as_str()).collect();
        for rule in [
            "info-title",
            "info-version",
            "servers-defined",
            "path-leading-slash",
            "path-trailing-slash",
            "path-casing",
            "path-params",
            "path-duplicate",
            "operation-id",
            "operation-id-unique",
            "operation-summary",
            "operation-tags",
            "operation-responses",
            "operation-success-response",
            "response-description",
            "response-schema",
            "request-body-on-get",
            "parameter-duplicate",
            "ref-resolves",
            "component-unused",
            "security-defined",
            "example-valid",
        ] {
            assert!(rules.contains(&rule), "{rule} not reported: {:#?}", r.findings);
        }
        assert!(r.errors > 0 && !r.passed());
        assert_eq!(r.findings[0].severity, Severity::Error, "errors first");
        let swagger = lint_text("swagger: '2.0'\ninfo: { title: x }");
        assert_eq!((swagger.errors, swagger.version.as_str()), (1, "2.0"));
        assert_eq!(lint_text("{nope").errors, 1);
    }
}
