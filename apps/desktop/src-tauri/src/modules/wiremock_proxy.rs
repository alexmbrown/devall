//! WireMock admin-API proxy for the mock server UI (`src/features/mocks`).
//!
//! The webview cannot call arbitrary hosts (the app CSP limits `connect-src`, and remote servers
//! may not answer CORS), and credentials kept in the OS keychain must never reach JavaScript. So
//! admin calls go through `wiremock_admin`: it only talks http(s) to paths under `/__admin`,
//! resolves the server's keychain secret on this side and returns the raw response.
//!
//! Commands:
//! - `wiremock_admin(request) -> AdminResponse`
//! - `wiremock_secret_status(root, name) -> bool` — is the secret set? (value never returned)
//! - `wiremock_write_file(path, dataBase64)` — saves bytes to a path the user picked in a save
//!   dialog (ZIP / binary exports; text goes through `write_text_file`).
//!
//! Integration (done by whoever wires the module in):
//!
//! `src/modules/mod.rs`:
//! ```ignore
//! pub mod wiremock_proxy;
//! ```
//!
//! `src/lib.rs`, inside `tauri::generate_handler![ … ]` (no state to manage; it uses `AppState`):
//! ```ignore
//! modules::wiremock_proxy::wiremock_admin,
//! modules::wiremock_proxy::wiremock_secret_status,
//! modules::wiremock_proxy::wiremock_write_file,
//! ```
//!
//! No new dependencies: HTTP goes through `devall_http::ClientPool` (reqwest), `base64`,
//! `devall-core` and `serde` are already dependencies of the desktop crate.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use devall_core::{Project, SecretLookup, SecretResolver};
use devall_http::{ClientPool, PreparedBody, PreparedRequest};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const MAX_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminRequest {
    /// Project root whose keychain scope holds `auth.secret`.
    pub root: Option<String>,
    /// Server base URL including any context path, e.g. `https://wiremock.example.com/mocks`.
    pub base_url: String,
    pub method: String,
    /// Must start with `/__admin`.
    pub path: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub body_base64: Option<String>,
    pub auth: Option<KeychainAuth>,
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub insecure_tls: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeychainAuth {
    /// `basic` | `bearer` | `header`.
    #[serde(rename = "type")]
    pub kind: String,
    pub username: Option<String>,
    pub header_name: Option<String>,
    /// Secret name in the project's all-environments keychain scope.
    pub secret: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body_base64: String,
}

fn pool() -> &'static ClientPool {
    static POOL: OnceLock<ClientPool> = OnceLock::new();
    POOL.get_or_init(ClientPool::new)
}

/// `base_url` + `path`, or why the combination is not allowed.
pub fn admin_url(base_url: &str, path: &str) -> Result<String, String> {
    let base = base_url.trim().trim_end_matches('/');
    let lower = base.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("http://") {
        r
    } else if let Some(r) = lower.strip_prefix("https://") {
        r
    } else {
        return Err("only http:// and https:// servers are supported".into());
    };
    if rest.is_empty() || rest.starts_with('/') || base.contains(['?', '#']) || base.chars().any(char::is_whitespace) {
        return Err(format!("invalid server URL {base_url:?}"));
    }
    let p = path.split(['?', '#']).next().unwrap_or("");
    if !(p == "/__admin" || p.starts_with("/__admin/")) {
        return Err("only paths under /__admin can be proxied".into());
    }
    if p.split('/').any(|seg| seg == ".." || seg == ".") || path.contains('\\') || path.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("invalid admin path".into());
    }
    Ok(format!("{base}{path}"))
}

fn is_loopback(url: &str) -> bool {
    let after = url.split("://").nth(1).unwrap_or("");
    let authority = after.split('/').next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = if host.starts_with('[') { host.split(']').next().unwrap_or("").trim_start_matches('[') } else { host.split(':').next().unwrap_or("") };
    let h = host.to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h == "::1" || h.starts_with("127.")
}

/// The header carrying `value` for `auth`.
pub fn auth_header(auth: &KeychainAuth, value: &str) -> Result<(String, String), String> {
    match auth.kind.as_str() {
        "basic" => Ok(("Authorization".into(), format!("Basic {}", B64.encode(format!("{}:{value}", auth.username.as_deref().unwrap_or("")))))),
        "bearer" => Ok(("Authorization".into(), format!("Bearer {value}"))),
        "header" => match auth.header_name.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
            Some(h) => Ok((h.to_string(), value.to_string())),
            None => Err("custom header authentication needs a header name".into()),
        },
        other => Err(format!("unknown authentication type {other:?}")),
    }
}

fn resolver<'a>(state: &'a AppState, root: &str) -> Result<SecretResolver<'a>, String> {
    let p = Project::open(root).map_err(|e| e.to_string())?;
    Ok(SecretResolver::new(state.secrets.as_ref(), p.id(), None, p.root()))
}

