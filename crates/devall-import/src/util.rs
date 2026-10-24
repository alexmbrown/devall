//! Small helpers shared by the importers and exporters.

use indexmap::IndexMap;
use serde_json::{Map, Value};

use devall_core::{Auth, Body, FolderFile, HttpRequestFile, KeyValue};

use crate::error::{Error, Result};

// ---------- loosely-typed JSON access ----------

/// The string at `key`, or `""`.
pub(crate) fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The scalar at `key` as text (numbers and booleans stringified), or `""`.
pub(crate) fn text_at(v: &Value, key: &str) -> String {
    v.get(key).map(scalar_string).unwrap_or_default()
}

pub(crate) fn arr_at<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// A boolean at `key`, also accepting `"true"`/`"false"` strings.
pub(crate) fn bool_at(v: &Value, key: &str) -> Option<bool> {
    match v.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn scalar_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// YAML → JSON with non-string keys stringified (OpenAPI response codes are YAML integers).
pub(crate) fn yaml_to_json(y: serde_yaml_ng::Value) -> Value {
    use serde_yaml_ng::Value as Y;
    match y {
        Y::Null => Value::Null,
        Y::Bool(b) => Value::Bool(b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else {
                n.as_f64().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::Null)
            }
        }
        Y::String(s) => Value::String(s),
        Y::Sequence(seq) => Value::Array(seq.into_iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                let key = match k {
                    Y::String(s) => s,
                    Y::Number(n) => n.to_string(),
                    Y::Bool(b) => b.to_string(),
                    Y::Null => "null".into(),
                    other => serde_yaml_ng::to_string(&other).unwrap_or_default().trim().to_string(),
                };
                out.insert(key, yaml_to_json(v));
            }
            Value::Object(out)
        }
        Y::Tagged(t) => yaml_to_json(t.value),
    }
}

/// Parses JSON, falling back to YAML.
pub(crate) fn parse_structured(text: &str) -> Result<Value> {
    let t = text.trim_start_matches('\u{feff}');
    match serde_json::from_str::<Value>(t) {
        Ok(v) => Ok(v),
        Err(json_err) => match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(t) {
            Ok(y) => Ok(yaml_to_json(y)),
            Err(yaml_err) => {
                let trimmed = t.trim_start();
                Err(Error::Parse(if trimmed.starts_with('{') || trimmed.starts_with('[') {
                    format!("invalid JSON: {json_err}")
                } else {
                    format!("invalid YAML: {yaml_err}")
                }))
            }
        },
    }
}

// ---------- text helpers ----------

/// Pretty-prints JSON objects/arrays; anything else (including templated JSON) is returned as is.
pub(crate) fn pretty_json(s: &str) -> String {
    match serde_json::from_str::<Value>(s) {
        Ok(v @ (Value::Object(_) | Value::Array(_))) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| s.to_string()),
        _ => s.to_string(),
    }
}

/// `GET /users` style name for requests that come without one.
pub(crate) fn default_name(method: &str, url: &str) -> String {
    let (_, path) = split_origin(url);
    let path = path.split(['?', '#']).next().unwrap_or("");
    format!("{} {}", method.to_uppercase(), if path.is_empty() { "/" } else { path })
}

/// Splits a URL into (scheme+authority or leading `{{var}}`, rest).
pub(crate) fn split_origin(url: &str) -> (&str, &str) {
    if let Some(i) = url.find("://") {
        let after = i + 3;
        return match url[after..].find(['/', '?', '#']) {
            Some(j) => (&url[..after + j], &url[after + j..]),
            None => (url, ""),
        };
    }
    if url.starts_with("{{")
        && let Some(j) = url.find("}}")
    {
        return (&url[..j + 2], &url[j + 2..]);
    }
    if url.starts_with('/') {
        return ("", url);
    }
    match url.find(['/', '?', '#']) {
        Some(j) => (&url[..j], &url[j..]),
        None => (url, ""),
    }
}

/// Splits `?a=1&b=2` off a URL into decoded params (fragment dropped).
pub(crate) fn split_query(url: &str) -> (String, Vec<KeyValue>) {
    devall_core::curl::split_query(url)
}

/// Converts `:name` path segments (Postman, Bruno, Insomnia path params) into `{{name}}`.
/// Returns the new URL and the converted names.
pub(crate) fn colon_path_vars(url: &str) -> (String, Vec<String>) {
    let (origin, rest) = split_origin(url);
    let cut = rest.find(['?', '#']).unwrap_or(rest.len());
    let (path, tail) = rest.split_at(cut);
    let mut names = Vec::new();
    let segs: Vec<String> = path
        .split('/')
        .map(|seg| match seg.strip_prefix(':') {
            Some(name) if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) => {
                names.push(name.to_string());
                format!("{{{{{name}}}}}")
            }
            _ => seg.to_string(),
        })
        .collect();
    (format!("{origin}{}{tail}", segs.join("/")), names)
}

