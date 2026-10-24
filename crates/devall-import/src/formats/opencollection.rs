//! OpenCollection YAML (Bruno ≥ 3.1): a bundled `opencollection.yml` with `items`, or a folder
//! with `opencollection.yml`, `folder.yml` per folder, one `.yml` per request and
//! `environments/*.yml`. Schema: `@opencollection/schema` 0.14.

use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use devall_core::{
    ApiKeyLocation, Auth, Body, Capture, CaptureSource, EnvironmentFile, FolderFile, HttpRequestFile, KeyValue, MultipartPart, OAuth2ClientAuth,
    OAuth2Config, OAuth2Grant,
};

use crate::detect::Format;
use crate::error::{Error, Result, io_err};
use crate::plan::{ImportPlan, PlanFolder, PlanItem};
use crate::util::{
    Warnings, add_path_vars, arr_at, bool_at, colon_path_vars, default_name, folder_has_settings, inline_vars, parse_structured, scalar_string, split_query,
    str_at, text_at,
};

pub(crate) fn parse_bundle(v: &Value) -> Result<ImportPlan> {
    let mut plan = ImportPlan::new(name_of(v, "OpenCollection"), Format::OpenCollection);
    let mut w = Warnings::default();
    let mut settings = request_defaults(v.get("request"), &mut w, "collection");
    if v.get("http").is_some() || v.get("graphql").is_some() {
        // A single request document.
        if let Some((PlanItem::Request(mut r), _, vars)) = item(v, &mut w) {
            add_path_vars(&mut settings, &mut r, vars);
            plan.items.push(PlanItem::Request(r));
        }
    } else {
        plan.items = items(arr_at(v, "items"), &mut settings, &mut w);
    }
    plan.settings = settings;
    for e in v.pointer("/config/environments").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        plan.environments.push(environment(e, "Environment"));
    }
    plan.warnings = w.finish();
    Ok(plan)
}

pub(crate) fn parse_dir(root: &Path) -> Result<ImportPlan> {
    let root_file = ["opencollection.yml", "opencollection.yaml"].iter().map(|f| root.join(f)).find(|p| p.is_file());
    let root_doc = match &root_file {
        Some(p) => parse_structured(&std::fs::read_to_string(p).map_err(io_err(p))?)?,
        None => Value::Null,
    };
    if !arr_at(&root_doc, "items").is_empty() {
        return parse_bundle(&root_doc);
    }
    let dir_name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "OpenCollection".into());
    let mut plan = ImportPlan::new(name_of(&root_doc, &dir_name), Format::OpenCollection);
    let mut w = Warnings::default();
    let mut settings = request_defaults(root_doc.get("request"), &mut w, "collection");
    for e in root_doc.pointer("/config/environments").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        plan.environments.push(environment(e, "Environment"));
    }
    let env_dir = root.join("environments");
    if let Ok(rd) = std::fs::read_dir(&env_dir) {
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| is_yaml(p)).collect();
        files.sort();
        for f in files {
            let v = parse_structured(&std::fs::read_to_string(&f).map_err(io_err(&f))?)?;
            let stem = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            plan.environments.push(environment(&v, &stem));
        }
    }
    plan.items = walk(root, true, &mut settings, &mut w)?;
    plan.settings = settings;
    if plan.items.is_empty() && plan.environments.is_empty() {
        return Err(Error::Parse(format!("no OpenCollection requests found in {}", root.display())));
    }
    plan.warnings = w.finish();
    Ok(plan)
}

fn is_yaml(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
}

