//! Project / folder → a Hurl file (<https://hurl.dev>), plus one `--variables-file` per
//! environment. Assertions and captures map back to `[Asserts]` / `[Captures]` where Hurl has an
//! equivalent; the rest become comments. Run it with
//! `hurl --test --variables-file <env>.hurl.env api.hurl`.

use indexmap::IndexMap;

use devall_core::{ApiKeyLocation, AssertOp, AssertTarget, Assertion, Auth, Body, Capture, CaptureSource, HttpRequestFile, Project, TreeNode};

use super::{ExportFile, ExportOutput, entries, file_slug, folder_context, scope, unsecret};
use crate::error::Result;

pub fn to_hurl(project: &Project, folder: Option<&str>) -> Result<ExportOutput> {
    let mut warnings: Vec<String> = Vec::new();
    let sc = scope(project, folder)?;
    let list = entries(project, &sc.children, &mut warnings)?;
    let ctx = folder_context(project, &sc.root)?;
    let mut secrets: Vec<String> = Vec::new();

    // Variables: project, folders above the export root (nearest wins), folders inside it.
    let mut vars: IndexMap<String, String> = project.manifest.variables.clone();
    for fv in ctx.folder_variables.iter().rev() {
        vars.extend(fv.clone());
    }
    collect_folder_vars(project, &sc.children, &mut vars);

    let mut out = format!("# {} — exported from /dev/all\n# Run: hurl --test --variables-file <environment>.hurl.env {}.hurl\n", sc.name, file_slug(&sc.name));
    for e in &list {
        let title = if e.folders.is_empty() { e.effective.name.clone() } else { format!("{} / {}", e.folders.join(" / "), e.effective.name) };
        out.push_str(&format!("\n# {title}\n"));
        if let Some(d) = e.effective.docs.as_deref() {
            for line in d.lines().filter(|l| !l.trim().is_empty()).take(20) {
                out.push_str(&format!("# {line}\n"));
            }
        }
        out.push_str(&entry(&e.effective, &mut secrets, &mut warnings));
    }

    // Variables files: shared variables + the environment's own, secrets as empty comments.
    let mut attachments = Vec::new();
    let envs = project.environments()?;
    let render = |extra: &IndexMap<String, String>, env_secrets: &[String], secrets: &mut Vec<String>| {
        let mut merged = vars.clone();
        merged.extend(extra.clone());
        let mut text = String::new();
        for (k, v) in &merged {
            if v.contains('\n') {
                continue;
            }
            text.push_str(&format!("{k}={}\n", unsecret(v, secrets)));
        }
        let mut names: Vec<&String> = env_secrets.iter().chain(secrets.iter()).collect();
        names.dedup();
        if !names.is_empty() {
            text.push_str("# Secrets: pass them with --secret NAME=value (or --variable)\n");
            for n in names {
                text.push_str(&format!("# {n}=\n"));
            }
        }
        text
    };
    if envs.is_empty() {
        if !vars.is_empty() || !secrets.is_empty() {
            let mut s = secrets.clone();
            attachments.push(ExportFile { file_name: "vars.hurl.env".into(), content: render(&IndexMap::new(), &[], &mut s) });
        }
    } else {
        for e in &envs {
            let mut s = secrets.clone();
            attachments.push(ExportFile { file_name: format!("{}.hurl.env", file_slug(&e.file.name)), content: render(&e.file.variables, &e.file.secrets, &mut s) });
        }
    }
    if !secrets.is_empty() {
        warnings.push(format!("Secret values are not exported; pass them with --secret ({})", secrets.join(", ")));
    }
    Ok(ExportOutput { content: out, file_name: format!("{}.hurl", file_slug(&sc.name)), language: "hurl".into(), warnings, attachments })
}

fn collect_folder_vars(project: &Project, nodes: &[TreeNode], vars: &mut IndexMap<String, String>) {
    for n in nodes {
        if let TreeNode::Folder { path, children, .. } = n {
            if let Ok(f) = project.read_folder(path) {
                for (k, v) in f.variables {
                    vars.entry(k).or_insert(v);
                }
            }
            collect_folder_vars(project, children, vars);
        }
    }
}

