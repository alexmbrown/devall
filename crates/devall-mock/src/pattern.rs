//! Request patterns: everything under a stub's `request` key, plus WireMock's weighted distance
//! for near misses and a per-field breakdown for reports.

use serde::Serialize;
use serde_json::Value;

use crate::matchers::{Eval, MultiPattern, ValuePattern};
use crate::request::MockRequest;
use crate::util::{java_regex, json_text, normalized_distance, regex_matches};

#[derive(Debug, Clone)]
pub enum MethodPattern {
    Any,
    One(String),
    OneOf(Vec<String>),
    NoneOf(Vec<String>),
}

impl MethodPattern {
    fn from_json(v: Option<&Value>) -> Result<Self, String> {
        Ok(match v {
            None | Some(Value::Null) => Self::Any,
            Some(Value::String(s)) if s.eq_ignore_ascii_case("ANY") => Self::Any,
            Some(Value::String(s)) => Self::One(s.to_ascii_uppercase()),
            Some(Value::Object(o)) => {
                let list = |k: &str| -> Option<Vec<String>> { o.get(k)?.as_array().map(|a| a.iter().map(|x| json_text(x).to_ascii_uppercase()).collect()) };
                if let Some(l) = list("oneOf") {
                    Self::OneOf(l)
                } else if let Some(l) = list("noneOf") {
                    Self::NoneOf(l)
                } else {
                    return Err("method must be a string or {oneOf|noneOf: [...]}".into());
                }
            }
            Some(other) => return Err(format!("invalid method {other}")),
        })
    }

    fn accepts(&self, method: &str) -> bool {
        match self {
            Self::Any => true,
            Self::One(m) => m.eq_ignore_ascii_case(method),
            Self::OneOf(l) => l.iter().any(|m| m.eq_ignore_ascii_case(method)),
            Self::NoneOf(l) => !l.iter().any(|m| m.eq_ignore_ascii_case(method)),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Any => "ANY".into(),
            Self::One(m) => m.clone(),
            Self::OneOf(l) => format!("one of {}", l.join(", ")),
            Self::NoneOf(l) => format!("none of {}", l.join(", ")),
        }
    }
}

/// `urlPathTemplate`: `/users/{id}/posts/{postId}` (`{name*}` captures the rest of the path).
#[derive(Debug, Clone)]
pub struct PathTemplate {
    raw: String,
    re: regex::Regex,
    names: Vec<String>,
}

impl PathTemplate {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let mut re = String::from("^");
        let mut names = Vec::new();
        let mut rest = raw;
        while let Some(start) = rest.find('{') {
            re.push_str(&regex::escape(&rest[..start]));
            let end = rest[start..].find('}').ok_or_else(|| format!("unclosed '{{' in path template {raw:?}"))? + start;
            let mut name = rest[start + 1..end].trim().to_string();
            let wildcard = name.ends_with('*');
            name = name.trim_start_matches(['+', '.', ';', '/', '?', '&']).trim_end_matches('*').to_string();
            if name.is_empty() {
                return Err(format!("empty variable in path template {raw:?}"));
            }
            re.push_str(if wildcard { "(.+)" } else { "([^/]+)" });
            names.push(name);
            rest = &rest[end + 1..];
        }
        re.push_str(&regex::escape(rest));
        re.push('$');
        Ok(Self { raw: raw.to_string(), re: regex::Regex::new(&re).map_err(|e| e.to_string())?, names })
    }

    /// Variables (percent-decoded) when the path fits the template.
    pub fn captures(&self, path: &str) -> Option<Vec<(String, String)>> {
        let caps = self.re.captures(path)?;
        Some(
            self.names
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let raw = caps.get(i + 1).map_or("", |m| m.as_str());
                    (n.clone(), percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned())
                })
                .collect(),
        )
    }
}

#[derive(Debug, Clone)]
pub enum UrlMatcher {
    Url(String),
    UrlPath(String),
    UrlPattern(Box<fancy_regex::Regex>, String),
    UrlPathPattern(Box<fancy_regex::Regex>, String),
    Template(PathTemplate),
}

