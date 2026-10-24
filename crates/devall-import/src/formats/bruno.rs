//! Bruno collections: a folder of `.bru` files (`bruno.json`, `collection.bru`, `folder.bru`,
//! `environments/*.bru`) or a single `.bru` request. The DSL is a list of blocks:
//! dictionaries (`headers { name: value }`, `~` disables a row), text blocks (`body:json { … }`)
//! and lists (`vars:secret [ a, b ]`).

use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use devall_core::{
    ApiKeyLocation, AssertOp, AssertTarget, Assertion, Auth, Body, Capture, CaptureSource, EnvironmentFile, FolderFile, HttpRequestFile, KeyValue,
    MultipartPart, OAuth2ClientAuth, OAuth2Config, OAuth2Grant,
};

use crate::detect::Format;
use crate::error::{Error, Result, io_err};
use crate::plan::{ImportPlan, PlanFolder, PlanItem};
use crate::util::{Warnings, add_path_vars, colon_path_vars, default_name, folder_has_settings, inline_vars, map_templates, split_query, str_at};

const METHOD_BLOCKS: &[&str] = &["get", "post", "put", "delete", "patch", "options", "head", "connect", "trace", "http"];
const SCRIPTS_KEPT: &str = "Bruno scripts are not run by /dev/all; they were kept in the request docs";

