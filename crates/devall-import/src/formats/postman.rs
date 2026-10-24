//! Postman Collection v2.0 / v2.1 and environment / globals exports.

use serde_json::Value;

use devall_core::{
    ApiKeyLocation, AssertOp, AssertTarget, Assertion, Auth, Body, Capture, CaptureSource, EnvironmentFile, FolderFile, HttpRequestFile, KeyValue,
    MultipartPart, OAuth2ClientAuth, OAuth2Config, OAuth2Grant,
};

use crate::detect::Format;
use crate::error::{Error, Result};
use crate::plan::{ImportPlan, PlanFolder, PlanItem};
use crate::util::{
    Warnings, add_path_vars, arr_at, body_from_text, bool_at, colon_path_vars, default_name, folder_has_settings, header_value, scalar_string,
    split_query, str_at, text_at,
};

const SCRIPTS_KEPT: &str = "Postman scripts are not run by /dev/all; they were kept in the request docs (status checks and variable captures were converted)";
const FILES_MOVED: &str = "File paths in form-data/binary bodies point to the original machine; re-select the files";

pub(crate) fn parse_collection(v: &Value) -> Result<ImportPlan> {
    if v.get("item").is_none() && (v.get("requests").is_some() || v.get("order").is_some()) {
        return Err(Error::Unsupported("Postman Collection v1 is not supported; export it again from Postman as Collection v2.1".into()));
    }
    let info = v.get("info").unwrap_or(&Value::Null);
    let name = Some(str_at(info, "name").trim()).filter(|n| !n.is_empty()).unwrap_or("Postman collection");
    let mut plan = ImportPlan::new(name, Format::Postman);
    let mut w = Warnings::default();

    for var in arr_at(v, "variable") {
        if bool_at(var, "disabled") == Some(true) {
            continue;
        }
        let key = Some(str_at(var, "key")).filter(|k| !k.is_empty()).unwrap_or(str_at(var, "id"));
        if !key.is_empty() {
            plan.project_variables.insert(key.to_string(), text_at(var, "value"));
        }
    }

    let mut settings = FolderFile::default();
    if let Some(a) = v.get("auth") {
        settings.auth = auth(a, &mut w, "collection");
    }
    if has_scripts(v) {
        w.add("Collection-level Postman scripts are not run by /dev/all and were not imported");
    }
    let mut settings = folder_has_settings(&settings).then_some(settings);
    plan.items = items(arr_at(v, "item"), &mut settings, &mut w);
    plan.settings = settings;
    plan.warnings = w.finish();
    Ok(plan)
}

pub(crate) fn parse_environment(v: &Value) -> Result<ImportPlan> {
    let name = Some(str_at(v, "name").trim()).filter(|n| !n.is_empty()).unwrap_or("Postman environment");
    let mut plan = ImportPlan::new(name, Format::PostmanEnvironment);
    let mut w = Warnings::default();
    let globals = str_at(v, "_postman_variable_scope") == "globals";
    let mut env = EnvironmentFile::new(name);
    for x in arr_at(v, "values") {
        let key = str_at(x, "key");
        if key.is_empty() {
            continue;
        }
        if bool_at(x, "enabled") == Some(false) {
            w.about("Disabled variables were skipped", key);
            continue;
        }
        let value = text_at(x, "value");
        if globals {
            plan.project_variables.insert(key.to_string(), value);
        } else {
            if str_at(x, "type") == "secret" {
                env.secrets.push(key.to_string());
            }
            env.variables.insert(key.to_string(), value);
        }
    }
    if !globals {
        plan.environments.push(env);
    }
    plan.warnings = w.finish();
    Ok(plan)
}

fn has_scripts(v: &Value) -> bool {
    arr_at(v, "event").iter().any(|e| !script_code(e).trim().is_empty())
}

fn items(list: &[Value], parent: &mut Option<FolderFile>, w: &mut Warnings) -> Vec<PlanItem> {
    let mut out = Vec::new();
    for it in list {
        let is_folder = it.get("item").is_some_and(Value::is_array) || (it.get("request").is_none() && it.get("item").is_some());
        if is_folder {
            let name = Some(str_at(it, "name").trim()).filter(|n| !n.is_empty()).unwrap_or("Folder");
            let mut f = PlanFolder::new(name);
            let mut s = FolderFile::default();
            if let Some(a) = it.get("auth") {
                s.auth = auth(a, w, name);
            }
            for var in arr_at(it, "variable") {
                let key = str_at(var, "key");
                if !key.is_empty() && bool_at(var, "disabled") != Some(true) {
                    s.variables.insert(key.to_string(), text_at(var, "value"));
                }
            }
            if has_scripts(it) {
                w.about("Folder-level Postman scripts were not imported", name);
            }
            let mut settings = folder_has_settings(&s).then_some(s);
            f.items = items(arr_at(it, "item"), &mut settings, w);
            f.settings = settings;
            out.push(PlanItem::Folder(f));
        } else {
            let (mut req, path_vars) = request(it, w);
            add_path_vars(parent, &mut req, path_vars);
            out.push(PlanItem::Request(Box::new(req)));
        }
    }
    out
}

