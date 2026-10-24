//! Project / folder → Postman Collection v2.1 (+ one environment file per project environment).

use serde_json::{Map, Value, json};

use devall_core::{ApiKeyLocation, AssertOp, AssertTarget, Assertion, Auth, Body, Capture, CaptureSource, HttpRequestFile, OAuth2ClientAuth, OAuth2Grant, Project, TreeNode};

use super::{ExportFile, ExportOutput, file_slug, folder_context, scope, unsecret};
use crate::error::Result;
use crate::util::{map_templates, split_origin};

const SCHEMA: &str = "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

pub fn to_postman(project: &Project, folder: Option<&str>) -> Result<ExportOutput> {
    let mut warnings: Vec<String> = Vec::new();
    let sc = scope(project, folder)?;
    let ctx = folder_context(project, &sc.root)?;
    let mut secrets: Vec<String> = Vec::new();

    let mut collection = Map::new();
    let mut info = Map::new();
    info.insert("name".into(), json!(sc.name));
    if sc.root.is_empty()
        && let Some(d) = project.manifest.description.as_deref().filter(|d| !d.trim().is_empty())
    {
        info.insert("description".into(), json!(d));
    }
    info.insert("schema".into(), json!(SCHEMA));
    collection.insert("info".into(), Value::Object(info));
    collection.insert("item".into(), Value::Array(items(project, &sc.children, &mut warnings)?));
    if let Some(a) = auth(&ctx.request.auth) {
        collection.insert("auth".into(), a);
    }
    // Collection variables: project variables, then folder variables (nearest folder wins).
    let mut vars: Vec<(String, String)> = project.manifest.variables.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for fv in ctx.folder_variables.iter().rev() {
        for (k, v) in fv {
            vars.retain(|(x, _)| x != k);
            vars.push((k.clone(), v.clone()));
        }
    }
    collect_folder_vars(project, &sc.children, &mut vars, &mut warnings);
    if !vars.is_empty() {
        collection.insert("variable".into(), Value::Array(vars.iter().map(|(k, v)| json!({ "key": k, "value": v })).collect()));
    }

    let mut doc = Value::Object(collection);
    rewrite_strings(&mut doc, &mut |s| postman_templates(s, &mut secrets));

    let mut attachments = Vec::new();
    let envs = project.environments()?;
    // Values first (they may reference more secrets), then the secret declarations.
    let mut env_values: Vec<Vec<Value>> = envs
        .iter()
        .map(|e| e.file.variables.iter().map(|(k, v)| json!({ "key": k, "value": postman_templates(v, &mut secrets), "type": "default", "enabled": true })).collect())
        .collect();
    for (e, values) in envs.iter().zip(env_values.iter_mut()) {
        for s in e.file.secrets.iter().chain(secrets.iter()) {
            if !values.iter().any(|x| x["key"] == json!(s)) {
                values.push(json!({ "key": s, "value": "", "type": "secret", "enabled": true }));
            }
        }
        let env_doc = json!({ "name": e.file.name, "values": values, "_postman_variable_scope": "environment", "_postman_exported_using": "/dev/all" });
        attachments.push(ExportFile { file_name: format!("{}.postman_environment.json", file_slug(&e.file.name)), content: serde_json::to_string_pretty(&env_doc).unwrap_or_default() });
    }
    if !secrets.is_empty() {
        if envs.is_empty() {
            // Nowhere else to declare them: empty collection variables.
            if let Some(list) = doc.get_mut("variable").and_then(Value::as_array_mut) {
                for s in &secrets {
                    list.push(json!({ "key": s, "value": "" }));
                }
            } else {
                doc["variable"] = Value::Array(secrets.iter().map(|s| json!({ "key": s, "value": "" })).collect());
            }
        }
        warnings.push(format!("Secret values are not exported; {} secret(s) are declared empty: {}", secrets.len(), secrets.join(", ")));
    }
    Ok(ExportOutput {
        content: serde_json::to_string_pretty(&doc).unwrap_or_default(),
        file_name: format!("{}.postman_collection.json", file_slug(&sc.name)),
        language: "json".into(),
        warnings,
        attachments,
    })
}

