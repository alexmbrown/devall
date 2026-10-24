//! Insomnia v4 exports (JSON `resources`) and v5 collections (YAML `collection.insomnia.rest/5.0`).

use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::Value;

use devall_core::{
    ApiKeyLocation, Auth, Body, EnvironmentFile, FolderFile, HttpRequestFile, KeyValue, MultipartPart, OAuth2ClientAuth, OAuth2Config, OAuth2Grant,
};

use crate::detect::Format;
use crate::error::{Error, Result};
use crate::plan::{ImportPlan, PlanFolder, PlanItem};
use crate::util::{
    Warnings, add_path_vars, arr_at, body_from_text, bool_at, colon_path_vars, default_name, folder_has_settings, header_value, map_templates,
    scalar_string, split_query, str_at, text_at,
};

const FILES_MOVED: &str = "File paths in multipart/binary bodies point to the original machine; re-select the files";

pub(crate) fn parse(v: &Value) -> Result<ImportPlan> {
    let ty = str_at(v, "type");
    if ty.starts_with("collection.insomnia.rest/5") {
        return Ok(parse_v5(v));
    }
    if ty.starts_with("spec.insomnia.rest/5") {
        if v.get("collection").is_some_and(Value::is_array) {
            return Ok(parse_v5(v));
        }
        if let Some(contents) = v.pointer("/spec/contents") {
            let spec = match contents {
                Value::String(s) => crate::util::parse_structured(s)?,
                other => other.clone(),
            };
            let mut plan = super::openapi::parse(&spec)?;
            plan.warnings.insert(0, "Imported the API spec of an Insomnia design document".into());
            return Ok(plan);
        }
        return Err(Error::Parse("Insomnia design document without a collection or spec".into()));
    }
    if ty.starts_with("environment.insomnia.rest/5") {
        let mut plan = ImportPlan::new(text_or(v, "name", "Insomnia environments"), Format::Insomnia);
        envs_v5(v, &mut plan);
        return Ok(plan);
    }
    if str_at(v, "_type") == "export" {
        return Ok(parse_v4(v));
    }
    Err(Error::Parse("not an Insomnia export (expected `_type: export` or a v5 `type`)".into()))
}

fn text_or<'a>(v: &'a Value, key: &str, default: &'a str) -> &'a str {
    Some(str_at(v, key).trim()).filter(|s| !s.is_empty()).unwrap_or(default)
}

// ---------- v4 ----------

fn parse_v4(v: &Value) -> ImportPlan {
    let resources = arr_at(v, "resources");
    let mut children: HashMap<&str, Vec<&Value>> = HashMap::new();
    let ids: std::collections::HashSet<&str> = resources.iter().map(|r| str_at(r, "_id")).collect();
    for r in resources {
        children.entry(str_at(r, "parentId")).or_default().push(r);
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| sort_key(a).partial_cmp(&sort_key(b)).unwrap_or(std::cmp::Ordering::Equal));
    }
    let workspaces: Vec<&Value> = resources.iter().filter(|r| str_at(r, "_type") == "workspace").collect();
    let mut w = Warnings::default();
    let mut plan;
    match workspaces.as_slice() {
        [ws] => {
            plan = ImportPlan::new(text_or(ws, "name", "Insomnia"), Format::Insomnia);
            let mut settings = None;
            plan.items = build_v4(str_at(ws, "_id"), &children, &mut settings, &mut w);
            envs_v4(str_at(ws, "_id"), &children, &mut plan, &mut settings, None);
            plan.settings = settings;
        }
        [] => {
            plan = ImportPlan::new("Insomnia", Format::Insomnia);
            let mut settings = None;
            for root in resources.iter().filter(|r| !ids.contains(str_at(r, "parentId"))) {
                let parent = str_at(root, "parentId");
                if plan.items.is_empty() {
                    plan.items = build_v4(parent, &children, &mut settings, &mut w);
                }
            }
            plan.settings = settings;
        }
        many => {
            plan = ImportPlan::new("Insomnia export", Format::Insomnia);
            for ws in many {
                let mut f = PlanFolder::new(text_or(ws, "name", "Workspace"));
                let mut settings = None;
                f.items = build_v4(str_at(ws, "_id"), &children, &mut settings, &mut w);
                envs_v4(str_at(ws, "_id"), &children, &mut plan, &mut settings, Some(&f.name));
                f.settings = settings;
                plan.items.push(PlanItem::Folder(f));
            }
        }
    }
    let api_specs = resources.iter().filter(|r| str_at(r, "_type") == "api_spec" && !str_at(r, "contents").trim().is_empty()).count();
    if api_specs > 0 {
        w.add("Embedded API specs were not imported as requests; import the spec file itself to generate requests from it");
    }
    plan.warnings = w.finish();
    plan
}

