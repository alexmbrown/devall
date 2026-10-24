//! Project / folder → a JetBrains-compatible `.http` file, plus `http-client.env.json` (variables
//! per environment) and `http-client.private.env.json` (secret names, empty values).

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use devall_core::{ApiKeyLocation, AssertOp, AssertTarget, Assertion, Auth, Body, Capture, CaptureSource, HttpRequestFile, OAuth2Grant, Project, TreeNode};

use super::postman::simple_json_path;
use super::{ExportFile, ExportOutput, entries, file_slug, folder_context, scope, unsecret};
use crate::error::Result;

const BOUNDARY: &str = "WebAppBoundary";

pub fn to_http_file(project: &Project, folder: Option<&str>) -> Result<ExportOutput> {
    let mut warnings: Vec<String> = Vec::new();
    let sc = scope(project, folder)?;
    let list = entries(project, &sc.children, &mut warnings)?;
    let ctx = folder_context(project, &sc.root)?;
    let mut secrets: Vec<String> = Vec::new();
    let mut oauth: Vec<(String, Value)> = Vec::new();

    // File variables: project, then folders above the export root (nearest wins), then folders
    // inside it (first wins — a .http file has a single variable scope).
    let mut vars: IndexMap<String, String> = project.manifest.variables.clone();
    for fv in ctx.folder_variables.iter().rev() {
        vars.extend(fv.clone());
    }
    collect_folder_vars(project, &sc.children, &mut vars, &mut warnings);

    let mut out = format!("# {} — exported from /dev/all\n", sc.name);
    let mut var_lines = String::new();
    for (k, v) in &vars {
        if v.contains('\n') {
            warnings.push(format!("Variable “{k}” spans several lines and was not exported"));
            continue;
        }
        var_lines.push_str(&format!("@{k} = {}\n", unsecret(v, &mut secrets)));
    }
    if !var_lines.is_empty() {
        out.push('\n');
        out.push_str(&var_lines);
    }

    for e in &list {
        let req = &e.effective;
        let title = if e.folders.is_empty() { req.name.clone() } else { format!("{} / {}", e.folders.join(" / "), req.name) };
        out.push_str(&format!("\n### {title}\n"));
        if let Some(d) = req.docs.as_deref() {
            for line in d.lines().filter(|l| !l.trim().is_empty()) {
                out.push_str(&format!("# {line}\n"));
            }
        }
        if !req.settings.follow_redirects {
            out.push_str("# @no-redirect\n");
        }
        if req.settings.timeout_ms != 30_000 {
            let t = req.settings.timeout_ms;
            out.push_str(&if t.is_multiple_of(1000) { format!("# @timeout {}\n", t / 1000) } else { format!("# @timeout {t} ms\n") });
        }
        out.push_str(&request(req, &mut secrets, &mut oauth, &mut warnings));
    }

    let mut attachments = Vec::new();
    let envs = project.environments()?;
    if !envs.is_empty() || !oauth.is_empty() {
        let mut public = Map::new();
        let mut private = Map::new();
        for e in &envs {
            let mut m: Map<String, Value> = e.file.variables.iter().map(|(k, v)| (k.clone(), json!(unsecret(v, &mut secrets)))).collect();
            if !oauth.is_empty() {
                m.insert("Security".into(), json!({ "Auth": oauth.iter().cloned().collect::<Map<String, Value>>() }));
            }
            public.insert(e.file.name.clone(), Value::Object(m));
        }
        for e in &envs {
            let names: Map<String, Value> = e.file.secrets.iter().chain(secrets.iter()).map(|s| (s.clone(), json!(""))).collect();
            if !names.is_empty() {
                private.insert(e.file.name.clone(), Value::Object(names));
            }
        }
        if envs.is_empty() && !secrets.is_empty() {
            private.insert("dev".into(), Value::Object(secrets.iter().map(|s| (s.clone(), json!(""))).collect()));
        }
        if !public.is_empty() {
            attachments.push(ExportFile { file_name: "http-client.env.json".into(), content: serde_json::to_string_pretty(&Value::Object(public)).unwrap_or_default() });
        }
        if !private.is_empty() {
            attachments.push(ExportFile { file_name: "http-client.private.env.json".into(), content: serde_json::to_string_pretty(&Value::Object(private)).unwrap_or_default() });
        }
    } else if !secrets.is_empty() {
        let private = json!({ "dev": secrets.iter().map(|s| (s.clone(), json!(""))).collect::<Map<String, Value>>() });
        attachments.push(ExportFile { file_name: "http-client.private.env.json".into(), content: serde_json::to_string_pretty(&private).unwrap_or_default() });
    }
    if !secrets.is_empty() {
        warnings.push(format!("Secret values are not exported; fill them in http-client.private.env.json ({})", secrets.join(", ")));
    }
    Ok(ExportOutput { content: out, file_name: format!("{}.http", file_slug(&sc.name)), language: "http".into(), warnings, attachments })
}