fn name_of(v: &Value, default: &str) -> String {
    v.pointer("/info/name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).unwrap_or(default).to_string()
}

fn walk(dir: &Path, is_root: bool, settings: &mut Option<FolderFile>, w: &mut Warnings) -> Result<Vec<PlanItem>> {
    let mut entries: Vec<(f64, String, PlanItem)> = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(dir).map_err(io_err(dir))?.flatten().map(|e| e.path()).collect();
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
            let folder_doc = ["folder.yml", "folder.yaml"]
                .iter()
                .map(|f| p.join(f))
                .find(|f| f.is_file())
                .map(|f| std::fs::read_to_string(&f).map_err(io_err(&f)))
                .transpose()?
                .map(|t| parse_structured(&t))
                .transpose()?
                .unwrap_or(Value::Null);
            let name = name_of(&folder_doc, &file);
            let seq = folder_doc.pointer("/info/seq").and_then(Value::as_f64).unwrap_or(f64::MAX);
            let mut f = PlanFolder::new(name.clone());
            let mut s = request_defaults(folder_doc.get("request"), w, &name);
            f.items = walk(&p, false, &mut s, w)?;
            f.settings = s;
            if !f.items.is_empty() || f.settings.is_some() {
                entries.push((seq, name.to_lowercase(), PlanItem::Folder(f)));
            }
        } else if is_yaml(&p) && !["opencollection.yml", "opencollection.yaml", "folder.yml", "folder.yaml"].contains(&file.as_str()) {
            let v = parse_structured(&std::fs::read_to_string(&p).map_err(io_err(&p))?)?;
            if let Some((PlanItem::Request(mut r), seq, vars)) = item(&v, w) {
                if r.name.is_empty() {
                    r.name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                }
                add_path_vars(settings, &mut r, vars);
                entries.push((seq, r.name.to_lowercase(), PlanItem::Request(r)));
            }
        }
    }
    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
    Ok(entries.into_iter().map(|(_, _, it)| it).collect())
}

fn items(list: &[Value], settings: &mut Option<FolderFile>, w: &mut Warnings) -> Vec<PlanItem> {
    let mut entries: Vec<(f64, usize, PlanItem)> = Vec::new();
    for (i, v) in list.iter().enumerate() {
        if let Some((mut it, seq, vars)) = item(v, w) {
            if let PlanItem::Request(r) = &mut it {
                add_path_vars(settings, r, vars);
            }
            entries.push((seq, i, it));
        }
    }
    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    entries.into_iter().map(|(_, _, it)| it).collect()
}

/// An item with its sequence number and path-parameter values.
type SeqItem = (PlanItem, f64, Vec<(String, String)>);

