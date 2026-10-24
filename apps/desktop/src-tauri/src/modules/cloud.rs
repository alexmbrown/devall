//! Hosted extras (Group 2, Batch D) on top of the relay module: raw TCP tunnels, custom domains,
//! publicly hosted mocks, shareable request/response snapshots and scheduled monitors.
//!
//! Everything goes through the relay chosen on the Tunnels page (hosted or self-hosted, where every
//! feature is free). Registration (lib.rs): the state lives in `modules::relay::RelayState`; add
//! these commands to `generate_handler!`:
//!
//! ```text
//! modules::cloud::relay_api, modules::cloud::relay_tcp_start, modules::cloud::relay_tcp_stop,
//! modules::cloud::relay_tcp_status, modules::cloud::relay_tcp_secret, modules::cloud::relay_tcp_connect,
//! modules::cloud::relay_tcp_disconnect, modules::cloud::relay_domain_add, modules::cloud::relay_domain_remove,
//! modules::cloud::relay_domain_refresh, modules::cloud::relay_mock_plan, modules::cloud::relay_mock_publish,
//! modules::cloud::relay_share_create, modules::cloud::relay_monitor_candidates,
//! modules::cloud::relay_monitor_prepare, modules::cloud::relay_monitor_save,
//! ```
//!
//! Secrets: shared snapshots are redacted here before upload (secret values of the request's
//! environment, sensitive headers and query parameters unless the user unticks that). Monitors
//! upload a secret value only when the user ticks it, by name.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use devall_core::vars::{NoSecrets, redact_values};
use devall_core::{Auth, Body, HttpRequestFile, KeyValue, Project, Resolver, SecretLookup, SecretResolver};
use devall_http::{PreparedBody, PreparedRequest};
use devall_relay::tcp::generate_access_secret;
use devall_relay::{ApiMethod, MockPublished, MockSettings, PublishPlan, TcpAccess, TcpConnectEvent, TcpConnectOptions, TcpConnectStatus, TcpTunnelEvent, TcpTunnelOptions, TcpTunnelStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::Channel;

use super::relay::{RelayMode, RelayState, Running};
use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

const MASK: &str = "••••••";
const TCP_SECRET_KEY: &str = "tcp-access-secret";

fn forward<E: Clone + Send + Serialize + 'static>(mut rx: tokio::sync::broadcast::Receiver<E>, ch: Channel<E>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if ch.send(ev).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    })
}

/// Generic pass-through to the relay's account API (`/v1/mocks…`, `/v1/shares…`, `/v1/monitors…`, `/v1/domains`).
#[tauri::command]
pub async fn relay_api(state: State<'_, RelayState>, method: String, path: String, body: Option<Value>) -> CmdResult<Value> {
    if !path.starts_with("/v1/") || path.contains("..") {
        return Err("only /v1/ relay API paths".into());
    }
    let m = ApiMethod::from_bytes(method.to_uppercase().as_bytes()).map_err(err)?;
    state.client()?.api(m, &path, body.as_ref()).await.map_err(err)
}

// ---------- raw TCP tunnels ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpView {
    pub publisher: Option<TcpTunnelStatus>,
    /// The access secret teammates use (stored in the OS keychain), when the tunnel uses one.
    pub secret: Option<String>,
    pub consumers: Vec<TcpConnectStatus>,
}

fn tcp_secret(state: &RelayState, rotate: bool) -> CmdResult<String> {
    let scope = devall_relay::auth::secret_scope();
    if !rotate && let Some(s) = state.secrets.get(&scope, TCP_SECRET_KEY).map_err(err)?.filter(|s| !s.is_empty()) {
        return Ok(s);
    }
    let s = generate_access_secret();
    state.secrets.set(&scope, TCP_SECRET_KEY, &s).map_err(err)?;
    Ok(s)
}