pub(crate) fn header_value<'a>(headers: &'a [KeyValue], name: &str) -> Option<&'a str> {
    headers.iter().find(|h| h.enabled && h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

/// Form-style percent decoding (`+` is a space); invalid escapes are kept literally.
pub(crate) fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn parse_form_pairs(s: &str) -> Vec<KeyValue> {
    s.split('&')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            KeyValue::new(url_decode(k.trim()), url_decode(v))
        })
        .collect()
}

pub(crate) fn looks_like_form(s: &str) -> bool {
    !s.is_empty() && s.split('&').all(|p| p.contains('=') && !p.contains(char::is_whitespace))
}

/// Picks a body type from raw text and an optional content type.
pub(crate) fn body_from_text(text: String, content_type: Option<&str>) -> Body {
    if text.is_empty() {
        return Body::None;
    }
    let ct = content_type.map(|c| c.to_ascii_lowercase());
    let trimmed = text.trim_start();
    match ct.as_deref() {
        Some(c) if c.contains("json") => Body::Json { content: text },
        Some(c) if c.contains("xml") => Body::Xml { content: text },
        Some(c) if c.contains("x-www-form-urlencoded") && looks_like_form(text.trim()) => Body::Form { fields: parse_form_pairs(text.trim()) },
        Some(c) if c.starts_with("text/plain") => Body::Text { content: text, content_type: None },
        Some(c) => Body::Text { content: text, content_type: Some(content_type.unwrap_or(c).to_string()) },
        None if trimmed.starts_with('{') || trimmed.starts_with('[') => Body::Json { content: text },
        None if trimmed.starts_with('<') => Body::Xml { content: text },
        None => Body::Text { content: text, content_type: None },
    }
}

pub(crate) fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// `bearerAuth` → `BEARER_AUTH`, `x-api-key` → `X_API_KEY`.
pub(crate) fn upper_snake(s: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && prev_lower && !out.ends_with('_') {
                out.push('_');
            }
            out.push(c.to_ascii_uppercase());
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else {
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            prev_lower = false;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "SECRET".into() } else { out }
}

/// A variable/secret name safe for `{{…}}` references and keychain accounts.
pub(crate) fn safe_var_name(s: &str) -> String {
    let out: String = s.trim().chars().map(|c| if c.is_ascii_alphanumeric() || "_-.".contains(c) { c } else { '_' }).collect();
    if out.is_empty() { "var".into() } else { out }
}