/// One item: folder (recursively) or HTTP/GraphQL request, with its sequence and path variables.
fn item(v: &Value, w: &mut Warnings) -> Option<SeqItem> {
    let info = v.get("info").unwrap_or(&Value::Null);
    let seq = info.get("seq").and_then(Value::as_f64).unwrap_or(f64::MAX);
    let name = str_at(info, "name").trim().to_string();
    if v.get("items").is_some() || str_at(info, "type") == "folder" {
        let mut f = PlanFolder::new(if name.is_empty() { "Folder".to_string() } else { name.clone() });
        let mut s = request_defaults(v.get("request"), w, &f.name);
        f.items = items(arr_at(v, "items"), &mut s, w);
        f.settings = s;
        return Some((PlanItem::Folder(f), seq, Vec::new()));
    }
    let (details, graphql) = match (v.get("http"), v.get("graphql")) {
        (Some(h), _) => (h, false),
        (None, Some(g)) => (g, true),
        _ => {
            if v.get("grpc").is_some() || v.get("websocket").is_some() {
                w.about("gRPC and WebSocket requests are not supported yet and were skipped", &name);
            }
            return None;
        }
    };
    let method = str_at(details, "method").trim().to_uppercase();
    let method = if method.is_empty() { if graphql { "POST".to_string() } else { "GET".to_string() } } else { method };
    let (base, url_params) = split_query(str_at(details, "url"));
    let (url, _) = colon_path_vars(&base);
    let mut req = HttpRequestFile::new(if name.is_empty() { default_name(&method, &url) } else { name });
    req.method = method;
    req.url = url;
    let params = arr_at(details, "params");
    let mut path_vars = Vec::new();
    if params.is_empty() {
        req.params = url_params;
    }
    for p in params {
        if str_at(p, "type") == "path" {
            path_vars.push((str_at(p, "name").to_string(), text_at(p, "value")));
        } else {
            req.params.push(kv(p));
        }
    }
    req.headers = arr_at(details, "headers").iter().map(kv).collect();
    req.auth = auth(details.get("auth"), w, &req.name.clone());
    let subject = req.name.clone();
    req.body = if graphql { graphql_body(details.get("body")) } else { body(details.get("body"), w, &subject) };

    let runtime = v.get("runtime").unwrap_or(&Value::Null);
    let vars: IndexMap<String, String> = arr_at(runtime, "variables")
        .iter()
        .filter(|x| bool_at(x, "disabled") != Some(true))
        .map(|x| (str_at(x, "name").to_string(), variable_value(x.get("value"))))
        .collect();
    inline_vars(&mut req, &vars);
    let mut kept: Vec<String> = Vec::new();
    for a in arr_at(runtime, "assertions") {
        let expr = str_at(a, "expression");
        let value = a.get("value").map(scalar_string);
        match super::bruno::assertion(expr, str_at(a, "operator"), value.as_deref()) {
            Some(mut x) => {
                x.enabled = bool_at(a, "disabled") != Some(true);
                req.tests.push(x);
            }
            None => kept.push(format!("assert {expr} {} {}", str_at(a, "operator"), value.unwrap_or_default())),
        }
    }
    for act in arr_at(runtime, "actions") {
        if str_at(act, "type") != "set-variable" || str_at(act, "phase") == "before-request" {
            continue;
        }
        let name = act.pointer("/variable/name").and_then(Value::as_str).unwrap_or("");
        let expr = act.pointer("/selector/expression").and_then(Value::as_str).unwrap_or("").trim();
        let source = if expr.starts_with('$') && !expr.starts_with("$res") {
            Some(CaptureSource::JsonPath { path: expr.to_string() })
        } else {
            super::bruno::capture_source(expr)
        };
        match source {
            Some(source) if !name.is_empty() => req.captures.push(Capture { name: name.to_string(), source, enabled: bool_at(act, "disabled") != Some(true) }),
            _ => kept.push(format!("set {name} = {expr}")),
        }
    }
    let mut docs: Vec<String> = Vec::new();
    if let Some(d) = docs_of(v.get("docs")) {
        docs.push(d);
    }
    if !kept.is_empty() {
        w.about("Some assertions/actions have no equivalent; they were kept in the request docs", &req.name);
        docs.push(format!("Not converted:\n\n```\n{}\n```", kept.join("\n")));
    }
    for s in arr_at(runtime, "scripts") {
        let code = str_at(s, "code");
        if !code.trim().is_empty() {
            docs.push(format!("### {} script (OpenCollection)\n\n```js\n{}\n```", str_at(s, "type"), code.trim_end()));
            w.about("Scripts are not run by /dev/all; they were kept in the request docs", &req.name);
        }
    }
    if !docs.is_empty() {
        req.docs = Some(docs.join("\n\n"));
    }
    if let Some(s) = v.get("settings") {
        if bool_at(s, "followRedirects") == Some(false) {
            req.settings.follow_redirects = false;
        }
        if let Some(t) = s.get("timeout").and_then(Value::as_u64).filter(|t| *t > 0) {
            req.settings.timeout_ms = t;
        }
    }
    Some((PlanItem::Request(Box::new(req)), seq, path_vars))
}

fn kv(x: &Value) -> KeyValue {
    let mut kv = KeyValue::new(str_at(x, "name"), text_at(x, "value"));
    kv.enabled = bool_at(x, "disabled") != Some(true);
    kv
}

fn docs_of(d: Option<&Value>) -> Option<String> {
    let t = match d? {
        Value::String(s) => s.clone(),
        o @ Value::Object(_) => text_at(o, "content"),
        _ => String::new(),
    };
    (!t.trim().is_empty()).then(|| t.trim().to_string())
}

/// A variable value: a string, `{type, data}`, or a list of variants (selected or first).
fn variable_value(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(variants)) => {
            let chosen = variants.iter().find(|x| bool_at(x, "selected") == Some(true)).or_else(|| variants.first());
            variable_value(chosen.and_then(|c| c.get("value")))
        }
        Some(o @ Value::Object(_)) => text_at(o, "data"),
        Some(other) => scalar_string(other),
        None => String::new(),
    }
}

fn request_defaults(r: Option<&Value>, w: &mut Warnings, name: &str) -> Option<FolderFile> {
    let r = r?;
    let mut s = FolderFile { headers: arr_at(r, "headers").iter().map(kv).collect(), ..Default::default() };
    s.auth = auth(r.get("auth"), w, name);
    s.variables = arr_at(r, "variables")
        .iter()
        .filter(|x| bool_at(x, "disabled") != Some(true) && !str_at(x, "name").is_empty())
        .map(|x| (str_at(x, "name").to_string(), variable_value(x.get("value"))))
        .collect();
    if !arr_at(r, "scripts").is_empty() {
        w.about("Folder/collection-level scripts were not imported", name);
    }
    folder_has_settings(&s).then_some(s)
}