/// Sends one admin API call. HTTP error statuses are returned as responses, not errors.
#[tauri::command]
pub async fn wiremock_admin(state: State<'_, AppState>, request: AdminRequest) -> Result<AdminResponse, String> {
    let url = admin_url(&request.base_url, &request.path)?;
    let method = request.method.trim().to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS") {
        return Err(format!("unsupported method {method}"));
    }
    let mut headers: Vec<(String, String)> = request.headers.into_iter().filter(|(k, _)| !k.trim().is_empty() && !k.eq_ignore_ascii_case("host")).collect();
    let mut secrets = BTreeSet::new();
    if let Some(auth) = &request.auth {
        let root = request.root.as_deref().ok_or("a project root is needed to read the server's credential")?;
        let value = resolver(&state, root)?
            .lookup(&auth.secret)
            .ok_or_else(|| format!("credential {} is not set in the keychain for this project", auth.secret))?;
        let (k, v) = auth_header(auth, &value)?;
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case(&k));
        headers.push((k, v));
        secrets.insert(value);
    }
    let body = match request.body_base64.as_deref() {
        Some(b) if !b.is_empty() => PreparedBody::Bytes(B64.decode(b).map_err(|e| format!("invalid body: {e}"))?),
        _ => PreparedBody::None,
    };
    let prepared = PreparedRequest {
        method,
        // The system proxy must not intercept loopback servers (the built-in engine).
        proxy: is_loopback(&url).then(|| "none".to_string()),
        url,
        headers,
        body,
        timeout_ms: request.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1_000, MAX_TIMEOUT_MS),
        follow_redirects: false,
        verify_tls: !request.insecure_tls,
        client_cert: None,
        digest: None,
        ntlm: None,
    };
    let resp = pool().execute(&prepared).await.map_err(|e| e.redacted(&secrets).to_string())?;
    Ok(AdminResponse { status: resp.status, status_text: resp.status_text.clone(), headers: resp.headers.clone(), body_base64: B64.encode(&resp.body) })
}

/// Whether secret `name` is available for the project (keychain or `.env`); the value is never returned.
#[tauri::command]
pub fn wiremock_secret_status(state: State<'_, AppState>, root: String, name: String) -> Result<bool, String> {
    Ok(resolver(&state, &root)?.source_of(name.trim()).is_some())
}

/// Writes bytes to a path the user picked in a save dialog.
#[tauri::command]
pub fn wiremock_write_file(path: String, data_base64: String) -> Result<(), String> {
    let bytes = B64.decode(data_base64.as_bytes()).map_err(|e| format!("invalid data: {e}"))?;
    devall_core::fsutil::atomic_write(std::path::Path::new(&path), &bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_admin_paths_on_http_servers() {
        assert_eq!(admin_url("http://127.0.0.1:8080/", "/__admin/mappings").unwrap(), "http://127.0.0.1:8080/__admin/mappings");
        assert_eq!(admin_url("https://h.example/mocks", "/__admin/requests?limit=5").unwrap(), "https://h.example/mocks/__admin/requests?limit=5");
        assert!(admin_url("file:///etc", "/__admin/x").is_err());
        assert!(admin_url("ftp://h", "/__admin/x").is_err());
        assert!(admin_url("http://h", "/api/x").is_err());
        assert!(admin_url("http://h", "/__adminx").is_err());
        assert!(admin_url("http://h", "/__admin/../secret").is_err());
        assert!(admin_url("http://h?x=1", "/__admin/x").is_err());
        assert!(admin_url("http://", "/__admin/x").is_err());
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback("http://127.0.0.1:9000/__admin"));
        assert!(is_loopback("http://localhost/__admin"));
        assert!(is_loopback("http://[::1]:8080/__admin"));
        assert!(!is_loopback("https://wiremock.example.com/__admin"));
        assert!(!is_loopback("http://10.0.0.5:8080/__admin"));
    }

    #[test]
    fn auth_headers() {
        let a = |kind: &str| KeychainAuth { kind: kind.into(), username: Some("u".into()), header_name: Some("X-Api-Key".into()), secret: "S".into() };
        assert_eq!(auth_header(&a("basic"), "p").unwrap(), ("Authorization".into(), format!("Basic {}", B64.encode("u:p"))));
        assert_eq!(auth_header(&a("bearer"), "t").unwrap(), ("Authorization".into(), "Bearer t".into()));
        assert_eq!(auth_header(&a("header"), "k").unwrap(), ("X-Api-Key".into(), "k".into()));
        assert!(auth_header(&KeychainAuth { header_name: None, ..a("header") }, "k").is_err());
        assert!(auth_header(&a("digest"), "k").is_err());
    }
}