/// Rewrites every `{{…}}` in `input`; `f` gets the trimmed inner text and returns the full
/// replacement (braces included) or `None` to keep it.
pub(crate) fn map_templates(input: &str, mut f: impl FnMut(&str) -> Option<String>) -> String {
    if !input.contains("{{") {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        match f(after[..end].trim()) {
            Some(rep) => out.push_str(&rep),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Replaces `{{name}}` references by literal values (request-scoped variables).
pub(crate) fn inline_vars(req: &mut HttpRequestFile, vars: &IndexMap<String, String>) {
    if vars.is_empty() {
        return;
    }
    visit_request_strings(req, &mut |s| {
        if s.contains("{{") {
            *s = map_templates(s, |k| vars.get(k).cloned());
        }
    });
}

// ---------- visiting every user string in the model ----------

pub(crate) fn visit_auth_strings(auth: &mut Auth, f: &mut dyn FnMut(&mut String)) {
    match auth {
        Auth::Basic { username, password } | Auth::Digest { username, password } => {
            f(username);
            f(password);
        }
        Auth::Ntlm { username, password, domain, workstation } => {
            f(username);
            f(password);
            for s in [domain, workstation].into_iter().flatten() {
                f(s);
            }
        }
        Auth::Bearer { token, prefix } => {
            f(token);
            if let Some(p) = prefix {
                f(p);
            }
        }
        Auth::ApiKey { key, value, .. } => {
            f(key);
            f(value);
        }
        Auth::OAuth2(c) => {
            for s in [&mut c.auth_url, &mut c.token_url, &mut c.client_id, &mut c.client_secret, &mut c.scope, &mut c.username, &mut c.password] {
                f(s);
            }
            if let Some(a) = &mut c.audience {
                f(a);
            }
            if let Some(p) = &mut c.token_prefix {
                f(p);
            }
            for v in c.extra_params.values_mut() {
                f(v);
            }
        }
        Auth::AwsSigV4 { access_key_id, secret_access_key, session_token, region, service } => {
            f(access_key_id);
            f(secret_access_key);
            if let Some(t) = session_token {
                f(t);
            }
            f(region);
            f(service);
        }
        Auth::Plugin { config, .. } => {
            for v in config.values_mut() {
                f(v);
            }
        }
        _ => {}
    }
}

pub(crate) fn visit_request_strings(req: &mut HttpRequestFile, f: &mut dyn FnMut(&mut String)) {
    f(&mut req.url);
    for kv in req.params.iter_mut().chain(req.headers.iter_mut()) {
        f(&mut kv.name);
        f(&mut kv.value);
    }
    visit_auth_strings(&mut req.auth, f);
    match &mut req.body {
        Body::Json { content } | Body::Xml { content } | Body::Text { content, .. } => f(content),
        Body::Form { fields } => {
            for kv in fields {
                f(&mut kv.name);
                f(&mut kv.value);
            }
        }
        Body::Multipart { parts } => {
            for p in parts {
                f(&mut p.name);
                f(&mut p.value);
                if let Some(file) = &mut p.file {
                    f(file);
                }
            }
        }
        Body::Graphql { query, variables, .. } => {
            f(query);
            f(variables);
        }
        Body::Binary { path, .. } => f(path),
        _ => {}
    }
    if let Some(p) = &mut req.settings.proxy {
        f(p);
    }
    for t in &mut req.tests {
        if let Some(v) = &mut t.value {
            f(v);
        }
    }
}

pub(crate) fn visit_folder_strings(folder: &mut FolderFile, f: &mut dyn FnMut(&mut String)) {
    for kv in &mut folder.headers {
        f(&mut kv.name);
        f(&mut kv.value);
    }
    visit_auth_strings(&mut folder.auth, f);
    for v in folder.variables.values_mut() {
        f(v);
    }
}

pub(crate) fn folder_has_settings(f: &FolderFile) -> bool {
    !f.headers.is_empty() || !f.auth.is_inherit() || !f.variables.is_empty()
}

/// Puts path-parameter example values into folder variables so `{{id}}` resolves out of the box.
/// A conflicting value already present is inlined into the request URL instead.
pub(crate) fn add_path_vars(settings: &mut Option<FolderFile>, req: &mut HttpRequestFile, vars: Vec<(String, String)>) {
    for (name, value) in vars {
        if value.is_empty() {
            continue;
        }
        let s = settings.get_or_insert_with(FolderFile::default);
        match s.variables.get(&name) {
            None => {
                s.variables.insert(name, value);
            }
            Some(existing) if *existing == value => {}
            Some(_) => {
                let needle = format!("{{{{{name}}}}}");
                req.url = req.url.replace(&needle, &value);
            }
        }
    }
}

// ---------- warnings ----------

/// Collects warnings, folding repeated ones into a single line with a count and examples.
#[derive(Debug, Default)]
pub(crate) struct Warnings {
    items: IndexMap<String, Vec<String>>,
}

impl Warnings {
    pub(crate) fn add(&mut self, msg: impl Into<String>) {
        self.items.entry(msg.into()).or_default();
    }

    /// A warning about `subject` (usually a request name); identical messages are grouped.
    pub(crate) fn about(&mut self, msg: &str, subject: &str) {
        let list = self.items.entry(msg.to_string()).or_default();
        if !list.iter().any(|s| s == subject) {
            list.push(subject.to_string());
        }
    }

    pub(crate) fn finish(self) -> Vec<String> {
        self.items
            .into_iter()
            .map(|(msg, subjects)| match subjects.len() {
                0 => msg,
                1 => format!("{msg} ({})", subjects[0]),
                n => {
                    let shown: Vec<&str> = subjects.iter().take(4).map(String::as_str).collect();
                    format!("{msg} ({n}: {}{})", shown.join(", "), if n > 4 { ", …" } else { "" })
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_vars_and_origin() {
        assert_eq!(colon_path_vars("{{baseUrl}}/users/:id/posts/:postId?x=:y").0, "{{baseUrl}}/users/{{id}}/posts/{{postId}}?x=:y");
        assert_eq!(colon_path_vars("http://localhost:8080/a/:b").0, "http://localhost:8080/a/{{b}}");
        assert_eq!(default_name("get", "https://x.io/a/b?c=1"), "GET /a/b");
        assert_eq!(default_name("post", "{{baseUrl}}"), "POST /");
        assert_eq!(split_origin("/v1/x"), ("", "/v1/x"));
    }

    #[test]
    fn names() {
        assert_eq!(upper_snake("bearerAuth"), "BEARER_AUTH");
        assert_eq!(upper_snake("x-api-key"), "X_API_KEY");
        assert_eq!(upper_snake("petstore_auth"), "PETSTORE_AUTH");
        assert_eq!(safe_var_name("my var!"), "my_var_");
    }

    #[test]
    fn templates() {
        let out = map_templates("{{ a }}/{{b}}/{{c", |k| (k == "a").then(|| "A".to_string()));
        assert_eq!(out, "A/{{b}}/{{c");
    }

    #[test]
    fn warnings_group() {
        let mut w = Warnings::default();
        w.add("plain");
        for s in ["a", "b", "c", "d", "e", "a"] {
            w.about("Scripts kept in docs", s);
        }
        let out = w.finish();
        assert_eq!(out[0], "plain");
        assert_eq!(out[1], "Scripts kept in docs (5: a, b, c, d, …)");
    }
}