fn environment(v: &Value, fallback: &str) -> EnvironmentFile {
    let name = Some(str_at(v, "name").trim()).filter(|n| !n.is_empty()).unwrap_or(fallback);
    let mut env = EnvironmentFile::new(name);
    env.color = Some(str_at(v, "color").to_string()).filter(|c| !c.is_empty());
    for x in arr_at(v, "variables") {
        let n = str_at(x, "name");
        if n.is_empty() || bool_at(x, "disabled") == Some(true) {
            continue;
        }
        if bool_at(x, "secret") == Some(true) {
            env.secrets.push(n.to_string());
            if let Some(val) = x.get("value") {
                env.variables.insert(n.to_string(), variable_value(Some(val)));
            }
        } else {
            env.variables.insert(n.to_string(), variable_value(x.get("value")));
        }
    }
    env
}

fn selected_body(b: Option<&Value>) -> Option<&Value> {
    match b? {
        Value::Array(variants) => variants.iter().find(|x| bool_at(x, "selected") == Some(true)).or_else(|| variants.first()).and_then(|x| x.get("body")),
        other => Some(other),
    }
}

fn body(b: Option<&Value>, w: &mut Warnings, subject: &str) -> Body {
    let Some(b) = selected_body(b) else { return Body::None };
    match str_at(b, "type") {
        "json" => Body::Json { content: text_at(b, "data") },
        "xml" => Body::Xml { content: text_at(b, "data") },
        "text" => Body::Text { content: text_at(b, "data"), content_type: None },
        "sparql" => Body::Text { content: text_at(b, "data"), content_type: Some("application/sparql-query".into()) },
        "form-urlencoded" => Body::Form { fields: arr_at(b, "data").iter().map(kv).collect() },
        "multipart-form" => Body::Multipart {
            parts: arr_at(b, "data")
                .iter()
                .map(|p| {
                    let is_file = str_at(p, "type") == "file";
                    let files: Vec<String> = match p.get("value") {
                        Some(Value::Array(a)) => a.iter().map(scalar_string).collect(),
                        Some(v) => vec![scalar_string(v)],
                        None => Vec::new(),
                    };
                    if is_file && files.len() > 1 {
                        w.about("Multipart fields with several files keep only the first file", subject);
                    }
                    MultipartPart {
                        name: str_at(p, "name").to_string(),
                        value: if is_file { String::new() } else { files.first().cloned().unwrap_or_default() },
                        file: is_file.then(|| files.first().cloned().unwrap_or_default()),
                        content_type: Some(str_at(p, "contentType").to_string()).filter(|c| !c.is_empty()),
                        enabled: bool_at(p, "disabled") != Some(true),
                    }
                })
                .collect(),
        },
        "file" => {
            let data = arr_at(b, "data");
            match data.iter().find(|x| bool_at(x, "selected") == Some(true)).or_else(|| data.first()) {
                Some(f) => Body::Binary { path: str_at(f, "filePath").to_string(), content_type: Some(str_at(f, "contentType").to_string()).filter(|c| !c.is_empty()) },
                None => Body::None,
            }
        }
        _ => Body::None,
    }
}

fn graphql_body(b: Option<&Value>) -> Body {
    match selected_body(b) {
        Some(g) => Body::Graphql { query: text_at(g, "query"), variables: text_at(g, "variables"), operation_name: None },
        None => Body::None,
    }
}