async fn tcp_view(state: &RelayState) -> TcpView {
    let publisher = state.tcp.lock().await.as_ref().map(|t| t.handle.status());
    let secret = if publisher.as_ref().is_some_and(|p| p.has_secret) { tcp_secret(state, false).ok() } else { None };
    let consumers = state.tcp_consumers.lock().await.values().map(|c| c.handle.status()).collect();
    TcpView { publisher, secret, consumers }
}

#[tauri::command]
pub async fn relay_tcp_status(state: State<'_, RelayState>) -> CmdResult<TcpView> {
    Ok(tcp_view(&state).await)
}

/// The tunnel's access secret (created on first use, kept in the keychain); `rotate` makes a new one.
#[tauri::command]
pub async fn relay_tcp_secret(state: State<'_, RelayState>, rotate: bool) -> CmdResult<String> {
    tcp_secret(&state, rotate)
}

/// Publishes `host:port` as a raw TCP tunnel (Supporter / self-hosted).
#[tauri::command]
pub async fn relay_tcp_start(
    state: State<'_, RelayState>,
    port: u16,
    host: Option<String>,
    subdomain: Option<String>,
    use_secret: bool,
    on_event: Channel<TcpTunnelEvent>,
) -> CmdResult<TcpView> {
    if port == 0 {
        return Err("Pick the local port to expose.".into());
    }
    let client = state.client()?;
    if !client.is_signed_in() {
        return Err("Sign in (or set a self-hosted token) first.".into());
    }
    if let Some(old) = state.tcp.lock().await.take() {
        old.handle.stop().await;
        old.forwarder.abort();
    }
    let secret = if use_secret { Some(tcp_secret(&state, false)?) } else { None };
    let host = host.map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).unwrap_or_else(|| "127.0.0.1".into());
    let opts = TcpTunnelOptions { local: format!("{host}:{port}"), subdomain: subdomain.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()), access_secret: secret, ..TcpTunnelOptions::for_port(port) };
    let handle = Arc::new(client.start_tcp_tunnel(opts));
    let forwarder = forward(handle.subscribe(), on_event);
    let _ = handle.wait_connected(Duration::from_secs(20)).await;
    *state.tcp.lock().await = Some(Running { handle, forwarder });
    Ok(tcp_view(&state).await)
}

#[tauri::command]
pub async fn relay_tcp_stop(state: State<'_, RelayState>) -> CmdResult<()> {
    if let Some(t) = state.tcp.lock().await.take() {
        t.handle.stop().await;
        t.forwarder.abort();
    }
    Ok(())
}

/// Listens on `127.0.0.1:<local_port>` and forwards connections through the TCP tunnel `name`.
#[tauri::command]
pub async fn relay_tcp_connect(state: State<'_, RelayState>, name: String, local_port: u16, secret: Option<String>, on_event: Channel<TcpConnectEvent>) -> CmdResult<TcpConnectStatus> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return Err("Enter the TCP tunnel's name.".into());
    }
    let client = state.client()?;
    if let Some(old) = state.tcp_consumers.lock().await.remove(&name) {
        old.handle.stop().await;
        old.forwarder.abort();
    }
    let access = match secret.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(s) => TcpAccess::Secret(s),
        None => TcpAccess::Account,
    };
    let bind = format!("127.0.0.1:{local_port}").parse().map_err(err)?;
    let handle = Arc::new(client.tcp_connect(TcpConnectOptions { name: name.clone(), bind, access }).await.map_err(err)?);
    let status = handle.status();
    let forwarder = forward(handle.subscribe(), on_event);
    state.tcp_consumers.lock().await.insert(name, Running { handle, forwarder });
    Ok(status)
}

#[tauri::command]
pub async fn relay_tcp_disconnect(state: State<'_, RelayState>, name: String) -> CmdResult<()> {
    if let Some(c) = state.tcp_consumers.lock().await.remove(&name) {
        c.handle.stop().await;
        c.forwarder.abort();
    }
    Ok(())
}

// ---------- custom domains (hosted: Firebase callables; self-host: CUSTOM_HOSTS) ----------