impl UrlMatcher {
    fn label(&self) -> &'static str {
        match self {
            Self::Url(_) => "URL",
            Self::UrlPath(_) => "URL path",
            Self::UrlPattern(..) => "URL pattern",
            Self::UrlPathPattern(..) => "URL path pattern",
            Self::Template(_) => "URL path template",
        }
    }

    fn expected(&self) -> &str {
        match self {
            Self::Url(s) | Self::UrlPath(s) | Self::UrlPattern(_, s) | Self::UrlPathPattern(_, s) => s,
            Self::Template(t) => &t.raw,
        }
    }

    fn actual<'a>(&self, req: &'a MockRequest) -> &'a str {
        match self {
            Self::Url(_) | Self::UrlPattern(..) => &req.url,
            _ => &req.path,
        }
    }

    fn eval(&self, req: &MockRequest, dist: bool) -> Eval {
        match self {
            Self::Url(e) | Self::UrlPath(e) => {
                let a = self.actual(req);
                let matched = e == a;
                Eval { matched, distance: if matched || !dist { f64::from(u8::from(!matched)) } else { normalized_distance(e, a) } }
            }
            Self::UrlPattern(re, _) => Eval::exact(regex_matches(re, &req.url)),
            Self::UrlPathPattern(re, _) => Eval::exact(regex_matches(re, &req.path)),
            Self::Template(t) => Eval::exact(t.captures(&req.path).is_some()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MultipartPattern {
    name: Option<String>,
    all: bool,
    headers: Vec<(String, MultiPattern)>,
    body: Vec<ValuePattern>,
}

/// `customMatcher`: the WireMock GraphQL extension's matcher is built in; others never match.
#[derive(Debug, Clone)]
pub enum CustomMatcher {
    Graphql(Box<crate::graphql::GqlPattern>),
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct RequestPattern {
    pub method: MethodPattern,
    pub url: Option<UrlMatcher>,
    pub path_params: Vec<(String, ValuePattern)>,
    pub query: Vec<(String, MultiPattern)>,
    pub headers: Vec<(String, MultiPattern)>,
    pub cookies: Vec<(String, MultiPattern)>,
    pub form: Vec<(String, MultiPattern)>,
    pub basic_auth: Option<(String, String)>,
    pub body: Vec<ValuePattern>,
    pub multipart: Vec<MultipartPattern>,
    pub host: Option<ValuePattern>,
    pub port: Option<u16>,
    pub scheme: Option<String>,
    pub client_ip: Option<ValuePattern>,
    pub custom_matcher: Option<CustomMatcher>,
    /// /dev/all extension: GraphQL operation / field / variables matchers.
    pub graphql: Option<Box<crate::graphql::GqlPattern>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldResult {
    pub field: String,
    pub expected: String,
    pub actual: Option<String>,
    pub matched: bool,
    pub distance: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MatchResult {
    pub matched: bool,
    pub distance: f64,
    pub fields: Vec<FieldResult>,
}

fn named_patterns(v: Option<&Value>, what: &str) -> Result<Vec<(String, MultiPattern)>, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let obj = v.as_object().ok_or_else(|| format!("{what} must be an object"))?;
    obj.iter().map(|(k, p)| MultiPattern::from_json(p).map(|m| (k.clone(), m)).map_err(|e| format!("{what}.{k}: {e}"))).collect()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

impl RequestPattern {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let obj = v.as_object().ok_or("request must be an object")?;
        let url = if let Some(u) = obj.get("url").filter(|v| !v.is_null()) {
            Some(UrlMatcher::Url(json_text(u)))
        } else if let Some(u) = obj.get("urlPath").filter(|v| !v.is_null()) {
            Some(UrlMatcher::UrlPath(json_text(u)))
        } else if let Some(u) = obj.get("urlPattern").filter(|v| !v.is_null()) {
            let s = json_text(u);
            Some(UrlMatcher::UrlPattern(Box::new(java_regex(&s)?), s))
        } else if let Some(u) = obj.get("urlPathPattern").filter(|v| !v.is_null()) {
            let s = json_text(u);
            Some(UrlMatcher::UrlPathPattern(Box::new(java_regex(&s)?), s))
        } else if let Some(u) = obj.get("urlPathTemplate").filter(|v| !v.is_null()) {
            Some(UrlMatcher::Template(PathTemplate::parse(&json_text(u))?))
        } else {
            None
        };
        let path_params = match obj.get("pathParameters").and_then(Value::as_object) {
            Some(m) => m.iter().map(|(k, p)| ValuePattern::from_json(p).map(|vp| (k.clone(), vp)).map_err(|e| format!("pathParameters.{k}: {e}"))).collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let basic_auth = obj
            .get("basicAuthCredentials")
            .filter(|v| !v.is_null())
            .map(|b| (b.get("username").map(json_text).unwrap_or_default(), b.get("password").map(json_text).unwrap_or_default()));
        let body = match obj.get("bodyPatterns").filter(|v| !v.is_null()) {
            Some(Value::Array(a)) => a.iter().enumerate().map(|(i, p)| ValuePattern::from_json(p).map_err(|e| format!("bodyPatterns[{i}]: {e}"))).collect::<Result<_, _>>()?,
            Some(_) => return Err("bodyPatterns must be an array".into()),
            None => Vec::new(),
        };
        let multipart = match obj.get("multipartPatterns").and_then(Value::as_array) {
            Some(a) => a
                .iter()
                .map(|m| -> Result<MultipartPattern, String> {
                    Ok(MultipartPattern {
                        name: m.get("name").and_then(Value::as_str).map(str::to_string),
                        all: m.get("matchingType").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("ALL")),
                        headers: named_patterns(m.get("headers"), "multipartPatterns.headers")?,
                        body: match m.get("bodyPatterns").and_then(Value::as_array) {
                            Some(b) => b.iter().map(ValuePattern::from_json).collect::<Result<_, _>>()?,
                            None => Vec::new(),
                        },
                    })
                })
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let opt_pattern = |k: &str| -> Result<Option<ValuePattern>, String> {
            obj.get(k).filter(|v| !v.is_null()).map(ValuePattern::from_json).transpose().map_err(|e| format!("{k}: {e}"))
        };
        Ok(Self {
            method: MethodPattern::from_json(obj.get("method"))?,
            url,
            path_params,
            query: named_patterns(obj.get("queryParameters"), "queryParameters")?,
            headers: named_patterns(obj.get("headers"), "headers")?,
            cookies: named_patterns(obj.get("cookies"), "cookies")?,
            form: named_patterns(obj.get("formParameters"), "formParameters")?,
            basic_auth,
            body,
            multipart,
            host: opt_pattern("host")?,
            port: obj.get("port").and_then(Value::as_u64).map(|p| p as u16),
            scheme: obj.get("scheme").and_then(Value::as_str).map(str::to_ascii_lowercase),
            client_ip: opt_pattern("clientIp")?,
            custom_matcher: match obj.get("customMatcher").filter(|v| !v.is_null()) {
                None => None,
                Some(c) => {
                    let name = c.get("name").map_or_else(|| c.to_string(), json_text);
                    if matches!(name.as_str(), "gql-matcher" | "graphql-body-matcher") {
                        let params = c.get("parameters").cloned().unwrap_or(Value::Null);
                        Some(CustomMatcher::Graphql(Box::new(crate::graphql::GqlPattern::from_custom_matcher(&params).map_err(|e| format!("customMatcher: {e}"))?)))
                    } else {
                        Some(CustomMatcher::Unsupported(name))
                    }
                }
            },
            graphql: obj.get("graphql").filter(|v| !v.is_null()).map(crate::graphql::GqlPattern::from_json).transpose()?.map(Box::new),
        })
    }

    /// Path variables when the pattern uses `urlPathTemplate` and the path fits.
    pub fn path_params(&self, req: &MockRequest) -> Vec<(String, String)> {
        match &self.url {
            Some(UrlMatcher::Template(t)) => t.captures(&req.path).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub fn is_match(&self, req: &MockRequest) -> bool {
        self.run(req, false).matched
    }

    pub fn evaluate(&self, req: &MockRequest) -> MatchResult {
        self.run(req, true)
    }

    fn run(&self, req: &MockRequest, detailed: bool) -> MatchResult {
        let mut fields: Vec<FieldResult> = Vec::new();
        // (distance, weight) per WireMock's RequestPattern weighting.
        let mut weighted: Vec<(f64, f64)> = Vec::new();
        let mut matched = true;
        macro_rules! bail {
            () => {
                if !matched && !detailed {
                    return MatchResult { matched: false, distance: 1.0, fields };
                }
            };
        }
        let field = |fields: &mut Vec<FieldResult>, name: String, expected: String, actual: Option<String>, e: Eval| {
            if detailed {
                fields.push(FieldResult { field: name, expected, actual, matched: e.matched, distance: e.distance, note: None });
            }
        };

        let method_ok = self.method.accepts(&req.method);
        matched &= method_ok;
        weighted.push((f64::from(u8::from(!method_ok)), 3.0));
        field(&mut fields, "Method".into(), self.method.describe(), Some(req.method.clone()), Eval::exact(method_ok));
        bail!();

        match &self.url {
            Some(u) => {
                let e = u.eval(req, detailed);
                matched &= e.matched;
                weighted.push((e.distance, 10.0));
                field(&mut fields, u.label().into(), u.expected().to_string(), Some(u.actual(req).to_string()), e);
            }
            None => weighted.push((0.0, 10.0)),
        }
        bail!();

        if let Some(s) = &self.scheme {
            let ok = s.eq_ignore_ascii_case(&req.scheme);
            matched &= ok;
            weighted.push((f64::from(u8::from(!ok)), 3.0));
            field(&mut fields, "Scheme".into(), s.clone(), Some(req.scheme.clone()), Eval::exact(ok));
        }
        if let Some(h) = &self.host {
            let e = if detailed { h.eval(Some(&req.host)) } else { Eval::exact(h.is_match(Some(&req.host))) };
            matched &= e.matched;
            weighted.push((e.distance, 10.0));
            field(&mut fields, "Host".into(), h.describe(), Some(req.host.clone()), e);
        }
        if let Some(p) = self.port {
            let ok = p == req.port;
            matched &= ok;
            weighted.push((f64::from(u8::from(!ok)), 10.0));
            field(&mut fields, "Port".into(), p.to_string(), Some(req.port.to_string()), Eval::exact(ok));
        }
        bail!();

        let mut group = |fields: &mut Vec<FieldResult>, matched: &mut bool, results: Vec<(String, String, Option<String>, Eval)>| {
            if results.is_empty() {
                weighted.push((0.0, 1.0));
                return;
            }
            let avg = results.iter().map(|r| r.3.distance).sum::<f64>() / results.len() as f64;
            weighted.push((avg, 1.0));
            for (name, expected, actual, e) in results {
                *matched &= e.matched;
                field(fields, name, expected, actual, e);
            }
        };

        // Path parameters
        let mut results = Vec::new();
        if !self.path_params.is_empty() {
            let vars = self.path_params(req);
            for (name, p) in &self.path_params {
                let actual = vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
                let e = if detailed { p.eval(actual) } else { Eval::exact(p.is_match(actual)) };
                results.push((format!("Path parameter {name}"), p.describe(), actual.map(str::to_string), e));
            }
        }
        group(&mut fields, &mut matched, results);
        bail!();

        // Headers (+ basic auth)
        let mut results = Vec::new();
        for (name, p) in &self.headers {
            let values = req.header_values(name);
            let e = p.eval(&values, detailed);
            results.push((format!("Header {name}"), p.describe(), (!values.is_empty()).then(|| values.join(", ")), e));
        }
        if let Some((u, pw)) = &self.basic_auth {
            let expected = format!("{u}:{pw}");
            let actual = req.basic_credentials();
            let ok = actual.as_deref() == Some(expected.as_str());
            results.push(("Basic auth".into(), expected, actual, Eval::exact(ok)));
        }
        group(&mut fields, &mut matched, results);
        bail!();

        // Query parameters
        let mut results = Vec::new();
        for (name, p) in &self.query {
            let values = req.query_values(name);
            let e = p.eval(&values, detailed);
            results.push((format!("Query {name}"), p.describe(), (!values.is_empty()).then(|| values.join(", ")), e));
        }
        group(&mut fields, &mut matched, results);
        bail!();

        // Form parameters
        let mut results = Vec::new();
        if !self.form.is_empty() {
            let form = req.form_params();
            for (name, p) in &self.form {
                let values: Vec<&str> = form.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).collect();
                let e = p.eval(&values, detailed);
                results.push((format!("Form {name}"), p.describe(), (!values.is_empty()).then(|| values.join(", ")), e));
            }
        }
        group(&mut fields, &mut matched, results);
        bail!();

        // Cookies
        let mut results = Vec::new();
        for (name, p) in &self.cookies {
            let values = req.cookie_values(name);
            let e = p.eval(&values, detailed);
            results.push((format!("Cookie {name}"), p.describe(), (!values.is_empty()).then(|| values.join(", ")), e));
        }
        group(&mut fields, &mut matched, results);
        bail!();

        // Body
        let mut results = Vec::new();
        let many = self.body.len() > 1;
        for (i, p) in self.body.iter().enumerate() {
            let e = p.eval_bytes(&req.body, detailed);
            let name = if many { format!("Body #{}", i + 1) } else { "Body".to_string() };
            let actual = if req.body.is_empty() { None } else { Some(truncate(&req.body_text(), 2000)) };
            results.push((name, truncate(&p.describe(), 2000), actual, e));
        }
        group(&mut fields, &mut matched, results);
        bail!();

        // Multipart
        let mut results = Vec::new();
        if !self.multipart.is_empty() {
            let parts = multipart_parts(req);
            for (i, mp) in self.multipart.iter().enumerate() {
                let ok = !parts.is_empty() && {
                    let check = |part: &Part| mp.matches(part);
                    if mp.all { parts.iter().all(check) } else { parts.iter().any(check) }
                };
                let label = mp.name.clone().unwrap_or_else(|| format!("#{}", i + 1));
                results.push((format!("Multipart {label}"), format!("{} part(s) matching", if mp.all { "all" } else { "any" }), Some(format!("{} part(s)", parts.len())), Eval::exact(ok)));
            }
        }
        group(&mut fields, &mut matched, results);
        bail!();

        if let Some(ip) = &self.client_ip {
            let e = if detailed { ip.eval(Some(&req.client_ip)) } else { Eval::exact(ip.is_match(Some(&req.client_ip))) };
            matched &= e.matched;
            weighted.push((e.distance, 1.0));
            field(&mut fields, "Client IP".into(), ip.describe(), Some(req.client_ip.clone()), e);
        }
        // GraphQL (devall `graphql` matcher and the extension's gql-matcher)
        let gql: Vec<&crate::graphql::GqlPattern> = self
            .graphql
            .as_deref()
            .into_iter()
            .chain(match &self.custom_matcher {
                Some(CustomMatcher::Graphql(g)) => Some(g.as_ref()),
                _ => None,
            })
            .collect();
        for g in gql {
            let results = g.eval(req);
            if !results.is_empty() {
                let avg = results.iter().map(|r| r.3.distance).sum::<f64>() / results.len() as f64;
                weighted.push((avg, 10.0));
            }
            for (name, expected, actual, e) in results {
                matched &= e.matched;
                field(&mut fields, name, truncate(&expected, 2000), actual.map(|a| truncate(&a, 2000)), e);
            }
            bail!();
        }
        if let Some(CustomMatcher::Unsupported(name)) = &self.custom_matcher {
            matched = false;
            weighted.push((1.0, 1.0));
            if detailed {
                fields.push(FieldResult {
                    field: "Custom matcher".into(),
                    expected: name.clone(),
                    actual: None,
                    matched: false,
                    distance: 1.0,
                    note: Some("custom matcher extensions are not supported by /dev/all".into()),
                });
            }
        }

        let total_w: f64 = weighted.iter().map(|(_, w)| w).sum();
        let distance = if matched { 0.0 } else { (weighted.iter().map(|(d, w)| d * w).sum::<f64>() / total_w.max(1.0)).max(0.001) };
        MatchResult { matched, distance, fields }
    }
}

struct Part {
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl MultipartPattern {
    fn matches(&self, part: &Part) -> bool {
        if let Some(name) = &self.name {
            let disp = part.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-disposition")).map_or("", |(_, v)| v.as_str());
            if !disp.contains(&format!("name=\"{name}\"")) && !disp.contains(&format!("name={name}")) {
                return false;
            }
        }
        let headers_ok = self.headers.iter().all(|(k, p)| {
            let vals: Vec<&str> = part.headers.iter().filter(|(hk, _)| hk.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str()).collect();
            p.eval(&vals, false).matched
        });
        headers_ok && self.body.iter().all(|p| p.eval_bytes(&part.body, false).matched)
    }
}

fn find_bytes(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn multipart_parts(req: &MockRequest) -> Vec<Part> {
    let Some(ct) = req.content_type() else { return Vec::new() };
    let Some(boundary) = ct.split(';').map(str::trim).find_map(|p| p.strip_prefix("boundary=")) else { return Vec::new() };
    let delim = format!("--{}", boundary.trim_matches('"'));
    let body = &req.body[..];
    let mut parts = Vec::new();
    let mut pos = match find_bytes(body, delim.as_bytes(), 0) {
        Some(p) => p + delim.len(),
        None => return parts,
    };
    loop {
        if body[pos..].starts_with(b"--") {
            break;
        }
        let start = pos + if body[pos..].starts_with(b"\r\n") { 2 } else { 0 };
        let Some(next) = find_bytes(body, delim.as_bytes(), start) else { break };
        let chunk = &body[start..next];
        let chunk = chunk.strip_suffix(b"\r\n").unwrap_or(chunk);
        let (head, content) = match find_bytes(chunk, b"\r\n\r\n", 0) {
            Some(i) => (&chunk[..i], &chunk[i + 4..]),
            None => (&chunk[..0], chunk),
        };
        let headers = String::from_utf8_lossy(head)
            .lines()
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        parts.push(Part { headers, body: content.to_vec() });
        pos = next + delim.len();
        if pos >= body.len() {
            break;
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> MockRequest {
        MockRequest::new(method, url, headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), body.to_string())
    }

    fn pat(v: Value) -> RequestPattern {
        RequestPattern::from_json(&v).unwrap()
    }

    #[test]
    fn urls_and_methods() {
        let r = req("GET", "/api/users/42?active=true", &[], "");
        assert!(pat(json!({"method": "GET", "url": "/api/users/42?active=true"})).is_match(&r));
        assert!(!pat(json!({"method": "GET", "url": "/api/users/42"})).is_match(&r));
        assert!(pat(json!({"method": "ANY", "urlPath": "/api/users/42"})).is_match(&r));
        assert!(pat(json!({"urlPattern": "/api/users/\\d+\\?.*"})).is_match(&r));
        assert!(pat(json!({"urlPathPattern": "/api/users/[0-9]+"})).is_match(&r));
        assert!(!pat(json!({"method": "POST", "urlPath": "/api/users/42"})).is_match(&r));
        assert!(pat(json!({"method": {"oneOf": ["GET", "HEAD"]}})).is_match(&r));
        assert!(!pat(json!({"method": {"noneOf": ["GET"]}})).is_match(&r));
        let t = pat(json!({"urlPathTemplate": "/api/users/{id}", "pathParameters": {"id": {"matches": "\\d+"}}}));
        assert!(t.is_match(&r));
        assert_eq!(t.path_params(&r), vec![("id".to_string(), "42".to_string())]);
        assert!(!t.is_match(&req("GET", "/api/users/abc", &[], "")));
        assert!(!t.is_match(&req("GET", "/api/users/1/x", &[], "")));
    }

    #[test]
    fn query_headers_cookies_auth_form() {
        let r = req(
            "POST",
            "/s?q=rust&tag=a&tag=b",
            &[("X-Api-Key", "k1"), ("Cookie", "sid=xyz"), ("Authorization", "Basic dXNlcjpwYXNz"), ("Content-Type", "application/x-www-form-urlencoded")],
            "name=Ann&age=30",
        );
        assert!(pat(json!({"queryParameters": {"q": {"equalTo": "rust"}, "tag": {"hasExactly": [{"equalTo": "a"}, {"equalTo": "b"}]}, "missing": {"absent": true}}})).is_match(&r));
        assert!(pat(json!({"headers": {"x-api-key": {"equalTo": "k1"}, "X-Other": {"absent": true}}})).is_match(&r));
        assert!(!pat(json!({"headers": {"X-Api-Key": {"equalTo": "k2"}}})).is_match(&r));
        assert!(pat(json!({"cookies": {"sid": {"matches": "x.z"}}})).is_match(&r));
        assert!(pat(json!({"basicAuthCredentials": {"username": "user", "password": "pass"}})).is_match(&r));
        assert!(!pat(json!({"basicAuthCredentials": {"username": "user", "password": "nope"}})).is_match(&r));
        assert!(pat(json!({"formParameters": {"name": {"equalTo": "Ann"}, "age": {"greaterThanNumber": 18}}})).is_match(&r));
        assert!(pat(json!({"bodyPatterns": [{"contains": "Ann"}, {"matches": ".*age=\\d+"}]})).is_match(&r));
    }

    #[test]
    fn distances_and_fields() {
        let r = req("GET", "/api/userz", &[], "");
        let close = pat(json!({"method": "GET", "urlPath": "/api/users"})).evaluate(&r);
        let far = pat(json!({"method": "POST", "urlPath": "/completely/different/path"})).evaluate(&r);
        assert!(!close.matched && !far.matched);
        assert!(close.distance < far.distance, "{} vs {}", close.distance, far.distance);
        assert_eq!(close.fields.len(), 2);
        assert!(close.fields[0].matched);
        assert!(!close.fields[1].matched);
        let exact = pat(json!({"method": "GET", "urlPath": "/api/userz"})).evaluate(&r);
        assert!(exact.matched && exact.distance == 0.0);
    }

    #[test]
    fn multipart() {
        let body = "--XyZ\r\nContent-Disposition: form-data; name=\"info\"\r\nContent-Type: application/json\r\n\r\n{\"a\":1}\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f.txt\"\r\n\r\nhello file\r\n--XyZ--\r\n";
        let r = req("POST", "/upload", &[("Content-Type", "multipart/form-data; boundary=XyZ")], body);
        assert_eq!(multipart_parts(&r).len(), 2);
        assert!(pat(json!({"multipartPatterns": [{"name": "info", "bodyPatterns": [{"equalToJson": {"a": 1}}]}]})).is_match(&r));
        assert!(pat(json!({"multipartPatterns": [{"matchingType": "ANY", "bodyPatterns": [{"contains": "hello"}]}]})).is_match(&r));
        assert!(!pat(json!({"multipartPatterns": [{"matchingType": "ALL", "bodyPatterns": [{"contains": "hello"}]}]})).is_match(&r));
    }
}