fn sort_key(r: &Value) -> f64 {
    r.get("metaSortKey").and_then(Value::as_f64).or_else(|| r.pointer("/meta/sortKey").and_then(Value::as_f64)).unwrap_or(0.0)
}

fn build_v4(parent: &str, children: &HashMap<&str, Vec<&Value>>, settings: &mut Option<FolderFile>, w: &mut Warnings) -> Vec<PlanItem> {
    let mut out = Vec::new();
    for r in children.get(parent).map(Vec::as_slice).unwrap_or(&[]) {
        match str_at(r, "_type") {
            "request_group" => {
                let mut f = PlanFolder::new(text_or(r, "name", "Folder"));
                let mut s = folder_settings(r, w, &f.name);
                f.items = build_v4(str_at(r, "_id"), children, &mut s, w);
                f.settings = s;
                out.push(PlanItem::Folder(f));
            }
            "request" => {
                let (mut req, path_vars) = request(r, w);
                add_path_vars(settings, &mut req, path_vars);
                out.push(PlanItem::Request(Box::new(req)));
            }
            "grpc_request" | "websocket_request" | "socketio_request" => {
                w.about("gRPC, WebSocket and Socket.IO requests are not supported yet and were skipped", str_at(r, "name"));
            }
            "unit_test_suite" => w.about("Insomnia unit test suites were not imported", str_at(r, "name")),
            _ => {}
        }
    }
    out
}

/// Base environment → collection (folder) variables; sub-environments → environments.
fn envs_v4(ws_id: &str, children: &HashMap<&str, Vec<&Value>>, plan: &mut ImportPlan, settings: &mut Option<FolderFile>, prefix: Option<&str>) {
    let Some(base) = children.get(ws_id).and_then(|l| l.iter().find(|r| str_at(r, "_type") == "environment")) else { return };
    let vars = flatten_env(base.get("data"));
    if !vars.is_empty() {
        settings.get_or_insert_with(FolderFile::default).variables.extend(vars);
    }
    for sub in children.get(str_at(base, "_id")).map(Vec::as_slice).unwrap_or(&[]) {
        if str_at(sub, "_type") != "environment" {
            continue;
        }
        let name = text_or(sub, "name", "Environment");
        let mut env = EnvironmentFile::new(match prefix {
            Some(p) => format!("{p} – {name}"),
            None => name.to_string(),
        });
        env.variables = flatten_env(sub.get("data"));
        env.color = Some(str_at(sub, "color").to_string()).filter(|c| !c.is_empty());
        plan.environments.push(env);
    }
}

// ---------- v5 ----------

fn parse_v5(v: &Value) -> ImportPlan {
    let mut plan = ImportPlan::new(text_or(v, "name", "Insomnia"), Format::Insomnia);
    let mut w = Warnings::default();
    let mut settings = None;
    plan.items = build_v5(arr_at(v, "collection"), &mut settings, &mut w);
    if let Some(envs) = v.get("environments") {
        let vars = flatten_env(envs.get("data"));
        if !vars.is_empty() {
            settings.get_or_insert_with(FolderFile::default).variables.extend(vars);
        }
        for sub in arr_at(envs, "subEnvironments") {
            let mut env = EnvironmentFile::new(text_or(sub, "name", "Environment"));
            env.variables = flatten_env(sub.get("data"));
            env.color = Some(str_at(sub, "color").to_string()).filter(|c| !c.is_empty());
            plan.environments.push(env);
        }
    }
    plan.settings = settings;
    plan.warnings = w.finish();
    plan
}

fn envs_v5(v: &Value, plan: &mut ImportPlan) {
    let base = v.get("environments").unwrap_or(v);
    plan.project_variables.extend(flatten_env(base.get("data")));
    for sub in arr_at(base, "subEnvironments") {
        let mut env = EnvironmentFile::new(text_or(sub, "name", "Environment"));
        env.variables = flatten_env(sub.get("data"));
        plan.environments.push(env);
    }
}