fn collect_folder_vars(project: &Project, nodes: &[TreeNode], vars: &mut IndexMap<String, String>, warnings: &mut Vec<String>) {
    for n in nodes {
        if let TreeNode::Folder { path, children, name, .. } = n {
            if let Ok(f) = project.read_folder(path) {
                for (k, v) in f.variables {
                    match vars.get(&k) {
                        Some(existing) if *existing != v => warnings.push(format!("Folder “{name}” variable “{k}” conflicts with another value; kept the first")),
                        Some(_) => {}
                        None => {
                            vars.insert(k, v);
                        }
                    }
                }
            }
            collect_folder_vars(project, children, vars, warnings);
        }
    }
}

/// Percent-encodes `s` for a query string, leaving `{{templates}}` intact.
fn encode(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        out.extend(url::form_urlencoded::byte_serialize(&rest.as_bytes()[..start]));
        match rest[start..].find("}}") {
            Some(end) => {
                out.push_str(&rest[start..start + end + 2]);
                rest = &rest[start + end + 2..];
            }
            None => {
                out.extend(url::form_urlencoded::byte_serialize(&rest.as_bytes()[start..]));
                rest = "";
            }
        }
    }
    out.extend(url::form_urlencoded::byte_serialize(rest.as_bytes()));
    out
}