fn request(it: &Value, w: &mut Warnings) -> (HttpRequestFile, Vec<(String, String)>) {
    let mut req = HttpRequestFile::new(str_at(it, "name").trim());
    let mut path_vars = Vec::new();
    let mut docs: Vec<String> = Vec::new();
    match it.get("request") {
        Some(Value::String(url)) => {
            let (base, params) = split_query(url);
            req.url = colon_path_vars(&base).0;
            req.params = params;
        }
        Some(r @ Value::Object(_)) => {
            let method = str_at(r, "method").trim().to_uppercase();
            req.method = if method.is_empty() { "GET".into() } else { method };
            let (url, params, vars) = url(r.get("url"));
            req.url = url;
            req.params = params;
            path_vars = vars;
            req.headers = match r.get("header") {
                Some(Value::String(s)) => s
                    .lines()
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| KeyValue::new(k.trim(), v.trim()))
                    .collect(),
                Some(Value::Array(list)) => kv_list(list),
                _ => Vec::new(),
            };
            if let Some(a) = r.get("auth") {
                req.auth = auth(a, w, &req.name);
            }
            req.body = body(r.get("body"), &req.headers, w, &req.name);
            if let Some(d) = description(r.get("description")) {
                docs.push(d);
            }
        }
        _ => {}
    }
    if req.name.is_empty() {
        req.name = default_name(&req.method, &req.url);
    }
    if let Some(p) = it.get("protocolProfileBehavior") {
        if bool_at(p, "followRedirects") == Some(false) {
            req.settings.follow_redirects = false;
        }
        if bool_at(p, "strictSSL") == Some(false) {
            req.settings.verify_tls = false;
        }
    }
    let mut scripts = String::new();
    for ev in arr_at(it, "event") {
        let code = script_code(ev);
        if code.trim().is_empty() {
            continue;
        }
        let title = match str_at(ev, "listen") {
            "prerequest" => "Pre-request script",
            "test" => "Tests",
            other => other,
        };
        if str_at(ev, "listen") == "test" {
            let (tests, captures) = script_checks(&code);
            req.tests.extend(tests);
            req.captures.extend(captures);
        }
        scripts.push_str(&format!("### {title} (Postman)\n\n```js\n{}\n```\n\n", code.trim_end()));
    }
    if !scripts.is_empty() {
        docs.push(scripts.trim_end().to_string());
        w.about(SCRIPTS_KEPT, &req.name);
    }
    if !docs.is_empty() {
        req.docs = Some(docs.join("\n\n"));
    }
    (req, path_vars)
}

