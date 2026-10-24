//! OpenAPI 3.0 / 3.1 and Swagger 2.0 → one request per operation, grouped in folders by first
//! tag. Servers become environments (`baseUrl`), security schemes become folder auth that
//! references `{{secret.*}}`, bodies are taken from examples or generated from schemas.

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use devall_core::{ApiKeyLocation, Auth, Body, EnvironmentFile, FolderFile, HttpRequestFile, KeyValue, MultipartPart, OAuth2Config, OAuth2Grant};

use crate::detect::Format;
use crate::error::{Error, Result};
use crate::plan::{ImportPlan, PlanFolder, PlanItem};
use crate::util::{Warnings, add_path_vars, arr_at, bool_at, folder_has_settings, scalar_string, str_at, upper_snake};

const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace"];
const MAX_DEPTH: usize = 12;

pub(crate) fn parse(doc: &Value) -> Result<ImportPlan> {
    let swagger2 = doc.get("swagger").is_some();
    if !swagger2 && !doc.get("openapi").is_some_and(Value::is_string) {
        return Err(Error::Parse("not an OpenAPI document (missing `openapi` or `swagger`)".into()));
    }
    if swagger2 && !text(doc, "swagger").starts_with('2') {
        return Err(Error::Unsupported(format!("Swagger {} is not supported (only 2.0)", text(doc, "swagger"))));
    }
    let info = doc.get("info").unwrap_or(&Value::Null);
    let title = Some(str_at(info, "title").trim()).filter(|t| !t.is_empty()).unwrap_or("API");
    let mut plan = ImportPlan::new(title, if swagger2 { Format::Swagger } else { Format::OpenApi });
    let mut cx = Cx { doc, swagger2, w: Warnings::default(), secrets: Vec::new() };

    // Collection-level auth from the global security requirement.
    let mut root = FolderFile::default();
    let global = doc.get("security").cloned();
    if let Some(sec) = &global {
        let (auth, cookie) = cx.security(sec, "the API");
        root.auth = auth;
        root.headers.extend(cookie);
    }

    // Folders in `tags` order, then in order of first use.
    let mut folders: IndexMap<String, PlanFolder> = IndexMap::new();
    for t in arr_at(doc, "tags") {
        let name = str_at(t, "name").trim();
        if !name.is_empty() {
            folders.insert(name.to_string(), PlanFolder::new(name));
        }
    }
    let mut root_settings = folder_has_settings(&root).then_some(root);
    let mut untagged: Vec<PlanItem> = Vec::new();
    let mut ops_with_servers = 0;
    if let Some(paths) = doc.get("paths").and_then(Value::as_object) {
        for (path, item) in paths {
            let item = cx.deref(item);
            for method in METHODS {
                let Some(op) = item.get(*method).filter(|o| o.is_object()) else { continue };
                if op.get("servers").is_some() || item.get("servers").is_some() {
                    ops_with_servers += 1;
                }
                let (mut req, path_vars) = cx.operation(path, method, item, op, global.as_ref());
                let tag = arr_at(op, "tags").iter().filter_map(Value::as_str).map(str::trim).find(|t| !t.is_empty());
                match tag {
                    Some(t) => {
                        let f = folders.entry(t.to_string()).or_insert_with(|| PlanFolder::new(t));
                        add_path_vars(&mut f.settings, &mut req, path_vars);
                        f.items.push(PlanItem::Request(Box::new(req)));
                    }
                    None => {
                        add_path_vars(&mut root_settings, &mut req, path_vars);
                        untagged.push(PlanItem::Request(Box::new(req)));
                    }
                }
            }
        }
    }
    if ops_with_servers > 0 {
        cx.w.add(format!("{ops_with_servers} operations declare their own servers; all requests use {{{{baseUrl}}}} from the environment"));
    }
    if doc.get("webhooks").and_then(Value::as_object).is_some_and(|m| !m.is_empty()) {
        cx.w.add("Webhooks (incoming requests) were not imported");
    }
    plan.items = folders.into_values().filter(|f| !f.items.is_empty()).map(PlanItem::Folder).collect();
    plan.items.extend(untagged);
    plan.settings = root_settings;

    plan.environments = cx.servers();
    if plan.environments.is_empty() {
        cx.w.add("The spec declares no servers; set `baseUrl` in an environment");
    }
    for env in &mut plan.environments {
        for s in &cx.secrets {
            if !env.secrets.contains(s) {
                env.secrets.push(s.clone());
            }
        }
    }
    plan.warnings = cx.w.finish();
    Ok(plan)
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).map(scalar_string).unwrap_or_default()
}