fn hosted_only(state: &RelayState) -> CmdResult<()> {
    if state.mode() == RelayMode::SelfHosted {
        return Err("On a self-hosted relay, map custom domains with CUSTOM_HOSTS in wrangler.toml and add a Worker route for each hostname (relay/README.md).".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn relay_domain_add(state: State<'_, RelayState>, hostname: String, subdomain: String) -> CmdResult<Value> {
    hosted_only(&state)?;
    state.client()?.function("addCustomDomain", &json!({ "hostname": hostname.trim(), "subdomain": subdomain.trim() })).await.map_err(err)
}

#[tauri::command]
pub async fn relay_domain_remove(state: State<'_, RelayState>, hostname: String) -> CmdResult<Value> {
    hosted_only(&state)?;
    state.client()?.function("removeCustomDomain", &json!({ "hostname": hostname.trim() })).await.map_err(err)
}

#[tauri::command]
pub async fn relay_domain_refresh(state: State<'_, RelayState>, hostname: String) -> CmdResult<Value> {
    hosted_only(&state)?;
    state.client()?.function("refreshCustomDomain", &json!({ "hostname": hostname.trim() })).await.map_err(err)
}

// ---------- hosted mocks ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MockPlanView {
    pub suggested_name: String,
    pub plan: PublishPlan,
    /// Mapping files that failed to load locally (not uploaded).
    pub load_errors: Vec<String>,
}

fn mock_payload(root: &str, settings: MockSettings, limits: Option<&devall_relay::Limits>) -> CmdResult<(devall_relay::MockPayload, PublishPlan, Vec<String>, String)> {
    let root_path = std::path::Path::new(root);
    let mocks = devall_mock::load_project(root_path);
    let load_errors = mocks.problems();
    let mappings: Vec<Value> = mocks.mappings.into_iter().map(|m| m.mapping).collect();
    let files_dir = root_path.join(devall_mock::MOCKS_DIR).join(devall_mock::FILES_DIR);
    let (payload, plan) = devall_relay::hosted_mock::build_payload(mappings, &files_dir, settings, limits).map_err(err)?;
    let name = Project::open(root).map(|p| p.manifest.name.clone()).unwrap_or_else(|_| "mocks".into());
    Ok((payload, plan, load_errors, devall_relay::hosted_mock::suggest_name(&name)))
}

/// What publishing would upload, with every unsupported feature listed (nothing is sent).
#[tauri::command]
pub async fn relay_mock_plan(state: State<'_, RelayState>, root: String, cors: bool, global_templating: bool) -> CmdResult<MockPlanView> {
    let limits = match state.client() {
        Ok(c) if c.is_signed_in() => c.account().await.ok().map(|a| a.limits),
        _ => None,
    };
    let (_, plan, load_errors, suggested_name) = tokio::task::spawn_blocking(move || mock_payload(&root, MockSettings { cors, global_templating }, limits.as_ref())).await.map_err(err)??;
    Ok(MockPlanView { suggested_name, plan, load_errors })
}

#[tauri::command]
pub async fn relay_mock_publish(state: State<'_, RelayState>, root: String, name: String, cors: bool, global_templating: bool) -> CmdResult<MockPublished> {
    let client = state.client()?;
    let limits = client.account().await.ok().map(|a| a.limits);
    let (payload, plan, _, _) = tokio::task::spawn_blocking(move || mock_payload(&root, MockSettings { cors, global_templating }, limits.as_ref())).await.map_err(err)??;
    if !plan.blocking.is_empty() {
        return Err(format!("{} Self-host the relay to publish without limits.", plan.blocking.join(" ")));
    }
    client.publish_mock(name.trim(), &payload).await.map_err(err)
}

// ---------- shareable snapshots ----------

/// Header names whose values are masked when sharing (unless the user unticks it).
const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "apikey",
    "x-auth-token",
    "x-access-token",
    "x-refresh-token",
    "x-csrf-token",
    "x-xsrf-token",
    "x-amz-security-token",
    "x-goog-api-key",
    "private-token",
    "x-vault-token",
    "ocp-apim-subscription-key",
];

fn sensitive_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SENSITIVE_HEADERS.contains(&n.as_str()) || n.ends_with("-token") || n.ends_with("-secret") || n.ends_with("-api-key") || n.contains("signature")
}