fn auth(a: Option<&Value>, w: &mut Warnings, subject: &str) -> Auth {
    let Some(a) = a else { return Auth::Inherit };
    if let Value::String(s) = a {
        return if s == "none" { Auth::None } else { Auth::Inherit };
    }
    let s = |k: &str| text_at(a, k);
    match str_at(a, "type") {
        "none" => Auth::None,
        "inherit" | "" => Auth::Inherit,
        "basic" => Auth::Basic { username: s("username"), password: s("password") },
        "bearer" => Auth::Bearer { token: s("token"), prefix: None },
        "digest" => Auth::Digest { username: s("username"), password: s("password") },
        "ntlm" => Auth::Ntlm { username: s("username"), password: s("password"), domain: Some(s("domain")).filter(|d| !d.is_empty()), workstation: None },
        "apikey" => Auth::ApiKey { key: s("key"), value: s("value"), location: if s("placement") == "query" { ApiKeyLocation::Query } else { ApiKeyLocation::Header } },
        "awsv4" => Auth::AwsSigV4 {
            access_key_id: s("accessKeyId"),
            secret_access_key: s("secretAccessKey"),
            session_token: Some(s("sessionToken")).filter(|t| !t.is_empty()),
            region: s("region"),
            service: s("service"),
        },
        "oauth2" => {
            let grant = match str_at(a, "flow") {
                "client_credentials" => OAuth2Grant::ClientCredentials,
                "resource_owner_password_credentials" => OAuth2Grant::Password,
                "authorization_code" => OAuth2Grant::AuthorizationCode,
                _ => {
                    w.about("OAuth 2.0 implicit flow is not supported; converted to a bearer token", subject);
                    return Auth::Bearer { token: String::new(), prefix: None };
                }
            };
            let creds = a.get("credentials").unwrap_or(&Value::Null);
            let owner = a.get("resourceOwner").unwrap_or(&Value::Null);
            let mut extra = IndexMap::new();
            if let Some(params) = a.get("additionalParameters").and_then(Value::as_object) {
                for (k, list) in params {
                    if k == "refreshTokenRequest" {
                        continue;
                    }
                    for p in list.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                        if bool_at(p, "enabled") != Some(false) && !str_at(p, "name").is_empty() {
                            extra.insert(str_at(p, "name").to_string(), text_at(p, "value"));
                        }
                    }
                }
            }
            Auth::OAuth2(OAuth2Config {
                grant,
                auth_url: s("authorizationUrl"),
                token_url: s("accessTokenUrl"),
                client_id: text_at(creds, "clientId"),
                client_secret: text_at(creds, "clientSecret"),
                scope: s("scope"),
                username: text_at(owner, "username"),
                password: text_at(owner, "password"),
                use_pkce: a.pointer("/pkce/enabled").and_then(Value::as_bool).unwrap_or(grant == OAuth2Grant::AuthorizationCode),
                redirect_port: super::postman::redirect_port(&s("callbackUrl")),
                client_auth: if str_at(creds, "placement") == "body" { OAuth2ClientAuth::Body } else { OAuth2ClientAuth::Basic },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_with_folders_envs_and_runtime() {
        let yaml = r#"
opencollection: 1.0.0
info:
  name: Shop
request:
  auth:
    type: bearer
    token: "{{token}}"
config:
  environments:
    - name: Dev
      variables:
        - name: baseUrl
          value: http://localhost:3000
        - secret: true
          name: token
items:
  - info: { name: Orders, type: folder, seq: 2 }
    items:
      - info: { name: Create order, type: http, seq: 1 }
        http:
          method: post
          url: "{{baseUrl}}/orders/:shop"
          params:
            - { name: shop, value: main, type: path }
            - { name: dry, value: "true", type: query, disabled: true }
          headers:
            - { name: X-Trace, value: "1" }
          body:
            type: json
            data: '{"sku": "A1"}'
          auth: inherit
        runtime:
          assertions:
            - { expression: res.status, operator: eq, value: "201" }
          actions:
            - type: set-variable
              phase: after-response
              selector: { expression: $.id, method: jsonq }
              variable: { name: orderId, scope: runtime }
  - info: { name: Health, type: http, seq: 1 }
    http: { method: GET, url: "{{baseUrl}}/health" }
"#;
        let v = crate::util::parse_structured(yaml).unwrap();
        let plan = parse_bundle(&v).unwrap();
        assert_eq!(plan.name, "Shop");
        let reqs = plan.requests();
        assert_eq!(reqs[0].1.name, "Health", "sorted by seq");
        let (path, create) = &reqs[1];
        assert_eq!(path, &vec!["Orders".to_string()]);
        assert_eq!(create.url, "{{baseUrl}}/orders/{{shop}}");
        assert!(!create.params[0].enabled);
        assert_eq!(create.body, Body::Json { content: "{\"sku\": \"A1\"}".into() });
        assert_eq!(create.tests.len(), 1);
        assert_eq!(create.captures[0].source, CaptureSource::JsonPath { path: "$.id".into() });
        let folder = plan.folders()[0].1;
        assert_eq!(folder.settings.as_ref().unwrap().variables.get("shop").map(String::as_str), Some("main"));
        assert!(matches!(plan.settings.as_ref().unwrap().auth, Auth::Bearer { .. }));
        assert_eq!(plan.environments[0].secrets, vec!["token"]);
    }
}