fn request(req: &HttpRequestFile, secrets: &mut Vec<String>, oauth: &mut Vec<(String, Value)>, warnings: &mut Vec<String>) -> String {
    let mut s = |x: &str| unsecret(x, secrets);
    let mut query: Vec<String> = req.params.iter().filter(|p| p.enabled && !p.name.is_empty()).map(|p| format!("{}={}", encode(&s(&p.name)), encode(&s(&p.value)))).collect();
    let mut headers: Vec<(String, String)> = req.headers.iter().filter(|h| h.enabled && !h.name.is_empty()).map(|h| (s(&h.name), s(&h.value))).collect();
    let has = |headers: &[(String, String)], n: &str| headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(n));
    match &req.auth {
        Auth::Basic { username, password } if !has(&headers, "authorization") => headers.push(("Authorization".into(), format!("Basic {} {}", s(username), s(password)))),
        Auth::Digest { username, password } if !has(&headers, "authorization") => headers.push(("Authorization".into(), format!("Digest {} {}", s(username), s(password)))),
        Auth::Bearer { token, prefix } if !has(&headers, "authorization") => {
            let p = prefix.as_deref().unwrap_or("Bearer");
            headers.push(("Authorization".into(), if p.is_empty() { s(token) } else { format!("{p} {}", s(token)) }));
        }
        Auth::ApiKey { key, value, location } => match location {
            ApiKeyLocation::Header => headers.push((s(key), s(value))),
            ApiKeyLocation::Query => query.push(format!("{}={}", encode(&s(key)), encode(&s(value)))),
        },
        Auth::OAuth2(c) if !has(&headers, "authorization") => {
            let grant = match c.grant {
                OAuth2Grant::ClientCredentials => "Client Credentials",
                OAuth2Grant::Password => "Password",
                _ => "Authorization Code",
            };
            let mut cfg = json!({ "Type": "OAuth2", "Grant Type": grant, "Token URL": s(&c.token_url), "Client ID": s(&c.client_id) });
            if !c.client_secret.is_empty() {
                cfg["Client Secret"] = json!(s(&c.client_secret));
            }
            if !c.auth_url.is_empty() {
                cfg["Auth URL"] = json!(s(&c.auth_url));
            }
            if !c.scope.is_empty() {
                cfg["Scope"] = json!(s(&c.scope));
            }
            if c.grant == OAuth2Grant::AuthorizationCode {
                cfg["PKCE"] = json!(c.use_pkce);
                if let Some(port) = c.redirect_port {
                    cfg["Redirect URL"] = json!(format!("http://localhost:{port}/callback"));
                }
            }
            if c.grant == OAuth2Grant::Password {
                cfg["Username"] = json!(s(&c.username));
                cfg["Password"] = json!(s(&c.password));
            }
            let id = match oauth.iter().find(|(_, v)| *v == cfg) {
                Some((id, _)) => id.clone(),
                None => {
                    let id = if oauth.is_empty() { "oauth2".to_string() } else { format!("oauth2-{}", oauth.len() + 1) };
                    oauth.push((id.clone(), cfg));
                    id
                }
            };
            headers.push(("Authorization".into(), format!("Bearer {{{{$auth.token(\"{id}\")}}}}")));
        }
        Auth::AwsSigV4 { .. } => warnings.push(format!("“{}”: AWS SigV4 auth cannot be expressed in a .http file; exported without auth", req.name)),
        Auth::Ntlm { .. } => warnings.push(format!("“{}”: NTLM auth cannot be expressed in a .http file; exported without auth", req.name)),
        Auth::Plugin { plugin, .. } => warnings.push(format!("“{}”: plugin auth ({plugin}) cannot be expressed in a .http file; exported without auth", req.name)),
        _ => {}
    }

    let url = s(req.url.trim());
    let full_url = if query.is_empty() { url } else { format!("{url}{}{}", if url.contains('?') { "&" } else { "?" }, query.join("&")) };
    let mut method = req.method.to_uppercase();
    let mut body = String::new();
    let mut ct: Option<String> = None;
    match &req.body {
        Body::Json { content } => {
            ct = Some("application/json".into());
            body = s(content);
        }
        Body::Xml { content } => {
            ct = Some("application/xml".into());
            body = s(content);
        }
        Body::Text { content, content_type } => {
            ct = Some(content_type.clone().unwrap_or_else(|| "text/plain".into()));
            body = s(content);
        }
        Body::Form { fields } => {
            ct = Some("application/x-www-form-urlencoded".into());
            body = fields.iter().filter(|f| f.enabled && !f.name.is_empty()).map(|f| format!("{}={}", encode(&s(&f.name)), encode(&s(&f.value)))).collect::<Vec<_>>().join("&");
        }
        Body::Multipart { parts } => {
            headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
            ct = Some(format!("multipart/form-data; boundary={BOUNDARY}"));
            for p in parts.iter().filter(|p| p.enabled && !p.name.is_empty()) {
                body.push_str(&format!("--{BOUNDARY}\n"));
                match &p.file {
                    Some(f) => {
                        let fname = crate::util::file_name_of(f);
                        body.push_str(&format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{fname}\"\n", s(&p.name)));
                        if let Some(c) = &p.content_type {
                            body.push_str(&format!("Content-Type: {c}\n"));
                        }
                        body.push_str(&format!("\n< {}\n", s(f)));
                    }
                    None => {
                        body.push_str(&format!("Content-Disposition: form-data; name=\"{}\"\n", s(&p.name)));
                        if let Some(c) = &p.content_type {
                            body.push_str(&format!("Content-Type: {c}\n"));
                        }
                        body.push_str(&format!("\n{}\n", s(&p.value)));
                    }
                }
            }
            body.push_str(&format!("--{BOUNDARY}--"));
        }
        Body::Graphql { query, variables, .. } => {
            method = "GRAPHQL".into();
            body = s(query);
            if !variables.trim().is_empty() {
                body.push_str(&format!("\n\n{}", s(variables)));
            }
        }
        Body::Binary { path, content_type } => {
            ct = Some(content_type.clone().unwrap_or_else(|| "application/octet-stream".into()));
            body = format!("< {}", s(path));
        }
        _ => {}
    }
    if let Some(ct) = ct
        && !has(&headers, "content-type")
    {
        headers.push(("Content-Type".into(), ct));
    }
    if body.lines().any(|l| l.starts_with("###")) {
        warnings.push(format!("“{}”: its body contains a line starting with ### which .http clients read as a separator", req.name));
    }

    let mut out = format!("{method} {full_url}\n");
    for (k, v) in &headers {
        out.push_str(&format!("{k}: {v}\n"));
    }
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body.trim_end());
        out.push('\n');
    }
    let handler = handler(&req.tests, &req.captures);
    if !handler.is_empty() {
        out.push_str(&format!("\n> {{%\n{}\n%}}\n", handler.join("\n")));
    }
    out
}