fn script_code(ev: &Value) -> String {
    match ev.pointer("/script/exec") {
        Some(Value::Array(lines)) => lines.iter().map(scalar_string).collect::<Vec<_>>().join("\n"),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

pub(crate) fn description(d: Option<&Value>) -> Option<String> {
    let text = match d? {
        Value::String(s) => s.clone(),
        o @ Value::Object(_) => text_at(o, "content"),
        _ => String::new(),
    };
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn kv_list(list: &[Value]) -> Vec<KeyValue> {
    list.iter()
        .filter_map(|x| {
            let key = Some(str_at(x, "key")).filter(|k| !k.is_empty()).unwrap_or(str_at(x, "name"));
            if key.is_empty() && text_at(x, "value").is_empty() {
                return None;
            }
            let mut kv = KeyValue::new(key, text_at(x, "value"));
            kv.enabled = bool_at(x, "disabled") != Some(true);
            kv.description = description(x.get("description"));
            Some(kv)
        })
        .collect()
}

/// URL string or object → (url without query, query params, path variables).
fn url(u: Option<&Value>) -> (String, Vec<KeyValue>, Vec<(String, String)>) {
    match u {
        Some(Value::String(s)) => {
            let (base, params) = split_query(s);
            (colon_path_vars(&base).0, params, Vec::new())
        }
        Some(o @ Value::Object(_)) => {
            let raw = str_at(o, "raw");
            let base = if !raw.is_empty() {
                raw.split(['?', '#']).next().unwrap_or("").to_string()
            } else {
                let host = match o.get("host") {
                    Some(Value::Array(h)) => h.iter().map(scalar_string).collect::<Vec<_>>().join("."),
                    Some(h) => scalar_string(h),
                    None => String::new(),
                };
                let path = match o.get("path") {
                    Some(Value::Array(p)) => p
                        .iter()
                        .map(|s| if s.is_object() { text_at(s, "value") } else { scalar_string(s) })
                        .collect::<Vec<_>>()
                        .join("/"),
                    Some(p) => scalar_string(p).trim_start_matches('/').to_string(),
                    None => String::new(),
                };
                let proto = str_at(o, "protocol");
                let port = text_at(o, "port");
                format!(
                    "{}{host}{}{}",
                    if proto.is_empty() { String::new() } else { format!("{proto}://") },
                    if port.is_empty() { String::new() } else { format!(":{port}") },
                    if path.is_empty() { String::new() } else { format!("/{path}") }
                )
            };
            let params = match o.get("query") {
                Some(Value::Array(q)) => kv_list(q),
                _ => split_query(raw).1,
            };
            let vars = arr_at(o, "variable")
                .iter()
                .filter_map(|x| {
                    let k = Some(str_at(x, "key")).filter(|k| !k.is_empty()).unwrap_or(str_at(x, "id"));
                    (!k.is_empty()).then(|| (k.to_string(), text_at(x, "value")))
                })
                .collect();
            (colon_path_vars(&base).0, params, vars)
        }
        _ => (String::new(), Vec::new(), Vec::new()),
    }
}

fn body(b: Option<&Value>, headers: &[KeyValue], w: &mut Warnings, subject: &str) -> Body {
    let Some(b) = b else { return Body::None };
    match str_at(b, "mode") {
        "raw" => {
            let raw = text_at(b, "raw");
            if raw.is_empty() {
                return Body::None;
            }
            match b.pointer("/options/raw/language").and_then(Value::as_str) {
                Some("json") => Body::Json { content: raw },
                Some("xml") => Body::Xml { content: raw },
                Some("html") => Body::Text { content: raw, content_type: Some("text/html".into()) },
                Some("javascript") => Body::Text { content: raw, content_type: Some("application/javascript".into()) },
                Some("graphql") => Body::Graphql { query: raw, variables: String::new(), operation_name: None },
                _ => body_from_text(raw, header_value(headers, "content-type")),
            }
        }
        "urlencoded" => Body::Form { fields: kv_list(arr_at(b, "urlencoded")) },
        "formdata" => Body::Multipart {
            parts: arr_at(b, "formdata")
                .iter()
                .map(|x| {
                    let is_file = str_at(x, "type") == "file";
                    let file = if is_file {
                        let src = match x.get("src") {
                            Some(Value::Array(a)) => a.first().map(scalar_string).unwrap_or_default(),
                            Some(s) => scalar_string(s),
                            None => String::new(),
                        };
                        w.about(FILES_MOVED, subject);
                        Some(src)
                    } else {
                        None
                    };
                    MultipartPart {
                        name: str_at(x, "key").to_string(),
                        value: if is_file { String::new() } else { text_at(x, "value") },
                        file,
                        content_type: Some(str_at(x, "contentType").to_string()).filter(|c| !c.is_empty()),
                        enabled: bool_at(x, "disabled") != Some(true),
                    }
                })
                .collect(),
        },
        "file" => {
            w.about(FILES_MOVED, subject);
            Body::Binary { path: b.pointer("/file/src").map(scalar_string).unwrap_or_default(), content_type: header_value(headers, "content-type").map(str::to_string) }
        }
        "graphql" => {
            let g = b.get("graphql").unwrap_or(&Value::Null);
            let variables = match g.get("variables") {
                Some(Value::String(s)) => s.clone(),
                Some(v @ (Value::Object(_) | Value::Array(_))) => serde_json::to_string_pretty(v).unwrap_or_default(),
                _ => String::new(),
            };
            Body::Graphql { query: text_at(g, "query"), variables, operation_name: None }
        }
        _ => Body::None,
    }
}

/// Reads one auth parameter from either the v2.1 list form or the v2.0 map form.
fn param(a: &Value, ty: &str, key: &str) -> String {
    match a.get(ty) {
        Some(Value::Array(list)) => list.iter().find(|x| str_at(x, "key") == key).map(|x| text_at(x, "value")).unwrap_or_default(),
        Some(o @ Value::Object(_)) => text_at(o, key),
        _ => String::new(),
    }
}

fn auth(a: &Value, w: &mut Warnings, subject: &str) -> Auth {
    let ty = str_at(a, "type");
    let p = |key: &str| param(a, ty, key);
    match ty {
        "" | "inherit" => Auth::Inherit,
        "noauth" => Auth::None,
        "basic" => Auth::Basic { username: p("username"), password: p("password") },
        "bearer" => Auth::Bearer { token: p("token"), prefix: None },
        "apikey" => Auth::ApiKey {
            key: p("key"),
            value: p("value"),
            location: if p("in") == "query" { ApiKeyLocation::Query } else { ApiKeyLocation::Header },
        },
        "digest" => Auth::Digest { username: p("username"), password: p("password") },
        "ntlm" => Auth::Ntlm {
            username: p("username"),
            password: p("password"),
            domain: Some(p("domain")).filter(|d| !d.is_empty()),
            workstation: Some(p("workstation")).filter(|w| !w.is_empty()),
        },
        "awsv4" => Auth::AwsSigV4 {
            access_key_id: p("accessKey"),
            secret_access_key: p("secretKey"),
            session_token: Some(p("sessionToken")).filter(|s| !s.is_empty()),
            region: p("region"),
            service: p("service"),
        },
        "oauth2" => {
            let grant_type = p("grant_type");
            let grant = match grant_type.as_str() {
                "client_credentials" => OAuth2Grant::ClientCredentials,
                "password_credentials" | "password" => OAuth2Grant::Password,
                "implicit" => {
                    w.about("OAuth 2.0 implicit grant is not supported; converted to a bearer token", subject);
                    return Auth::Bearer { token: p("accessToken"), prefix: Some(p("headerPrefix")).filter(|x| !x.is_empty() && x != "Bearer") };
                }
                _ => OAuth2Grant::AuthorizationCode,
            };
            let mut extra = indexmap::IndexMap::new();
            for list in ["authRequestParams", "tokenRequestParams"] {
                if let Some(Value::Array(items)) = a.get(ty).and_then(|x| if x.is_array() { x.as_array().and_then(|l| l.iter().find(|i| str_at(i, "key") == list)).and_then(|i| i.get("value")) } else { x.get(list) }) {
                    for i in items {
                        if bool_at(i, "enabled") != Some(false) && !str_at(i, "key").is_empty() {
                            extra.insert(str_at(i, "key").to_string(), text_at(i, "value"));
                        }
                    }
                }
            }
            if !p("audience").is_empty() {
                extra.entry("audience".to_string()).or_insert_with(|| p("audience"));
            }
            Auth::OAuth2(OAuth2Config {
                grant,
                auth_url: p("authUrl"),
                token_url: p("accessTokenUrl"),
                client_id: p("clientId"),
                client_secret: p("clientSecret"),
                scope: p("scope"),
                username: p("username"),
                password: p("password"),
                use_pkce: grant_type == "authorization_code_with_pkce" || grant == OAuth2Grant::AuthorizationCode && p("usePkce") != "false",
                redirect_port: redirect_port(&p("redirect_uri")),
                client_auth: if p("client_authentication") == "body" { OAuth2ClientAuth::Body } else { OAuth2ClientAuth::Basic },
                token_prefix: Some(p("headerPrefix")).filter(|x| !x.is_empty() && x != "Bearer"),
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

/// The port of a `http://localhost:PORT/...` redirect URI.
pub(crate) fn redirect_port(uri: &str) -> Option<u16> {
    let u = url::Url::parse(uri).ok()?;
    matches!(u.host_str()?, "localhost" | "127.0.0.1" | "[::1]").then_some(())?;
    u.port()
}

// ---------- converting common test-script idioms ----------

/// Converts the most common Postman test idioms into assertions and captures:
/// `pm.response.to.have.status(200)`, `pm.expect(pm.response.code).to.eql(200)` and
/// `pm.environment.set("token", pm.response.json().data.token)` (and friends).
pub(crate) fn script_checks(code: &str) -> (Vec<Assertion>, Vec<Capture>) {
    let mut tests = Vec::new();
    let mut captures = Vec::new();
    for needle in ["to.have.status(", "pm.response.code).to.eql(", "pm.response.code).to.equal(", "responseCode.code === "] {
        let mut rest = code;
        while let Some(i) = rest.find(needle) {
            rest = &rest[i + needle.len()..];
            let digits: String = rest.trim_start().chars().take_while(char::is_ascii_digit).collect();
            if digits.len() == 3 && !tests.iter().any(|t: &Assertion| t.value.as_deref() == Some(digits.as_str())) {
                tests.push(Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some(digits), enabled: true });
            }
        }
    }
    // Names bound to the parsed body: `var jsonData = pm.response.json();`
    let mut json_vars: Vec<String> = Vec::new();
    for line in code.lines() {
        let l = line.trim().trim_start_matches("var ").trim_start_matches("let ").trim_start_matches("const ");
        if let Some((name, expr)) = l.split_once('=') {
            let expr = expr.trim().trim_end_matches(';');
            if expr == "pm.response.json()" || expr == "JSON.parse(responseBody)" {
                json_vars.push(name.trim().to_string());
            }
        }
    }
    for setter in [
        "pm.environment.set(",
        "pm.collectionVariables.set(",
        "pm.globals.set(",
        "pm.variables.set(",
        "postman.setEnvironmentVariable(",
        "postman.setGlobalVariable(",
    ] {
        let mut rest = code;
        while let Some(i) = rest.find(setter) {
            rest = &rest[i + setter.len()..];
            let Some((args, after)) = balanced_args(rest) else { break };
            rest = after;
            let Some((name, expr)) = split_top_comma(args) else { continue };
            let name = name.trim().trim_matches(['"', '\'', '`']);
            if name.is_empty() || captures.iter().any(|c: &Capture| c.name == name) {
                continue;
            }
            if let Some(source) = capture_source(expr.trim(), &json_vars) {
                captures.push(Capture { name: name.to_string(), source, enabled: true });
            }
        }
    }
    (tests, captures)
}

/// Splits `a, b)` → ("a, b", rest after `)`), respecting nested parentheses and quotes.
fn balanced_args(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')') if depth == 0 => return Some((&s[..i], &s[i + 1..])),
            (None, ')' | ']' | '}') => depth -= 1,
            _ => {}
        }
    }
    None
}

fn split_top_comma(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth -= 1,
            (None, ',') if depth == 0 => return Some((&s[..i], &s[i + 1..])),
            _ => {}
        }
    }
    None
}

fn capture_source(expr: &str, json_vars: &[String]) -> Option<CaptureSource> {
    let expr = expr.trim_end_matches(';').trim();
    if expr == "pm.response.code" {
        return Some(CaptureSource::Status);
    }
    if expr == "pm.response.text()" || expr == "responseBody" {
        return Some(CaptureSource::Body);
    }
    if let Some(rest) = expr.strip_prefix("pm.response.headers.get(") {
        let name = rest.trim_end_matches(')').trim().trim_matches(['"', '\'']);
        return (!name.is_empty()).then(|| CaptureSource::Header { header: name.to_string() });
    }
    let rest = ["pm.response.json()", "JSON.parse(responseBody)"]
        .iter()
        .find_map(|p| expr.strip_prefix(p))
        .or_else(|| json_vars.iter().find_map(|v| expr.strip_prefix(v.as_str()).filter(|r| r.is_empty() || r.starts_with(['.', '[']))))?;
    let ok = rest.chars().all(|c| c.is_ascii_alphanumeric() || "._[]\"'-$".contains(c));
    ok.then(|| CaptureSource::JsonPath { path: format!("${rest}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_test_idioms() {
        let code = "pm.test('ok', function () { pm.response.to.have.status(201); });\nvar jsonData = pm.response.json();\npm.environment.set(\"token\", jsonData.data.token);\npm.collectionVariables.set('id', pm.response.json().items[0].id);\npm.globals.set(\"loc\", pm.response.headers.get(\"Location\"));\npm.environment.set(\"x\", someFn(1, 2));";
        let (tests, caps) = script_checks(code);
        assert_eq!(tests, vec![Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some("201".into()), enabled: true }]);
        assert_eq!(caps.len(), 3, "{caps:?}");
        assert_eq!(caps[0].source, CaptureSource::JsonPath { path: "$.data.token".into() });
        assert_eq!(caps[1].source, CaptureSource::JsonPath { path: "$.items[0].id".into() });
        assert_eq!(caps[2].source, CaptureSource::Header { header: "Location".into() });
    }

    #[test]
    fn v1_is_rejected() {
        let v: Value = serde_json::from_str(r#"{"id":"x","name":"old","order":[],"requests":[]}"#).unwrap();
        assert!(matches!(parse_collection(&v), Err(Error::Unsupported(_))));
    }

    #[test]
    fn redirect_ports() {
        assert_eq!(redirect_port("http://localhost:5173/callback"), Some(5173));
        assert_eq!(redirect_port("https://app.example.com/cb"), None);
    }
}