fn collect_folder_vars(project: &Project, nodes: &[TreeNode], vars: &mut Vec<(String, String)>, warnings: &mut Vec<String>) {
    for n in nodes {
        if let TreeNode::Folder { path, children, name, .. } = n {
            if let Ok(f) = project.read_folder(path) {
                for (k, v) in f.variables {
                    match vars.iter().find(|(x, _)| *x == k) {
                        Some((_, existing)) if *existing != v => warnings.push(format!("Folder “{name}” variable “{k}” conflicts with another value; kept the first")),
                        Some(_) => {}
                        None => vars.push((k, v)),
                    }
                }
            }
            collect_folder_vars(project, children, vars, warnings);
        }
    }
}

/// /dev/all templates → Postman: `{{secret.X}}` → `{{X}}`, dynamic values renamed.
fn postman_templates(s: &str, secrets: &mut Vec<String>) -> String {
    let s = unsecret(s, secrets);
    map_templates(&s, |k| {
        Some(match k {
            "$uuid" | "$uuidv7" | "$guid" | "$randomUUID" => "{{$guid}}".to_string(),
            "$timestampMs" => "{{$timestamp}}000".to_string(),
            "$ulid" => "{{$guid}}".to_string(),
            _ if k.starts_with("$env.") => format!("{{{{{}}}}}", &k[5..]),
            _ => return None,
        })
    })
}

fn rewrite_strings(v: &mut Value, f: &mut dyn FnMut(&str) -> String) {
    match v {
        Value::String(s) if s.contains("{{") => *s = f(s),
        Value::Array(a) => a.iter_mut().for_each(|x| rewrite_strings(x, f)),
        Value::Object(m) => m.values_mut().for_each(|x| rewrite_strings(x, f)),
        _ => {}
    }
}

fn items(project: &Project, nodes: &[TreeNode], warnings: &mut Vec<String>) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for n in nodes {
        match n {
            TreeNode::Folder { name, path, children, .. } => {
                let mut f = Map::new();
                f.insert("name".into(), json!(name));
                f.insert("item".into(), Value::Array(items(project, children, warnings)?));
                if let Ok(folder) = project.read_folder(path)
                    && let Some(a) = auth(&folder.auth)
                {
                    f.insert("auth".into(), a);
                }
                out.push(Value::Object(f));
            }
            TreeNode::Request { name, path, error, .. } => {
                if error.is_some() || !path.ends_with(devall_core::project::REQUEST_EXT) {
                    warnings.push(format!("“{name}” is not an HTTP request (or unreadable); skipped"));
                    continue;
                }
                let raw = project.read_request(path)?;
                let eff = project.effective_request(path, &raw)?.request;
                out.push(request_item(&raw, &eff));
            }
        }
    }
    Ok(out)
}

fn request_item(raw: &HttpRequestFile, eff: &HttpRequestFile) -> Value {
    let mut r = Map::new();
    r.insert("method".into(), json!(raw.method.to_uppercase()));
    // Postman has no folder headers: inherited ones are pushed down into each request.
    r.insert(
        "header".into(),
        Value::Array(
            eff.headers
                .iter()
                .map(|h| {
                    let mut x = json!({ "key": h.name, "value": h.value, "type": "text" });
                    if !h.enabled {
                        x["disabled"] = json!(true);
                    }
                    if let Some(d) = &h.description {
                        x["description"] = json!(d);
                    }
                    x
                })
                .collect(),
        ),
    );
    if let Some(b) = body(&raw.body) {
        r.insert("body".into(), b);
    }
    r.insert("url".into(), url(raw));
    if let Some(a) = auth(&raw.auth) {
        r.insert("auth".into(), a);
    }
    if let Some(d) = raw.docs.as_deref().filter(|d| !d.trim().is_empty()) {
        r.insert("description".into(), json!(d));
    }
    let mut item = Map::new();
    item.insert("name".into(), json!(raw.name));
    let script = test_script(&raw.tests, &raw.captures);
    if !script.is_empty() {
        item.insert("event".into(), json!([{ "listen": "test", "script": { "type": "text/javascript", "exec": script } }]));
    }
    let mut proto = Map::new();
    if !raw.settings.follow_redirects {
        proto.insert("followRedirects".into(), json!(false));
    }
    if !raw.settings.verify_tls {
        proto.insert("strictSSL".into(), json!(false));
    }
    if !proto.is_empty() {
        item.insert("protocolProfileBehavior".into(), Value::Object(proto));
    }
    item.insert("request".into(), Value::Object(r));
    item.insert("response".into(), json!([]));
    Value::Object(item)
}