/// Assertions and captures → a JetBrains response handler (common cases; others as comments).
fn handler(tests: &[Assertion], captures: &[Capture]) -> Vec<String> {
    let mut out = Vec::new();
    let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let lit = |v: &str| if v.parse::<f64>().is_ok() || v == "true" || v == "false" || v == "null" { v.to_string() } else { q(v) };
    for t in tests.iter().filter(|t| t.enabled) {
        let v = t.value.clone().unwrap_or_default();
        let subject = match &t.target {
            AssertTarget::Status => "response.status".to_string(),
            AssertTarget::Header { name } => format!("response.headers.valueOf({})", q(name)),
            AssertTarget::ContentType => "response.contentType.mimeType".to_string(),
            AssertTarget::JsonPath { path } => match simple_json_path(path) {
                Some(rest) => format!("response.body{rest}"),
                None => {
                    out.push(format!("    // Not converted: jsonPath {path} {:?} {v}", t.op));
                    continue;
                }
            },
            AssertTarget::Body => "response.body".to_string(),
            AssertTarget::ResponseTime | AssertTarget::Size => {
                out.push(format!("    // Not converted: {:?} {:?} {v}", t.target, t.op));
                continue;
            }
            AssertTarget::Snapshot { file, .. } => {
                out.push(format!("    // Not converted: response snapshot {file}"));
                continue;
            }
        };
        let cond = match t.op {
            AssertOp::Eq => format!("{subject} == {}", lit(&v)),
            AssertOp::Ne => format!("{subject} != {}", lit(&v)),
            AssertOp::Lt => format!("{subject} < {}", lit(&v)),
            AssertOp::Lte => format!("{subject} <= {}", lit(&v)),
            AssertOp::Gt => format!("{subject} > {}", lit(&v)),
            AssertOp::Gte => format!("{subject} >= {}", lit(&v)),
            AssertOp::Contains => format!("String({subject}).includes({})", q(&v)),
            AssertOp::Exists => format!("{subject} !== undefined && {subject} !== null"),
            AssertOp::NotExists => format!("{subject} === undefined || {subject} === null"),
            _ => {
                out.push(format!("    // Not converted: {subject} {:?} {v}", t.op));
                continue;
            }
        };
        out.push(format!("    client.test({}, function () {{ client.assert({cond}, {}); }});", q(&format!("{subject} {:?} {v}", t.op)), q(&cond)));
    }
    for c in captures.iter().filter(|c| c.enabled) {
        let expr = match &c.source {
            CaptureSource::Status => "response.status".to_string(),
            CaptureSource::Body => "response.body".to_string(),
            CaptureSource::Header { header } => format!("response.headers.valueOf({})", q(header)),
            CaptureSource::JsonPath { path } => match simple_json_path(path) {
                Some(rest) => format!("response.body{rest}"),
                None => {
                    out.push(format!("    // Not converted: capture {} from {path}", c.name));
                    continue;
                }
            },
            CaptureSource::Cookie { .. } | CaptureSource::Regex { .. } => {
                out.push(format!("    // Not converted: capture {}", c.name));
                continue;
            }
        };
        out.push(format!("    client.global.set({}, {expr});", q(&c.name)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_around_templates() {
        assert_eq!(encode("a b&{{x y}}/c"), "a+b%26{{x y}}%2Fc");
    }
}