/// Hurl key/value text: `#` starts a comment, so it is escaped (and so are backslashes).
fn kv(s: &str) -> String {
    s.replace('\\', "\\\\").replace('#', "\\#")
}

/// A quoted Hurl string (`"a \"b\""`).
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A predicate value: numbers, booleans and null bare, the rest quoted.
fn literal(v: &str) -> String {
    let t = v.trim();
    if t == "true" || t == "false" || t == "null" || (t.parse::<f64>().is_ok() && !t.starts_with('+') && !t.starts_with('.')) || (t.starts_with("{{") && t.ends_with("}}")) {
        t.to_string()
    } else {
        quoted(v)
    }
}

fn fenced(lang: &str, content: &str) -> String {
    let body = content.trim_end_matches(['\r', '\n']);
    if body.contains("```") {
        // A fence inside the body would end the block early: use a one-line JSON string body.
        return format!("{}\n", quoted(body));
    }
    format!("```{lang}\n{body}\n```\n")
}

/// One Hurl entry (request + optional response section) for an effective request.
pub(crate) fn entry(req: &HttpRequestFile, secrets: &mut Vec<String>, warnings: &mut Vec<String>) -> String {
    let mut s = |x: &str| unsecret(x, secrets);
    let mut out = String::new();
    let mut headers: Vec<(String, String)> = req.headers.iter().filter(|h| h.enabled && !h.name.is_empty()).map(|h| (s(&h.name), s(&h.value))).collect();
    let mut query: Vec<(String, String)> = req.params.iter().filter(|p| p.enabled && !p.name.is_empty()).map(|p| (s(&p.name), s(&p.value))).collect();
    let mut options: Vec<(String, String)> = Vec::new();
    let mut basic: Option<(String, String)> = None;
    let has = |h: &[(String, String)], n: &str| h.iter().any(|(k, _)| k.eq_ignore_ascii_case(n));
    match &req.auth {
        Auth::Basic { username, password } => basic = Some((s(username), s(password))),
        Auth::Bearer { token, prefix } if !has(&headers, "authorization") => {
            let p = prefix.as_deref().unwrap_or("Bearer");
            headers.push(("Authorization".into(), if p.is_empty() { s(token) } else { format!("{p} {}", s(token)) }));
        }
        Auth::ApiKey { key, value, location } => match location {
            ApiKeyLocation::Header => headers.push((s(key), s(value))),
            ApiKeyLocation::Query => query.push((s(key), s(value))),
        },
        Auth::Digest { username, password } => {
            options.push(("digest".into(), "true".into()));
            options.push(("user".into(), format!("{}:{}", s(username), s(password))));
        }
        Auth::Ntlm { username, password, domain, .. } => {
            let user = match domain.as_deref().filter(|d| !d.is_empty()) {
                Some(d) => format!("{}\\{}", s(d), s(username)),
                None => s(username),
            };
            options.push(("ntlm".into(), "true".into()));
            options.push(("user".into(), format!("{user}:{}", s(password))));
        }
        Auth::AwsSigV4 { access_key_id, secret_access_key, region, service, .. } => {
            options.push(("aws-sigv4".into(), format!("aws:amz:{}:{}", s(region), s(service))));
            options.push(("user".into(), format!("{}:{}", s(access_key_id), s(secret_access_key))));
        }
        Auth::OAuth2(_) if !has(&headers, "authorization") => {
            warnings.push(format!("“{}”: OAuth 2.0 can't run inside Hurl; it sends Bearer {{{{access_token}}}} (pass --variable access_token=…)", req.name));
            headers.push(("Authorization".into(), "Bearer {{access_token}}".into()));
        }
        _ => {}
    }
    if req.settings.follow_redirects {
        options.push(("location".into(), "true".into()));
    }
    if !req.settings.verify_tls {
        options.push(("insecure".into(), "true".into()));
    }
    if let Some(p) = req.settings.proxy.as_deref().filter(|p| !p.is_empty() && *p != "none") {
        options.push(("proxy".into(), s(p)));
    }

    // Body (decides the content type header too).
    let mut body = String::new();
    let mut form: Vec<(String, String)> = Vec::new();
    let mut multipart: Vec<String> = Vec::new();
    let mut method = req.method.to_uppercase();
    let mut ct: Option<String> = None;
    match &req.body {
        Body::None => {}
        Body::Json { content } => {
            ct = Some("application/json".into());
            body = fenced("json", &s(content));
        }
        Body::Xml { content } => {
            ct = Some("application/xml".into());
            body = fenced("xml", &s(content));
        }
        Body::Text { content, content_type } => {
            ct = Some(content_type.clone().unwrap_or_else(|| "text/plain".into()));
            body = fenced("", &s(content));
        }
        Body::Form { fields } => form = fields.iter().filter(|f| f.enabled && !f.name.is_empty()).map(|f| (s(&f.name), s(&f.value))).collect(),
        Body::Multipart { parts } => {
            for p in parts.iter().filter(|p| p.enabled && !p.name.is_empty()) {
                match &p.file {
                    Some(f) => {
                        let ctype = p.content_type.as_deref().map(|c| format!(" {c}")).unwrap_or_default();
                        multipart.push(format!("{}: file,{};{ctype}", kv(&s(&p.name)), s(f).replace('\\', "/")));
                    }
                    None => multipart.push(format!("{}: {}", kv(&s(&p.name)), kv(&s(&p.value)))),
                }
            }
        }
        Body::Graphql { query, variables, .. } => {
            if method == "GRAPHQL" {
                method = "POST".into();
            }
            let mut g = s(query).trim_end().to_string();
            if !variables.trim().is_empty() {
                g.push_str(&format!("\nvariables {}", s(variables).trim()));
            }
            body = fenced("graphql", &g);
        }
        Body::Binary { path, content_type } => {
            ct = content_type.clone();
            body = format!("file,{};\n", s(path).replace('\\', "/"));
        }
    }
    if let Some(ct) = ct
        && !has(&headers, "content-type")
    {
        headers.push(("Content-Type".into(), ct));
    }

    out.push_str(&format!("{method} {}\n", s(req.url.trim())));
    for (k, v) in &headers {
        out.push_str(&format!("{}: {}\n", kv(k), kv(v)));
    }
    if !query.is_empty() {
        out.push_str("[Query]\n");
        for (k, v) in &query {
            out.push_str(&format!("{}: {}\n", kv(k), kv(v)));
        }
    }
    if !form.is_empty() {
        out.push_str("[Form]\n");
        for (k, v) in &form {
            out.push_str(&format!("{}: {}\n", kv(k), kv(v)));
        }
    }
    if !multipart.is_empty() {
        out.push_str("[Multipart]\n");
        for l in &multipart {
            out.push_str(l);
            out.push('\n');
        }
    }
    if let Some((u, p)) = basic {
        out.push_str(&format!("[BasicAuth]\n{}: {}\n", kv(&u), kv(&p)));
    }
    if !options.is_empty() {
        out.push_str("[Options]\n");
        for (k, v) in &options {
            out.push_str(&format!("{k}: {}\n", kv(v)));
        }
    }
    out.push_str(&body);

    // Response section.
    let tests: Vec<&Assertion> = req.tests.iter().filter(|t| t.enabled).collect();
    let captures: Vec<&Capture> = req.captures.iter().filter(|c| c.enabled).collect();
    if tests.is_empty() && captures.is_empty() {
        return out;
    }
    let status_eq = tests.iter().find(|t| t.target == AssertTarget::Status && t.op == AssertOp::Eq).and_then(|t| t.value.as_deref()).filter(|v| v.trim().parse::<u16>().is_ok());
    out.push_str(&format!("HTTP {}\n", status_eq.map(str::trim).unwrap_or("*")));
    let mut captures_out: Vec<String> = Vec::new();
    for c in captures {
        let q = match &c.source {
            CaptureSource::Status => "status".to_string(),
            CaptureSource::Body => "body".to_string(),
            CaptureSource::Header { header } => format!("header {}", quoted(header)),
            CaptureSource::JsonPath { path } => format!("jsonpath {}", quoted(path)),
            CaptureSource::Cookie { cookie } => format!("cookie {}", quoted(cookie)),
            CaptureSource::Regex { pattern, .. } => format!("regex {}", quoted(pattern)),
        };
        captures_out.push(format!("{}: {q}", c.name));
    }
    if !captures_out.is_empty() {
        out.push_str("[Captures]\n");
        for l in captures_out {
            out.push_str(&l);
            out.push('\n');
        }
    }
    let mut asserts: Vec<String> = Vec::new();
    let mut skipped_status_eq = false;
    for t in tests {
        if t.target == AssertTarget::Status && t.op == AssertOp::Eq && !skipped_status_eq && status_eq.is_some() {
            skipped_status_eq = true; // already on the HTTP line
            continue;
        }
        match assertion(t) {
            Some(a) => asserts.push(a),
            None => asserts.push(format!("# not converted: {:?} {:?} {}", t.target, t.op, t.value.as_deref().unwrap_or(""))),
        }
    }
    if !asserts.is_empty() {
        out.push_str("[Asserts]\n");
        for a in asserts {
            out.push_str(&a);
            out.push('\n');
        }
    }
    out
}

