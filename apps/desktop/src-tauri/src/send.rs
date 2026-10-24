//! Sending requests, collection runs, history, cookies, OAuth tokens and runtime variables.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use devall_core::secrets::SecretResolver;
use devall_core::vars::redact_values;
use devall_core::{Auth, HttpRequestFile, Project, Variables};
use devall_history::{HistoryEntry, HistorySummary};
use devall_script::{ResponseInfo, ScriptEngine, ScriptEnv, ScriptReport, ScriptSource, apply_post_response, apply_pre_request, collect_scripts};
use devall_http::checks::{self, AssertionResult, CaptureResult};
use devall_http::runner::{self, RunContext, RunHooks, RunItemResult, RunOptions, RunSummary};
use devall_http::sse::SseEvent;
use devall_http::{CookieInfo, ExecObserver, HttpError, HttpResponseData, PreparedBody, PreparedRequest, ResponseView, prepare_authorized, to_curl};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::commands::resolver_for;
use crate::state::{AppState, KeychainTokens, now_ms};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SentSummary {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

pub fn summary(req: &PreparedRequest, secrets: &BTreeSet<String>) -> SentSummary {
    let body = match &req.body {
        PreparedBody::None => None,
        PreparedBody::Bytes(b) => Some(match std::str::from_utf8(b) {
            Ok(t) if t.len() <= 256 * 1024 => redact_values(t, secrets),
            Ok(t) => format!("{}… ({} bytes)", redact_values(&t[..t.floor_char_boundary(256 * 1024)], secrets), t.len()),
            Err(_) => format!("<{} bytes of binary data>", b.len()),
        }),
        PreparedBody::Form(pairs) => Some(redact_values(&pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"), secrets)),
        PreparedBody::Multipart(parts) => Some(
            parts
                .iter()
                .map(|p| match &p.file {
                    Some(f) => format!("{}=@{}", p.name, f.display()),
                    None => format!("{}={}", p.name, redact_values(&p.value, secrets)),
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    };
    SentSummary {
        method: req.method.clone(),
        url: redact_values(&req.url, secrets),
        headers: req.headers.iter().map(|(k, v)| (k.clone(), redact_values(v, secrets))).collect(),
        body,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    response: Option<ResponseView>,
    error: Option<HttpError>,
    cancelled: bool,
    /// What was actually sent, with secret values masked.
    sent: Option<SentSummary>,
    unresolved: Vec<String>,
    /// Keychain problems hit while resolving secrets (e.g. locked keychain).
    secret_errors: Vec<String>,
    tests: Vec<AssertionResult>,
    captures: Vec<CaptureResult>,
    history_id: Option<String>,
    at: i64,
    /// Console output and errors from pre-request / post-response scripts.
    scripts: ScriptReport,
}

/// Messages streamed while a response arrives.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StreamMsg {
    Sse { event: SseEvent },
    Progress { bytes: usize },
}

struct ChannelObserver(Channel<StreamMsg>);

impl ExecObserver for ChannelObserver {
    fn sse_event(&self, event: &SseEvent) {
        let _ = self.0.send(StreamMsg::Sse { event: event.clone() });
    }
    fn progress(&self, bytes: usize) {
        let _ = self.0.send(StreamMsg::Progress { bytes });
    }
}

fn browser(app: &AppHandle) -> impl Fn(&str) -> Result<(), String> + Send + Sync + use<> {
    let app = app.clone();
    move |url: &str| app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// Script state carried from `prepare_for` (pre-request) to the response (post-response).
#[derive(Default)]
struct ScriptRun {
    report: ScriptReport,
    tests: Vec<AssertionResult>,
    post: Vec<ScriptSource>,
    /// The request after pre-request edits (what post-response scripts see as `pm.request`).
    sent_request: Option<HttpRequestFile>,
}

struct Prepared {
    request: PreparedRequest,
    unresolved: Vec<String>,
    secret_values: BTreeSet<String>,
    secret_errors: Vec<String>,
    tests: Vec<devall_core::Assertion>,
    captures: Vec<devall_core::Capture>,
}

/// Resolves `request` for sending. With `scripts`, pre-request scripts run first (never for
/// Copy as cURL, which passes `None`).
async fn prepare_for(
    app: &AppHandle,
    state: &AppState,
    root: &str,
    path: &str,
    request: &HttpRequestFile,
    env: Option<&str>,
    scripts: Option<&mut ScriptRun>,
) -> Result<Prepared, HttpError> {
    let invalid = |e: String| HttpError::InvalidRequest(e);
    let p = Project::open(root).map_err(|e| invalid(e.to_string()))?;
    let env = env.filter(|e| !e.is_empty());
    let env_file = env.map(|id| p.read_environment(id)).transpose().map_err(|e| invalid(e.to_string()))?;
    let mut eff = p.effective_request(path, request).map_err(|e| invalid(e.to_string()))?;
    let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env, p.root(), env_file.as_ref(), &state.providers);
    let mut runtime = state.runtime_vars(root);
    let mut script_secrets = BTreeSet::new();
    let mut scripts = scripts;
    if let Some(run) = scripts.as_deref_mut() {
        let chain = collect_scripts(&p, path, request).map_err(|e| invalid(e.to_string()))?;
        run.post = chain.post;
        if !chain.pre.is_empty() {
            let senv = ScriptEnv {
                environment: env_file.as_ref().map(|e| &e.variables),
                environment_name: env_file.as_ref().map(|e| e.name.as_str()),
                folder_variables: &eff.folder_variables,
                project_variables: Some(&p.manifest.variables),
                request_path: path,
                secrets: Some(&secrets),
                ..Default::default()
            };
            let out = apply_pre_request(&ScriptEngine::new(), &chain.pre, &mut eff.request, &mut runtime, &senv);
            // Variables a script sets are session runtime variables (like captures).
            state.runtime.lock().unwrap().insert(root.to_string(), runtime.clone());
            run.tests.extend(out.tests.into_iter().map(AssertionResult::from));
            run.report.merge(out.report);
            script_secrets = out.secret_values;
            if let Some(e) = out.error {
                return Err(HttpError::InvalidRequest(e.to_string()));
            }
            if out.skip_request {
                return Err(HttpError::InvalidRequest("skipped by a pre-request script".into()));
            }
        }
        run.sent_request = Some(eff.request.clone());
    }
    let mut r = resolver_for(&p, &runtime, env, env_file.as_ref(), &eff.folder_variables, &secrets);
    r.secret_values.extend(script_secrets);
    let tokens = KeychainTokens { store: state.secrets.as_ref(), project_id: p.id().to_string() };
    let open = browser(app);
    let mut prepared = prepare_authorized(&eff.request, &mut r, p.root(), &tokens, Some(&open)).await.map_err(|e| e.redacted(&r.secret_values))?;
    // Plugin pre-request hooks (sends only, never Copy as cURL) see the resolved request.
    if let Some(run) = scripts
        && let Some(plugins) = devall_http::plugins::installed().filter(|pl| pl.has_hooks(p.root(), devall_script::ScriptPhase::PreRequest))
    {
        let root_path = p.root().to_path_buf();
        let (name, env_name) = (request.name.clone(), env_file.as_ref().map(|e| e.name.clone()));
        let (req, out) = tauri::async_runtime::spawn_blocking(move || {
            let mut req = prepared;
            let out = plugins.pre_request(&root_path, &mut req, &name, env_name.as_deref());
            (req, out)
        })
        .await
        .map_err(|e| HttpError::InvalidRequest(e.to_string()))?;
        prepared = req;
        run.report.merge(devall_http::plugins::report(devall_script::ScriptPhase::PreRequest, out.logs));
        if let Some(e) = out.errors.first() {
            return Err(HttpError::InvalidRequest(redact_values(&format!("plugin hook: {e}"), &r.secret_values)));
        }
    }
    let secret_errors = secrets.errors.lock().unwrap().clone();
    Ok(Prepared {
        request: prepared,
        unresolved: r.unresolved.iter().cloned().collect(),
        secret_values: r.secret_values.clone(),
        secret_errors,
        tests: eff.request.tests.clone(),
        captures: eff.request.captures.clone(),
    })
}

/// Resolves and sends a request. `path` is the request's location (for folder inheritance) and
/// may be empty for unsaved drafts; `request` is the editor's current (possibly unsaved) state.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn send_request(
    app: AppHandle,
    state: State<'_, AppState>,
    send_id: String,
    root: String,
    path: String,
    request: HttpRequestFile,
    env: Option<String>,
    on_event: Channel<StreamMsg>,
) -> CmdResult<SendResult> {
    let at = now_ms();
    let blank = |error: HttpError| SendResult {
        response: None,
        error: Some(error),
        cancelled: false,
        sent: None,
        unresolved: vec![],
        secret_errors: vec![],
        tests: vec![],
        captures: vec![],
        history_id: None,
        at,
        scripts: ScriptReport::default(),
    };
    let mut run = ScriptRun::default();
    let prepared = match prepare_for(&app, &state, &root, &path, &request, env.as_deref(), Some(&mut run)).await {
        Ok(p) => p,
        Err(e) => {
            let mut res = blank(e);
            res.tests = std::mem::take(&mut run.tests);
            if run.report.skipped && run.report.errors.is_empty() {
                res.error = None;
                res.cancelled = true;
            }
            res.scripts = run.report;
            return Ok(res);
        }
    };
    let sent = summary(&prepared.request, &prepared.secret_values);
    let req = prepared.request.clone();
    let pool = state.pool(&root);
    let task = tokio::spawn(async move {
        let obs = ChannelObserver(on_event);
        pool.execute_with(&req, Some(&obs)).await
    });
    state.in_flight.lock().unwrap().insert(send_id.clone(), task.abort_handle());
    let joined = task.await;
    state.in_flight.lock().unwrap().remove(&send_id);

    let (resp, error, cancelled) = match joined {
        Ok(Ok(resp)) => (Some(resp), None, false),
        Ok(Err(e)) => (None, Some(e.redacted(&prepared.secret_values)), false),
        Err(j) if j.is_cancelled() => (None, None, true),
        Err(j) => (None, Some(HttpError::Request(j.to_string())), false),
    };

    let (mut tests, mut captures) = (Vec::new(), Vec::new());
    if let Some(resp) = &resp {
        // Assertions resolve `{{vars}}` in expected values with the same layers as the request.
        let p = Project::open(&root).map_err(err)?;
        let env_file = env.as_deref().filter(|e| !e.is_empty()).and_then(|id| p.read_environment(id).ok());
        let eff = p.effective_request(&path, &request).map_err(err)?;
        let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env.as_deref(), p.root(), env_file.as_ref(), &state.providers);
        let runtime = state.runtime_vars(&root);
        let mut r = resolver_for(&p, &runtime, env.as_deref(), env_file.as_ref(), &eff.folder_variables, &secrets);
        let cx = checks::CheckContext { project_root: Some(p.root()), request_path: Some(&path), update_snapshots: false };
        tests = checks::evaluate_in(&prepared.tests, resp, &mut r, &cx);
        let (vars, caps) = checks::capture(&prepared.captures, resp);
        if !vars.is_empty() {
            state.runtime.lock().unwrap().entry(root.clone()).or_default().extend(vars);
        }
        captures = caps;
        if !run.post.is_empty() {
            let mut runtime = state.runtime_vars(&root);
            let senv = ScriptEnv {
                environment: env_file.as_ref().map(|e| &e.variables),
                environment_name: env_file.as_ref().map(|e| e.name.as_str()),
                folder_variables: &eff.folder_variables,
                project_variables: Some(&p.manifest.variables),
                request_path: &path,
                secrets: Some(&secrets),
                ..Default::default()
            };
            let info = ResponseInfo::new(resp.status, &resp.status_text, &resp.headers, &resp.body, resp.timings.total_ms, resp.body_size);
            let out = apply_post_response(&ScriptEngine::new(), &run.post, run.sent_request.as_ref().unwrap_or(&eff.request), &info, &mut runtime, &senv);
            state.runtime.lock().unwrap().insert(root.clone(), runtime);
            tests.extend(out.tests.into_iter().map(AssertionResult::from));
            if let Some(e) = &out.error {
                tests.push(AssertionResult { label: format!("{} script ({})", e.phase.label(), e.source), passed: false, actual: None, message: Some(e.to_string()) });
            }
            run.report.merge(out.report);
        }
        if let Some(plugins) = devall_http::plugins::installed().filter(|pl| pl.has_hooks(std::path::Path::new(&root), devall_script::ScriptPhase::PostResponse)) {
            let (root_path, sent_req, resp_copy, name) = (std::path::PathBuf::from(&root), prepared.request.clone(), resp.clone(), request.name.clone());
            if let Ok(out) = tauri::async_runtime::spawn_blocking(move || plugins.post_response(&root_path, &sent_req, &resp_copy, &name)).await {
                tests.extend(out.tests);
                tests.extend(devall_http::plugins::error_rows(&out.errors));
                if !out.variables.is_empty() {
                    state.runtime.lock().unwrap().entry(root.clone()).or_default().extend(out.variables);
                }
                run.report.merge(devall_http::plugins::report(devall_script::ScriptPhase::PostResponse, out.logs));
            }
        }
    }
    // Pre-request `pm.test` results come first (and are saved in history with the rest).
    tests.splice(0..0, std::mem::take(&mut run.tests));

    let history_id = if cancelled {
        None
    } else {
        record(&state, &root, &path, &request.name, env.as_deref(), &sent, resp.as_ref(), error.as_ref(), &tests, at)
    };
    if !cancelled {
        crate::modules::logs::record_http_send(&root, &path, &request.name, &sent, resp.as_ref(), error.as_ref().map(|e| e as &dyn std::fmt::Display), history_id.as_deref(), at);
    }

    Ok(SendResult {
        response: resp.map(HttpResponseData::into_view),
        error,
        cancelled,
        sent: Some(sent),
        unresolved: prepared.unresolved,
        secret_errors: prepared.secret_errors,
        tests,
        captures,
        history_id,
        at,
        scripts: run.report,
    })
}

#[allow(clippy::too_many_arguments)]
fn record(
    state: &AppState,
    root: &str,
    path: &str,
    name: &str,
    env: Option<&str>,
    sent: &SentSummary,
    resp: Option<&HttpResponseData>,
    error: Option<&HttpError>,
    tests: &[AssertionResult],
    at: i64,
) -> Option<String> {
    let h = state.history(root).ok()?;
    // Settings → History: bodies can be left out (status, timing and headers are always kept).
    let keep_body = devall_core::local::load_settings().response_bodies;
    let body: &[u8] = if keep_body { resp.map(|r| r.body.as_slice()).unwrap_or_default() } else { &[] };
    let entry = HistoryEntry {
        id: String::new(),
        request_path: path.to_string(),
        request_name: name.to_string(),
        environment: env.map(str::to_string),
        method: sent.method.clone(),
        url: sent.url.clone(),
        at,
        status: resp.map(|r| r.status),
        error: error.map(|e| e.to_string()),
        duration_ms: resp.map(|r| r.timings.total_ms).unwrap_or(0.0),
        size: resp.map(|r| r.body_size).unwrap_or(0),
        content_type: resp.and_then(|r| r.header("content-type").map(str::to_string)),
        sent: serde_json::to_value(sent).unwrap_or_default(),
        response_headers: resp.map(|r| r.headers.clone()).unwrap_or_default(),
        tests: serde_json::to_value(tests).unwrap_or_default(),
        body_truncated: resp.is_some_and(|r| r.body_truncated || (!keep_body && !r.body.is_empty())),
    };
    h.insert(entry, body).ok()
}

#[tauri::command]
pub fn cancel_request(state: State<'_, AppState>, send_id: String) -> bool {
    match state.in_flight.lock().unwrap().remove(&send_id) {
        Some(h) => {
            h.abort();
            true
        }
        None => false,
    }
}

/// cURL for the current editor state. Secrets are masked unless `include_secrets` is set.
#[tauri::command]
pub async fn copy_as_curl(
    app: AppHandle,
    state: State<'_, AppState>,
    root: String,
    path: String,
    request: HttpRequestFile,
    env: Option<String>,
    include_secrets: bool,
) -> CmdResult<String> {
    let p = prepare_for(&app, &state, &root, &path, &request, env.as_deref(), None).await.map_err(err)?;
    let cmd = to_curl(&p.request);
    Ok(if include_secrets { cmd } else { redact_values(&cmd, &p.secret_values) })
}

// ---------- runtime variables ----------

#[tauri::command]
pub fn runtime_vars(state: State<'_, AppState>, root: String) -> Variables {
    state.runtime_vars(&root)
}

#[tauri::command]
pub fn runtime_vars_set(state: State<'_, AppState>, root: String, vars: Variables) {
    state.runtime.lock().unwrap().insert(root, vars);
}

// ---------- history ----------

#[tauri::command]
pub fn history_list(state: State<'_, AppState>, root: String, path: Option<String>, limit: Option<usize>) -> CmdResult<Vec<HistorySummary>> {
    state.history(&root)?.list(path.as_deref(), limit.unwrap_or(100)).map_err(err)
}

/// Every request's responses (History page), newest first, filtered by `query`.
#[tauri::command]
pub fn history_search(state: State<'_, AppState>, root: String, query: Option<String>, offset: Option<usize>, limit: Option<usize>) -> CmdResult<Vec<HistorySummary>> {
    state.history(&root)?.search(query.as_deref(), offset.unwrap_or(0), limit.unwrap_or(200)).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDetail {
    entry: HistoryEntry,
    body_text: Option<String>,
    body_base64: Option<String>,
    previous_id: Option<String>,
}

#[tauri::command]
pub fn history_get(state: State<'_, AppState>, root: String, id: String) -> CmdResult<Option<HistoryDetail>> {
    let h = state.history(&root)?;
    let Some((entry, body)) = h.get(&id).map_err(err)? else { return Ok(None) };
    let previous_id = h.previous(&id).map_err(err)?;
    let (body_text, body_base64) = match String::from_utf8(body) {
        Ok(t) => (Some(t), None),
        Err(e) => {
            use base64::Engine;
            (None, Some(base64::engine::general_purpose::STANDARD.encode(e.as_bytes())))
        }
    };
    Ok(Some(HistoryDetail { entry, body_text, body_base64, previous_id }))
}

#[tauri::command]
pub fn history_delete(state: State<'_, AppState>, root: String, id: String) -> CmdResult<bool> {
    state.history(&root)?.delete(&id).map_err(err)
}

#[tauri::command]
pub fn history_clear(state: State<'_, AppState>, root: String, path: Option<String>) -> CmdResult<usize> {
    state.history(&root)?.clear(path.as_deref()).map_err(err)
}

// ---------- cookies ----------

#[tauri::command]
pub fn cookies_list(state: State<'_, AppState>, root: String) -> Vec<CookieInfo> {
    state.pool(&root).cookies().list()
}

#[tauri::command]
pub fn cookies_delete(state: State<'_, AppState>, root: String, domain: String, path: String, name: String) -> bool {
    state.pool(&root).cookies().remove(&domain, &path, &name)
}

#[tauri::command]
pub fn cookies_clear(state: State<'_, AppState>, root: String) {
    state.pool(&root).cookies().clear();
}

// ---------- OAuth 2.0 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    /// First/last characters only.
    preview: String,
    expires_at: Option<i64>,
    scope: Option<String>,
    has_refresh_token: bool,
}

/// Fetches (or refreshes / re-uses) the token for the request's effective OAuth2 auth.
#[tauri::command]
pub async fn oauth2_token(
    app: AppHandle,
    state: State<'_, AppState>,
    root: String,
    path: String,
    request: HttpRequestFile,
    env: Option<String>,
    force: bool,
) -> CmdResult<TokenInfo> {
    let p = Project::open(&root).map_err(err)?;
    let eff = p.effective_request(&path, &request).map_err(err)?;
    let Auth::OAuth2(cfg) = &eff.request.auth else { return Err("this request doesn't use OAuth 2.0 (check folder auth too)".into()) };
    let env_file = env.as_deref().filter(|e| !e.is_empty()).map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env.as_deref(), p.root(), env_file.as_ref(), &state.providers);
    let runtime = state.runtime_vars(&root);
    let mut r = resolver_for(&p, &runtime, env.as_deref(), env_file.as_ref(), &eff.folder_variables, &secrets);
    let resolved = devall_http::resolve_oauth2(cfg, &mut r);
    let tokens = KeychainTokens { store: state.secrets.as_ref(), project_id: p.id().to_string() };
    let open = browser(&app);
    let t = devall_http::oauth2::get_token(&resolved, &tokens, Some(&open), force).await.map_err(err)?;
    let a = &t.access_token;
    let preview = if a.len() > 12 { format!("{}…{}", &a[..6], &a[a.len() - 4..]) } else { "••••".into() };
    Ok(TokenInfo { preview, expires_at: t.expires_at, scope: t.scope, has_refresh_token: t.refresh_token.is_some() })
}

#[tauri::command]
pub fn oauth2_clear(state: State<'_, AppState>, root: String, path: String, request: HttpRequestFile, env: Option<String>) -> CmdResult<()> {
    let p = Project::open(&root).map_err(err)?;
    let eff = p.effective_request(&path, &request).map_err(err)?;
    let Auth::OAuth2(cfg) = &eff.request.auth else { return Ok(()) };
    let env_file = env.as_deref().filter(|e| !e.is_empty()).map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env.as_deref(), p.root(), env_file.as_ref(), &state.providers);
    let mut r = resolver_for(&p, &state.runtime_vars(&root), env.as_deref(), env_file.as_ref(), &eff.folder_variables, &secrets);
    let resolved = devall_http::resolve_oauth2(cfg, &mut r);
    devall_http::oauth2::clear_token(&resolved, &KeychainTokens { store: state.secrets.as_ref(), project_id: p.id().to_string() });
    Ok(())
}

// ---------- collection runner ----------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    /// Folder (relative to `http/`) to run; empty = whole project.
    folder: String,
    /// Explicit request paths (overrides `folder`).
    #[serde(default)]
    paths: Vec<String>,
    env: Option<String>,
    #[serde(default)]
    iterations: Option<usize>,
    #[serde(default)]
    bail: bool,
    #[serde(default)]
    delay_ms: u64,
    #[serde(default)]
    fail_on_http_error: bool,
    /// CSV or JSON text with one row per iteration.
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    keep_variables: bool,
    /// Record missing / accept changed response snapshots.
    #[serde(default)]
    update_snapshots: bool,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RunEvent {
    Started { total: usize },
    Item { result: Box<RunItemResult> },
}

struct UiHooks<'a> {
    channel: Channel<RunEvent>,
    cancel: Arc<AtomicBool>,
    state: &'a AppState,
    root: String,
    env: Option<String>,
}

impl RunHooks for UiHooks<'_> {
    fn on_item(&self, result: &RunItemResult, sent: Option<&PreparedRequest>, response: Option<&HttpResponseData>, secrets: &BTreeSet<String>) {
        if let Some(sent) = sent {
            let s = summary(sent, secrets);
            let err = result.error.as_ref().map(|e| HttpError::Request(e.clone()));
            record(self.state, &self.root, &result.path, &result.name, self.env.as_deref(), &s, response, err.as_ref(), &result.tests, now_ms());
        }
        let _ = self.channel.send(RunEvent::Item { result: Box::new(result.clone()) });
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[tauri::command]
pub async fn run_collection(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
    root: String,
    run: RunRequest,
    on_event: Channel<RunEvent>,
) -> CmdResult<RunSummary> {
    let p = Project::open(&root).map_err(err)?;
    let paths: Vec<String> = if !run.paths.is_empty() {
        run.paths.clone()
    } else {
        let tree = p.tree().map_err(err)?;
        let all = tree.request_paths();
        let prefix = devall_core::fsutil::normalize_rel(&run.folder);
        all.into_iter().filter(|x| prefix.is_empty() || x.starts_with(&format!("{prefix}/"))).collect()
    };
    let env_file = run.env.as_deref().filter(|e| !e.is_empty()).map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let data = match run.data.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(text) => runner::parse_data_file(text, text.starts_with('[')).map_err(err)?,
        None => Vec::new(),
    };
    let opts = RunOptions {
        iterations: run.iterations.unwrap_or(1).clamp(1, 10_000),
        data,
        bail: run.bail,
        delay_ms: run.delay_ms,
        fail_on_http_error: run.fail_on_http_error,
        fail_on_unresolved: true,
        update_snapshots: run.update_snapshots,
    };
    let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), run.env.as_deref(), p.root(), env_file.as_ref(), &state.providers);
    let tokens = KeychainTokens { store: state.secrets.as_ref(), project_id: p.id().to_string() };
    let open = browser(&app);
    let pool = state.pool(&root);
    let ctx = RunContext { project: &p, environment: env_file.as_ref(), secrets: &secrets, tokens: &tokens, browser: Some(&open), pool: &pool };
    let cancel = Arc::new(AtomicBool::new(false));
    state.runs.lock().unwrap().insert(run_id.clone(), cancel.clone());
    let total = paths.len() * if opts.data.is_empty() { opts.iterations } else { opts.data.len() };
    let _ = on_event.send(RunEvent::Started { total });
    let hooks = UiHooks { channel: on_event, cancel, state: &state, root: root.clone(), env: run.env.clone() };
    let mut runtime = if run.keep_variables { state.runtime_vars(&root) } else { Variables::new() };
    let summary = runner::run(&ctx, &paths, &mut runtime, &opts, &hooks).await;
    state.runs.lock().unwrap().remove(&run_id);
    crate::modules::logs::record_collection_run(&root, &run.folder, &summary);
    if run.keep_variables {
        state.runtime.lock().unwrap().insert(root, runtime);
    }
    Ok(summary)
}