fn sensitive_param(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["token", "key", "secret", "password", "passwd", "pwd", "signature", "sig", "auth", "credential", "session", "code"].iter().any(|w| n == *w || n.contains(w))
}

/// Masks sensitive query parameter values in a URL, keeping everything else byte-for-byte.
fn mask_url(url: &str) -> String {
    let (base, rest) = match url.split_once('?') {
        Some(x) => x,
        None => return url.to_string(),
    };
    let (query, frag) = match rest.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (rest, None),
    };
    let masked: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) if !v.is_empty() && sensitive_param(&percent_decode(k)) => format!("{k}={MASK}"),
            _ => pair.to_string(),
        })
        .collect();
    let mut out = format!("{base}?{}", masked.join("&"));
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(s.as_bytes()).map(|(k, _)| k.into_owned()).next().unwrap_or_default()
}

/// Secret values of the request's environment (to mask them anywhere in the snapshot).
fn environment_secret_values(app: &AppState, p: &Project, env: Option<&str>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(env_id) = env.filter(|e| !e.is_empty()) else { return out };
    let Ok(env_file) = p.read_environment(env_id) else { return out };
    let resolver = SecretResolver::for_environment(app.secrets.as_ref(), p.id(), Some(env_id), p.root(), Some(&env_file), &app.providers);
    for name in &env_file.secrets {
        if let Some(v) = resolver.lookup(name).filter(|v| v.len() >= 4) {
            out.insert(v);
        }
    }
    out
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SentSummary {
    #[serde(default)]
    method: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    body: Option<String>,
}

fn header_value<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// Builds the snapshot uploaded by `POST /v1/shares` (pure; tested below).
#[allow(clippy::too_many_arguments)]
fn build_snapshot(
    title: &str,
    entry: &devall_history::HistoryEntry,
    body: &[u8],
    secret_values: &BTreeSet<String>,
    mask_headers: bool,
    include_response_body: bool,
    max_body: usize,
) -> Value {
    let sent: SentSummary = serde_json::from_value(entry.sent.clone()).unwrap_or_default();
    let scrub = |s: &str| redact_values(s, secret_values);
    let headers = |h: &[(String, String)]| -> Vec<(String, String)> {
        h.iter().map(|(k, v)| (k.clone(), if mask_headers && sensitive_header(k) { MASK.to_string() } else { scrub(v) })).collect()
    };
    let method = if sent.method.is_empty() { entry.method.clone() } else { sent.method.clone() };
    let url_raw = if sent.url.is_empty() { entry.url.clone() } else { sent.url.clone() };
    let url = if mask_headers { mask_url(&scrub(&url_raw)) } else { scrub(&url_raw) };
    let req_headers = headers(&sent.headers);
    let req_body = sent.body.as_deref().map(scrub);
    let mut redactions = vec!["secret values".to_string()];
    if mask_headers {
        redactions.push("authorization, cookie and API-key headers".into());
        redactions.push("token / key / password query parameters".into());
    }

    let response = entry.status.map(|status| {
        let ct = entry.content_type.clone();
        let (text, b64, truncated) = if !include_response_body {
            (None, None, false)
        } else {
            match std::str::from_utf8(body) {
                Ok(t) => {
                    let t = scrub(t);
                    if t.len() > max_body { (Some(t[..t.floor_char_boundary(max_body)].to_string()), None, true) } else { (Some(t), None, entry.body_truncated) }
                }
                Err(_) if body.len() <= max_body => {
                    use base64::Engine as _;
                    (None, Some(base64::engine::general_purpose::STANDARD.encode(body)), entry.body_truncated)
                }
                Err(_) => (None, None, true),
            }
        };
        json!({
            "status": status,
            "headers": headers(&entry.response_headers),
            "body": text,
            "bodyBase64": b64,
            "bodyTruncated": truncated,
            "size": entry.size,
            "contentType": ct,
        })
    });

    // cURL and a .req.yaml from what was sent (already masked).
    let prepared = PreparedRequest {
        method: method.clone(),
        url: url.clone(),
        headers: req_headers.clone(),
        body: match &req_body {
            Some(b) => PreparedBody::Bytes(b.clone().into_bytes()),
            None => PreparedBody::None,
        },
        timeout_ms: 30_000,
        follow_redirects: true,
        verify_tls: true,
        proxy: None,
        client_cert: None,
        digest: None,
        ntlm: None,
    };
    let curl = devall_http::to_curl(&prepared);
    let mut file = HttpRequestFile::new(title);
    file.method = method.clone();
    file.url = url.clone();
    file.headers = req_headers.iter().map(|(k, v)| KeyValue { name: k.clone(), value: v.clone(), enabled: true, description: None }).collect();
    file.auth = Auth::None;
    if let Some(b) = &req_body {
        let ct = header_value(&req_headers, "content-type").unwrap_or("");
        file.body = if ct.contains("json") { Body::Json { content: b.clone() } } else { Body::Text { content: b.clone(), content_type: (!ct.is_empty()).then(|| ct.to_string()) } };
    }
    let req_yaml = serde_yaml_ng::to_string(&file).unwrap_or_default();

    json!({
        "version": 1,
        "title": title,
        "request": { "method": method, "url": url, "headers": req_headers, "body": req_body },
        "response": response,
        "error": entry.error.as_deref().map(scrub),
        "timing": { "totalMs": entry.duration_ms },
        "environment": entry.environment,
        "at": entry.at,
        "curl": curl,
        "reqYaml": req_yaml,
        "redactions": redactions,
    })
}

/// Uploads a redacted snapshot of a history entry; returns `{id, url, expiresAt, …}`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn relay_share_create(
    state: State<'_, RelayState>,
    app: State<'_, AppState>,
    root: String,
    history_id: String,
    title: Option<String>,
    mask_headers: bool,
    include_response_body: bool,
    ttl_seconds: Option<u64>,
) -> CmdResult<Value> {
    let client = state.client()?;
    if !client.is_signed_in() {
        return Err("Sharing needs a relay account (sign in) or a self-hosted relay token — links are never anonymous.".into());
    }
    let limits = client.account().await.map_err(err)?.limits;
    let (entry, body) = app.history(&root)?.get(&history_id).map_err(err)?.ok_or("That history entry no longer exists.")?;
    let p = Project::open(&root).map_err(err)?;
    let secret_values = environment_secret_values(&app, &p, entry.environment.as_deref());
    let title = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| if entry.request_name.is_empty() { format!("{} {}", entry.method, entry.url) } else { entry.request_name.clone() });
    let max_body = (limits.share_max_bytes as usize).saturating_sub(16 * 1024).clamp(4 * 1024, 4 * 1024 * 1024);
    let snapshot = build_snapshot(&title, &entry, &body, &secret_values, mask_headers, include_response_body, max_body);
    let mut payload = json!({ "snapshot": snapshot });
    if let Some(t) = ttl_seconds {
        payload["ttlSeconds"] = json!(t);
    }
    client.api(ApiMethod::POST, "/v1/shares", Some(&payload)).await.map_err(err)
}

