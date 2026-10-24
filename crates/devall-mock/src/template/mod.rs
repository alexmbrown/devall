//! WireMock-compatible response templating on top of `handlebars`.
//!
//! The request model mirrors WireMock's (`request.url`, `request.path`, `request.pathSegments.[n]`,
//! `request.path.<var>` for `urlPathTemplate`, `request.query.x`, `request.headers.x` (case
//! insensitive), `request.cookies.x`, `request.method`, `request.body`, `request.baseUrl`, …) and
//! `parameters` holds the stub's `transformerParameters`. Output is not HTML-escaped (WireMock 3's
//! default). Multi-valued fields render their first value when single and `[a, b]` when repeated;
//! `.[n]` indexing and `#each` always see the full list.

mod data;
mod faker;
mod helpers;

use std::borrow::Cow;
use std::sync::{Arc, LazyLock};

use handlebars::Handlebars;
use serde_json::{Map, Value, json};

use crate::data::DataSources;
use crate::request::MockRequest;
use crate::util::b64_encode;

pub use faker::{KEYS as FAKER_KEYS, fake_value};

pub struct TemplateEngine {
    hb: Handlebars<'static>,
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TemplateEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TemplateEngine")
    }
}

static ARRAY_CONTEXT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"((?:#each|size|join|arrayJoin|pickRandom|toJson)\s+(?:'[^']*'\s+|"[^"]*"\s+)?)request\.(query|headers|cookies)\."#).unwrap()
});
static INDEXED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"request\.(query|headers|cookies)\.(\[[^\]]+\]|[\w\-]+)\.\["#).unwrap());
static HEADER_NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"request\.(headers|__headersAll)\.(\[[^\]]+\]|[\w\-]+)"#).unwrap());
static PATH_VARS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r#"\brequest\.path\."#).unwrap());

/// Handlebars.java keeps backslashes in string literals verbatim (`'\d+'`), handlebars-rust
/// parses them as JSON escapes. Doubling them inside `{{ … }}` literals restores Java semantics.
fn escape_literal_backslashes(template: &str) -> Cow<'_, str> {
    if !template.contains('\\') {
        return Cow::Borrowed(template);
    }
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' && chars.get(i + 1) == Some(&'{') {
            // copy the opening braces
            while i < chars.len() && chars[i] == '{' {
                out.push('{');
                i += 1;
            }
            if chars.get(i) == Some(&'!') {
                // comment: copy verbatim up to the closing braces
                while i < chars.len() && !(chars[i] == '}' && chars.get(i + 1) == Some(&'}')) {
                    out.push(chars[i]);
                    i += 1;
                }
                continue;
            }
            let mut quote: Option<char> = None;
            while i < chars.len() {
                let c = chars[i];
                match quote {
                    None if c == '}' && chars.get(i + 1) == Some(&'}') => break,
                    None => {
                        if c == '\'' || c == '"' {
                            quote = Some(c);
                        }
                        out.push(c);
                    }
                    Some(q) => {
                        if c == '\\' {
                            if chars.get(i + 1) == Some(&q) {
                                out.push('\\');
                                out.push(q);
                                i += 2;
                                continue;
                            }
                            out.push_str("\\\\");
                        } else {
                            if c == q {
                                quote = None;
                            }
                            out.push(c);
                        }
                    }
                }
                i += 1;
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    Cow::Owned(out)
}

/// Rewrites WireMock model paths into the shape of our JSON model (see module docs).
fn preprocess(template: &str) -> Cow<'_, str> {
    let template = escape_literal_backslashes(template);
    if !template.contains("request.") {
        return template;
    }
    let s = PATH_VARS.replace_all(&template, "request.__pathVars.");
    let s = ARRAY_CONTEXT.replace_all(&s, "${1}request.__${2}All.");
    let s = INDEXED.replace_all(&s, "request.__${1}All.${2}.[");
    let s = HEADER_NAME.replace_all(&s, |c: &regex::Captures| format!("request.{}.{}", &c[1], c[2].to_ascii_lowercase()));
    Cow::Owned(s.into_owned())
}

impl TemplateEngine {
    /// An engine whose data-source helpers have no project (they render an error).
    pub fn new() -> Self {
        Self::with_data(Arc::new(DataSources::none()))
    }

    /// An engine whose `dataSource` / `dataRows` / `dataCount` helpers read `sources`.
    pub fn with_data(sources: Arc<DataSources>) -> Self {
        let mut hb = Handlebars::new();
        hb.register_escape_fn(handlebars::no_escape);
        hb.set_strict_mode(false);
        helpers::register(&mut hb);
        data::register(&mut hb, sources);
        Self { hb }
    }

    /// Renders a template against a model built by [`model`].
    pub fn render(&self, template: &str, model: &Value) -> Result<String, String> {
        if !template.contains("{{") {
            return Ok(template.to_string());
        }
        let pre = preprocess(template);
        self.hb.render_template(&pre, model).map_err(|e| format!("template error: {e}"))
    }
}

