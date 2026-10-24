//! Traffic inspector commands — a thin layer over `crates/devall-proxy`.
//!
//! # Integration (done by whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    ```toml
//!    devall-proxy = { path = "../../../crates/devall-proxy" }
//!    ```
//! 2. `src/modules/mod.rs`: `pub mod proxy;`
//! 3. `src/lib.rs`, inside `.setup(|app| { … })` after `data_dir` is computed:
//!    ```ignore
//!    app.manage(modules::proxy::ProxyState::new(data_dir.clone()));
//!    ```
//!    and in `tauri::generate_handler![ … ]`:
//!    ```text
//!    modules::proxy::proxy_start,
//!    modules::proxy::proxy_stop,
//!    modules::proxy::proxy_status,
//!    modules::proxy::proxy_settings_get,
//!    modules::proxy::proxy_settings_save,
//!    modules::proxy::proxy_ca_info,
//!    modules::proxy::proxy_ca_install,
//!    modules::proxy::proxy_ca_uninstall,
//!    modules::proxy::proxy_ca_regenerate,
//!    modules::proxy::proxy_subscribe,
//!    modules::proxy::proxy_unsubscribe,
//!    modules::proxy::proxy_list,
//!    modules::proxy::proxy_get,
//!    modules::proxy::proxy_clear,
//!    modules::proxy::proxy_remove,
//!    modules::proxy::proxy_paused,
//!    modules::proxy::proxy_rules_get,
//!    modules::proxy::proxy_rules_save,
//!    modules::proxy::proxy_rules_export,
//!    modules::proxy::proxy_rules_import,
//!    modules::proxy::proxy_breakpoint_resume,
//!    modules::proxy::proxy_breakpoint_abort,
//!    modules::proxy::proxy_replay,
//!    modules::proxy::proxy_copy_curl,
//!    modules::proxy::proxy_export_har,
//!    modules::proxy::proxy_to_stubs,
//!    modules::proxy::proxy_to_requests,
//!    modules::proxy::proxy_to_openapi,
//!    modules::proxy::proxy_save_openapi,
//!    modules::proxy::proxy_to_load,
//!    modules::proxy::proxy_sessions,
//!    modules::proxy::proxy_save_session,
//!    modules::proxy::proxy_load_session,
//!    modules::proxy::proxy_delete_session,
//!    modules::proxy::proxy_import_har,
//!    modules::proxy::proxy_launch_browser,
//!    ```
//!
//! State lives in `<app data>/proxy/`: `ca/` (root CA; private key never in a project),
//! `settings.json`, `rules.json`. The engine (and the CA) are created on first use. Installing
//! the CA (`proxy_ca_install`) and launching a browser are explicit user actions — the UI shows
//! the exact command and confirms first. Nothing changes system proxy settings.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use devall_proxy::browser::{self, BrowserInfo, BrowserKind, EnvSnippet, Launched};
use devall_proxy::export::har::{HarOptions, to_har_string};
use devall_proxy::export::load::LoadExport;
use devall_proxy::export::openapi::{OpenApiOptions, infer_yaml};
use devall_proxy::export::wiremock::{StubOptions, StubsResult, write_stubs};
use devall_proxy::session::SessionEntry;
use devall_proxy::trust::{self, CommandOutput, TrustInfo};
use devall_proxy::{
    BreakpointEdit, CaInfo, CertAuthority, ExchangeDetail, ExchangeFilter, ExchangeId, ExchangeSummary, PausedExchange, ProxyConfig, ProxyEngine, ProxyEvent, ProxyStatus, Rule,
};
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;
use tokio::sync::broadcast::error::RecvError;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub struct ProxyState {
    dir: PathBuf,
    engine: Mutex<Option<ProxyEngine>>,
    subscription: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl ProxyState {
    /// `app_data_dir` is the app's data directory; the module keeps its files in `proxy/` below it.
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self { dir: app_data_dir.join("proxy"), engine: Mutex::new(None), subscription: Mutex::new(None) }
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    fn rules_path(&self) -> PathBuf {
        self.dir.join("rules.json")
    }

    fn ca_dir(&self) -> PathBuf {
        self.dir.join("ca")
    }

    /// The engine, created (with the CA, settings and rules) on first use.
    fn engine(&self) -> CmdResult<ProxyEngine> {
        let mut guard = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        let ca = CertAuthority::load_or_create(self.ca_dir()).map_err(err)?;
        let engine = ProxyEngine::new(ca, ProxyConfig::load(&self.settings_path())).map_err(err)?;
        // Invalid saved rules (edited by hand) are skipped rather than blocking the proxy.
        if let Ok(rules) = devall_proxy::rules::load_rules(&self.rules_path()) {
            let _ = engine.set_rules(rules);
        }
        *guard = Some(engine.clone());
        Ok(engine)
    }
}

fn check_root(root: &str) -> CmdResult<&Path> {
    let p = Path::new(root);
    if p.join("devall.yaml").is_file() { Ok(p) } else { Err(format!("{root} is not a /dev/all project")) }
}

// ---------- lifecycle & settings ----------

#[tauri::command]
pub async fn proxy_start(state: State<'_, ProxyState>, port: Option<u16>) -> CmdResult<ProxyStatus> {
    let engine = state.engine()?;
    if let Some(p) = port.filter(|p| *p != 0) {
        let mut cfg = engine.config();
        if cfg.port != p {
            cfg.port = p;
            engine.set_config(cfg.clone()).map_err(err)?;
            cfg.save(&state.settings_path()).map_err(err)?;
        }
    }
    engine.start(port).await.map_err(err)
}

#[tauri::command]
pub fn proxy_stop(state: State<'_, ProxyState>) -> CmdResult<ProxyStatus> {
    let engine = state.engine()?;
    engine.stop();
    Ok(engine.status())
}

#[tauri::command]
pub fn proxy_status(state: State<'_, ProxyState>) -> CmdResult<ProxyStatus> {
    Ok(state.engine()?.status())
}

#[tauri::command]
pub fn proxy_settings_get(state: State<'_, ProxyState>) -> CmdResult<ProxyConfig> {
    Ok(state.engine()?.config())
}

#[tauri::command]
pub fn proxy_settings_save(state: State<'_, ProxyState>, settings: ProxyConfig) -> CmdResult<ProxyConfig> {
    let engine = state.engine()?;
    engine.set_config(settings).map_err(err)?;
    let cfg = engine.config();
    cfg.save(&state.settings_path()).map_err(err)?;
    Ok(cfg)
}

// ---------- CA ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaView {
    ca: CaInfo,
    trust: TrustInfo,
    env: Vec<EnvSnippet>,
    browsers: Vec<BrowserInfo>,
}

