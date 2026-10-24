//! Loading a spec and the lookups every check needs: operations, local `$ref`s, server base
//! paths, matching a concrete request path to a path template, response definitions.

use std::path::Path;

use serde_json::{Map, Value};

use crate::error::{Error, Result};

pub const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace"];

/// OpenAPI major/minor line. Swagger 2.0 is refused at load time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SpecVersion {
    V30,
    V31,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub doc: Value,
    pub version: SpecVersion,
}

/// One operation (`paths./users/{id}.get`).
#[derive(Debug, Clone)]
pub struct Operation<'a> {
    /// Path template as written, e.g. `/users/{id}`.
    pub path: &'a str,
    /// Upper case, e.g. `GET`.
    pub method: String,
    pub op: &'a Value,
    pub path_item: &'a Value,
}

impl Operation<'_> {
    /// `GET /users/{id}`.
    pub fn key(&self) -> String {
        format!("{} {}", self.method, self.path)
    }

    pub fn operation_id(&self) -> Option<&str> {
        self.op.get("operationId").and_then(Value::as_str)
    }

    /// JSON-pointer-ish location for messages: `paths./users/{id}.get`.
    pub fn location(&self) -> String {
        format!("paths.{}.{}", self.path, self.method.to_ascii_lowercase())
    }
}

/// Result of looking up a concrete request against the spec.
#[derive(Debug, Clone)]
pub enum PathMatch<'a> {
    /// Path and method documented.
    Operation(Operation<'a>),
    /// The path exists but not with this method; the documented methods are listed.
    MethodNotDocumented { path: &'a str, methods: Vec<String> },
    /// No path template matches.
    NoPath,
}

/// YAML or JSON text → JSON value (YAML keys such as `200:` become strings).
pub fn parse_document(text: &str) -> Result<Value> {
    let t = text.trim_start_matches('\u{feff}');
    match serde_json::from_str::<Value>(t) {
        Ok(v) => Ok(v),
        Err(json_err) => match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(t) {
            Ok(y) => Ok(yaml_to_json(y)),
            Err(yaml_err) => Err(Error::Parse(if t.trim_start().starts_with('{') { format!("invalid JSON: {json_err}") } else { format!("invalid YAML: {yaml_err}") })),
        },
    }
}

fn yaml_to_json(y: serde_yaml_ng::Value) -> Value {
    use serde_yaml_ng::Value as Y;
    match y {
        Y::Null => Value::Null,
        Y::Bool(b) => Value::Bool(b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else {
                n.as_f64().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::Null)
            }
        }
        Y::String(s) => Value::String(s),
        Y::Sequence(seq) => Value::Array(seq.into_iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                let key = match k {
                    Y::String(s) => s,
                    Y::Number(n) => n.to_string(),
                    Y::Bool(b) => b.to_string(),
                    Y::Null => "null".into(),
                    other => serde_yaml_ng::to_string(&other).unwrap_or_default().trim().to_string(),
                };
                out.insert(key, yaml_to_json(v));
            }
            Value::Object(out)
        }
        Y::Tagged(t) => yaml_to_json(t.value),
    }
}

/// `true` when `text` looks like an OpenAPI 3.x document (cheap sniff for file pickers).
pub fn looks_like_openapi(text: &str) -> bool {
    let head: String = text.chars().take(4096).collect();
    head.contains("openapi") && (head.contains("\"openapi\"") || head.lines().any(|l| l.trim_start().starts_with("openapi:")))
}

impl Spec {
    pub fn from_value(doc: Value) -> Result<Self> {
        if doc.get("swagger").is_some() {
            return Err(Error::Unsupported("Swagger 2.0 documents are not supported; convert to OpenAPI 3 first (e.g. import it, then export OpenAPI)".into()));
        }
        let v = doc.get("openapi").and_then(Value::as_str).ok_or_else(|| Error::Parse("not an OpenAPI document (no `openapi` version field)".into()))?;
        let version = if v.starts_with("3.0") {
            SpecVersion::V30
        } else if v.starts_with("3.1") || v.starts_with("3.2") {
            SpecVersion::V31
        } else {
            return Err(Error::Unsupported(format!("OpenAPI {v} is not supported (3.0 and 3.1 are)")));
        };
        if !doc.is_object() {
            return Err(Error::Parse("the document is not an object".into()));
        }
        Ok(Self { doc, version })
    }