// ---------- the DSL ----------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pair {
    pub key: String,
    pub value: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum BlockBody {
    Dict(Vec<Pair>),
    Text(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BruFile {
    pub blocks: Vec<(String, BlockBody)>,
}

impl BruFile {
    pub(crate) fn dict(&self, name: &str) -> &[Pair] {
        self.blocks
            .iter()
            .find_map(|(n, b)| match b {
                BlockBody::Dict(d) if n == name => Some(d.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }

    pub(crate) fn text(&self, name: &str) -> Option<&str> {
        self.blocks.iter().find_map(|(n, b)| match b {
            BlockBody::Text(t) if n == name => Some(t.as_str()),
            _ => None,
        })
    }

    pub(crate) fn list(&self, name: &str) -> &[String] {
        self.blocks
            .iter()
            .find_map(|(n, b)| match b {
                BlockBody::List(l) if n == name => Some(l.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }

    pub(crate) fn get(&self, block: &str, key: &str) -> Option<&str> {
        self.dict(block).iter().find(|p| p.key == key).map(|p| p.value.as_str())
    }

    fn has(&self, name: &str) -> bool {
        self.blocks.iter().any(|(n, _)| n == name)
    }
}

fn is_text_block(name: &str) -> bool {
    match name {
        "body" | "tests" | "docs" | "example" => true,
        _ if name.starts_with("script:") => true,
        _ if name.starts_with("body:") => !matches!(name, "body:form-urlencoded" | "body:multipart-form" | "body:file" | "body:grpc" | "body:ws"),
        _ => false,
    }
}

pub(crate) fn parse_bru(text: &str) -> BruFile {
    let lines: Vec<&str> = text.trim_start_matches('\u{feff}').lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut out = BruFile::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_end();
        if t.is_empty() || line.starts_with(char::is_whitespace) {
            i += 1;
            continue;
        }
        if let Some(name) = t.strip_suffix('{').map(str::trim) {
            let start = i + 1;
            let mut end = start;
            while end < lines.len() && lines[end].trim_end() != "}" {
                end += 1;
            }
            let content = &lines[start..end.min(lines.len())];
            let body = if is_text_block(name) { BlockBody::Text(outdent(content, 2)) } else { BlockBody::Dict(parse_pairs(content)) };
            out.blocks.push((name.to_string(), body));
            i = end + 1;
        } else if let Some(name) = t.strip_suffix('[').map(str::trim) {
            let mut items = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != "]" {
                let item = lines[i].trim().trim_end_matches(',').trim();
                if !item.is_empty() {
                    items.push(item.to_string());
                }
                i += 1;
            }
            out.blocks.push((name.to_string(), BlockBody::List(items)));
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

fn outdent(lines: &[&str], n: usize) -> String {
    let text = lines
        .iter()
        .map(|l| {
            let strip = l.chars().take(n).take_while(|c| *c == ' ').count();
            &l[strip..]
        })
        .collect::<Vec<_>>()
        .join("\n");
    text.trim_end_matches('\n').to_string()
}

fn parse_pairs(lines: &[&str]) -> Vec<Pair> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        i += 1;
        if t.is_empty() || (t.starts_with('@') && (t.ends_with(')') || !t.contains(':'))) {
            continue; // blank or annotation
        }
        let (enabled, rest) = match t.strip_prefix('~') {
            Some(r) => (false, r),
            None => (true, t),
        };
        let (key, after) = if let Some(q) = rest.strip_prefix('"') {
            let mut key = String::new();
            let mut chars = q.char_indices();
            let mut end = q.len();
            while let Some((j, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            key.push(n);
                        }
                    }
                    '"' => {
                        end = j + 1;
                        break;
                    }
                    c => key.push(c),
                }
            }
            (key, &q[end..])
        } else {
            match rest.find(':') {
                Some(j) => (rest[..j].trim().to_string(), &rest[j..]),
                None => continue,
            }
        };
        let Some(value) = after.trim_start().strip_prefix(':') else { continue };
        let mut value = value.trim().to_string();
        if let Some(inline) = value.strip_prefix("'''") {
            if let Some(end) = inline.find("'''") {
                value = inline[..end].to_string();
            } else {
                let mut body: Vec<&str> = Vec::new();
                let mut suffix = String::new();
                while i < lines.len() {
                    let l = lines[i];
                    i += 1;
                    if let Some(after) = l.trim_start().strip_prefix("'''") {
                        suffix = after.trim().to_string();
                        break;
                    }
                    body.push(l);
                }
                value = outdent(&body, 4);
                if !suffix.is_empty() {
                    value = format!("{value} {suffix}");
                }
            }
        } else if value == "[" {
            while i < lines.len() && lines[i].trim() != "]" {
                i += 1;
            }
            i += 1;
            value.clear();
        }
        out.push(Pair { key, value, enabled });
    }
    out
}

fn conv(s: &str) -> String {
    map_templates(s, |k| k.strip_prefix("process.env.").map(|n| format!("{{{{$env.{n}}}}}")))
}

fn kvs(pairs: &[Pair]) -> Vec<KeyValue> {
    pairs
        .iter()
        .map(|p| {
            let mut kv = KeyValue::new(conv(&p.key), conv(&p.value));
            kv.enabled = p.enabled;
            kv
        })
        .collect()
}

// ---------- requests ----------

pub(crate) struct BruRequest {
    pub request: HttpRequestFile,
    pub seq: f64,
    pub path_vars: Vec<(String, String)>,
}

pub(crate) fn request(b: &BruFile, fallback_name: &str, w: &mut Warnings) -> Option<BruRequest> {
    let ty = b.get("meta", "type").unwrap_or("http");
    let name = b.get("meta", "name").filter(|n| !n.trim().is_empty()).unwrap_or(fallback_name).trim().to_string();
    if matches!(ty, "grpc" | "ws") {
        w.about("gRPC and WebSocket requests are not supported yet and were skipped", &name);
        return None;
    }
    let seq = b.get("meta", "seq").and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::MAX);
    let (block, pairs) = b.blocks.iter().find_map(|(n, body)| match body {
        BlockBody::Dict(d) if METHOD_BLOCKS.contains(&n.as_str()) => Some((n.as_str(), d)),
        _ => None,
    })?;
    let field = |k: &str| pairs.iter().find(|p| p.key == k).map(|p| p.value.as_str());
    let method = if block == "http" { field("method").unwrap_or("GET").to_uppercase() } else { block.to_uppercase() };
    let (base, url_params) = split_query(&conv(field("url").unwrap_or("")));
    let (url, _) = colon_path_vars(&base);

    let mut req = HttpRequestFile::new(if name.is_empty() { default_name(&method, &url) } else { name });
    req.method = method;
    req.url = url;
    req.params = if b.has("params:query") || b.has("query") {
        let q = if b.has("params:query") { b.dict("params:query") } else { b.dict("query") };
        kvs(q)
    } else {
        url_params
    };
    req.headers = kvs(b.dict("headers"));
    req.auth = auth(b, field("auth").unwrap_or("inherit"), w, &req.name.clone());
    req.body = body(b, field("body"), w, &req.name.clone());
    let path_vars = b.dict("params:path").iter().map(|p| (p.key.clone(), conv(&p.value))).collect();

    let vars: IndexMap<String, String> = b.dict("vars:pre-request").iter().filter(|p| p.enabled).map(|p| (p.key.clone(), conv(&p.value))).collect();
    inline_vars(&mut req, &vars);

    let mut docs: Vec<String> = Vec::new();
    if let Some(d) = b.text("docs").filter(|d| !d.trim().is_empty()) {
        docs.push(d.trim().to_string());
    }
    let mut kept: Vec<String> = Vec::new();
    for p in b.dict("assert") {
        let (op, value) = p.value.split_once(char::is_whitespace).map(|(o, v)| (o, Some(v))).unwrap_or((p.value.as_str(), None));
        let converted = match assertion(&p.key, op, value) {
            Some(a) => Some(a),
            None => assertion(&p.key, "eq", Some(&p.value)).filter(|_| !is_op(op)),
        };
        match converted {
            Some(mut a) => {
                a.enabled = p.enabled;
                req.tests.push(a);
            }
            None => kept.push(format!("assert {}: {}", p.key, p.value)),
        }
    }
    for p in b.dict("vars:post-response") {
        let name = p.key.trim_start_matches('@');
        match capture_source(&p.value) {
            Some(source) => req.captures.push(Capture { name: name.to_string(), source, enabled: p.enabled }),
            None => kept.push(format!("vars:post-response {name}: {}", p.value)),
        }
    }
    if !kept.is_empty() {
        w.about("Some Bruno asserts/post-response vars have no equivalent; they were kept in the request docs", &req.name);
        docs.push(format!("Not converted from Bruno:\n\n```\n{}\n```", kept.join("\n")));
    }
    for (block, title) in [("script:pre-request", "Pre-request script"), ("script:post-response", "Post-response script"), ("tests", "Tests")] {
        if let Some(code) = b.text(block).filter(|c| !c.trim().is_empty()) {
            docs.push(format!("### {title} (Bruno)\n\n```js\n{}\n```", code.trim_end()));
            w.about(SCRIPTS_KEPT, &req.name);
        }
    }
    if !docs.is_empty() {
        req.docs = Some(docs.join("\n\n"));
    }
    if b.get("settings", "followRedirects") == Some("false") {
        req.settings.follow_redirects = false;
    }
    if let Some(t) = b.get("settings", "timeout").and_then(|t| t.parse::<u64>().ok()).filter(|t| *t > 0) {
        req.settings.timeout_ms = t;
    }
    Some(BruRequest { request: req, seq, path_vars })
}

fn auth(b: &BruFile, mode: &str, w: &mut Warnings, subject: &str) -> Auth {
    let g = |block: &str, k: &str| conv(b.get(block, k).unwrap_or(""));
    match mode {
        "none" => Auth::None,
        "inherit" | "" => Auth::Inherit,
        "basic" => Auth::Basic { username: g("auth:basic", "username"), password: g("auth:basic", "password") },
        "bearer" => Auth::Bearer { token: g("auth:bearer", "token"), prefix: None },
        "digest" => Auth::Digest { username: g("auth:digest", "username"), password: g("auth:digest", "password") },
        "ntlm" => Auth::Ntlm {
            username: g("auth:ntlm", "username"),
            password: g("auth:ntlm", "password"),
            domain: Some(g("auth:ntlm", "domain")).filter(|d| !d.is_empty()),
            workstation: None,
        },
        "apikey" => Auth::ApiKey {
            key: g("auth:apikey", "key"),
            value: g("auth:apikey", "value"),
            location: if g("auth:apikey", "placement").starts_with("query") { ApiKeyLocation::Query } else { ApiKeyLocation::Header },
        },
        "awsv4" => Auth::AwsSigV4 {
            access_key_id: g("auth:awsv4", "accessKeyId"),
            secret_access_key: g("auth:awsv4", "secretAccessKey"),
            session_token: Some(g("auth:awsv4", "sessionToken")).filter(|t| !t.is_empty()),
            region: g("auth:awsv4", "region"),
            service: g("auth:awsv4", "service"),
        },
        "oauth2" => {
            let o = |k: &str| g("auth:oauth2", k);
            let grant = match o("grant_type").as_str() {
                "client_credentials" => OAuth2Grant::ClientCredentials,
                "password" => OAuth2Grant::Password,
                "implicit" => {
                    w.about("OAuth 2.0 implicit grant is not supported; converted to a bearer token", subject);
                    return Auth::Bearer { token: String::new(), prefix: None };
                }
                _ => OAuth2Grant::AuthorizationCode,
            };
            let mut extra = IndexMap::new();
            for (name, body) in &b.blocks {
                if name.starts_with("auth:oauth2:additional_params:")
                    && (name.ends_with(":body") || name.ends_with(":queryparams"))
                    && !name.contains("refresh_token_req")
                    && let BlockBody::Dict(d) = body
                {
                    for p in d.iter().filter(|p| p.enabled) {
                        extra.insert(p.key.clone(), conv(&p.value));
                    }
                }
            }
            Auth::OAuth2(OAuth2Config {
                grant,
                auth_url: o("authorization_url"),
                token_url: o("access_token_url"),
                client_id: o("client_id"),
                client_secret: o("client_secret"),
                scope: o("scope"),
                username: o("username"),
                password: o("password"),
                use_pkce: o("pkce") != "false",
                redirect_port: super::postman::redirect_port(&o("callback_url")),
                client_auth: if o("credentials_placement") == "body" { OAuth2ClientAuth::Body } else { OAuth2ClientAuth::Basic },
                token_prefix: Some(o("token_header_prefix")).filter(|p| !p.is_empty() && p != "Bearer"),
                extra_params: extra,
                ..Default::default()
            })
        }
        other => {
            w.about(&format!("“{other}” auth is not supported yet; affected items inherit their folder's auth"), subject);
            Auth::Inherit
        }
    }
}

fn body(b: &BruFile, mode: Option<&str>, w: &mut Warnings, subject: &str) -> Body {
    let mode = match mode {
        Some(m) => m.to_string(),
        None => ["json", "text", "xml", "graphql", "form-urlencoded", "multipart-form"]
            .iter()
            .find(|m| b.has(&format!("body:{m}")))
            .map(|m| match *m {
                "form-urlencoded" => "formUrlEncoded".to_string(),
                "multipart-form" => "multipartForm".to_string(),
                m => m.to_string(),
            })
            .unwrap_or_default(),
    };
    let text = |n: &str| conv(b.text(n).unwrap_or(""));
    match mode.as_str() {
        "json" => Body::Json { content: text("body:json") },
        "text" => Body::Text { content: text("body:text"), content_type: None },
        "xml" => Body::Xml { content: text("body:xml") },
        "sparql" => Body::Text { content: text("body:sparql"), content_type: Some("application/sparql-query".into()) },
        "graphql" => Body::Graphql { query: text("body:graphql"), variables: text("body:graphql:vars"), operation_name: None },
        "formUrlEncoded" => Body::Form { fields: kvs(b.dict("body:form-urlencoded")) },
        "multipartForm" => Body::Multipart {
            parts: b
                .dict("body:multipart-form")
                .iter()
                .map(|p| {
                    let (value, ct) = split_content_type(&p.value);
                    let file = value.strip_prefix("@file(").and_then(|v| v.strip_suffix(')')).map(|files| {
                        let mut list = files.split('|').filter(|f| !f.is_empty());
                        let first = list.next().unwrap_or("").to_string();
                        if list.next().is_some() {
                            w.about("Multipart fields with several files keep only the first file", subject);
                        }
                        first
                    });
                    MultipartPart {
                        name: conv(&p.key),
                        value: if file.is_some() { String::new() } else { conv(&value) },
                        file,
                        content_type: ct,
                        enabled: p.enabled,
                    }
                })
                .collect(),
        },
        "file" => {
            let chosen = b.dict("body:file").iter().find(|p| p.enabled).or_else(|| b.dict("body:file").first());
            match chosen {
                Some(p) => {
                    let (value, ct) = split_content_type(&p.value);
                    let path = value.strip_prefix("@file(").and_then(|v| v.strip_suffix(')')).unwrap_or(&value).to_string();
                    Body::Binary { path, content_type: ct }
                }
                None => Body::None,
            }
        }
        _ => Body::None,
    }
}

/// `value @contentType(x)` → (value, Some(x)).
fn split_content_type(v: &str) -> (String, Option<String>) {
    match v.rfind("@contentType(") {
        Some(i) if v.ends_with(')') => {
            let ct = v[i + "@contentType(".len()..v.len() - 1].trim().to_string();
            (v[..i].trim().to_string(), Some(ct).filter(|c| !c.is_empty()))
        }
        _ => (v.trim().to_string(), None),
    }
}

// ---------- asserts and post-response vars (shared with OpenCollection) ----------

fn is_op(op: &str) -> bool {
    matches!(
        op,
        "eq" | "neq" | "gt" | "gte" | "lt" | "lte" | "in" | "notIn" | "contains" | "notContains" | "length" | "matches" | "notMatches" | "startsWith" | "endsWith"
            | "between" | "isEmpty" | "isNotEmpty" | "isNull" | "isUndefined" | "isDefined" | "isTruthy" | "isFalsy" | "isJson" | "isNumber" | "isString"
            | "isBoolean" | "isArray"
    )
}

fn assert_target(expr: &str) -> Option<AssertTarget> {
    let e = expr.trim().trim_start_matches('$');
    match e {
        "res.status" | "res.statusCode" | "res.getStatus()" => return Some(AssertTarget::Status),
        "res.body" | "res.getBody()" => return Some(AssertTarget::Body),
        "res.responseTime" | "res.getResponseTime()" => return Some(AssertTarget::ResponseTime),
        _ => {}
    }
    if let Some(rest) = e.strip_prefix("res.body").filter(|r| r.starts_with(['.', '['])) {
        return Some(AssertTarget::JsonPath { path: format!("${rest}") });
    }
    let header = e
        .strip_prefix("res.headers.")
        .map(str::to_string)
        .or_else(|| e.strip_prefix("res.headers[").and_then(|r| r.strip_suffix(']')).map(|r| r.trim_matches(['\'', '"']).to_string()))
        .or_else(|| e.strip_prefix("res.getHeader(").and_then(|r| r.strip_suffix(')')).map(|r| r.trim_matches(['\'', '"']).to_string()))?;
    Some(if header.eq_ignore_ascii_case("content-type") { AssertTarget::ContentType } else { AssertTarget::Header { name: header } })
}

/// Bruno assertion (`res.status`, `eq`, `200`) → /dev/all assertion.
pub(crate) fn assertion(expr: &str, op: &str, value: Option<&str>) -> Option<Assertion> {
    let target = assert_target(expr)?;
    let value = value.map(|v| {
        let v = v.trim();
        if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) { v[1..v.len() - 1].to_string() } else { v.to_string() }
    });
    let (op, value) = match op.trim() {
        "eq" => (AssertOp::Eq, value),
        "neq" => (AssertOp::Ne, value),
        "gt" => (AssertOp::Gt, value),
        "gte" => (AssertOp::Gte, value),
        "lt" => (AssertOp::Lt, value),
        "lte" => (AssertOp::Lte, value),
        "contains" => (AssertOp::Contains, value),
        "notContains" => (AssertOp::NotContains, value),
        "startsWith" => (AssertOp::StartsWith, value),
        "endsWith" => (AssertOp::EndsWith, value),
        "matches" => (AssertOp::Matches, value),
        "isEmpty" => (AssertOp::IsEmpty, None),
        "isNotEmpty" => (AssertOp::IsNotEmpty, None),
        "isDefined" => (AssertOp::Exists, None),
        "isUndefined" => (AssertOp::NotExists, None),
        "isNull" => (AssertOp::IsType, Some("null".into())),
        "isString" => (AssertOp::IsType, Some("string".into())),
        "isNumber" => (AssertOp::IsType, Some("number".into())),
        "isBoolean" => (AssertOp::IsType, Some("boolean".into())),
        "isArray" => (AssertOp::IsType, Some("array".into())),
        "isJson" => (AssertOp::IsType, Some("object".into())),
        "in" => (AssertOp::OneOf, value.map(|v| v.trim_matches(['[', ']']).split(',').map(|x| x.trim().trim_matches(['"', '\''])).collect::<Vec<_>>().join(","))),
        _ => return None,
    };
    Some(Assertion { target, op, value, enabled: true })
}

/// Bruno post-response variable expression → capture source.
pub(crate) fn capture_source(expr: &str) -> Option<CaptureSource> {
    let e = expr.trim().trim_start_matches('$');
    if let Some(q) = e.strip_prefix("res(").and_then(|r| r.strip_suffix(')')) {
        let q = q.trim().trim_matches(['\'', '"']);
        return Some(CaptureSource::JsonPath { path: if q.starts_with('$') { q.to_string() } else { format!("$.{q}") } });
    }
    if e.starts_with('.') || e.starts_with('[') {
        return Some(CaptureSource::JsonPath { path: format!("${e}") });
    }
    match assert_target(e)? {
        AssertTarget::Status => Some(CaptureSource::Status),
        AssertTarget::Body => Some(CaptureSource::Body),
        AssertTarget::JsonPath { path } => Some(CaptureSource::JsonPath { path }),
        AssertTarget::Header { name } => Some(CaptureSource::Header { header: name }),
        AssertTarget::ContentType => Some(CaptureSource::Header { header: "Content-Type".into() }),
        _ => None,
    }
}

// ---------- folders, environments, collections ----------

fn folder_settings(b: &BruFile, w: &mut Warnings, name: &str) -> Option<FolderFile> {
    let mut s = FolderFile { headers: kvs(b.dict("headers")), ..Default::default() };
    let mode = b.get("auth", "mode").unwrap_or(if b.has("auth:bearer") || b.has("auth:basic") { "" } else { "inherit" });
    s.auth = auth(b, mode, w, name);
    s.variables = b.dict("vars:pre-request").iter().filter(|p| p.enabled).map(|p| (p.key.clone(), conv(&p.value))).collect();
    if ["script:pre-request", "script:post-response", "tests"].iter().any(|n| b.text(n).is_some_and(|t| !t.trim().is_empty())) {
        w.about("Folder/collection-level Bruno scripts were not imported", name);
    }
    folder_has_settings(&s).then_some(s)
}

pub(crate) fn environment(name: &str, b: &BruFile) -> EnvironmentFile {
    let mut env = EnvironmentFile::new(name);
    for p in b.dict("vars").iter().filter(|p| p.enabled) {
        env.variables.insert(p.key.clone(), conv(&p.value));
    }
    env.secrets = b.list("vars:secret").iter().map(|s| s.trim_start_matches('~').to_string()).collect();
    env
}

/// A single `.bru` file: a request or an environment.
pub(crate) fn parse_single(text: &str, name: &str) -> Result<ImportPlan> {
    let b = parse_bru(text);
    let mut w = Warnings::default();
    let mut plan = ImportPlan::new(name, Format::Bruno);
    if b.has("vars") && !b.blocks.iter().any(|(n, _)| METHOD_BLOCKS.contains(&n.as_str())) {
        plan.environments.push(environment(name, &b));
        return Ok(plan);
    }
    let r = request(&b, name, &mut w).ok_or_else(|| Error::Parse("not a Bruno request (no get/post/… block)".into()))?;
    let mut req = r.request;
    add_path_vars(&mut plan.settings, &mut req, r.path_vars);
    plan.items.push(PlanItem::Request(Box::new(req)));
    plan.warnings = w.finish();
    Ok(plan)
}

/// A collection folder (`bruno.json` at the root).
pub(crate) fn parse_dir(root: &Path) -> Result<ImportPlan> {
    let dir_name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Bruno collection".into());
    let name = std::fs::read_to_string(root.join("bruno.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| str_at(&v, "name").trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or(dir_name);
    let mut plan = ImportPlan::new(name, Format::Bruno);
    let mut w = Warnings::default();
    if let Ok(t) = std::fs::read_to_string(root.join("collection.bru")) {
        plan.settings = folder_settings(&parse_bru(&t), &mut w, "collection");
    }
    let env_dir = root.join("environments");
    if let Ok(rd) = std::fs::read_dir(&env_dir) {
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "bru")).collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).map_err(io_err(&f))?;
            let name = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            plan.environments.push(environment(&name, &parse_bru(&text)));
        }
    }
    let mut settings = plan.settings.take();
    plan.items = walk(root, true, &mut settings, &mut w)?;
    plan.settings = settings;
    if plan.items.is_empty() && plan.environments.is_empty() {
        return Err(Error::Parse(format!("no .bru requests found in {}", root.display())));
    }
    plan.warnings = w.finish();
    Ok(plan)
}

fn walk(dir: &Path, is_root: bool, settings: &mut Option<FolderFile>, w: &mut Warnings) -> Result<Vec<PlanItem>> {
    let mut entries: Vec<(f64, String, PlanItem)> = Vec::new();
    let rd = std::fs::read_dir(dir).map_err(io_err(dir))?;
    let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let file = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if file.starts_with('.') || file == "node_modules" {
            continue;
        }
        if p.is_dir() {
            if is_root && file == "environments" {
                continue;
            }
            let folder_bru = std::fs::read_to_string(p.join("folder.bru")).ok().map(|t| parse_bru(&t));
            let name = folder_bru.as_ref().and_then(|b| b.get("meta", "name")).map(str::to_string).filter(|n| !n.trim().is_empty()).unwrap_or(file.clone());
            let seq = folder_bru.as_ref().and_then(|b| b.get("meta", "seq")).and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::MAX);
            let mut f = PlanFolder::new(name.clone());
            let mut s = folder_bru.as_ref().and_then(|b| folder_settings(b, w, &name));
            f.items = walk(&p, false, &mut s, w)?;
            f.settings = s;
            if !f.items.is_empty() || f.settings.is_some() {
                entries.push((seq, name.to_lowercase(), PlanItem::Folder(f)));
            }
        } else if file.ends_with(".bru") && file != "folder.bru" && file != "collection.bru" {
            let text = std::fs::read_to_string(&p).map_err(io_err(&p))?;
            let stem = file.trim_end_matches(".bru");
            if let Some(r) = request(&parse_bru(&text), stem, w) {
                let mut req = r.request;
                add_path_vars(settings, &mut req, r.path_vars);
                entries.push((r.seq, req.name.to_lowercase(), PlanItem::Request(Box::new(req))));
            }
        }
    }
    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
    Ok(entries.into_iter().map(|(_, _, it)| it).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQ: &str = "meta {\n  name: Send SMS\n  type: http\n  seq: 2\n  tags: [\n    foo\n  ]\n}\n\npost {\n  url: {{baseUrl}}/send/:id?x=1\n  body: json\n  auth: bearer\n}\n\nparams:query {\n  apiKey: {{key}}\n  \"key with spaces\": ok\n  ~message: hello\n}\n\nparams:path {\n  id: 123\n}\n\nheaders {\n  content-type: application/json\n  ~transaction-id: {{transactionId}}\n}\n\nauth:bearer {\n  token: {{token}}\n}\n\nbody:json {\n  {\n    \"hello\": \"{{who}}\"\n  }\n}\n\nvars:pre-request {\n  who: world\n}\n\nvars:post-response {\n  token: res.body.token\n  @orderId: $res.body.order.id\n  weird: someFn()\n}\n\nassert {\n  res.status: eq 200\n  res.body.ok: isDefined\n  ~res.headers['x-id']: neq \"0\"\n  res.body.items: length 2\n}\n\nscript:pre-request {\n  const foo = 'bar';\n}\n\ndocs {\n  Needs a token.\n}\n";

    #[test]
    fn parses_request_file() {
        let mut w = Warnings::default();
        let r = request(&parse_bru(REQ), "x", &mut w).unwrap();
        assert_eq!(r.seq, 2.0);
        assert_eq!(r.path_vars, vec![("id".to_string(), "123".to_string())]);
        let req = r.request;
        assert_eq!(req.name, "Send SMS");
        assert_eq!(req.method, "POST");
        assert_eq!(req.url, "{{baseUrl}}/send/{{id}}");
        assert_eq!(req.params.len(), 3);
        assert_eq!(req.params[1].name, "key with spaces");
        assert!(!req.params[2].enabled);
        assert!(!req.headers[1].enabled);
        assert_eq!(req.auth, Auth::Bearer { token: "{{token}}".into(), prefix: None });
        assert_eq!(req.body, Body::Json { content: "{\n  \"hello\": \"world\"\n}".into() });
        assert_eq!(req.tests.len(), 3, "{:?}", req.tests);
        assert!(!req.tests[2].enabled);
        assert_eq!(req.tests[2].target, AssertTarget::Header { name: "x-id".into() });
        assert_eq!(req.captures.len(), 2);
        assert_eq!(req.captures[1].name, "orderId");
        assert_eq!(req.captures[1].source, CaptureSource::JsonPath { path: "$.order.id".into() });
        let docs = req.docs.unwrap();
        assert!(docs.starts_with("Needs a token."));
        assert!(docs.contains("length 2") && docs.contains("someFn()") && docs.contains("const foo"));
    }

    #[test]
    fn multiline_values_and_multipart() {
        let text = "post {\n  url: https://x.io\n  body: multipartForm\n}\n\nbody:multipart-form {\n  file: @file(a.png|b.png) @contentType(image/png)\n  note: '''\n    line 1\n    line 2\n  '''\n}\n";
        let mut w = Warnings::default();
        let req = request(&parse_bru(text), "up", &mut w).unwrap().request;
        let Body::Multipart { parts } = req.body else { panic!() };
        assert_eq!(parts[0].file.as_deref(), Some("a.png"));
        assert_eq!(parts[0].content_type.as_deref(), Some("image/png"));
        assert_eq!(parts[1].value, "line 1\nline 2");
        assert_eq!(req.name, "up");
    }

    #[test]
    fn environments() {
        let env = environment("Prod", &parse_bru("vars {\n  baseUrl: https://api.x.io\n  ~old: 1\n}\nvars:secret [\n  token,\n  apiKey\n]\n"));
        assert_eq!(env.variables.len(), 1);
        assert_eq!(env.secrets, vec!["token", "apiKey"]);
    }
}