/// CA details, the exact install/uninstall commands for this OS, terminal snippets and the
/// browsers that can be launched. `check` also asks the OS whether the CA is trusted (runs a
/// read-only `certutil`/`security` query).
#[tauri::command]
pub async fn proxy_ca_info(state: State<'_, ProxyState>, check: Option<bool>) -> CmdResult<CaView> {
    let engine = state.engine()?;
    let check = check.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let port = engine.status().port.unwrap_or_else(|| engine.config().port);
        let ca = engine.ca();
        CaView { ca: ca.info(), trust: trust::trust_info(ca, port, check), env: browser::env_snippets(port, &ca.cert_path()), browsers: browser::available() }
    })
    .await
    .map_err(err)
}

/// Runs the OS command that trusts the CA for the current user. Call only after the user
/// confirmed the command shown by `proxy_ca_info`; the OS shows its own prompt as well.
#[tauri::command]
pub async fn proxy_ca_install(state: State<'_, ProxyState>) -> CmdResult<CommandOutput> {
    let engine = state.engine()?;
    tauri::async_runtime::spawn_blocking(move || {
        let cmd = trust::install_command(trust::Os::current(), &engine.ca().cert_path());
        if !cmd.runnable {
            return Err(format!("Run this in a terminal: {}", cmd.display));
        }
        Ok(trust::run(&cmd.program, &cmd.args))
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn proxy_ca_uninstall(state: State<'_, ProxyState>) -> CmdResult<CommandOutput> {
    let engine = state.engine()?;
    tauri::async_runtime::spawn_blocking(move || {
        let ca = engine.ca();
        let cmd = trust::uninstall_command(trust::Os::current(), &ca.cert_path(), &ca.sha1_hex());
        if !cmd.runnable {
            return Err(format!("Run this in a terminal: {}", cmd.display));
        }
        Ok(trust::run(&cmd.program, &cmd.args))
    })
    .await
    .map_err(err)?
}

/// Replaces the CA with a new one. Stops the proxy and clears the capture; anything that trusted
/// the old CA must trust the new one (uninstall the old one first).
#[tauri::command]
pub fn proxy_ca_regenerate(state: State<'_, ProxyState>) -> CmdResult<CaInfo> {
    if let Some(old) = state.engine.lock().unwrap_or_else(|e| e.into_inner()).take() {
        old.stop();
    }
    if let Some(sub) = state.subscription.lock().unwrap_or_else(|e| e.into_inner()).take() {
        sub.abort();
    }
    CertAuthority::create(state.ca_dir(), &devall_proxy::ca::default_ca_name()).map_err(err)?;
    Ok(state.engine()?.ca().info())
}

// ---------- live events ----------

fn push(batch: &mut Vec<ProxyEvent>, ev: ProxyEvent) {
    if let ProxyEvent::Exchange { exchange } = &ev
        && let Some(slot) = batch.iter_mut().find(|e| matches!(e, ProxyEvent::Exchange { exchange: x } if x.id == exchange.id))
    {
        *slot = ev;
        return;
    }
    batch.push(ev);
}

/// Streams events in batches (at most every 50 ms; repeated updates of one exchange coalesce).
/// A new subscription replaces the previous one. On overflow a `resync` event asks the UI to
/// reload the list.
#[tauri::command]
pub fn proxy_subscribe(state: State<'_, ProxyState>, on_event: Channel<Vec<ProxyEvent>>) -> CmdResult<()> {
    let engine = state.engine()?;
    let mut rx = engine.subscribe();
    let handle = tauri::async_runtime::spawn(async move {
        let mut batch: Vec<ProxyEvent> = Vec::new();
        'outer: loop {
            match rx.recv().await {
                Ok(ev) => push(&mut batch, ev),
                Err(RecvError::Lagged(_)) => batch = vec![ProxyEvent::Resync],
                Err(RecvError::Closed) => break,
            }
            let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
            loop {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Ok(ev)) => push(&mut batch, ev),
                    Ok(Err(RecvError::Lagged(_))) => batch = vec![ProxyEvent::Resync],
                    Ok(Err(RecvError::Closed)) => break 'outer,
                    Err(_) => break,
                }
            }
            if on_event.send(std::mem::take(&mut batch)).is_err() {
                break;
            }
        }
    });
    if let Some(old) = state.subscription.lock().unwrap_or_else(|e| e.into_inner()).replace(handle) {
        old.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn proxy_unsubscribe(state: State<'_, ProxyState>) {
    if let Some(old) = state.subscription.lock().unwrap_or_else(|e| e.into_inner()).take() {
        old.abort();
    }
}

// ---------- capture ----------

#[tauri::command]
pub fn proxy_list(state: State<'_, ProxyState>, filter: Option<ExchangeFilter>) -> CmdResult<Vec<ExchangeSummary>> {
    Ok(state.engine()?.list(&filter.unwrap_or_default()))
}

#[tauri::command]
pub fn proxy_get(state: State<'_, ProxyState>, id: ExchangeId) -> CmdResult<Option<ExchangeDetail>> {
    Ok(state.engine()?.get(id))
}

#[tauri::command]
pub fn proxy_clear(state: State<'_, ProxyState>) -> CmdResult<()> {
    state.engine()?.clear();
    Ok(())
}

#[tauri::command]
pub fn proxy_remove(state: State<'_, ProxyState>, ids: Vec<ExchangeId>) -> CmdResult<usize> {
    Ok(state.engine()?.store().remove(&ids))
}

#[tauri::command]
pub fn proxy_paused(state: State<'_, ProxyState>) -> CmdResult<Vec<PausedExchange>> {
    Ok(state.engine()?.paused())
}

// ---------- rules ----------

#[tauri::command]
pub fn proxy_rules_get(state: State<'_, ProxyState>) -> CmdResult<Vec<Rule>> {
    Ok(state.engine()?.rules())
}

/// Validates, installs and saves the rules; returns them with ids assigned.
#[tauri::command]
pub fn proxy_rules_save(state: State<'_, ProxyState>, rules: Vec<Rule>) -> CmdResult<Vec<Rule>> {
    let engine = state.engine()?;
    let rules = engine.set_rules(rules).map_err(err)?;
    devall_proxy::rules::save_rules(&state.rules_path(), &rules).map_err(err)?;
    Ok(rules)
}

/// The rules as shareable JSON (`{"version":1,"rules":[…]}`).
#[tauri::command]
pub fn proxy_rules_export(state: State<'_, ProxyState>) -> CmdResult<String> {
    devall_proxy::rules::rules_json(&state.engine()?.rules()).map_err(err)
}

/// Imports exported rules, appended to (or replacing) the current ones.
#[tauri::command]
pub fn proxy_rules_import(state: State<'_, ProxyState>, text: String, replace: Option<bool>) -> CmdResult<Vec<Rule>> {
    let engine = state.engine()?;
    let mut incoming = devall_proxy::rules::parse_rules(&text).map_err(err)?;
    let mut rules = if replace.unwrap_or(false) { Vec::new() } else { engine.rules() };
    for r in &mut incoming {
        if rules.iter().any(|x| x.id == r.id) {
            r.id = String::new();
        }
    }
    rules.extend(incoming);
    let rules = engine.set_rules(rules).map_err(err)?;
    devall_proxy::rules::save_rules(&state.rules_path(), &rules).map_err(err)?;
    Ok(rules)
}

// ---------- breakpoints & replay ----------

#[tauri::command]
pub fn proxy_breakpoint_resume(state: State<'_, ProxyState>, id: ExchangeId, edited: Option<BreakpointEdit>) -> CmdResult<()> {
    state.engine()?.resume(id, edited).map_err(err)
}

#[tauri::command]
pub fn proxy_breakpoint_abort(state: State<'_, ProxyState>, id: ExchangeId) -> CmdResult<()> {
    state.engine()?.abort(id).map_err(err)
}

/// Sends a captured request again (rules apply, breakpoints don't); returns the new exchange id.
#[tauri::command]
pub async fn proxy_replay(state: State<'_, ProxyState>, id: ExchangeId, edits: Option<BreakpointEdit>) -> CmdResult<ExchangeId> {
    let engine = state.engine()?;
    engine.replay(id, edits).await.map_err(err)
}

#[tauri::command]
pub fn proxy_copy_curl(state: State<'_, ProxyState>, id: ExchangeId, include_sensitive: Option<bool>) -> CmdResult<String> {
    let engine = state.engine()?;
    let ex = engine.store().get(id).ok_or_else(|| format!("exchange {id} is no longer captured"))?;
    Ok(devall_proxy::export::curl::to_curl(&ex, include_sensitive.unwrap_or(false)))
}

// ---------- exports ----------

/// Captured exchanges (all, or `ids`) for other modules (OpenAPI drift checks).
pub(crate) fn captured(state: &ProxyState, ids: &[ExchangeId]) -> CmdResult<Vec<devall_proxy::Exchange>> {
    let engine = state.engine()?;
    Ok(if ids.is_empty() { engine.store().all() } else { engine.exchanges(ids) })
}

fn selected(engine: &ProxyEngine, ids: &[ExchangeId]) -> CmdResult<Vec<devall_proxy::Exchange>> {
    let ex = if ids.is_empty() { engine.store().all() } else { engine.exchanges(ids) };
    if ex.is_empty() { Err("nothing selected (the exchanges may have been cleared)".into()) } else { Ok(ex) }
}

/// HAR 1.2 for the exchanges (all when `ids` is empty). With `path`, writes the file there and
/// returns the path; otherwise returns the HAR text.
#[tauri::command]
pub async fn proxy_export_har(state: State<'_, ProxyState>, ids: Vec<ExchangeId>, include_sensitive: bool, path: Option<String>) -> CmdResult<String> {
    let ex = selected(&state.engine()?, &ids)?;
    tauri::async_runtime::spawn_blocking(move || {
        let text = to_har_string(&ex, &HarOptions { include_sensitive });
        match path.filter(|p| !p.trim().is_empty()) {
            Some(p) => {
                devall_core::fsutil::atomic_write(Path::new(&p), text.as_bytes()).map_err(err)?;
                Ok(p)
            }
            None => Ok(text),
        }
    })
    .await
    .map_err(err)?
}

/// Writes WireMock stub mappings to `<root>/mocks/mappings/`.
#[tauri::command]
pub async fn proxy_to_stubs(state: State<'_, ProxyState>, root: String, ids: Vec<ExchangeId>, options: Option<StubOptions>) -> CmdResult<StubsResult> {
    check_root(&root)?;
    let ex = selected(&state.engine()?, &ids)?;
    tauri::async_runtime::spawn_blocking(move || write_stubs(Path::new(&root), &ex, &options.unwrap_or_default()).map_err(err)).await.map_err(err)?
}

/// Saves the requests under `http/<folder>/` (default `captured`); returns their paths.
#[tauri::command]
pub async fn proxy_to_requests(state: State<'_, ProxyState>, root: String, ids: Vec<ExchangeId>, folder: Option<String>) -> CmdResult<Vec<String>> {
    check_root(&root)?;
    let ex = selected(&state.engine()?, &ids)?;
    tauri::async_runtime::spawn_blocking(move || devall_proxy::export::requests::save_requests(Path::new(&root), &ex, folder.as_deref().unwrap_or("captured")).map_err(err))
        .await
        .map_err(err)?
}

/// An OpenAPI 3.1 draft (YAML) inferred from the exchanges. Nothing is written; see
/// `proxy_save_openapi`.
#[tauri::command]
pub async fn proxy_to_openapi(state: State<'_, ProxyState>, root: String, ids: Vec<ExchangeId>, options: Option<OpenApiOptions>) -> CmdResult<String> {
    let _ = root;
    let ex = selected(&state.engine()?, &ids)?;
    tauri::async_runtime::spawn_blocking(move || infer_yaml(&ex, &options.unwrap_or_default())).await.map_err(err)
}

/// Writes (possibly edited) spec text to a project-relative path, e.g. `openapi/api.yaml`.
/// Returns the normalized relative path.
#[tauri::command]
pub fn proxy_save_openapi(root: String, path: String, yaml: String) -> CmdResult<String> {
    let root = check_root(&root)?;
    let rel = devall_core::fsutil::normalize_rel(&path);
    if rel.is_empty() || !(rel.ends_with(".yaml") || rel.ends_with(".yml") || rel.ends_with(".json")) {
        return Err("choose a .yaml, .yml or .json file name".into());
    }
    let full = devall_core::fsutil::safe_join(root, &rel).map_err(err)?;
    devall_core::fsutil::atomic_write(&full, yaml.as_bytes()).map_err(err)?;
    Ok(rel)
}

/// Saves the requests and a `load/<slug>.load.yaml` flow replaying them.
#[tauri::command]
pub async fn proxy_to_load(state: State<'_, ProxyState>, root: String, ids: Vec<ExchangeId>, name: String) -> CmdResult<LoadExport> {
    check_root(&root)?;
    let ex = selected(&state.engine()?, &ids)?;
    tauri::async_runtime::spawn_blocking(move || devall_proxy::export::load::save_load_test(Path::new(&root), &ex, &name).map_err(err)).await.map_err(err)?
}

// ---------- saved captures (.devall/captures) ----------

#[tauri::command]
pub fn proxy_sessions(root: String) -> CmdResult<Vec<SessionEntry>> {
    devall_proxy::session::list_sessions(check_root(&root)?).map_err(err)
}

#[tauri::command]
pub async fn proxy_save_session(state: State<'_, ProxyState>, root: String, ids: Vec<ExchangeId>, name: Option<String>, include_sensitive: Option<bool>) -> CmdResult<String> {
    check_root(&root)?;
    let ex = selected(&state.engine()?, &ids)?;
    tauri::async_runtime::spawn_blocking(move || devall_proxy::session::save_session(Path::new(&root), name.as_deref(), &ex, include_sensitive.unwrap_or(false)).map_err(err))
        .await
        .map_err(err)?
}

/// Loads a saved capture into the inspector; returns the new exchange ids.
#[tauri::command]
pub fn proxy_load_session(state: State<'_, ProxyState>, root: String, file: String) -> CmdResult<Vec<ExchangeId>> {
    let ex = devall_proxy::session::read_session(check_root(&root)?, &file).map_err(err)?;
    Ok(state.engine()?.import(ex))
}

#[tauri::command]
pub fn proxy_delete_session(root: String, file: String) -> CmdResult<()> {
    devall_proxy::session::delete_session(check_root(&root)?, &file).map_err(err)
}

/// Imports a HAR document (e.g. exported from browser dev tools); returns the new exchange ids.
#[tauri::command]
pub fn proxy_import_har(state: State<'_, ProxyState>, text: String) -> CmdResult<Vec<ExchangeId>> {
    let ex = devall_proxy::export::har::from_har(&text).map_err(err)?;
    Ok(state.engine()?.import(ex))
}

// ---------- browser ----------

/// Starts Chrome / Edge / Brave / Chromium with an isolated temporary profile routed through the
/// proxy, accepting the CA by its key pin (no trust-store change). Starts the proxy if needed.
#[tauri::command]
pub async fn proxy_launch_browser(state: State<'_, ProxyState>, kind: BrowserKind, url: Option<String>) -> CmdResult<Launched> {
    let engine = state.engine()?;
    let status = engine.start(None).await.map_err(err)?;
    let port = status.port.ok_or("the proxy is not running")?;
    let spki = engine.ca().spki_sha256();
    tauri::async_runtime::spawn_blocking(move || browser::launch(kind, port, &spki, url.as_deref()).map_err(err)).await.map_err(err)?
}