    pub fn parse(text: &str) -> Result<Self> {
        Self::from_value(parse_document(text)?)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        Self::parse(&text)
    }

    pub fn title(&self) -> String {
        self.doc.pointer("/info/title").and_then(Value::as_str).unwrap_or("API").to_string()
    }

    /// Every operation in document order.
    pub fn operations(&self) -> Vec<Operation<'_>> {
        let mut out = Vec::new();
        let Some(paths) = self.doc.get("paths").and_then(Value::as_object) else { return out };
        for (path, item) in paths {
            let item = self.resolve(item);
            let Some(obj) = item.as_object() else { continue };
            for m in METHODS {
                if let Some(op) = obj.get(*m).filter(|v| v.is_object()) {
                    out.push(Operation { path, method: m.to_ascii_uppercase(), op, path_item: item });
                }
            }
        }
        out
    }

    /// Follows a local `$ref` chain (`#/components/schemas/User`). Unresolvable or external
    /// references return the value itself.
    pub fn resolve<'a>(&'a self, v: &'a Value) -> &'a Value {
        let mut cur = v;
        for _ in 0..32 {
            match cur.get("$ref").and_then(Value::as_str) {
                Some(r) => match self.pointer(r) {
                    Some(t) => cur = t,
                    None => return cur,
                },
                None => return cur,
            }
        }
        cur
    }

    /// The target of a local reference, `None` when it is external or dangling.
    pub fn pointer(&self, r: &str) -> Option<&Value> {
        let frag = r.strip_prefix('#')?;
        if frag.is_empty() {
            return Some(&self.doc);
        }
        let decoded = percent_encoding::percent_decode_str(frag).decode_utf8_lossy();
        self.doc.pointer(&decoded)
    }

    /// Path prefixes from `servers` (`https://api.x.com/v1` → `/v1`), longest first; always
    /// ends with `""` (no prefix).
    pub fn base_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in self.servers() {
            let path = server_path(&s);
            let p = path.trim_end_matches('/').to_string();
            if !p.is_empty() && !out.contains(&p) {
                out.push(p);
            }
        }
        out.sort_by_key(|p| std::cmp::Reverse(p.len()));
        out.push(String::new());
        out
    }

    /// Server URLs with `{variables}` replaced by their defaults.
    pub fn servers(&self) -> Vec<String> {
        let Some(list) = self.doc.get("servers").and_then(Value::as_array) else { return Vec::new() };
        list.iter()
            .filter_map(|s| {
                let mut url = s.get("url")?.as_str()?.to_string();
                if let Some(vars) = s.get("variables").and_then(Value::as_object) {
                    for (k, v) in vars {
                        if let Some(d) = v.get("default").map(|d| d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string())) {
                            url = url.replace(&format!("{{{k}}}"), &d);
                        }
                    }
                }
                Some(url)
            })
            .collect()
    }

    /// Hosts (`host[:port]`) of absolute server URLs.
    pub fn server_hosts(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in self.servers() {
            if let Ok(u) = url::Url::parse(&s)
                && let Some(h) = u.host_str()
            {
                let h = match u.port() {
                    Some(p) => format!("{h}:{p}"),
                    None => h.to_string(),
                };
                if !out.contains(&h) {
                    out.push(h.to_ascii_lowercase());
                }
            }
        }
        out
    }

    /// Finds the operation for a concrete request path (no query string). Literal segments win
    /// over templated ones, as OpenAPI requires; server base paths are stripped first.
    pub fn match_path(&self, method: &str, path: &str) -> PathMatch<'_> {
        let path = path.split(['?', '#']).next().unwrap_or("");
        let Some(paths) = self.doc.get("paths").and_then(Value::as_object) else { return PathMatch::NoPath };
        let mut best: Option<(&str, usize)> = None;
        for base in self.base_paths() {
            let rest = if base.is_empty() {
                path
            } else {
                match path.strip_prefix(&base) {
                    Some(r) if r.is_empty() || r.starts_with('/') => r,
                    _ => continue,
                }
            };
            let rest = if rest.is_empty() { "/" } else { rest };
            for template in paths.keys() {
                if let Some(score) = template_score(template, rest)
                    && best.is_none_or(|(_, s)| score > s)
                {
                    best = Some((template.as_str(), score));
                }
            }
            if best.is_some() {
                break;
            }
        }
        let Some((template, _)) = best else { return PathMatch::NoPath };
        let item = self.resolve(&paths[template]);
        let m = method.to_ascii_lowercase();
        if let Some(op) = item.get(&m).filter(|v| v.is_object()) {
            return PathMatch::Operation(Operation { path: template, method: m.to_ascii_uppercase(), op, path_item: item });
        }
        let methods = METHODS.iter().filter(|x| item.get(**x).is_some()).map(|x| x.to_ascii_uppercase()).collect();
        PathMatch::MethodNotDocumented { path: template, methods }
    }

    /// The response object for `status` (exact code, then `2XX`, then `default`) and the key
    /// it was found under.
    pub fn response_for<'a>(&'a self, op: &'a Value, status: u16) -> Option<(&'a str, &'a Value)> {
        let responses = op.get("responses")?.as_object()?;
        let exact = status.to_string();
        let range = format!("{}XX", status / 100);
        for key in [exact.as_str(), range.as_str(), "default"] {
            if let Some((k, v)) = responses.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
                return Some((k.as_str(), self.resolve(v)));
            }
        }
        None
    }

    /// The schema for a media type in a `content` map: exact type, then `type/*`, `*/*`, then
    /// any JSON type when `content_type` is JSON. `None` when no content entry fits.
    pub fn media_schema<'a>(&'a self, content: &'a Value, content_type: Option<&str>) -> Option<(&'a str, Option<&'a Value>)> {
        let map = content.as_object()?;
        if map.is_empty() {
            return None;
        }
        let ct = content_type.map(|c| c.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
        let pick = |key: &str| map.iter().find(|(k, _)| k.eq_ignore_ascii_case(key));
        let found = match ct.as_deref() {
            Some(c) if !c.is_empty() => pick(c)
                .or_else(|| c.split('/').next().and_then(|major| pick(&format!("{major}/*"))))
                .or_else(|| pick("*/*"))
                .or_else(|| is_json(c).then(|| map.iter().find(|(k, _)| is_json(k))).flatten()),
            _ => map.iter().find(|(k, _)| is_json(k)).or_else(|| map.iter().next()),
        }?;
        let media = self.resolve(found.1);
        Some((found.0.as_str(), media.get("schema")))
    }
}