struct Cx<'a> {
    doc: &'a Value,
    swagger2: bool,
    w: Warnings,
    /// Secret names referenced by security schemes (declared on every environment).
    secrets: Vec<String>,
}

impl<'a> Cx<'a> {
    // ---------- $ref ----------

    fn pointer(&mut self, r: &str) -> Option<&'a Value> {
        let Some(ptr) = r.strip_prefix('#') else {
            self.w.about("External $refs are not followed", r);
            return None;
        };
        let decoded = crate::util::url_decode(&ptr.replace('+', "%2B"));
        let found = self.doc.pointer(&decoded);
        if found.is_none() {
            self.w.about("Unresolvable $ref", r);
        }
        found
    }

    /// Follows `$ref` chains on non-schema objects (parameters, request bodies, examples, …).
    fn deref(&mut self, v: &'a Value) -> &'a Value {
        let mut cur = v;
        for _ in 0..32 {
            match cur.get("$ref").and_then(Value::as_str) {
                Some(r) => match self.pointer(r) {
                    Some(t) => cur = t,
                    None => return &Value::Null,
                },
                None => return cur,
            }
        }
        cur
    }

    // ---------- environments ----------

    fn servers(&mut self) -> Vec<EnvironmentFile> {
        let mut list: Vec<(String, String)> = Vec::new();
        if self.swagger2 {
            let host = str_at(self.doc, "host");
            let base = str_at(self.doc, "basePath");
            let schemes: Vec<&str> = arr_at(self.doc, "schemes").iter().filter_map(Value::as_str).collect();
            let scheme = if schemes.contains(&"https") { "https" } else { schemes.first().copied().unwrap_or("https") };
            if !host.is_empty() {
                list.push((host.to_string(), format!("{scheme}://{host}{base}")));
            } else if !base.is_empty() {
                list.push(("Default".into(), base.to_string()));
            }
        } else {
            for s in arr_at(self.doc, "servers") {
                let mut url = str_at(s, "url").to_string();
                if let Some(vars) = s.get("variables").and_then(Value::as_object) {
                    for (k, v) in vars {
                        url = url.replace(&format!("{{{k}}}"), &text(v, "default"));
                    }
                }
                if url.is_empty() {
                    continue;
                }
                let desc = str_at(s, "description").trim();
                let name = if !desc.is_empty() && desc.len() <= 40 {
                    desc.to_string()
                } else if url.contains("://") {
                    host_of(&url)
                } else {
                    "localhost".to_string()
                };
                list.push((name, url));
            }
        }
        let mut envs: Vec<EnvironmentFile> = Vec::new();
        for (name, url) in list {
            let mut url = url.trim().trim_end_matches('/').to_string();
            if !url.contains("://") {
                self.w.add(format!("Server URL “{url}” is relative; adjust `baseUrl` in the environment"));
                url = format!("http://localhost{}{}", if url.starts_with('/') || url.is_empty() { "" } else { "/" }, url);
            }
            let mut unique = name.clone();
            let mut n = 2;
            while envs.iter().any(|e| e.name.eq_ignore_ascii_case(&unique)) {
                unique = format!("{name} {n}");
                n += 1;
            }
            let mut env = EnvironmentFile::new(unique);
            env.variables.insert("baseUrl".into(), url);
            envs.push(env);
        }
        envs
    }

    // ---------- security ----------

    fn secret(&mut self, name: String) -> String {
        if !self.secrets.contains(&name) {
            self.secrets.push(name.clone());
        }
        format!("{{{{secret.{name}}}}}")
    }

    /// First alternative of a security requirement list → auth (+ cookie header for apiKey in
    /// cookie). `[]` means "no auth".
    fn security(&mut self, sec: &Value, subject: &str) -> (Auth, Option<KeyValue>) {
        let Some(list) = sec.as_array() else { return (Auth::Inherit, None) };
        let Some(first) = list.first().and_then(Value::as_object) else { return (Auth::None, None) };
        let Some((scheme_name, scopes)) = first.iter().next() else { return (Auth::None, None) };
        if first.len() > 1 {
            self.w.about("Combined security requirements (several schemes at once) are not supported; only the first scheme was applied", subject);
        }
        let defs = if self.swagger2 { self.doc.get("securityDefinitions") } else { self.doc.pointer("/components/securitySchemes") };
        let Some(scheme) = defs.and_then(|d| d.get(scheme_name.as_str())) else {
            self.w.about("Unknown security scheme", scheme_name);
            return (Auth::Inherit, None);
        };
        let scheme = self.deref(scheme);
        let scopes: Vec<String> = scopes.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        self.scheme_auth(scheme_name, scheme, &scopes)
    }

    fn scheme_auth(&mut self, name: &str, s: &'a Value, scopes: &[String]) -> (Auth, Option<KeyValue>) {
        let base = upper_snake(name);
        match str_at(s, "type") {
            "http" => match str_at(s, "scheme").to_ascii_lowercase().as_str() {
                "bearer" => (Auth::Bearer { token: self.secret(base), prefix: None }, None),
                "basic" => (Auth::Basic { username: self.secret(format!("{base}_USERNAME")), password: self.secret(format!("{base}_PASSWORD")) }, None),
                "digest" => (Auth::Digest { username: self.secret(format!("{base}_USERNAME")), password: self.secret(format!("{base}_PASSWORD")) }, None),
                other => {
                    self.w.about(&format!("HTTP auth scheme “{other}” is not supported"), name);
                    (Auth::Inherit, None)
                }
            },
            "basic" => (Auth::Basic { username: self.secret(format!("{base}_USERNAME")), password: self.secret(format!("{base}_PASSWORD")) }, None),
            "apiKey" => {
                let key = str_at(s, "name").to_string();
                let value = self.secret(base);
                match str_at(s, "in") {
                    "query" => (Auth::ApiKey { key, value, location: ApiKeyLocation::Query }, None),
                    "cookie" => (Auth::Inherit, Some(KeyValue::new("Cookie", format!("{key}={value}")))),
                    _ => (Auth::ApiKey { key, value, location: ApiKeyLocation::Header }, None),
                }
            }
            "oauth2" => {
                let (grant, flow) = if self.swagger2 {
                    let g = match str_at(s, "flow") {
                        "application" => Some(OAuth2Grant::ClientCredentials),
                        "accessCode" => Some(OAuth2Grant::AuthorizationCode),
                        "password" => Some(OAuth2Grant::Password),
                        _ => None,
                    };
                    (g, s)
                } else {
                    let flows = s.get("flows").unwrap_or(&Value::Null);
                    [("clientCredentials", OAuth2Grant::ClientCredentials), ("authorizationCode", OAuth2Grant::AuthorizationCode), ("password", OAuth2Grant::Password)]
                        .into_iter()
                        .find_map(|(k, g)| flows.get(k).map(|f| (Some(g), f)))
                        .unwrap_or((None, &Value::Null))
                };
                let Some(grant) = grant else {
                    self.w.about("OAuth 2.0 implicit flow is not supported; using a bearer token secret instead", name);
                    return (Auth::Bearer { token: self.secret(format!("{base}_TOKEN")), prefix: None }, None);
                };
                let mut cfg = OAuth2Config {
                    grant,
                    auth_url: str_at(flow, "authorizationUrl").to_string(),
                    token_url: str_at(flow, "tokenUrl").to_string(),
                    client_id: self.secret(format!("{base}_CLIENT_ID")),
                    scope: scopes.join(" "),
                    use_pkce: true,
                    ..Default::default()
                };
                if grant != OAuth2Grant::AuthorizationCode {
                    cfg.client_secret = self.secret(format!("{base}_CLIENT_SECRET"));
                }
                if grant == OAuth2Grant::Password {
                    cfg.username = self.secret(format!("{base}_USERNAME"));
                    cfg.password = self.secret(format!("{base}_PASSWORD"));
                }
                (Auth::OAuth2(cfg), None)
            }
            "openIdConnect" => {
                self.w.about("OpenID Connect discovery is not supported; using a bearer token secret instead", name);
                (Auth::Bearer { token: self.secret(format!("{base}_TOKEN")), prefix: None }, None)
            }
            other => {
                self.w.about(&format!("Security scheme type “{other}” is not supported"), name);
                (Auth::Inherit, None)
            }
        }
    }

    // ---------- operations ----------

    fn operation(&mut self, path: &str, method: &str, item: &'a Value, op: &'a Value, global: Option<&Value>) -> (HttpRequestFile, Vec<(String, String)>) {
        let summary = str_at(op, "summary").trim();
        let op_id = str_at(op, "operationId").trim();
        let name = if !summary.is_empty() {
            summary.to_string()
        } else if !op_id.is_empty() {
            op_id.to_string()
        } else {
            format!("{} {path}", method.to_uppercase())
        };
        let mut req = HttpRequestFile::new(name);
        req.method = method.to_uppercase();
        req.url = format!("{{{{baseUrl}}}}{}", template_path(path));

        // Path-level parameters, overridden by operation-level ones with the same name + location.
        let mut params: Vec<&'a Value> = Vec::new();
        for p in arr_at(item, "parameters").iter().chain(arr_at(op, "parameters")) {
            let p = self.deref(p);
            if p.is_null() {
                continue;
            }
            params.retain(|x| !(str_at(x, "name") == str_at(p, "name") && str_at(x, "in") == str_at(p, "in")));
            params.push(p);
        }
        let mut path_vars = Vec::new();
        let mut cookies: Vec<String> = Vec::new();
        let mut cookie_required = false;
        let mut body_param: Option<&'a Value> = None;
        let mut form_params: Vec<&'a Value> = Vec::new();
        for p in params {
            let pname = str_at(p, "name").to_string();
            let loc = str_at(p, "in");
            let required = bool_at(p, "required") == Some(true);
            match loc {
                "path" => path_vars.push((pname, self.explicit_value(p))),
                "query" => req.params.push(kv(pname, self.param_value(p), required, p)),
                "header" => {
                    if !["accept", "content-type", "authorization"].contains(&pname.to_ascii_lowercase().as_str()) {
                        req.headers.push(kv(pname, self.param_value(p), required, p));
                    }
                }
                "cookie" => {
                    cookies.push(format!("{pname}={}", self.param_value(p)));
                    cookie_required |= required;
                }
                "body" => body_param = Some(p),
                "formData" => form_params.push(p),
                _ => {}
            }
        }
        if !cookies.is_empty() {
            let mut c = KeyValue::new("Cookie", cookies.join("; "));
            c.enabled = cookie_required;
            req.headers.push(c);
        }

        if self.swagger2 {
            let consumes: Vec<String> = arr_at(op, "consumes")
                .iter()
                .chain(if op.get("consumes").is_none() { arr_at(self.doc, "consumes") } else { &[] })
                .filter_map(Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect();
            if let Some(bp) = body_param {
                let ex = bp.get("schema").map(|s| self.example(s, 0, &mut Vec::new(), true)).unwrap_or(Value::Null);
                let xml_only = !consumes.is_empty() && consumes.iter().all(|c| c.contains("xml"));
                req.body = if xml_only {
                    Body::Xml { content: to_xml(&root_name(bp.get("schema")), &ex) }
                } else {
                    Body::Json { content: pretty(&ex) }
                };
            } else if !form_params.is_empty() {
                let multipart = consumes.iter().any(|c| c.contains("multipart")) || form_params.iter().any(|p| str_at(p, "type") == "file");
                if multipart {
                    req.body = Body::Multipart {
                        parts: form_params
                            .iter()
                            .map(|&p| {
                                let file = str_at(p, "type") == "file";
                                MultipartPart {
                                    name: str_at(p, "name").to_string(),
                                    value: if file { String::new() } else { self.param_value(p) },
                                    file: file.then(String::new),
                                    content_type: None,
                                    enabled: bool_at(p, "required") == Some(true) || file,
                                }
                            })
                            .collect(),
                    };
                } else {
                    req.body = Body::Form {
                        fields: form_params.iter().map(|&p| kv(str_at(p, "name").to_string(), self.param_value(p), bool_at(p, "required") == Some(true), p)).collect(),
                    };
                }
            }
        } else if let Some(rb) = op.get("requestBody") {
            let rb = self.deref(rb);
            self.request_body(rb, &mut req);
        }

        if let Some(sec) = op.get("security")
            && Some(sec) != global
        {
            let (auth, cookie) = self.security(sec, &req.name);
            req.auth = match auth {
                Auth::Inherit if sec.as_array().is_some_and(Vec::is_empty) => Auth::None,
                a => a,
            };
            req.headers.extend(cookie);
        }

        let mut docs: Vec<String> = Vec::new();
        if bool_at(op, "deprecated") == Some(true) {
            docs.push("**Deprecated.**".into());
        }
        let description = str_at(op, "description").trim();
        if !description.is_empty() {
            docs.push(description.to_string());
        }
        if !docs.is_empty() {
            req.docs = Some(docs.join("\n\n"));
        }
        (req, path_vars)
    }

    /// Only values the spec states (example, default, enum) — path parameters without one stay
    /// as unresolved `{{name}}` references instead of a made-up `0`.
    fn explicit_value(&mut self, p: &'a Value) -> String {
        let schema = self.deref(p.get("schema").unwrap_or(p));
        let explicit = p.get("example").is_some()
            || p.get("x-example").is_some()
            || p.get("examples").is_some()
            || ["example", "examples", "default", "enum", "const"].iter().any(|k| schema.get(*k).is_some());
        if explicit { self.param_value(p) } else { String::new() }
    }

    fn param_value(&mut self, p: &'a Value) -> String {
        if let Some(ex) = p.get("example").or_else(|| p.get("x-example")) {
            return param_string(ex);
        }
        if let Some(exs) = p.get("examples").and_then(Value::as_object)
            && let Some((_, first)) = exs.iter().next()
        {
            let first = self.deref(first);
            if let Some(v) = first.get("value") {
                return param_string(v);
            }
        }
        // Swagger 2 parameters carry their schema inline (type, format, enum, default, items).
        let schema = p.get("schema").unwrap_or(p);
        let ex = self.example(schema, 0, &mut Vec::new(), true);
        param_string(&ex)
    }

    fn request_body(&mut self, rb: &'a Value, req: &mut HttpRequestFile) {
        let Some(content) = rb.get("content").and_then(Value::as_object) else { return };
        let pick = |pred: &dyn Fn(&str) -> bool| content.iter().find(|(k, _)| pred(&k.to_ascii_lowercase()));
        let chosen = pick(&|k| k == "application/json")
            .or_else(|| pick(&|k| k.contains("json")))
            .or_else(|| pick(&|k| k == "application/x-www-form-urlencoded"))
            .or_else(|| pick(&|k| k.starts_with("multipart/")))
            .or_else(|| pick(&|k| k.contains("xml")))
            .or_else(|| pick(&|k| k.starts_with("text/")))
            .or_else(|| content.iter().next());
        let Some((mt, media)) = chosen else { return };
        let mtl = mt.to_ascii_lowercase();
        let schema = media.get("schema");
        let media_example = self.media_example(media);
        if mtl.contains("json") {
            let ex = media_example.unwrap_or_else(|| schema.map(|s| self.example(s, 0, &mut Vec::new(), true)).unwrap_or(Value::Null));
            req.body = Body::Json { content: pretty(&ex) };
            if mtl != "application/json" {
                req.headers.push(KeyValue::new("Content-Type", mt.as_str()));
            }
        } else if mtl == "application/x-www-form-urlencoded" || mtl.starts_with("multipart/") {
            let fields = self.object_fields(schema, media_example.as_ref());
            let encoding = media.get("encoding");
            if mtl.starts_with("multipart/") {
                req.body = Body::Multipart {
                    parts: fields
                        .into_iter()
                        .map(|(name, value, required, is_file)| MultipartPart {
                            content_type: encoding.and_then(|e| e.get(&name)).map(|e| str_at(e, "contentType").to_string()).filter(|c| !c.is_empty()),
                            file: is_file.then(String::new),
                            value: if is_file { String::new() } else { value },
                            enabled: required || is_file,
                            name,
                        })
                        .collect(),
                };
            } else {
                req.body = Body::Form {
                    fields: fields
                        .into_iter()
                        .map(|(name, value, required, _)| {
                            let mut kv = KeyValue::new(name, value);
                            kv.enabled = required;
                            kv
                        })
                        .collect(),
                };
            }
        } else if mtl.contains("xml") {
            let content = match media_example {
                Some(Value::String(s)) => s,
                Some(v) => to_xml(&root_name(schema), &v),
                None => {
                    let ex = schema.map(|s| self.example(s, 0, &mut Vec::new(), true)).unwrap_or(Value::Null);
                    to_xml(&root_name(schema), &ex)
                }
            };
            req.body = Body::Xml { content };
        } else if mtl.starts_with("text/") {
            let content = media_example.map(|v| param_string(&v)).unwrap_or_default();
            req.body = Body::Text { content, content_type: (mtl != "text/plain").then(|| mt.clone()) };
        } else {
            req.body = Body::Binary { path: String::new(), content_type: Some(mt.clone()) };
        }
    }

    fn media_example(&mut self, media: &'a Value) -> Option<Value> {
        if let Some(ex) = media.get("example") {
            return Some(ex.clone());
        }
        let exs = media.get("examples").and_then(Value::as_object)?;
        let (_, first) = exs.iter().next()?;
        let first = self.deref(first);
        first.get("value").cloned()
    }

    /// (name, example value, required, is file) for each property of an object schema.
    fn object_fields(&mut self, schema: Option<&'a Value>, example: Option<&Value>) -> Vec<(String, String, bool, bool)> {
        let Some(schema) = schema else { return Vec::new() };
        let mut props: Vec<(String, &'a Value)> = Vec::new();
        let mut required: Vec<String> = Vec::new();
        self.collect_props(schema, &mut props, &mut required, 0);
        let mut out = Vec::new();
        for (name, ps) in props {
            let resolved = self.deref(ps);
            if bool_at(resolved, "readOnly") == Some(true) || bool_at(ps, "readOnly") == Some(true) {
                continue;
            }
            let is_binary = |s: &Value| matches!(str_at(s, "format"), "binary" | "base64") || s.get("contentMediaType").is_some();
            let is_file = is_binary(resolved) || resolved.get("items").is_some_and(|i| is_binary(self_deref(self.doc, i)));
            let value = match example.and_then(|e| e.get(&name)) {
                Some(v) => param_string(v),
                None if is_file => String::new(),
                None => param_string(&self.example(ps, 1, &mut Vec::new(), true)),
            };
            let is_required = required.contains(&name);
            out.push((name, value, is_required, is_file));
        }
        out
    }

    fn collect_props(&mut self, schema: &'a Value, props: &mut Vec<(String, &'a Value)>, required: &mut Vec<String>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let s = self.deref(schema);
        for sub in arr_at(s, "allOf") {
            self.collect_props(sub, props, required, depth + 1);
        }
        if let Some(m) = s.get("properties").and_then(Value::as_object) {
            for (k, v) in m {
                if !props.iter().any(|(n, _)| n == k) {
                    props.push((k.clone(), v));
                }
            }
        }
        required.extend(arr_at(s, "required").iter().filter_map(Value::as_str).map(str::to_string));
    }

    // ---------- example generation ----------

    /// An example value for `schema`: explicit examples first, then a value generated from the
    /// schema (required properties always, optional ones near the top, `readOnly` skipped in
    /// requests, `$ref` cycles cut).
    fn example(&mut self, schema: &'a Value, depth: usize, stack: &mut Vec<&'a str>, for_request: bool) -> Value {
        if depth > MAX_DEPTH || !schema.is_object() {
            return Value::Null;
        }
        if let Some(ex) = schema.get("example") {
            return ex.clone();
        }
        if let Some(first) = schema.get("examples").and_then(Value::as_array).and_then(|a| a.first()) {
            return first.clone();
        }
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            let Some(target) = self.pointer(r) else { return Value::Null };
            if stack.contains(&r) {
                return match schema_type(target) {
                    "object" => json!({}),
                    "array" => json!([]),
                    _ => Value::Null,
                };
            }
            stack.push(r);
            let v = self.example(target, depth, stack, for_request);
            stack.pop();
            return v;
        }
        for key in ["const", "default"] {
            if let Some(v) = schema.get(key) {
                return v.clone();
            }
        }
        if let Some(first) = schema.get("enum").and_then(Value::as_array).and_then(|e| e.iter().find(|x| !x.is_null())) {
            return first.clone();
        }
        if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
            let mut merged = Map::new();
            let mut other = None;
            for s in all {
                match self.example(s, depth + 1, stack, for_request) {
                    Value::Object(m) => merged.extend(m),
                    v if !v.is_null() => other = Some(v),
                    _ => {}
                }
            }
            if schema.get("properties").is_some()
                && let Value::Object(m) = self.object_example(schema, depth, stack, for_request)
            {
                merged.extend(m);
            }
            return if merged.is_empty() { other.unwrap_or(Value::Object(merged)) } else { Value::Object(merged) };
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(first) = schema.get(key).and_then(Value::as_array).and_then(|l| l.iter().find(|s| schema_type(s) != "null")) {
                return self.example(first, depth + 1, stack, for_request);
            }
        }
        match schema_type(schema) {
            "object" => self.object_example(schema, depth, stack, for_request),
            "array" => match schema.get("items") {
                Some(items) => match self.example(items, depth + 1, stack, for_request) {
                    Value::Null => json!([]),
                    v => json!([v]),
                },
                None => json!([]),
            },
            "string" => Value::String(string_example(schema)),
            "integer" => json!(number_floor(schema).map(|n| n.ceil() as i64).unwrap_or(0)),
            "number" => match number_floor(schema) {
                Some(n) if n.fract() != 0.0 => json!(n),
                Some(n) => json!(n as i64),
                None => json!(0),
            },
            "boolean" => json!(true),
            _ => Value::Null,
        }
    }

    fn object_example(&mut self, schema: &'a Value, depth: usize, stack: &mut Vec<&'a str>, for_request: bool) -> Value {
        let mut out = Map::new();
        let required: Vec<&str> = arr_at(schema, "required").iter().filter_map(Value::as_str).collect();
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (name, ps) in props {
                let is_required = required.contains(&name.as_str());
                if !is_required && depth >= 2 {
                    continue;
                }
                let resolved = self.deref(ps);
                let flag = |k: &str| bool_at(ps, k) == Some(true) || bool_at(resolved, k) == Some(true);
                if (for_request && flag("readOnly")) || (!for_request && flag("writeOnly")) {
                    continue;
                }
                out.insert(name.clone(), self.example(ps, depth + 1, stack, for_request));
            }
        } else if let Some(ap) = schema.get("additionalProperties").filter(|a| a.is_object()) {
            out.insert("key".into(), self.example(ap, depth + 1, stack, for_request));
        }
        Value::Object(out)
    }
}