fn url(req: &HttpRequestFile) -> Value {
    let base = req.url.trim();
    let enabled: Vec<String> = req.params.iter().filter(|p| p.enabled && !p.name.is_empty()).map(|p| if p.value.is_empty() { p.name.clone() } else { format!("{}={}", p.name, p.value) }).collect();
    let raw = if enabled.is_empty() { base.to_string() } else { format!("{base}{}{}", if base.contains('?') { "&" } else { "?" }, enabled.join("&")) };
    let mut u = Map::new();
    u.insert("raw".into(), json!(raw));
    let (origin, path) = split_origin(base);
    let path = path.split(['?', '#']).next().unwrap_or("");
    if let Some((proto, host)) = origin.split_once("://") {
        u.insert("protocol".into(), json!(proto));
        let (host, port) = match host.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
            _ => (host, None),
        };
        u.insert("host".into(), json!(host.split('.').collect::<Vec<_>>()));
        if let Some(p) = port {
            u.insert("port".into(), json!(p));
        }
    } else if !origin.is_empty() {
        u.insert("host".into(), json!([origin]));
    }
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if !segs.is_empty() {
        u.insert("path".into(), json!(segs));
    }
    if !req.params.is_empty() {
        u.insert(
            "query".into(),
            Value::Array(
                req.params
                    .iter()
                    .map(|p| {
                        let mut q = json!({ "key": p.name, "value": p.value });
                        if !p.enabled {
                            q["disabled"] = json!(true);
                        }
                        if let Some(d) = &p.description {
                            q["description"] = json!(d);
                        }
                        q
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(u)
}

fn kv_list(pairs: &[(&str, String)]) -> Value {
    Value::Array(pairs.iter().map(|(k, v)| json!({ "key": k, "value": v, "type": "string" })).collect())
}

fn auth(a: &Auth) -> Option<Value> {
    Some(match a {
        // Postman has no equivalent of a /dev/all plugin auth provider.
        Auth::Inherit | Auth::Plugin { .. } => return None,
        Auth::None => json!({ "type": "noauth" }),
        Auth::Basic { username, password } => json!({ "type": "basic", "basic": kv_list(&[("username", username.clone()), ("password", password.clone())]) }),
        Auth::Digest { username, password } => json!({ "type": "digest", "digest": kv_list(&[("username", username.clone()), ("password", password.clone())]) }),
        Auth::Ntlm { username, password, domain, workstation } => json!({
            "type": "ntlm",
            "ntlm": kv_list(&[
                ("username", username.clone()),
                ("password", password.clone()),
                ("domain", domain.clone().unwrap_or_default()),
                ("workstation", workstation.clone().unwrap_or_default()),
            ])
        }),
        Auth::Bearer { token, .. } => json!({ "type": "bearer", "bearer": kv_list(&[("token", token.clone())]) }),
        Auth::ApiKey { key, value, location } => json!({
            "type": "apikey",
            "apikey": kv_list(&[("key", key.clone()), ("value", value.clone()), ("in", if *location == ApiKeyLocation::Query { "query".into() } else { "header".into() })])
        }),
        Auth::AwsSigV4 { access_key_id, secret_access_key, session_token, region, service } => json!({
            "type": "awsv4",
            "awsv4": kv_list(&[
                ("accessKey", access_key_id.clone()),
                ("secretKey", secret_access_key.clone()),
                ("sessionToken", session_token.clone().unwrap_or_default()),
                ("region", region.clone()),
                ("service", service.clone()),
            ])
        }),
        Auth::OAuth2(c) => {
            let grant = match c.grant {
                OAuth2Grant::ClientCredentials => "client_credentials",
                OAuth2Grant::Password => "password_credentials",
                _ if c.use_pkce => "authorization_code_with_pkce",
                _ => "authorization_code",
            };
            let mut pairs = vec![
                ("grant_type", grant.to_string()),
                ("authUrl", c.auth_url.clone()),
                ("accessTokenUrl", c.token_url.clone()),
                ("clientId", c.client_id.clone()),
                ("clientSecret", c.client_secret.clone()),
                ("scope", c.scope.clone()),
                ("client_authentication", if c.client_auth == OAuth2ClientAuth::Body { "body".into() } else { "header".into() }),
                ("addTokenTo", "header".into()),
            ];
            if c.grant == OAuth2Grant::Password {
                pairs.push(("username", c.username.clone()));
                pairs.push(("password", c.password.clone()));
            }
            if let Some(p) = &c.token_prefix {
                pairs.push(("headerPrefix", p.clone()));
            }
            if let Some(port) = c.redirect_port {
                pairs.push(("redirect_uri", format!("http://localhost:{port}/callback")));
            }
            json!({ "type": "oauth2", "oauth2": kv_list(&pairs) })
        }
    })
}

fn body(b: &Body) -> Option<Value> {
    Some(match b {
        Body::Json { content } => json!({ "mode": "raw", "raw": content, "options": { "raw": { "language": "json" } } }),
        Body::Xml { content } => json!({ "mode": "raw", "raw": content, "options": { "raw": { "language": "xml" } } }),
        Body::Text { content, content_type } => {
            let lang = match content_type.as_deref() {
                Some(c) if c.contains("html") => "html",
                Some(c) if c.contains("javascript") => "javascript",
                _ => "text",
            };
            json!({ "mode": "raw", "raw": content, "options": { "raw": { "language": lang } } })
        }
        Body::Form { fields } => json!({
            "mode": "urlencoded",
            "urlencoded": fields.iter().map(|f| {
                let mut x = json!({ "key": f.name, "value": f.value, "type": "text" });
                if !f.enabled { x["disabled"] = json!(true); }
                x
            }).collect::<Vec<_>>()
        }),
        Body::Multipart { parts } => json!({
            "mode": "formdata",
            "formdata": parts.iter().map(|p| {
                let mut x = match &p.file {
                    Some(f) => json!({ "key": p.name, "type": "file", "src": f }),
                    None => json!({ "key": p.name, "value": p.value, "type": "text" }),
                };
                if let Some(ct) = &p.content_type { x["contentType"] = json!(ct); }
                if !p.enabled { x["disabled"] = json!(true); }
                x
            }).collect::<Vec<_>>()
        }),
        Body::Graphql { query, variables, .. } => json!({ "mode": "graphql", "graphql": { "query": query, "variables": variables } }),
        Body::Binary { path, .. } => json!({ "mode": "file", "file": { "src": path } }),
        _ => return None,
    })
}

/// `$.a.b[0]['c']` → `.a.b[0]['c']` when the path only uses names and indexes.
pub(crate) fn simple_json_path(path: &str) -> Option<String> {
    let rest = path.trim().strip_prefix('$')?;
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                let mut n = 0;
                while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$') {
                    chars.next();
                    n += 1;
                }
                if n == 0 {
                    return None;
                }
            }
            '[' => {
                let mut inner = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    inner.push(c);
                }
                let quoted = (inner.starts_with('\'') && inner.ends_with('\'')) || (inner.starts_with('"') && inner.ends_with('"'));
                if !(quoted && inner.len() >= 2 || (!inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()))) {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(rest.to_string())
}

fn js_literal(v: &str) -> String {
    if v.parse::<f64>().is_ok() || v == "true" || v == "false" || v == "null" { v.to_string() } else { serde_json::to_string(v).unwrap_or_default() }
}

/// Assertions and captures → a Postman test script (the common cases; others as comments).
fn test_script(tests: &[Assertion], captures: &[Capture]) -> Vec<String> {
    let mut out = Vec::new();
    for t in tests.iter().filter(|t| t.enabled) {
        let v = t.value.clone().unwrap_or_default();
        let subject = match &t.target {
            AssertTarget::Status if t.op == AssertOp::Eq => {
                out.push(format!("pm.test(\"Status is {v}\", () => pm.response.to.have.status({}));", js_literal(&v)));
                continue;
            }
            AssertTarget::Status => "pm.response.code".to_string(),
            AssertTarget::Header { name } => format!("pm.response.headers.get({})", serde_json::to_string(name).unwrap_or_default()),
            AssertTarget::ContentType => "pm.response.headers.get(\"Content-Type\")".to_string(),
            AssertTarget::JsonPath { path } => match simple_json_path(path) {
                Some(rest) => format!("pm.response.json(){rest}"),
                None => {
                    out.push(format!("// Not converted: jsonPath {path} {:?} {v}", t.op));
                    continue;
                }
            },
            AssertTarget::Body => "pm.response.text()".to_string(),
            AssertTarget::ResponseTime => "pm.response.responseTime".to_string(),
            AssertTarget::Size => "pm.response.responseSize".to_string(),
            AssertTarget::Snapshot { file, .. } => {
                out.push(format!("// Not converted: response snapshot {file}"));
                continue;
            }
        };
        let lit = js_literal(&v);
        let chai = match t.op {
            AssertOp::Eq => format!("to.eql({lit})"),
            AssertOp::Ne => format!("to.not.eql({lit})"),
            AssertOp::Lt => format!("to.be.below({lit})"),
            AssertOp::Lte => format!("to.be.at.most({lit})"),
            AssertOp::Gt => format!("to.be.above({lit})"),
            AssertOp::Gte => format!("to.be.at.least({lit})"),
            AssertOp::Contains => format!("to.include({lit})"),
            AssertOp::NotContains => format!("to.not.include({lit})"),
            AssertOp::StartsWith => format!("to.match(new RegExp(\"^\" + {lit}))"),
            AssertOp::Matches => format!("to.match(new RegExp({}))", serde_json::to_string(&v).unwrap_or_default()),
            AssertOp::Exists => "to.not.be.undefined".to_string(),
            AssertOp::NotExists => "to.be.undefined".to_string(),
            AssertOp::IsEmpty => "to.be.empty".to_string(),
            AssertOp::IsNotEmpty => "to.not.be.empty".to_string(),
            AssertOp::IsType => format!("to.be.a({})", serde_json::to_string(&v).unwrap_or_default()),
            AssertOp::OneOf => format!("to.be.oneOf({})", serde_json::to_string(&v.split(',').map(str::trim).collect::<Vec<_>>()).unwrap_or_default()),
            _ => {
                out.push(format!("// Not converted: {subject} {:?} {v}", t.op));
                continue;
            }
        };
        out.push(format!("pm.test({}, () => pm.expect({subject}).{chai});", serde_json::to_string(&format!("{subject} {:?} {v}", t.op)).unwrap_or_default()));
    }
    for c in captures.iter().filter(|c| c.enabled) {
        let expr = match &c.source {
            CaptureSource::Status => "pm.response.code".to_string(),
            CaptureSource::Body => "pm.response.text()".to_string(),
            CaptureSource::Header { header } => format!("pm.response.headers.get({})", serde_json::to_string(header).unwrap_or_default()),
            CaptureSource::Cookie { cookie } => format!("pm.cookies.get({})", serde_json::to_string(cookie).unwrap_or_default()),
            CaptureSource::JsonPath { path } => match simple_json_path(path) {
                Some(rest) => format!("pm.response.json(){rest}"),
                None => {
                    out.push(format!("// Not converted: capture {} from {path}", c.name));
                    continue;
                }
            },
            CaptureSource::Regex { pattern, group } => format!("(pm.response.text().match(new RegExp({})) || [])[{group}]", serde_json::to_string(pattern).unwrap_or_default()),
        };
        out.push(format!("pm.collectionVariables.set({}, {expr});", serde_json::to_string(&c.name).unwrap_or_default()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_paths() {
        assert_eq!(simple_json_path("$.a.b[0]['c d']").as_deref(), Some(".a.b[0]['c d']"));
        assert_eq!(simple_json_path("$"), Some(String::new()));
        assert!(simple_json_path("$..a").is_none());
        assert!(simple_json_path("$.items[?(@.x)]").is_none());
    }

    #[test]
    fn templates() {
        let mut s = Vec::new();
        assert_eq!(postman_templates("{{secret.TOKEN}}/{{$uuid}}/{{$env.HOME}}", &mut s), "{{TOKEN}}/{{$guid}}/{{HOME}}");
        assert_eq!(s, vec!["TOKEN"]);
    }
}