pub fn is_json(content_type: &str) -> bool {
    let c = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    c == "application/json" || c.ends_with("+json") || c == "text/json" || c.ends_with("/json")
}

/// Path part of a server URL (absolute or relative).
fn server_path(url: &str) -> String {
    if let Ok(u) = url::Url::parse(url) {
        return u.path().to_string();
    }
    // Relative (`/v1`) or scheme-less (`api.example.com/v1`).
    if url.starts_with('/') {
        return url.to_string();
    }
    match url.find('/') {
        Some(i) if !url[..i].contains(':') || url[..i].contains('.') => url[i..].to_string(),
        _ => String::new(),
    }
}

/// Matches a concrete path against a template; the score counts literal segments (higher is
/// more specific). Trailing slashes are ignored.
pub fn template_score(template: &str, path: &str) -> Option<usize> {
    let t: Vec<&str> = template.trim_end_matches('/').split('/').collect();
    let p: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    if t.len() != p.len() {
        return None;
    }
    let mut score = 0;
    for (ts, ps) in t.iter().zip(&p) {
        if ts.contains('{') {
            if !segment_matches(ts, ps) {
                return None;
            }
        } else if ts == ps || percent_encoding::percent_decode_str(ps).decode_utf8_lossy() == *ts {
            score += 1;
        } else {
            return None;
        }
    }
    Some(score)
}