fn build_v5(items: &[Value], settings: &mut Option<FolderFile>, w: &mut Warnings) -> Vec<PlanItem> {
    let mut out = Vec::new();
    for it in items {
        if it.get("children").is_some_and(Value::is_array) {
            let mut f = PlanFolder::new(text_or(it, "name", "Folder"));
            let mut s = folder_settings(it, w, &f.name);
            f.items = build_v5(arr_at(it, "children"), &mut s, w);
            f.settings = s;
            out.push(PlanItem::Folder(f));
        } else if it.get("url").is_some() || it.get("method").is_some() {
            let (mut req, path_vars) = request(it, w);
            add_path_vars(settings, &mut req, path_vars);
            out.push(PlanItem::Request(Box::new(req)));
        } else if it.get("protoFileId").is_some() || str_at(it, "url").starts_with("ws") {
            w.about("gRPC and WebSocket requests are not supported yet and were skipped", str_at(it, "name"));
        }
    }
    out
}

// ---------- shared ----------

fn folder_settings(r: &Value, w: &mut Warnings, name: &str) -> Option<FolderFile> {
    let mut s = FolderFile { variables: flatten_env(r.get("environment")), ..Default::default() };
    s.headers = kv(arr_at(r, "headers"));
    s.auth = auth(r.get("authentication"), w, name);
    if has_scripts(r) {
        w.about("Insomnia scripts are not run by /dev/all and were not imported", name);
    }
    folder_has_settings(&s).then_some(s)
}

fn has_scripts(r: &Value) -> bool {
    ["preRequestScript", "afterResponseScript"].iter().any(|k| !str_at(r, k).trim().is_empty())
        || r.get("scripts").is_some_and(|s| ["preRequest", "afterResponse"].iter().any(|k| !str_at(s, k).trim().is_empty()))
}

fn request(r: &Value, w: &mut Warnings) -> (HttpRequestFile, Vec<(String, String)>) {
    let mut req = HttpRequestFile::new(str_at(r, "name").trim());
    let method = str_at(r, "method").trim().to_uppercase();
    req.method = if method.is_empty() { "GET".into() } else { method };
    let (base, mut query) = split_query(&conv(str_at(r, "url"), w, &req.name));
    let (url, _) = colon_path_vars(&base);
    req.url = url;
    let mut params = kv_conv(arr_at(r, "parameters"), w, &req.name);
    params.append(&mut query);
    req.params = params;
    req.headers = kv_conv(arr_at(r, "headers"), w, &req.name);
    req.auth = auth(r.get("authentication"), w, &req.name);
    let subject = req.name.clone();
    req.body = body(r.get("body"), &req.headers, w, &subject);
    if req.name.is_empty() {
        req.name = default_name(&req.method, &req.url);
    }
    let follow = Some(str_at(r, "settingFollowRedirects")).filter(|s| !s.is_empty()).or_else(|| r.pointer("/settings/followRedirects").and_then(Value::as_str));
    if follow == Some("off") {
        req.settings.follow_redirects = false;
    }
    let mut docs = Vec::new();
    let description = Some(text_at(r, "description")).filter(|d| !d.trim().is_empty()).or_else(|| r.pointer("/meta/description").map(scalar_string).filter(|d| !d.trim().is_empty()));
    if let Some(d) = description {
        docs.push(d.trim().to_string());
    }
    let pre = Some(text_at(r, "preRequestScript")).filter(|s| !s.trim().is_empty()).or_else(|| r.pointer("/scripts/preRequest").map(scalar_string).filter(|s| !s.trim().is_empty()));
    let post = Some(text_at(r, "afterResponseScript")).filter(|s| !s.trim().is_empty()).or_else(|| r.pointer("/scripts/afterResponse").map(scalar_string).filter(|s| !s.trim().is_empty()));
    for (title, code) in [("Pre-request script", pre), ("After-response script", post)] {
        if let Some(code) = code {
            docs.push(format!("### {title} (Insomnia)\n\n```js\n{}\n```", code.trim_end()));
            w.about("Insomnia scripts are not run by /dev/all; they were kept in the request docs", &req.name);
        }
    }
    if !docs.is_empty() {
        req.docs = Some(docs.join("\n\n"));
    }
    let path_vars = arr_at(r, "pathParameters").iter().map(|p| (str_at(p, "name").to_string(), conv(&text_at(p, "value"), w, ""))).filter(|(n, _)| !n.is_empty()).collect();
    (req, path_vars)
}

fn kv(list: &[Value]) -> Vec<KeyValue> {
    list.iter()
        .filter(|x| !str_at(x, "name").is_empty() || !text_at(x, "value").is_empty())
        .map(|x| {
            let mut kv = KeyValue::new(str_at(x, "name"), text_at(x, "value"));
            kv.enabled = bool_at(x, "disabled") != Some(true);
            kv.description = Some(str_at(x, "description").to_string()).filter(|d| !d.is_empty());
            kv
        })
        .collect()
}