fn assertion(t: &Assertion) -> Option<String> {
    let query = match &t.target {
        AssertTarget::Status => "status".to_string(),
        AssertTarget::Header { name } => format!("header {}", quoted(name)),
        AssertTarget::ContentType => "header \"Content-Type\"".to_string(),
        AssertTarget::JsonPath { path } => format!("jsonpath {}", quoted(path)),
        AssertTarget::Body => "body".to_string(),
        AssertTarget::ResponseTime => "duration".to_string(),
        AssertTarget::Size => "bytes count".to_string(),
        AssertTarget::Snapshot { .. } => return None,
    };
    let v = t.value.clone().unwrap_or_default();
    let pred = match t.op {
        AssertOp::Eq => format!("== {}", literal(&v)),
        AssertOp::Ne => format!("!= {}", literal(&v)),
        AssertOp::Lt => format!("< {}", literal(&v)),
        AssertOp::Lte => format!("<= {}", literal(&v)),
        AssertOp::Gt => format!("> {}", literal(&v)),
        AssertOp::Gte => format!(">= {}", literal(&v)),
        AssertOp::Contains => format!("contains {}", quoted(&v)),
        AssertOp::NotContains => format!("not contains {}", quoted(&v)),
        AssertOp::StartsWith => format!("startsWith {}", quoted(&v)),
        AssertOp::EndsWith => format!("endsWith {}", quoted(&v)),
        AssertOp::Matches => format!("matches {}", quoted(&v)),
        AssertOp::Exists => "exists".to_string(),
        AssertOp::NotExists => "not exists".to_string(),
        AssertOp::IsEmpty => "isEmpty".to_string(),
        AssertOp::IsNotEmpty => "not isEmpty".to_string(),
        AssertOp::IsType => match v.trim() {
            "string" => "isString".to_string(),
            "number" => "isNumber".to_string(),
            "boolean" => "isBoolean".to_string(),
            "array" | "object" => "isCollection".to_string(),
            _ => return None,
        },
        AssertOp::OneOf => return None,
    };
    Some(format!("{query} {pred}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use devall_core::{KeyValue, MultipartPart};

    #[test]
    fn round_trips_through_the_hurl_importer() {
        let mut req = HttpRequestFile::new("Create user");
        req.method = "POST".into();
        req.url = "{{baseUrl}}/users".into();
        req.headers.push(KeyValue::new("X-Tag", "a#b"));
        req.params.push(KeyValue::new("dry", "true"));
        req.auth = Auth::Bearer { token: "{{secret.TOKEN}}".into(), prefix: None };
        req.body = Body::Json { content: "{\n  \"name\": \"Ann\"\n}".into() };
        req.tests.push(Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some("201".into()), enabled: true });
        req.tests.push(Assertion { target: AssertTarget::JsonPath { path: "$.name".into() }, op: AssertOp::Eq, value: Some("Ann".into()), enabled: true });
        req.tests.push(Assertion { target: AssertTarget::Header { name: "Location".into() }, op: AssertOp::Exists, value: None, enabled: true });
        req.tests.push(Assertion { target: AssertTarget::ResponseTime, op: AssertOp::Lt, value: Some("500".into()), enabled: true });
        req.tests.push(Assertion { target: AssertTarget::JsonPath { path: "$.id".into() }, op: AssertOp::IsType, value: Some("number".into()), enabled: true });
        req.captures.push(Capture { name: "userId".into(), source: CaptureSource::JsonPath { path: "$.id".into() }, enabled: true });
        let mut secrets = Vec::new();
        let mut warnings = Vec::new();
        let text = entry(&req, &mut secrets, &mut warnings);
        assert_eq!(secrets, vec!["TOKEN".to_string()]);
        assert!(text.starts_with("POST {{baseUrl}}/users\n"), "{text}");
        assert!(text.contains("X-Tag: a\\#b\n"), "{text}");
        assert!(text.contains("[Options]\nlocation: true\n"), "{text}");
        assert!(text.contains("HTTP 201\n[Captures]\nuserId: jsonpath \"$.id\"\n[Asserts]\n"), "{text}");

        let plan = crate::formats::hurl::parse(&text, "x");
        let reqs = plan.requests();
        assert_eq!(reqs.len(), 1);
        let back = reqs[0].1;
        assert_eq!((back.method.as_str(), back.url.as_str()), ("POST", "{{baseUrl}}/users"));
        assert!(back.headers.iter().any(|h| h.name == "Authorization" && h.value == "Bearer {{TOKEN}}"));
        assert!(back.params.iter().any(|p| p.name == "dry" && p.value == "true"));
        assert!(matches!(&back.body, Body::Json { content } if content.contains("\"Ann\"")));
        assert!(back.settings.follow_redirects);
        assert_eq!(back.tests.len(), 5, "{:#?}", back.tests);
        assert_eq!(back.tests[0], Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some("201".into()), enabled: true });
        assert!(back.tests.contains(&Assertion { target: AssertTarget::JsonPath { path: "$.name".into() }, op: AssertOp::Eq, value: Some("Ann".into()), enabled: true }));
        assert!(back.tests.iter().any(|t| t.target == AssertTarget::ResponseTime && t.op == AssertOp::Lt));
        assert_eq!(back.captures, req.captures);
    }

    #[test]
    fn forms_multipart_and_auth_options() {
        let mut req = HttpRequestFile::new("Upload");
        req.method = "POST".into();
        req.url = "https://x.test/upload".into();
        req.settings.follow_redirects = false;
        req.settings.verify_tls = false;
        req.auth = Auth::Basic { username: "ann".into(), password: "{{secret.PW}}".into() };
        req.body = Body::Multipart { parts: vec![MultipartPart { name: "doc".into(), value: String::new(), file: Some("files\\a.pdf".into()), content_type: Some("application/pdf".into()), enabled: true }, MultipartPart { name: "n".into(), value: "1".into(), file: None, content_type: None, enabled: true }] };
        let text = entry(&req, &mut Vec::new(), &mut Vec::new());
        assert!(text.contains("[Multipart]\ndoc: file,files/a.pdf; application/pdf\nn: 1\n[BasicAuth]\nann: {{PW}}\n[Options]\ninsecure: true\n"), "{text}");
        assert!(!text.contains("location"));
        let back = &crate::formats::hurl::parse(&text, "x").requests()[0].1.clone();
        assert!(matches!(&back.auth, Auth::Basic { username, .. } if username == "ann"));
        assert!(!back.settings.verify_tls);
    }
}