// ---------- monitors ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorCandidate {
    pub path: String,
    pub name: String,
    pub method: String,
}

#[tauri::command]
pub fn relay_monitor_candidates(root: String) -> CmdResult<Vec<MonitorCandidate>> {
    let p = Project::open(&root).map_err(err)?;
    let mut out = Vec::new();
    for path in p.tree().map_err(err)?.request_paths() {
        if let Ok(r) = p.read_request(&path) {
            out.push(MonitorCandidate { path, name: r.name, method: r.method });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorDraft {
    /// `MonitorRequest` JSON for `POST /v1/monitors` (secrets stay `{{secret.NAME}}` placeholders).
    pub requests: Vec<Value>,
    /// Secret names the requests use — none is uploaded unless the user ticks it.
    pub secret_names: Vec<String>,
    /// `(request path, problem)`: skipped requests and ignored parts.
    pub warnings: Vec<(String, String)>,
}

/// Turns saved requests into monitor requests: variables of `env` resolved, secrets kept as
/// placeholders, assertions copied (snapshot assertions and scripts can't run at the edge).
fn prepare_monitor(p: &Project, paths: &[String], env: Option<&str>) -> CmdResult<MonitorDraft> {
    let env_file = env.filter(|e| !e.is_empty()).map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let mut requests = Vec::new();
    let mut secret_names = BTreeSet::new();
    let mut warnings = Vec::new();
    for path in paths {
        let req = match p.read_request(path) {
            Ok(r) => r,
            Err(e) => {
                warnings.push((path.clone(), e.to_string()));
                continue;
            }
        };
        let eff = p.effective_request(path, &req).map_err(err)?;
        let mut r = Resolver::new(&NoSecrets);
        if let Some(e) = &env_file {
            r.push_layer(e.variables.clone());
        }
        for fv in &eff.folder_variables {
            r.push_layer(fv.clone());
        }
        r.push_layer(p.manifest.variables.clone());
        let mut request = eff.request.clone();
        let mut basic: Option<(String, String)> = None;
        match &request.auth {
            Auth::Basic { username, password } => {
                basic = Some((r.resolve(username), r.resolve(password)));
                request.auth = Auth::None;
            }
            Auth::None | Auth::Inherit | Auth::Bearer { .. } | Auth::ApiKey { .. } => {}
            other => {
                warnings.push((path.clone(), format!("{} auth can't run from the edge; skipped", auth_label(other))));
                continue;
            }
        }
        if matches!(request.body, Body::Multipart { .. } | Body::Binary { .. }) {
            warnings.push((path.clone(), "multipart / file bodies aren't supported in monitors; skipped".into()));
            continue;
        }
        if request.scripts.as_ref().is_some_and(|s| !s.is_empty()) {
            warnings.push((path.clone(), "pre/post scripts don't run in monitors (ignored)".into()));
        }
        let prepared = match devall_http::prepare(&request, &mut r, p.root()) {
            Ok(x) => x,
            Err(e) => {
                warnings.push((path.clone(), e.to_string()));
                continue;
            }
        };
        for u in &r.unresolved {
            match u.strip_prefix("secret.") {
                Some(name) => {
                    secret_names.insert(name.trim().to_string());
                }
                None => warnings.push((path.clone(), format!("{{{{{u}}}}} isn't defined in this environment (sent literally)"))),
            }
        }
        let mut headers = prepared.headers.clone();
        let body = match &prepared.body {
            PreparedBody::None => None,
            PreparedBody::Bytes(b) => match String::from_utf8(b.clone()) {
                Ok(s) => Some(s),
                Err(_) => {
                    warnings.push((path.clone(), "binary bodies aren't supported in monitors; skipped".into()));
                    continue;
                }
            },
            PreparedBody::Form(pairs) => {
                if header_value(&headers, "content-type").is_none() {
                    headers.push(("Content-Type".into(), "application/x-www-form-urlencoded".into()));
                }
                Some(url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs.iter()).finish())
            }
            PreparedBody::Multipart(_) => {
                warnings.push((path.clone(), "multipart bodies aren't supported in monitors; skipped".into()));
                continue;
            }
        };
        let mut assertions = Vec::new();
        for a in request.tests.iter().filter(|a| a.enabled) {
            if matches!(a.target, devall_core::AssertTarget::Snapshot { .. }) {
                warnings.push((path.clone(), "snapshot assertions are skipped in monitors".into()));
                continue;
            }
            let mut v = serde_json::to_value(a).map_err(err)?;
            if let Some(o) = v.as_object_mut() {
                o.remove("enabled");
            }
            assertions.push(v);
        }
        if assertions.is_empty() {
            assertions.push(json!({ "assert": "status", "op": "lt", "value": "400" }));
        }
        requests.push(json!({
            "name": request.name,
            "method": prepared.method,
            "url": prepared.url,
            "headers": headers,
            "body": body,
            "basicAuth": basic.map(|(u, p)| json!({ "username": u, "password": p })),
            "timeoutMs": prepared.timeout_ms.clamp(100, 30_000),
            "followRedirects": prepared.follow_redirects,
            "assertions": assertions,
        }));
    }
    Ok(MonitorDraft { requests, secret_names: secret_names.into_iter().collect(), warnings })
}

fn auth_label(a: &Auth) -> &'static str {
    match serde_json::to_value(a).ok().and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string)).as_deref() {
        Some("oauth2") => "OAuth 2.0",
        Some("awsSigV4") => "AWS SigV4",
        Some("digest") => "Digest",
        Some("ntlm") => "NTLM",
        _ => "This",
    }
}