#[tauri::command]
pub fn cancel_run(state: State<'_, AppState>, run_id: String) -> bool {
    match state.runs.lock().unwrap().get(&run_id) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

#[tauri::command]
pub fn run_to_junit(summary: RunSummaryInput, suite: String) -> String {
    runner::to_junit(&summary.into(), &suite)
}

/// The UI sends a summary back for JUnit export.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummaryInput {
    total: usize,
    passed: usize,
    failed: usize,
    duration_ms: f64,
    items: Vec<RunItemInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunItemInput {
    iteration: usize,
    path: String,
    name: String,
    method: String,
    url: String,
    status: Option<u16>,
    error: Option<String>,
    duration_ms: f64,
    #[serde(default)]
    tests: Vec<TestInput>,
    passed: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestInput {
    label: String,
    passed: bool,
    actual: Option<String>,
    message: Option<String>,
}

impl From<RunSummaryInput> for RunSummary {
    fn from(s: RunSummaryInput) -> Self {
        RunSummary {
            total: s.total,
            passed: s.passed,
            failed: s.failed,
            duration_ms: s.duration_ms,
            items: s
                .items
                .into_iter()
                .map(|i| RunItemResult {
                    iteration: i.iteration,
                    path: i.path,
                    name: i.name,
                    method: i.method,
                    url: i.url,
                    status: i.status,
                    error: i.error,
                    duration_ms: i.duration_ms,
                    size: 0,
                    tests: i.tests.into_iter().map(|t| AssertionResult { label: t.label, passed: t.passed, actual: t.actual, message: t.message }).collect(),
                    captures: vec![],
                    passed: i.passed,
                    scripts: Default::default(),
                    skipped: false,
                })
                .collect(),
            ..Default::default()
        }
    }
}