fn kv_conv(list: &[Value], w: &mut Warnings, subject: &str) -> Vec<KeyValue> {
    let mut out = kv(list);
    for x in &mut out {
        x.name = conv(&x.name, w, subject);
        x.value = conv(&x.value, w, subject);
    }
    out
}

fn body(b: Option<&Value>, headers: &[KeyValue], w: &mut Warnings, subject: &str) -> Body {
    let Some(b) = b.filter(|b| b.is_object()) else { return Body::None };
    let mime = str_at(b, "mimeType").to_ascii_lowercase();
    let text = conv(&text_at(b, "text"), w, subject);
    match mime.as_str() {
        m if m.contains("x-www-form-urlencoded") => Body::Form { fields: kv_conv(arr_at(b, "params"), w, subject) },
        m if m.starts_with("multipart/") => Body::Multipart {
            parts: arr_at(b, "params")
                .iter()
                .map(|p| {
                    let is_file = str_at(p, "type") == "file";
                    if is_file {
                        w.about(FILES_MOVED, subject);
                    }
                    MultipartPart {
                        name: conv(str_at(p, "name"), w, subject),
                        value: if is_file { String::new() } else { conv(&text_at(p, "value"), w, subject) },
                        file: is_file.then(|| str_at(p, "fileName").to_string()),
                        content_type: None,
                        enabled: bool_at(p, "disabled") != Some(true),
                    }
                })
                .collect(),
        },
        "application/graphql" => {
            let g: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let variables = match g.get("variables") {
                Some(v @ (Value::Object(_) | Value::Array(_))) => serde_json::to_string_pretty(v).unwrap_or_default(),
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            Body::Graphql {
                query: if g.is_null() { text } else { text_at(&g, "query") },
                variables,
                operation_name: Some(text_at(&g, "operationName")).filter(|o| !o.is_empty()),
            }
        }
        _ if !str_at(b, "fileName").is_empty() => {
            w.about(FILES_MOVED, subject);
            Body::Binary { path: str_at(b, "fileName").to_string(), content_type: Some(mime.clone()).filter(|m| !m.is_empty()) }
        }
        "" => body_from_text(text, header_value(headers, "content-type")),
        _ => body_from_text(text, Some(str_at(b, "mimeType"))),
    }
}

fn auth(a: Option<&Value>, w: &mut Warnings, subject: &str) -> Auth {
    let Some(a) = a.filter(|a| a.as_object().is_some_and(|o| !o.is_empty())) else { return Auth::Inherit };
    if bool_at(a, "disabled") == Some(true) {
        return Auth::None;
    }
    let s = |k: &str| conv_plain(&text_at(a, k));
    match str_at(a, "type") {
        "none" => Auth::None,
        "inherit" | "" => Auth::Inherit,
        "basic" => Auth::Basic { username: s("username"), password: s("password") },
        "bearer" => Auth::Bearer { token: s("token"), prefix: Some(s("prefix")).filter(|p| !p.is_empty() && p != "Bearer") },
        "apikey" => Auth::ApiKey {
            key: s("key"),
            value: s("value"),
            location: if str_at(a, "addTo") == "queryParams" { ApiKeyLocation::Query } else { ApiKeyLocation::Header },
        },
        "digest" => Auth::Digest { username: s("username"), password: s("password") },
        "ntlm" => Auth::Ntlm { username: s("username"), password: s("password"), domain: None, workstation: None },
        "iam" => Auth::AwsSigV4 {
            access_key_id: s("accessKeyId"),
            secret_access_key: s("secretAccessKey"),
            session_token: Some(s("sessionToken")).filter(|t| !t.is_empty()),
            region: s("region"),
            service: s("service"),
        },
        "oauth2" => {
            let grant = match str_at(a, "grantType") {
                "client_credentials" => OAuth2Grant::ClientCredentials,
                "password" => OAuth2Grant::Password,
                "implicit" => {
                    w.about("OAuth 2.0 implicit grant is not supported; converted to a bearer token", subject);
                    return Auth::Bearer { token: s("accessToken"), prefix: None };
                }
                _ => OAuth2Grant::AuthorizationCode,
            };
            let mut extra = IndexMap::new();
            if !s("resource").is_empty() {
                extra.insert("resource".to_string(), s("resource"));
            }
            let mut cfg = OAuth2Config {
                grant,
                auth_url: s("authorizationUrl"),
                token_url: s("accessTokenUrl"),
                client_id: s("clientId"),
                client_secret: s("clientSecret"),
                scope: s("scope"),
                username: s("username"),
                password: s("password"),
                use_pkce: bool_at(a, "usePkce").unwrap_or(grant == OAuth2Grant::AuthorizationCode),
                redirect_port: super::postman::redirect_port(&s("redirectUrl")),
                client_auth: if bool_at(a, "credentialsInBody") == Some(true) { OAuth2ClientAuth::Body } else { OAuth2ClientAuth::Basic },
                token_prefix: Some(s("tokenPrefix")).filter(|p| !p.is_empty() && p != "Bearer"),
                extra_params: extra,
                ..Default::default()
            };
            cfg.audience = Some(s("audience")).filter(|x| !x.is_empty());
            Auth::OAuth2(cfg)
        }
        other => {
            w.about(&format!("“{other}” auth is not supported yet; affected items inherit their folder's auth"), subject);
            Auth::Inherit
        }
    }
}

/// Flattens nested environment data to dotted keys (`{{ _.api.url }}` → `api.url`).
fn flatten_env(data: Option<&Value>) -> IndexMap<String, String> {
    fn walk(prefix: &str, v: &Value, out: &mut IndexMap<String, String>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                    walk(&key, x, out);
                }
            }
            other if !prefix.is_empty() => {
                out.insert(prefix.to_string(), conv_plain(&scalar_string(other)));
            }
            _ => {}
        }
    }
    let mut out = IndexMap::new();
    if let Some(d) = data {
        walk("", d, &mut out);
    }
    out
}

