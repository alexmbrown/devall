//! Project / folder → OpenAPI 3.1 (YAML).

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use devall_core::vars::NoSecrets;
use devall_core::{ApiKeyLocation, AssertOp, AssertTarget, Auth, Body, HttpRequestFile, OAuth2Grant, Project, Resolver};

use super::{ExportOutput, entries, file_slug, folder_context, scope};
use crate::error::{Error, Result};
use crate::util::{map_templates, split_origin, split_query};

const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace"];

/// Exports `folder` (or the whole project) as OpenAPI 3.1: paths from request URLs (with
/// `{{baseUrl}}` stripped and `{{var}}` segments as path parameters), tags from top-level
/// folders, servers from the environments' `baseUrl`, security schemes from auth.
/// `env` limits the servers to one environment.
pub fn to_openapi(project: &Project, folder: Option<&str>, env: Option<&str>) -> Result<ExportOutput> {
    let mut warnings = Vec::new();
    let sc = scope(project, folder)?;
    let list = entries(project, &sc.children, &mut warnings)?;

    struct Op {
        path: String,
        method: String,
        op: Map<String, Value>,
        security: Option<Value>,
    }
    let mut ops: Vec<Op> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut op_ids: HashSet<String> = HashSet::new();
    let mut tags: Vec<String> = Vec::new();
    let mut schemes: Vec<(String, Value)> = Vec::new();
    let mut origins: Vec<String> = Vec::new();

    for e in &list {
        let req = &e.effective;
        let method = req.method.trim().to_lowercase();
        if !METHODS.contains(&method.as_str()) {
            warnings.push(format!("“{}”: method {} cannot be expressed in OpenAPI; skipped", req.name, req.method));
            continue;
        }
        let (path, path_params, origin, url_query) = split_url(&req.url);
        if let Some(o) = origin
            && !origins.contains(&o)
        {
            origins.push(o);
        }
        if !seen.insert((path.clone(), method.clone())) {
            warnings.push(format!("“{}”: another request already uses {} {path}; skipped", req.name, method.to_uppercase()));
            continue;
        }
        let mut op = Map::new();
        if let Some(tag) = e.folders.first() {
            op.insert("tags".into(), json!([tag]));
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        op.insert("summary".into(), json!(req.name));
        op.insert("operationId".into(), json!(unique_op_id(&req.name, &mut op_ids)));
        if let Some(d) = req.docs.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            op.insert("description".into(), json!(d));
        }
        let mut params: Vec<Value> = Vec::new();
        for p in &path_params {
            params.push(json!({ "name": p, "in": "path", "required": true, "schema": { "type": "string" } }));
        }
        for kv in url_query.iter().chain(req.params.iter()).filter(|p| !p.name.trim().is_empty()) {
            params.push(param(&kv.name, "query", kv.enabled, &kv.value, kv.description.as_deref()));
        }
        for h in req.headers.iter().filter(|h| !h.name.trim().is_empty()) {
            if ["accept", "content-type", "authorization"].contains(&h.name.to_ascii_lowercase().as_str()) {
                continue;
            }
            params.push(param(&h.name, "header", h.enabled, &h.value, h.description.as_deref()));
        }
        if !params.is_empty() {
            op.insert("parameters".into(), Value::Array(params));
        }
        if let Some(body) = request_body(req) {
            op.insert("requestBody".into(), body);
        }
        let status = req
            .tests
            .iter()
            .find(|t| t.enabled && t.target == AssertTarget::Status && t.op == AssertOp::Eq)
            .and_then(|t| t.value.clone())
            .filter(|v| v.len() == 3 && v.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or_else(|| "200".into());
        op.insert("responses".into(), json!({ status: { "description": "Successful response" } }));

        let security = match scheme_for(&req.auth) {
            Some((name, scheme, scopes)) => {
                let name = register_scheme(&mut schemes, name, scheme);
                Some(json!({ name: scopes }))
            }
            None => {
                if !matches!(req.auth, Auth::None | Auth::Inherit) {
                    warnings.push(format!("“{}”: its auth type has no OpenAPI security scheme equivalent", req.name));
                }
                None
            }
        };
        ops.push(Op { path, method, op, security });
    }

    // Shared security goes to the root; per-operation only when they differ.
    let first = ops.first().and_then(|o| o.security.clone());
    let uniform = first.is_some() && ops.iter().all(|o| o.security == first);
    let any = ops.iter().any(|o| o.security.is_some());
    let mut paths = Map::new();
    for o in ops {
        let mut op = o.op;
        if !uniform && any {
            op.insert("security".into(), match o.security {
                Some(s) => json!([s]),
                None => json!([]),
            });
        }
        let entry = paths.entry(o.path).or_insert_with(|| json!({}));
        if let Some(m) = entry.as_object_mut() {
            m.insert(o.method, Value::Object(op));
        }
    }

    let mut doc = Map::new();
    doc.insert("openapi".into(), json!("3.1.0"));
    let mut info = Map::new();
    info.insert("title".into(), json!(sc.name));
    info.insert("version".into(), json!("1.0.0"));
    if sc.root.is_empty()
        && let Some(d) = project.manifest.description.as_deref().filter(|d| !d.trim().is_empty())
    {
        info.insert("description".into(), json!(d));
    }
    doc.insert("info".into(), Value::Object(info));
    let servers = servers(project, &sc.root, env, &origins, &mut warnings)?;
    if !servers.is_empty() {
        doc.insert("servers".into(), Value::Array(servers));
    }
    if uniform && let Some(s) = first {
        doc.insert("security".into(), json!([s]));
    }
    if !tags.is_empty() {
        doc.insert("tags".into(), Value::Array(tags.iter().map(|t| json!({ "name": t })).collect()));
    }
    doc.insert("paths".into(), Value::Object(paths));
    if !schemes.is_empty() {
        let map: Map<String, Value> = schemes.into_iter().collect();
        doc.insert("components".into(), json!({ "securitySchemes": map }));
    }
    let content = serde_yaml_ng::to_string(&Value::Object(doc)).map_err(|e| Error::Parse(format!("cannot write YAML: {e}")))?;
    Ok(ExportOutput { content, file_name: format!("{}.openapi.yaml", file_slug(&sc.name)), language: "yaml".into(), warnings, attachments: Vec::new() })
}

/// URL → (OpenAPI path, path parameter names, absolute origin if any, query params in the URL).
fn split_url(url: &str) -> (String, Vec<String>, Option<String>, Vec<devall_core::KeyValue>) {
    let (base, query) = split_query(url.trim());
    let (origin, path) = split_origin(&base);
    let origin = if origin.is_empty() || origin.starts_with("{{") {
        None
    } else if origin.contains("://") {
        Some(origin.to_string())
    } else {
        Some(format!("https://{origin}"))
    };
    let mut names: Vec<String> = Vec::new();
    let path = map_templates(path, |k| {
        let name: String = k.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
        let name = name.trim_matches('_').to_string();
        let name = if name.is_empty() { "param".to_string() } else { name };
        if !names.contains(&name) {
            names.push(name.clone());
        }
        Some(format!("{{{name}}}"))
    });
    let path = if path.starts_with('/') { path } else { format!("/{path}") };
    (path, names, origin, query)
}

fn unique_op_id(name: &str, taken: &mut HashSet<String>) -> String {
    let mut out = String::new();
    for (i, word) in name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).enumerate() {
        if i == 0 {
            out.push_str(&word.to_lowercase());
        } else {
            let mut cs = word.chars();
            if let Some(f) = cs.next() {
                out.push(f.to_ascii_uppercase());
                out.push_str(&cs.as_str().to_lowercase());
            }
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out = format!("op{out}");
    }
    let mut id = out.clone();
    let mut n = 2;
    while !taken.insert(id.clone()) {
        id = format!("{out}{n}");
        n += 1;
    }
    id
}

fn infer_scalar(v: &str) -> Value {
    if v.parse::<i64>().is_ok() {
        json!({ "type": "integer" })
    } else if v.parse::<f64>().is_ok() {
        json!({ "type": "number" })
    } else if v == "true" || v == "false" {
        json!({ "type": "boolean" })
    } else {
        json!({ "type": "string" })
    }
}

fn param(name: &str, location: &str, required: bool, value: &str, description: Option<&str>) -> Value {
    let mut p = Map::new();
    p.insert("name".into(), json!(name));
    p.insert("in".into(), json!(location));
    if let Some(d) = description.filter(|d| !d.trim().is_empty()) {
        p.insert("description".into(), json!(d));
    }
    p.insert("required".into(), json!(required));
    p.insert("schema".into(), infer_scalar(value));
    if !value.is_empty() {
        p.insert("example".into(), json!(value));
    }
    Value::Object(p)
}

/// JSON Schema inferred from an example value.
fn infer(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let props: Map<String, Value> = m.iter().map(|(k, x)| (k.clone(), infer(x))).collect();
            json!({ "type": "object", "properties": props })
        }
        Value::Array(a) => match a.first() {
            Some(first) => json!({ "type": "array", "items": infer(first) }),
            None => json!({ "type": "array", "items": {} }),
        },
        Value::String(_) => json!({ "type": "string" }),
        Value::Number(n) if n.is_i64() || n.is_u64() => json!({ "type": "integer" }),
        Value::Number(_) => json!({ "type": "number" }),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Null => json!({ "type": "null" }),
    }
}