fn self_deref<'v>(doc: &'v Value, v: &'v Value) -> &'v Value {
    let mut cur = v;
    for _ in 0..32 {
        match cur.get("$ref").and_then(Value::as_str).and_then(|r| r.strip_prefix('#')).and_then(|p| doc.pointer(p)) {
            Some(t) => cur = t,
            None => break,
        }
    }
    cur
}

fn kv(name: String, value: String, required: bool, p: &Value) -> KeyValue {
    let mut kv = KeyValue::new(name, value);
    kv.enabled = required;
    kv.description = Some(str_at(p, "description").trim().to_string()).filter(|d| !d.is_empty());
    kv
}

/// `/users/{id}` → `/users/{{id}}`.
pub(crate) fn template_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    let mut rest = path;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        match rest[i + 1..].find('}') {
            Some(j) => {
                out.push_str(&format!("{{{{{}}}}}", rest[i + 1..i + 1 + j].trim()));
                rest = &rest[i + 1 + j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| url.to_string())
}

fn schema_type(s: &Value) -> &str {
    match s.get("type") {
        Some(Value::String(t)) => t,
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("null"),
        _ if s.get("properties").is_some() || s.get("additionalProperties").is_some() => "object",
        _ if s.get("items").is_some() => "array",
        _ => "",
    }
}

fn number_floor(s: &Value) -> Option<f64> {
    let min = s.get("minimum").and_then(Value::as_f64);
    match s.get("exclusiveMinimum") {
        Some(Value::Number(n)) => n.as_f64().map(|n| n + 1.0),
        Some(Value::Bool(true)) => min.map(|m| m + 1.0),
        _ => min,
    }
}

fn string_example(s: &Value) -> String {
    match str_at(s, "format") {
        "date-time" => "2024-01-01T00:00:00Z",
        "date" => "2024-01-01",
        "time" => "12:00:00",
        "email" | "idn-email" => "user@example.com",
        "uuid" => "3fa85f64-5717-4562-b3fc-2c963f66afa6",
        "uri" | "url" | "uri-reference" | "iri" => "https://example.com",
        "hostname" | "idn-hostname" => "example.com",
        "ipv4" => "192.0.2.1",
        "ipv6" => "2001:db8::1",
        "byte" => "ZXhhbXBsZQ==",
        "binary" => "",
        "password" => "password",
        _ => "string",
    }
    .to_string()
}

fn param_string(v: &Value) -> String {
    match v {
        Value::Array(a) => a.iter().map(param_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => v.to_string(),
        other => scalar_string(other),
    }
}

fn pretty(v: &Value) -> String {
    match v {
        Value::Null => "{}".into(),
        v => serde_json::to_string_pretty(v).unwrap_or_default(),
    }
}

fn root_name(schema: Option<&Value>) -> String {
    let Some(s) = schema else { return "root".into() };
    if let Some(n) = s.pointer("/xml/name").and_then(Value::as_str) {
        return n.to_string();
    }
    s.get("$ref").and_then(Value::as_str).and_then(|r| r.rsplit('/').next()).map(str::to_string).unwrap_or_else(|| "root".into())
}

fn to_xml(root: &str, v: &Value) -> String {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }
    fn walk(name: &str, v: &Value, indent: usize, out: &mut String) {
        let pad = "  ".repeat(indent);
        match v {
            Value::Object(m) => {
                out.push_str(&format!("{pad}<{name}>\n"));
                for (k, x) in m {
                    walk(k, x, indent + 1, out);
                }
                out.push_str(&format!("{pad}</{name}>\n"));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(name, x, indent, out)),
            other => out.push_str(&format!("{pad}<{name}>{}</{name}>\n", esc(&scalar_string(other)))),
        }
    }
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    walk(root, v, 0, &mut out);
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_paths() {
        assert_eq!(template_path("/pets/{petId}/photos/{ id }"), "/pets/{{petId}}/photos/{{id}}");
        assert_eq!(template_path("/a/{broken"), "/a/{broken");
    }

    #[test]
    fn examples_respect_required_readonly_and_cycles() {
        let doc: Value = serde_json::from_str(
            r##"{"openapi":"3.1.0","components":{"schemas":{
              "Node":{"type":"object","required":["name","child"],"properties":{"id":{"type":"integer","readOnly":true},"name":{"type":"string"},"child":{"$ref":"#/components/schemas/Node"},"tags":{"type":"array","items":{"type":"string","format":"uuid"}}}},
              "Status":{"type":["string","null"],"enum":["active","disabled"]}
            }}}"##,
        )
        .unwrap();
        let mut cx = Cx { doc: &doc, swagger2: false, w: Warnings::default(), secrets: Vec::new() };
        let schema = json!({"$ref": "#/components/schemas/Node"});
        let ex = cx.example(&schema, 0, &mut Vec::new(), true);
        assert_eq!(ex["name"], "string");
        assert!(ex.get("id").is_none(), "readOnly skipped: {ex}");
        assert_eq!(ex["tags"][0], "3fa85f64-5717-4562-b3fc-2c963f66afa6");
        assert_eq!(ex["child"], json!({}), "cycle cut: {ex}");
        let st = json!({"$ref": "#/components/schemas/Status"});
        assert_eq!(cx.example(&st, 0, &mut Vec::new(), true), "active");
    }
}
