//! Captured exchanges → saved requests (`http/<folder>/<slug>.req.yaml`).
//!
//! Secrets never land in the file: `Authorization` becomes request auth that references
//! `{{secret.…}}` (Bearer → `BEARER_TOKEN`, Basic → the captured user + `BASIC_PASSWORD`,
//! anything else → `AUTHORIZATION`), API-key headers reference `{{secret.API_KEY}}`, and cookies
//! are left to the client's cookie jar. Hop-by-hop, `Host`, `Content-Length` and
//! `Accept-Encoding` (the client negotiates compression itself) are dropped.

use std::path::Path;

use base64::Engine;
use devall_core::{Auth, Body, HttpRequestFile, KeyValue, Project};

use super::{decoded_body, query_pairs};
use crate::error::Result;
use crate::model::Exchange;
use crate::util::{header_value, is_hop_by_hop, is_json_type, is_xml_type, mime_essence};

const DROP_HEADERS: &[&str] = &["host", "content-length", "accept-encoding", "cookie", "priority"];

fn name_for(ex: &Exchange) -> String {
    let path = url::Url::parse(&ex.request.url).map(|u| u.path().to_string()).unwrap_or_else(|_| ex.request.url.clone());
    let mut name = format!("{} {}", ex.request.method, path);
    if name.len() > 80 {
        name.truncate(name.floor_char_boundary(79));
        name.push('…');
    }
    name
}

/// The request file for one exchange (not yet saved).
pub fn to_request_file(ex: &Exchange) -> HttpRequestFile {
    let mut req = HttpRequestFile::new(name_for(ex));
    req.method = ex.request.method.clone();
    let (base, params) = match url::Url::parse(&ex.request.url) {
        Ok(mut u) => {
            let params = query_pairs(&ex.request.url);
            u.set_query(None);
            u.set_fragment(None);
            (u.to_string(), params)
        }
        Err(_) => (ex.request.url.clone(), Vec::new()),
    };
    req.url = base;
    req.params = params.into_iter().map(|(k, v)| KeyValue::new(k, v)).collect();
    let mut secrets: Vec<&str> = Vec::new();
    for (k, v) in &ex.request.headers {
        let lk = k.to_ascii_lowercase();
        if is_hop_by_hop(&lk) || DROP_HEADERS.contains(&lk.as_str()) || lk.starts_with(':') {
            continue;
        }
        match lk.as_str() {
            "authorization" => {
                let (scheme, rest) = v.split_once(' ').unwrap_or((v.as_str(), ""));
                if scheme.eq_ignore_ascii_case("bearer") {
                    req.auth = Auth::Bearer { token: "{{secret.BEARER_TOKEN}}".into(), prefix: None };
                    secrets.push("BEARER_TOKEN");
                } else if scheme.eq_ignore_ascii_case("basic") {
                    let user = base64::engine::general_purpose::STANDARD
                        .decode(rest.trim())
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .and_then(|s| s.split_once(':').map(|(u, _)| u.to_string()))
                        .unwrap_or_default();
                    req.auth = Auth::Basic { username: user, password: "{{secret.BASIC_PASSWORD}}".into() };
                    secrets.push("BASIC_PASSWORD");
                } else {
                    req.headers.push(KeyValue::new("Authorization", "{{secret.AUTHORIZATION}}"));
                    secrets.push("AUTHORIZATION");
                }
            }
            "x-api-key" | "api-key" | "x-auth-token" => {
                req.headers.push(KeyValue::new(k.clone(), "{{secret.API_KEY}}"));
                secrets.push("API_KEY");
            }
            _ => req.headers.push(KeyValue::new(k.clone(), v.clone())),
        }
    }
    if req.auth.is_inherit() {
        req.auth = Auth::None;
    }
    let ct = header_value(&ex.request.headers, "content-type").unwrap_or("").to_string();
    let mut notes: Vec<String> = Vec::new();
    if ex.request.body.size > 0 {
        match decoded_body(&ex.request.body, &ex.request.headers).and_then(|b| String::from_utf8(b).ok()) {
            Some(text) => {
                let essence = mime_essence(&ct);
                req.body = if is_json_type(&ct) {
                    let pretty = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or(text);
                    Body::Json { content: pretty }
                } else if essence == "application/x-www-form-urlencoded" {
                    Body::Form { fields: url::form_urlencoded::parse(text.as_bytes()).map(|(k, v)| KeyValue::new(k.into_owned(), v.into_owned())).collect() }
                } else if is_xml_type(&ct) {
                    Body::Xml { content: text }
                } else {
                    // The captured Content-Type header is kept on the request.
                    Body::Text { content: text, content_type: None }
                };
                if ex.request.body.truncated {
                    notes.push("The captured body was truncated by the capture limit.".into());
                }
            }
            None => notes.push(format!("The captured {} byte binary body was not saved.", ex.request.body.size)),
        }
    }
    let mut docs = format!("Captured by the traffic inspector from `{} {}`.", ex.request.method, base_without_secrets(&ex.request.url));
    if !secrets.is_empty() {
        secrets.dedup();
        docs.push_str(&format!(
            "\n\nCredentials were replaced by secret references ({}); set them on the Environments page.",
            secrets.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", ")
        ));
    }
    for n in notes {
        docs.push_str("\n\n");
        docs.push_str(&n);
    }
    req.docs = Some(docs);
    req
}

fn base_without_secrets(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) => {
            let _ = u.set_password(None);
            u.set_query(None);
            u.to_string()
        }
        Err(_) => url.to_string(),
    }
}

/// Saves one request per exchange under `http/<folder>/` and returns their paths (relative to
/// `http/`).
pub fn save_requests(root: &Path, exchanges: &[Exchange], folder: &str) -> Result<Vec<String>> {
    let project = Project::open(root)?;
    let folder = devall_core::fsutil::normalize_rel(if folder.trim().is_empty() { "captured" } else { folder });
    let mut out = Vec::new();
    for ex in exchanges.iter().filter(|e| e.request.method != "CONNECT") {
        out.push(project.create_request(&folder, to_request_file(ex))?);
    }
    Ok(out)
}