/// Parses JSON that may contain bare `{{templates}}` (they become strings).
pub(crate) fn json_lenient(s: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(s) {
        return Some(v);
    }
    let mut out = String::with_capacity(s.len() + 16);
    let mut in_str = false;
    let mut escaped = false;
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            rest = &rest[c.len_utf8()..];
        } else if c == '"' {
            in_str = true;
            out.push(c);
            rest = &rest[1..];
        } else if rest.starts_with("{{")
            && let Some(end) = rest.find("}}")
        {
            out.push('"');
            out.push_str(&rest[..end + 2].replace('"', "\\\""));
            out.push('"');
            rest = &rest[end + 2..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    serde_json::from_str(&out).ok()
}

fn header<'a>(req: &'a HttpRequestFile, name: &str) -> Option<&'a str> {
    req.headers.iter().find(|h| h.enabled && h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

fn request_body(req: &HttpRequestFile) -> Option<Value> {
    let ct = header(req, "content-type").map(|c| c.split(';').next().unwrap_or(c).trim().to_string());
    let (media_type, media) = match &req.body {
        Body::Json { content } => {
            let mt = ct.filter(|c| c.contains("json")).unwrap_or_else(|| "application/json".into());
            match json_lenient(content) {
                Some(v) => (mt, json!({ "schema": infer(&v), "example": v })),
                None => (mt, json!({ "schema": { "type": "string" }, "example": content })),
            }
        }
        Body::Text { content, content_type } => {
            let mt = content_type.clone().or(ct).unwrap_or_else(|| "text/plain".into());
            (mt, json!({ "schema": { "type": "string" }, "example": content }))
        }
        Body::Xml { content } => (ct.filter(|c| c.contains("xml")).unwrap_or_else(|| "application/xml".into()), json!({ "schema": { "type": "string" }, "example": content })),
        Body::Form { fields } => {
            let mut props = Map::new();
            let mut required = Vec::new();
            for f in fields.iter().filter(|f| !f.name.is_empty()) {
                props.insert(f.name.clone(), json!({ "type": "string", "example": f.value }));
                if f.enabled {
                    required.push(json!(f.name));
                }
            }
            ("application/x-www-form-urlencoded".into(), json!({ "schema": { "type": "object", "properties": props, "required": required } }))
        }
        Body::Multipart { parts } => {
            let mut props = Map::new();
            let mut required = Vec::new();
            let mut encoding = Map::new();
            for p in parts.iter().filter(|p| !p.name.is_empty()) {
                let schema = if p.file.is_some() { json!({ "type": "string", "format": "binary" }) } else { json!({ "type": "string", "example": p.value }) };
                props.insert(p.name.clone(), schema);
                if p.enabled {
                    required.push(json!(p.name));
                }
                if let Some(ct) = &p.content_type {
                    encoding.insert(p.name.clone(), json!({ "contentType": ct }));
                }
            }
            let mut media = json!({ "schema": { "type": "object", "properties": props, "required": required } });
            if !encoding.is_empty() {
                media["encoding"] = Value::Object(encoding);
            }
            ("multipart/form-data".into(), media)
        }
        Body::Graphql { query, variables, operation_name } => {
            let mut ex = json!({ "query": query, "variables": json_lenient(variables).unwrap_or(json!({})) });
            if let Some(op) = operation_name {
                ex["operationName"] = json!(op);
            }
            ("application/json".into(), json!({ "schema": infer(&ex), "example": ex }))
        }
        Body::Binary { content_type, .. } => (
            content_type.clone().or(ct).unwrap_or_else(|| "application/octet-stream".into()),
            json!({ "schema": { "type": "string", "format": "binary" } }),
        ),
        _ => return None,
    };
    Some(json!({ "required": true, "content": { media_type: media } }))
}

/// `{{secret.X}}` → `X`.
fn secret_ref(s: &str) -> Option<String> {
    let inner = s.trim().strip_prefix("{{")?.strip_suffix("}}")?.trim();
    let name = inner.strip_prefix("secret.")?;
    (!name.contains("{{")).then(|| name.to_string())
}

fn scheme_for(auth: &Auth) -> Option<(String, Value, Vec<String>)> {
    match auth {
        Auth::Bearer { token, .. } => Some((secret_ref(token).unwrap_or_else(|| "bearerAuth".into()), json!({ "type": "http", "scheme": "bearer" }), Vec::new())),
        Auth::Basic { password, .. } => Some((
            secret_ref(password).and_then(|n| n.strip_suffix("_PASSWORD").map(str::to_string)).unwrap_or_else(|| "basicAuth".into()),
            json!({ "type": "http", "scheme": "basic" }),
            Vec::new(),
        )),
        Auth::Digest { password, .. } => Some((
            secret_ref(password).and_then(|n| n.strip_suffix("_PASSWORD").map(str::to_string)).unwrap_or_else(|| "digestAuth".into()),
            json!({ "type": "http", "scheme": "digest" }),
            Vec::new(),
        )),
        Auth::ApiKey { key, value, location } => Some((
            secret_ref(value).unwrap_or_else(|| "apiKeyAuth".into()),
            json!({ "type": "apiKey", "name": key, "in": if *location == ApiKeyLocation::Query { "query" } else { "header" } }),
            Vec::new(),
        )),
        Auth::OAuth2(c) => {
            let scopes: Vec<String> = c.scope.split_whitespace().map(str::to_string).collect();
            let scope_map: Map<String, Value> = scopes.iter().map(|s| (s.clone(), json!(""))).collect();
            let (flow, mut body) = match c.grant {
                OAuth2Grant::ClientCredentials => ("clientCredentials", json!({ "tokenUrl": c.token_url })),
                OAuth2Grant::Password => ("password", json!({ "tokenUrl": c.token_url })),
                _ => ("authorizationCode", json!({ "authorizationUrl": c.auth_url, "tokenUrl": c.token_url })),
            };
            body["scopes"] = Value::Object(scope_map);
            let name = secret_ref(&c.client_id).and_then(|n| n.strip_suffix("_CLIENT_ID").map(str::to_string)).unwrap_or_else(|| "oauth2".into());
            Some((name, json!({ "type": "oauth2", "flows": { flow: body } }), scopes))
        }
        Auth::AwsSigV4 { .. } => Some((
            "sigv4".into(),
            json!({ "type": "apiKey", "name": "Authorization", "in": "header", "x-amazon-apigateway-authtype": "awsSigv4" }),
            Vec::new(),
        )),
        _ => None,
    }
}

/// Adds a scheme (or reuses an identical one); returns its final name.
fn register_scheme(schemes: &mut Vec<(String, Value)>, name: String, scheme: Value) -> String {
    if let Some((n, _)) = schemes.iter().find(|(n, s)| *n == name && *s == scheme) {
        return n.clone();
    }
    let mut candidate = name.clone();
    let mut i = 2;
    while schemes.iter().any(|(n, _)| *n == candidate) {
        if let Some((n, _)) = schemes.iter().find(|(n, s)| *n == candidate && *s == scheme) {
            return n.clone();
        }
        candidate = format!("{name}{i}");
        i += 1;
    }
    schemes.push((candidate.clone(), scheme));
    candidate
}

fn servers(project: &Project, root: &str, env: Option<&str>, origins: &[String], warnings: &mut Vec<String>) -> Result<Vec<Value>> {
    let folder_vars = folder_context(project, root)?.folder_variables;
    let mut out: Vec<Value> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for e in project.environments()?.into_iter().filter(|e| env.is_none_or(|id| e.id == id)) {
        let mut r = Resolver::new(&NoSecrets);
        r.push_layer(e.file.variables.clone());
        for fv in &folder_vars {
            r.push_layer(fv.clone());
        }
        r.push_layer(project.manifest.variables.clone());
        let url = r.resolve("{{baseUrl}}").trim().trim_end_matches('/').to_string();
        if url.is_empty() || url.contains("{{") || urls.contains(&url) {
            continue;
        }
        urls.push(url.clone());
        out.push(json!({ "url": url, "description": e.file.name }));
    }
    for o in origins {
        if !urls.contains(o) {
            urls.push(o.clone());
            out.push(json!({ "url": o }));
        }
    }
    if !origins.is_empty() {
        warnings.push("Some requests use absolute URLs; their origins were added as servers".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_to_paths() {
        let (p, params, origin, q) = split_url("{{baseUrl}}/users/{{id}}/posts?page=2");
        assert_eq!(p, "/users/{id}/posts");
        assert_eq!(params, vec!["id"]);
        assert_eq!(origin, None);
        assert_eq!(q[0].name, "page");
        let (p, _, origin, _) = split_url("https://api.x.io:8443/v1/a");
        assert_eq!(p, "/v1/a");
        assert_eq!(origin.as_deref(), Some("https://api.x.io:8443"));
        assert_eq!(split_url("{{baseUrl}}").0, "/");
    }

    #[test]
    fn lenient_json() {
        assert_eq!(json_lenient("{\"n\": {{count}}, \"s\": \"{{x}}\"}").unwrap(), json!({ "n": "{{count}}", "s": "{{x}}" }));
        assert!(json_lenient("{nope").is_none());
    }

    #[test]
    fn op_ids() {
        let mut taken = HashSet::new();
        assert_eq!(unique_op_id("List users", &mut taken), "listUsers");
        assert_eq!(unique_op_id("list USERS", &mut taken), "listUsers2");
        assert_eq!(unique_op_id("2fa", &mut taken), "op2fa");
    }
}