/// `{id}` matches any non-empty segment; `{id}.json` / `v{version}` need the literal parts.
fn segment_matches(template: &str, seg: &str) -> bool {
    if seg.is_empty() {
        return false;
    }
    let mut rest = seg;
    let mut t = template;
    // Literal prefix.
    let Some(open) = t.find('{') else { return t == seg };
    let prefix = &t[..open];
    if !rest.starts_with(prefix) {
        return false;
    }
    rest = &rest[prefix.len()..];
    t = &t[open..];
    let close = t.find('}').unwrap_or(t.len() - 1);
    let suffix = &t[close + 1..];
    if suffix.contains('{') {
        return true; // several variables in one segment: accept
    }
    rest.len() > suffix.len() && rest.ends_with(suffix)
}

/// Parameter names in a path template (`/a/{x}/b/{y}` → `x`, `y`).
pub fn template_params(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else { break };
        out.push(rest[i + 1..i + j].to_string());
        rest = &rest[i + j + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec() -> Spec {
        Spec::parse(
            r#"
openapi: 3.0.3
info: { title: Shop, version: "1" }
servers:
  - url: https://api.example.com/v1
  - url: "{scheme}://localhost:{port}"
    variables: { scheme: { default: http }, port: { default: "8080" } }
paths:
  /users:
    get: { responses: { 200: { description: ok } } }
  /users/{id}:
    get:
      responses:
        200:
          description: ok
          content: { application/json: { schema: { $ref: '#/components/schemas/User' } } }
        4XX: { description: client error }
        default: { description: error }
    delete: { responses: { 204: { description: gone } } }
  /users/me:
    get: { responses: { 200: { description: me } } }
  /files/{name}.json:
    get: { responses: { 200: { description: file } } }
components:
  schemas:
    User: { type: object }
"#,
        )
        .unwrap()
    }

    #[test]
    fn matching_and_lookups() {
        let s = spec();
        assert_eq!(s.version, SpecVersion::V30);
        assert_eq!(s.base_paths(), vec!["/v1".to_string(), String::new()]);
        assert_eq!(s.server_hosts(), vec!["api.example.com".to_string(), "localhost:8080".into()]);
        let key = |m: &str, p: &str| match s.match_path(m, p) {
            PathMatch::Operation(o) => o.key(),
            PathMatch::MethodNotDocumented { path, methods } => format!("405 {path} {}", methods.join(",")),
            PathMatch::NoPath => "none".into(),
        };
        assert_eq!(key("GET", "/v1/users/42?x=1"), "GET /users/{id}");
        assert_eq!(key("GET", "/users/me"), "GET /users/me", "literal wins");
        assert_eq!(key("get", "/users/"), "GET /users");
        assert_eq!(key("POST", "/users/1"), "405 /users/{id} GET,DELETE");
        assert_eq!(key("GET", "/orders"), "none");
        assert_eq!(key("GET", "/files/report.json"), "GET /files/{name}.json");
        assert_eq!(key("GET", "/files/report.xml"), "none");
        let PathMatch::Operation(op) = s.match_path("GET", "/users/1") else { panic!() };
        assert_eq!(s.response_for(op.op, 200).unwrap().0, "200");
        assert_eq!(s.response_for(op.op, 404).unwrap().0, "4XX");
        assert_eq!(s.response_for(op.op, 500).unwrap().0, "default");
        let resp = s.response_for(op.op, 200).unwrap().1;
        let (mt, schema) = s.media_schema(&resp["content"], Some("application/json; charset=utf-8")).unwrap();
        assert_eq!(mt, "application/json");
        assert_eq!(s.resolve(schema.unwrap()), &json!({ "type": "object" }));
        assert_eq!(template_params("/a/{x}/b/{y}.json"), vec!["x".to_string(), "y".into()]);
        assert!(Spec::parse("swagger: '2.0'").is_err());
        assert!(looks_like_openapi("openapi: 3.1.0\ninfo: {}"));
    }
}