fn single_or_list(values: Vec<String>) -> Value {
    if values.len() == 1 { Value::String(values.into_iter().next().unwrap_or_default()) } else { json!(values) }
}

fn grouped(pairs: impl IntoIterator<Item = (String, String)>) -> (Map<String, Value>, Map<String, Value>) {
    let mut lists: Vec<(String, Vec<String>)> = Vec::new();
    for (k, v) in pairs {
        match lists.iter_mut().find(|(lk, _)| *lk == k) {
            Some((_, l)) => l.push(v),
            None => lists.push((k, vec![v])),
        }
    }
    let all: Map<String, Value> = lists.iter().map(|(k, l)| (k.clone(), json!(l))).collect();
    let first: Map<String, Value> = lists.into_iter().map(|(k, l)| (k, single_or_list(l))).collect();
    (first, all)
}

/// Builds the template model for a request.
pub fn model(req: &MockRequest, path_params: &[(String, String)], parameters: &Value) -> Value {
    let segments: Vec<String> = req
        .path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned())
        .collect();
    let mut path_vars: Map<String, Value> = segments.iter().enumerate().map(|(i, s)| (i.to_string(), Value::String(s.clone()))).collect();
    for (k, v) in path_params {
        path_vars.insert(k.clone(), Value::String(v.clone()));
    }
    let (query, query_all) = grouped(req.query_params().iter().cloned());
    let (headers, headers_all) = grouped(req.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())));
    let (cookies, cookies_all) = grouped(req.cookies().iter().cloned());
    json!({
        "request": {
            "id": req.id.to_string(),
            "url": req.url,
            "path": req.path,
            "pathSegments": segments,
            "__pathVars": path_vars,
            "method": req.method,
            "scheme": req.scheme,
            "host": req.host,
            "port": req.port,
            "baseUrl": req.base_url(),
            "clientIp": req.client_ip,
            "protocol": req.protocol,
            "query": query,
            "__queryAll": query_all,
            "headers": headers,
            "__headersAll": headers_all,
            "cookies": cookies,
            "__cookiesAll": cookies_all,
            "body": req.body_text(),
            "bodyAsBase64": b64_encode(&req.body),
        },
        "parameters": parameters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(tpl: &str, req: &MockRequest) -> String {
        let engine = TemplateEngine::new();
        engine.render(tpl, &model(req, &[("id".into(), "42".into())], &json!({"greeting": "hi"}))).unwrap()
    }

    fn sample() -> MockRequest {
        MockRequest::new(
            "POST",
            "/users/42/orders?page=2&tag=a&tag=b",
            vec![("Host".into(), "localhost:9000".into()), ("X-Request-Id".into(), "abc-123".into()), ("Cookie".into(), "session=s1".into())],
            r#"{"name":"Ann","items":[{"sku":"A1"},{"sku":"B2"}]}"#,
        )
    }

    #[test]
    fn request_model() {
        let r = sample();
        assert_eq!(render("{{request.method}} {{request.url}}", &r), "POST /users/42/orders?page=2&tag=a&tag=b");
        assert_eq!(render("{{request.path}}", &r), "/users/42/orders");
        assert_eq!(render("{{request.pathSegments.[1]}}", &r), "42");
        assert_eq!(render("{{request.path.[2]}}", &r), "orders");
        assert_eq!(render("{{request.path.id}}", &r), "42");
        assert_eq!(render("{{request.query.page}}", &r), "2");
        assert_eq!(render("{{request.query.tag}}", &r), "[a, b]");
        assert_eq!(render("{{request.query.tag.[1]}}", &r), "b");
        assert_eq!(render("{{request.query.page.[0]}}", &r), "2");
        assert_eq!(render("{{#each request.query.tag}}<{{this}}>{{/each}}", &r), "<a><b>");
        assert_eq!(render("{{size request.query.page}}", &r), "1");
        assert_eq!(render("{{request.headers.X-Request-Id}}", &r), "abc-123");
        assert_eq!(render("{{request.headers.x-request-id}}", &r), "abc-123");
        assert_eq!(render("{{request.headers.[X-Request-Id]}}", &r), "abc-123");
        assert_eq!(render("{{request.cookies.session}}", &r), "s1");
        assert_eq!(render("{{request.baseUrl}}", &r), "http://localhost:9000");
        assert_eq!(render("{{parameters.greeting}}", &r), "hi");
        assert_eq!(render("{{{request.body}}}", &r), r#"{"name":"Ann","items":[{"sku":"A1"},{"sku":"B2"}]}"#);
        assert_eq!(render("{{request.body}}", &r), r#"{"name":"Ann","items":[{"sku":"A1"},{"sku":"B2"}]}"#, "no HTML escaping");
        assert_eq!(render("plain text", &r), "plain text");
        assert!(TemplateEngine::new().render("{{#if}}", &json!({})).is_err());
    }
}