/// Nunjucks → /dev/all templates without warnings (environment values, auth fields).
fn conv_plain(s: &str) -> String {
    conv(s, &mut Warnings::default(), "")
}

/// Converts Insomnia's Nunjucks templating: `{{ _.name }}` → `{{name}}`, common `{% tag %}`s
/// → dynamic values; anything else is kept and reported.
pub(crate) fn conv(s: &str, w: &mut Warnings, subject: &str) -> String {
    let mut out = map_templates(s, |k| {
        let k = k.strip_prefix("_.").unwrap_or(k).trim();
        let k = k.strip_prefix("_['").and_then(|r| r.strip_suffix("']")).unwrap_or(k);
        Some(format!("{{{{{k}}}}}"))
    });
    if !out.contains("{%") {
        return out;
    }
    let mut result = String::with_capacity(out.len());
    while let Some(start) = out.find("{%") {
        result.push_str(&out[..start]);
        let Some(end) = out[start..].find("%}") else {
            result.push_str(&out[start..]);
            out.clear();
            break;
        };
        let tag = out[start + 2..start + end].trim();
        let mut words = tag.split_whitespace();
        let name = words.next().unwrap_or("");
        let arg = words.next().unwrap_or("").trim_matches(['\'', '"', ',']);
        let rep = match (name, arg) {
            ("uuid", _) => Some("{{$uuid}}"),
            ("now", "millis") => Some("{{$timestampMs}}"),
            ("now", "unix") => Some("{{$timestamp}}"),
            ("now", _) => Some("{{$isoTimestamp}}"),
            ("timestamp", _) => Some("{{$timestampMs}}"),
            _ => None,
        };
        match rep {
            Some(r) => result.push_str(r),
            None => {
                if !subject.is_empty() {
                    w.about(&format!("Insomnia template tags like “{{% {name} %}}” have no equivalent and were kept as text"), subject);
                }
                result.push_str(&out[start..start + end + 2]);
            }
        }
        out = out[start + end + 2..].to_string();
    }
    result.push_str(&out);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_templates() {
        let mut w = Warnings::default();
        assert_eq!(conv("{{ _.baseUrl }}/x/{{_.id}}?t={% now 'millis' %}&u={% uuid 'v4' %}", &mut w, "r"), "{{baseUrl}}/x/{{id}}?t={{$timestampMs}}&u={{$uuid}}");
        assert_eq!(conv("{% response 'body', 'req_1', '$.token' %}", &mut w, "r"), "{% response 'body', 'req_1', '$.token' %}");
        assert_eq!(w.finish().len(), 1);
    }

    #[test]
    fn flattens_nested_env() {
        let v: Value = serde_json::from_str(r#"{"api":{"url":"https://x","v":2},"flag":true}"#).unwrap();
        let f = flatten_env(Some(&v));
        assert_eq!(f.get("api.url").map(String::as_str), Some("https://x"));
        assert_eq!(f.get("api.v").map(String::as_str), Some("2"));
        assert_eq!(f.get("flag").map(String::as_str), Some("true"));
    }
}