#[tauri::command]
pub async fn relay_monitor_prepare(root: String, paths: Vec<String>, env: Option<String>) -> CmdResult<MonitorDraft> {
    tokio::task::spawn_blocking(move || {
        let p = Project::open(&root).map_err(err)?;
        prepare_monitor(&p, &paths, env.as_deref())
    })
    .await
    .map_err(err)?
}

/// Creates (`id` = None) or replaces a monitor. Only the secrets named in `upload_secrets` are
/// read from the keychain and uploaded; names in `remove_secrets` are deleted on the relay.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn relay_monitor_save(
    state: State<'_, RelayState>,
    app: State<'_, AppState>,
    root: String,
    id: Option<String>,
    input: Value,
    env: Option<String>,
    upload_secrets: Vec<String>,
    remove_secrets: Vec<String>,
) -> CmdResult<Value> {
    let client = state.client()?;
    let mut input = input;
    let mut secrets: BTreeMap<String, Value> = BTreeMap::new();
    if !upload_secrets.is_empty() {
        let p = Project::open(&root).map_err(err)?;
        let env = env.filter(|e| !e.is_empty());
        let env_file = env.as_deref().map(|id| p.read_environment(id)).transpose().map_err(err)?;
        let resolver = SecretResolver::for_environment(app.secrets.as_ref(), p.id(), env.as_deref(), p.root(), env_file.as_ref(), &app.providers);
        for name in &upload_secrets {
            let v = resolver.lookup(name).ok_or_else(|| format!("No value for secret {name} in this environment."))?;
            secrets.insert(name.clone(), Value::String(v));
        }
    }
    for name in remove_secrets {
        secrets.entry(name).or_insert(Value::Null);
    }
    if !secrets.is_empty() {
        input["secrets"] = serde_json::to_value(secrets).map_err(err)?;
    }
    match id {
        Some(id) => client.api(ApiMethod::PUT, &format!("/v1/monitors/{id}"), Some(&input)).await.map_err(err),
        None => client.api(ApiMethod::POST, "/v1/monitors", Some(&input)).await.map_err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> devall_history::HistoryEntry {
        devall_history::HistoryEntry {
            id: "h1".into(),
            request_path: "users/get.req.yaml".into(),
            request_name: "Get user".into(),
            environment: Some("staging".into()),
            method: "POST".into(),
            url: "https://api.example.com/users?id=1&api_key=k-123&page=2".into(),
            at: 1_800_000_000_000,
            status: Some(200),
            error: None,
            duration_ms: 87.0,
            size: 64,
            content_type: Some("application/json".into()),
            sent: json!({
                "method": "POST",
                "url": "https://api.example.com/users?id=1&api_key=k-123&page=2",
                "headers": [["Authorization", "Bearer ••••••"], ["X-Trace", "abc"], ["Content-Type", "application/json"], ["X-Api-Key", "plain-key-999"]],
                "body": "{\"name\":\"ada\"}"
            }),
            response_headers: vec![("Set-Cookie".into(), "sid=s3cr3t-cookie".into()), ("Content-Type".into(), "application/json".into())],
            tests: json!([]),
            body_truncated: false,
        }
    }

    #[test]
    fn snapshots_mask_secrets_headers_and_query_parameters() {
        let secrets: BTreeSet<String> = ["topsecretvalue".to_string()].into_iter().collect();
        let body = br#"{"id":1,"echo":"topsecretvalue"}"#;
        let s = build_snapshot("Get user", &entry(), body, &secrets, true, true, 100_000);
        let text = s.to_string();
        assert!(!text.contains("topsecretvalue"), "secret values are masked everywhere");
        assert!(!text.contains("s3cr3t-cookie") && !text.contains("plain-key-999") && !text.contains("k-123"));
        assert_eq!(s["request"]["url"], "https://api.example.com/users?id=1&api_key=••••••&page=2");
        let curl = s["curl"].as_str().unwrap();
        assert!(curl.starts_with("curl") && curl.contains("-X POST") && curl.contains("••••••"), "{curl}");
        assert!(s["reqYaml"].as_str().unwrap().contains("method: POST"));
        assert_eq!(s["response"]["status"], 200);

        // Unticked: headers stay, secret values are still masked; bodies can be left out.
        let open = build_snapshot("Get user", &entry(), body, &secrets, false, false, 100_000);
        let text = open.to_string();
        assert!(text.contains("plain-key-999") && !text.contains("topsecretvalue"));
        assert!(open["response"]["body"].is_null());
        let small = build_snapshot("t", &entry(), &[b'x'; 300], &BTreeSet::new(), true, true, 100);
        assert_eq!(small["response"]["bodyTruncated"], true);
    }

    #[test]
    fn monitors_keep_secrets_as_placeholders() {
        let dir = std::env::temp_dir().join(format!("devall-cloud-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = Project::init(&dir, "m").unwrap();
        let mut env = devall_core::EnvironmentFile::new("Staging");
        env.variables.insert("baseUrl".into(), "https://api.example.com".into());
        env.secrets.push("TOKEN".into());
        p.save_environment("staging", &env).unwrap();
        let mut req = HttpRequestFile::new("Health");
        req.url = "{{baseUrl}}/health?v={{missing}}".into();
        req.auth = Auth::Bearer { token: "{{secret.TOKEN}}".into(), prefix: None };
        req.tests.push(devall_core::Assertion { target: devall_core::AssertTarget::Status, op: devall_core::AssertOp::Eq, value: Some("200".into()), enabled: true });
        p.save_request("health.req.yaml", &req).unwrap();
        let mut basic = HttpRequestFile::new("Basic");
        basic.url = "{{baseUrl}}/b".into();
        basic.auth = Auth::Basic { username: "u".into(), password: "{{secret.PW}}".into() };
        p.save_request("basic.req.yaml", &basic).unwrap();
        let d = prepare_monitor(&p, &["health.req.yaml".into(), "basic.req.yaml".into()], Some("staging")).unwrap();
        assert_eq!(d.secret_names, vec!["PW", "TOKEN"]);
        assert_eq!(d.requests[0]["url"], "https://api.example.com/health?v={{missing}}");
        let auth = d.requests[0]["headers"].as_array().unwrap().iter().find(|h| h[0] == "Authorization").unwrap();
        assert_eq!(auth[1], "Bearer {{secret.TOKEN}}");
        assert_eq!(d.requests[0]["assertions"], json!([{ "assert": "status", "op": "eq", "value": "200" }]));
        assert_eq!(d.requests[1]["basicAuth"], json!({ "username": "u", "password": "{{secret.PW}}" }));
        assert!(d.warnings.iter().any(|(_, w)| w.contains("missing")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn masks_only_sensitive_query_parameters() {
        assert_eq!(mask_url("https://x/a?token=abc&q=1#f"), "https://x/a?token=••••••&q=1#f");
        assert_eq!(mask_url("https://x/a"), "https://x/a");
        assert!(sensitive_header("X-Custom-Token") && sensitive_header("Cookie") && !sensitive_header("Accept"));
    }
}
