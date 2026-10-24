//! Webhook inbox + bring-your-own tunnel commands — a thin layer over `crates/devall-webhooks`.
//!
//! # Integration (done by whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    ```toml
//!    devall-webhooks = { path = "../../../crates/devall-webhooks" }
//!    ```
//!    (`serde`, `serde_json`, `tokio` and `tauri` are already dependencies.)
//! 2. `src/modules/mod.rs`: `pub mod webhooks;`
//! 3. `src/lib.rs`, inside `.setup(|app| { … })` next to the other `app.manage(…)` calls:
//!    `app.manage(modules::webhooks::WebhookState::new(app.handle().clone()));`
//!    (the handle lets events that arrive without a running receiver — the hosted relay — resolve
//!    inbox secrets; `WebhookState::default()` also works but then only verifies after the
//!    Webhooks page started the receiver or set the environment), and in
//!    `tauri::generate_handler![…]`:
//!    ```text
//!    modules::webhooks::wh_providers,
//!    modules::webhooks::wh_inbox_schema,
//!    modules::webhooks::wh_inboxes_list,
//!    modules::webhooks::wh_inboxes_read,
//!    modules::webhooks::wh_inboxes_save,
//!    modules::webhooks::wh_inboxes_create,
//!    modules::webhooks::wh_inboxes_delete,
//!    modules::webhooks::wh_receiver_start,
//!    modules::webhooks::wh_receiver_stop,
//!    modules::webhooks::wh_receiver_status,
//!    modules::webhooks::wh_set_env,
//!    modules::webhooks::wh_events,
//!    modules::webhooks::wh_event,
//!    modules::webhooks::wh_subscribe,
//!    modules::webhooks::wh_unsubscribe,
//!    modules::webhooks::wh_verify,
//!    modules::webhooks::wh_replay,
//!    modules::webhooks::wh_copy_curl,
//!    modules::webhooks::wh_save_as_request,
//!    modules::webhooks::wh_save_fixture,
//!    modules::webhooks::wh_delete,
//!    modules::webhooks::wh_clear,
//!    modules::webhooks::wh_retention,
//!    modules::webhooks::wh_set_retention,
//!    modules::webhooks::tunnel_detect,
//!    modules::webhooks::tunnel_start,
//!    modules::webhooks::tunnel_stop,
//!    modules::webhooks::tunnel_status,
//!    modules::webhooks::tunnel_settings_get,
//!    modules::webhooks::tunnel_settings_save,
//!    modules::webhooks::tunnel_secret_set,
//!    modules::webhooks::tunnel_test,
//!    modules::webhooks::tunnel_command,
//!    ```
//!
//! Bring-your-own tunnel settings live in `<app data>/tunnels.json` (no secrets); the ngrok
//! authtoken / basic-auth password and the Cloudflare tunnel token are keychain entries in
//! `devall_webhooks::tunnels::config::secret_scope()` and only reach the child's environment.
//! 4. Optional, for a clean exit: in the `.run(…)` closure on `RunEvent::Exit`, call
//!    `tauri::async_runtime::block_on(app.state::<modules::webhooks::WebhookState>().shutdown())`
//!    so tunnel process trees are killed (children are also `kill_on_drop`).
//!
//! Event log: every received or relayed webhook is also written with
//! `crate::modules::logs::record_webhook` (hooked in `WebhookState::hub`); replays are not.
//!
//! Secrets: inbox `secret` templates (`{{secret.NAME}}`) are resolved like `send.rs::prepare_for`
//! — `SecretResolver::for_environment` over `AppState.secrets` + `AppState.providers` with the
//! given environment, and the project/environment/runtime variable layers of `resolver_for`.
//! No command returns a secret value: verification results carry a masked signature only.
//!
//! Hosted relay: events that arrive through the relay are passed to
//! `WebhookHub::ingest(IncomingEvent { source: EventSource::Relay, public_url, external_id, … })`
//! on the project's hub (`WebhookState::hub(root)`); they then appear, verify and forward exactly
//! like local ones and reach subscribers of `wh_subscribe`. From a `devall_relay::InboxEvent` `ev`:
//! ```ignore
//! IncomingEvent {
//!     method: ev.method.clone(),
//!     url: ev.url.clone(),
//!     path: ev.path.clone(),                 // "/stripe?x=1" — split into path + query by ingest
//!     headers: ev.headers.clone(),
//!     body: ev.body()?,
//!     received_at: chrono::DateTime::parse_from_rfc3339(&ev.received_at)?.timestamp_millis(),
//!     remote_addr: ev.source_ip.clone(),
//!     public_url: Some(ev.url.clone()),      // the URL the sender called (Twilio signs it)
//!     external_id: Some(ev.id.clone()),      // redeliveries are ignored
//!     source: EventSource::Relay,
//!     ..Default::default()
//! }
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use devall_core::{Project, SecretResolver, SecretStore};
use devall_webhooks::hub::resolve_with;
use devall_webhooks::tunnels::config::{self as tunnel_config, TunnelSettings};
use devall_webhooks::{
    EventPage, EventQuery, EventView, InboxEntry, InboxFile, Issue, ProviderInfo, ProviderKind, Receiver, ReceiverConfig, ReceiverInfo,
    ReplayEdits, ReplayOptions, Retention, StoreNotice, TemplateResolver, ToolInfo, Tunnel, TunnelKind, TunnelState, TunnelStatus,
    VerifyResult, WebhookHub,
};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::commands::resolver_for;
use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Map key for a project root: separators unified, trailing separators dropped, case-folded on
/// Windows — so `C:\p\` and `c:/p` share one receiver.
fn key(root: &str) -> String {
    let unified = root.trim().replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    let k = if trimmed.is_empty() { unified.as_str() } else { trimmed };
    if cfg!(windows) { k.to_lowercase() } else { k.to_string() }
}

/// Resolves `{{…}}` with the app's keychain, secret-manager cache and the project's variables for
/// one environment (the same layers as sending a request).
struct AppTemplates {
    app: AppHandle,
    root: String,
    env: Option<String>,
}

impl TemplateResolver for AppTemplates {
    fn resolve(&self, template: &str) -> Result<String, String> {
        let state = self.app.state::<AppState>();
        let p = Project::open(&self.root).map_err(err)?;
        let env = self.env.as_deref().filter(|e| !e.is_empty());
        let env_file = match env {
            Some(id) => Some(p.read_environment(id).map_err(err)?),
            None => None,
        };
        let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env, p.root(), env_file.as_ref(), &state.providers);
        let runtime = state.runtime_vars(&self.root);
        let mut r = resolver_for(&p, &runtime, env, env_file.as_ref(), &[], &secrets);
        let out = resolve_with(&mut r, template);
        let store_errors = secrets.errors.lock().unwrap().clone();
        match out {
            Err(e) if !store_errors.is_empty() => Err(format!("{e} — secret store: {}", store_errors.join("; "))),
            other => other,
        }
    }
}

fn templates(app: &AppHandle, root: &str, env: Option<String>) -> Arc<dyn TemplateResolver> {
    Arc::new(AppTemplates { app: app.clone(), root: root.to_string(), env: env.filter(|e| !e.is_empty()) })
}

struct ProjectHooks {
    hub: Arc<WebhookHub>,
    receiver: Option<Receiver>,
    env: Option<String>,
}

struct TunnelEntry {
    tunnel: Arc<Tunnel>,
    /// Project whose inboxes this tunnel exposes (its URL becomes their public URL).
    root: Option<String>,
}

#[derive(Default)]
pub struct WebhookState {
    /// Resolves secrets for hubs created outside a command (relay ingest).
    app: Option<AppHandle>,
    projects: Mutex<HashMap<String, ProjectHooks>>,
    tunnels: Mutex<HashMap<String, TunnelEntry>>,
    subscriptions: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

impl WebhookState {
    pub fn new(app: AppHandle) -> Self {
        Self { app: Some(app), ..Default::default() }
    }

    /// The project's hub (opening `<root>/.devall/webhooks.sqlite` on first use). The hosted
    /// relay feeds events into it with `WebhookHub::ingest`.
    pub fn hub(&self, root: &str) -> CmdResult<Arc<WebhookHub>> {
        let mut map = self.projects.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = map.get(&key(root)) {
            return Ok(p.hub.clone());
        }
        if !Path::new(root).is_dir() {
            return Err(format!("{root} is not a folder"));
        }
        // Project-wide secrets until the UI reports the active environment (`wh_set_env`).
        let t: Arc<dyn TemplateResolver> = match &self.app {
            Some(app) => templates(app, root, None),
            None => Arc::new(devall_webhooks::NoTemplates),
        };
        let hub = WebhookHub::open(root, t).map_err(err)?;
        // Every received/relayed webhook also goes to the project's event log (devall-logs).
        let log_root = root.to_string();
        hub.set_on_received(Some(Arc::new(move |e: &devall_webhooks::WebhookEvent| {
            let path = if e.query.is_empty() { e.path.clone() } else { format!("{}?{}", e.path, e.query) };
            let body = std::str::from_utf8(&e.body).ok();
            // Relayed events were answered by the hosted inbox, not by us.
            crate::modules::logs::record_webhook(&log_root, &e.method, &path, &e.headers, body, e.response_status.unwrap_or(200));
        })));
        map.insert(key(root), ProjectHooks { hub: hub.clone(), receiver: None, env: None });
        Ok(hub)
    }

    /// Stops every receiver and tunnel (call on app exit).
    pub async fn shutdown(&self) {
        let receivers: Vec<Receiver> = self.projects.lock().unwrap_or_else(|e| e.into_inner()).values_mut().filter_map(|p| p.receiver.take()).collect();
        for r in receivers {
            r.stop().await;
        }
        let tunnels: Vec<Arc<Tunnel>> = self.tunnels.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, t)| t.tunnel).collect();
        for t in tunnels {
            t.stop().await;
        }
        for (_, s) in self.subscriptions.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            s.abort();
        }
    }
}

// ---------- inboxes ----------

#[tauri::command]
pub fn wh_providers() -> Vec<ProviderInfo> {
    devall_webhooks::model::providers()
}

#[tauri::command]
pub fn wh_inbox_schema() -> serde_json::Value {
    devall_webhooks::model::inbox_schema()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    #[serde(flatten)]
    entry: InboxEntry,
    /// `http://127.0.0.1:<port><path>` while the receiver runs.
    local_url: Option<String>,
    /// LAN URL when the receiver listens on all interfaces.
    lan_url: Option<String>,
    /// The inbox's `publicUrl`, else the exposed tunnel URL + path.
    public_url: Option<String>,
    events: usize,
}

/// Every inbox with its URLs and event count, plus a pseudo-entry count for unrouted events
/// (`id: ""` in `wh_events`).
#[tauri::command]
pub async fn wh_inboxes_list(state: State<'_, WebhookState>, root: String) -> CmdResult<Vec<InboxItem>> {
    let hub = state.hub(&root)?;
    let entries = devall_webhooks::inbox::list(Path::new(&root)).map_err(err)?;
    let counts: HashMap<String, usize> = hub.store().counts().map_err(err)?.into_iter().collect();
    let (local, lan) = {
        let map = state.projects.lock().unwrap_or_else(|e| e.into_inner());
        let info = map.get(&key(&root)).and_then(|p| p.receiver.as_ref()).map(|r| r.info().clone());
        (info.as_ref().map(|i| i.local_url.clone()), info.and_then(|i| i.lan_url))
    };
    let public = hub.public_base();
    Ok(entries
        .into_iter()
        .map(|entry| {
            let path = entry.file.as_ref().map(|f| devall_webhooks::inbox::normalize_path(&f.path));
            let file_public = entry.file.as_ref().and_then(|f| f.public_url.clone()).filter(|u| !u.trim().is_empty());
            InboxItem {
                local_url: local.as_ref().zip(path.as_ref()).map(|(b, p)| format!("{b}{p}")),
                lan_url: lan.as_ref().zip(path.as_ref()).map(|(b, p)| format!("{b}{p}")),
                public_url: file_public.or_else(|| public.as_ref().zip(path.as_ref()).map(|(b, p)| format!("{b}{p}"))),
                events: counts.get(&entry.id).copied().unwrap_or(0),
                entry,
            }
        })
        .collect())
}

#[tauri::command]
pub fn wh_inboxes_read(root: String, id: String) -> CmdResult<InboxFile> {
    devall_webhooks::inbox::read(Path::new(&root), &id).map_err(err)
}

/// Saves (even with problems, so drafts persist) and returns the validation issues.
#[tauri::command]
pub fn wh_inboxes_save(root: String, id: String, inbox: InboxFile) -> CmdResult<Vec<Issue>> {
    devall_webhooks::inbox::save(Path::new(&root), &id, &inbox).map_err(err)
}

/// Creates `webhooks/<slug>.webhook.yaml` from the provider's template; returns its id.
#[tauri::command]
pub fn wh_inboxes_create(root: String, name: String, provider: Option<ProviderKind>) -> CmdResult<String> {
    devall_webhooks::inbox::create(Path::new(&root), &name, provider.unwrap_or_default()).map_err(err)
}

/// Moves the inbox file to the OS trash (its events stay in history).
#[tauri::command]
pub fn wh_inboxes_delete(root: String, id: String) -> CmdResult<()> {
    devall_webhooks::inbox::delete(Path::new(&root), &id).map_err(err)
}

// ---------- receiver ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiverStatus {
    running: bool,
    info: Option<ReceiverInfo>,
    /// URL of a tunnel exposing this receiver (this session).
    public_base: Option<String>,
    env: Option<String>,
    default_port: u16,
}

fn status_of(state: &WebhookState, root: &str) -> ReceiverStatus {
    let map = state.projects.lock().unwrap_or_else(|e| e.into_inner());
    let p = map.get(&key(root));
    let info = p.and_then(|p| p.receiver.as_ref()).filter(|r| r.is_running()).map(|r| r.info().clone());
    ReceiverStatus {
        running: info.is_some(),
        info,
        public_base: p.and_then(|p| p.hub.public_base()),
        env: p.and_then(|p| p.env.clone()),
        default_port: devall_webhooks::DEFAULT_PORT,
    }
}

/// Starts the project's receiver (default port 4590, loopback only) or updates the running one.
/// `env` is the environment used to resolve inbox secrets and forward URLs. A different port or
/// bind mode restarts it. `bind_all` exposes it to the LAN (show a warning).
#[tauri::command]
pub async fn wh_receiver_start(
    app: AppHandle,
    state: State<'_, WebhookState>,
    root: String,
    port: Option<u16>,
    bind_all: Option<bool>,
    env: Option<String>,
) -> CmdResult<ReceiverStatus> {
    let hub = state.hub(&root)?;
    let env = env.filter(|e| !e.is_empty());
    hub.set_templates(templates(&app, &root, env.clone()));
    let bind_all = bind_all.unwrap_or(false);
    let wanted = port.unwrap_or(devall_webhooks::DEFAULT_PORT);
    let replace = {
        let mut map = state.projects.lock().unwrap_or_else(|e| e.into_inner());
        let p = map.get_mut(&key(&root)).ok_or("project not open")?;
        p.env = env.clone();
        let keep = p.receiver.as_ref().is_some_and(|r| r.is_running() && (port.is_none() || r.port() == wanted) && r.info().bind_all == bind_all);
        if keep { None } else { Some(p.receiver.take()) }
    };
    match replace {
        None => return Ok(status_of(&state, &root)),
        Some(Some(old)) => old.stop().await,
        Some(None) => {}
    }
    let receiver = Receiver::start(hub, ReceiverConfig { port: wanted, bind_all }).await.map_err(err)?;
    if let Some(p) = state.projects.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&key(&root)) {
        p.receiver = Some(receiver);
    }
    Ok(status_of(&state, &root))
}

#[tauri::command]
pub async fn wh_receiver_stop(state: State<'_, WebhookState>, root: String) -> CmdResult<ReceiverStatus> {
    let r = state.projects.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&key(&root)).and_then(|p| p.receiver.take());
    if let Some(r) = r {
        r.stop().await;
    }
    Ok(status_of(&state, &root))
}

#[tauri::command]
pub fn wh_receiver_status(state: State<'_, WebhookState>, root: String) -> ReceiverStatus {
    status_of(&state, &root)
}

/// The active environment changed: background verification/forwarding resolve with it.
#[tauri::command]
pub fn wh_set_env(app: AppHandle, state: State<'_, WebhookState>, root: String, env: Option<String>) -> CmdResult<()> {
    let hub = state.hub(&root)?;
    let env = env.filter(|e| !e.is_empty());
    hub.set_templates(templates(&app, &root, env.clone()));
    if let Some(p) = state.projects.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&key(&root)) {
        p.env = env;
    }
    Ok(())
}

// ---------- events ----------

/// Newest first. `inbox`: an inbox id, `""` for events that matched no inbox, absent for all.
#[tauri::command]
pub async fn wh_events(state: State<'_, WebhookState>, root: String, inbox: Option<String>, page: Option<EventQuery>) -> CmdResult<EventPage> {
    let hub = state.hub(&root)?;
    let mut q = page.unwrap_or_default();
    if inbox.is_some() {
        q.inbox = inbox;
    }
    hub.store().list(&q).map_err(err)
}

#[tauri::command]
pub async fn wh_event(state: State<'_, WebhookState>, root: String, id: String) -> CmdResult<Option<EventView>> {
    let hub = state.hub(&root)?;
    Ok(hub.store().get(&id).map_err(err)?.map(|e| e.into_view()))
}

/// Streams store changes (added / updated / deleted / cleared / error) to `on_event`. Returns a
/// subscription id for [`wh_unsubscribe`]. After a lag the UI gets `cleared` and should reload.
#[tauri::command]
pub fn wh_subscribe(state: State<'_, WebhookState>, root: String, on_event: Channel<StoreNotice>) -> CmdResult<String> {
    let hub = state.hub(&root)?;
    let mut rx = hub.store().subscribe();
    let id = devall_core::new_id();
    let task = tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            let notice = match rx.recv().await {
                Ok(n) => n,
                Err(RecvError::Lagged(_)) => StoreNotice::Cleared { inbox: None },
                Err(RecvError::Closed) => break,
            };
            if on_event.send(notice).is_err() {
                break;
            }
        }
    });
    state.subscriptions.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), task);
    Ok(id)
}

#[tauri::command]
pub fn wh_unsubscribe(state: State<'_, WebhookState>, id: String) -> bool {
    match state.subscriptions.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
        Some(t) => {
            t.abort();
            true
        }
        None => false,
    }
}

/// Re-verifies an event with the secret resolved for `env` and stores the result.
#[tauri::command]
pub async fn wh_verify(app: AppHandle, state: State<'_, WebhookState>, root: String, id: String, env: Option<String>) -> CmdResult<VerifyResult> {
    let hub = state.hub(&root)?;
    hub.verify(&id, Some(templates(&app, &root, env))).await.map_err(err)
}

/// Sends a copy of event `id` to `target` (default: the inbox's forward URL) with optional
/// edits, re-signed with a fresh timestamp when `resign`. Returns the new (linked) event.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn wh_replay(
    app: AppHandle,
    state: State<'_, WebhookState>,
    root: String,
    id: String,
    target: Option<String>,
    edits: Option<ReplayEdits>,
    resign: bool,
    env: Option<String>,
) -> CmdResult<EventView> {
    let hub = state.hub(&root)?;
    let opts = ReplayOptions { target, edits, resign, timeout_ms: None };
    hub.replay(&id, opts, templates(&app, &root, env)).await.map(|e| e.into_view()).map_err(err)
}

/// The event as a cURL command for `target` (default: forward URL, else the original URL),
/// optionally re-signed (valid for the provider's tolerance window).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn wh_copy_curl(
    app: AppHandle,
    state: State<'_, WebhookState>,
    root: String,
    id: String,
    target: Option<String>,
    edits: Option<ReplayEdits>,
    resign: bool,
    env: Option<String>,
) -> CmdResult<String> {
    let hub = state.hub(&root)?;
    let opts = ReplayOptions { target, edits, resign, timeout_ms: None };
    hub.curl(&id, &opts, templates(&app, &root, env)).await.map_err(err)
}

/// Saves the event as `http/<folder>/<slug>.req.yaml` (signature headers re-signed by a
/// generated pre-request script); returns the path relative to `http/`.
#[tauri::command]
pub async fn wh_save_as_request(state: State<'_, WebhookState>, root: String, id: String, folder: Option<String>, target: Option<String>) -> CmdResult<String> {
    let hub = state.hub(&root)?;
    let event = hub.store().get(&id).map_err(err)?.ok_or_else(|| format!("event {id} not found"))?;
    let p = Project::open(&root).map_err(err)?;
    let router = hub.router();
    let inbox = event.inbox.as_deref().and_then(|i| router.get(i));
    let folder = folder.unwrap_or_else(|| "webhooks".into());
    devall_webhooks::convert::save_as_request(&p, &folder, &event, inbox, target.as_deref().filter(|t| !t.trim().is_empty())).map_err(err)
}

/// Writes `webhooks/fixtures/<slug>.json`; returns the path relative to the project root.
#[tauri::command]
pub async fn wh_save_fixture(state: State<'_, WebhookState>, root: String, id: String) -> CmdResult<String> {
    let hub = state.hub(&root)?;
    let event = hub.store().get(&id).map_err(err)?.ok_or_else(|| format!("event {id} not found"))?;
    let router = hub.router();
    let inbox = event.inbox.as_deref().and_then(|i| router.get(i));
    devall_webhooks::convert::save_fixture(Path::new(&root), &event, inbox).map_err(err)
}

#[tauri::command]
pub async fn wh_delete(state: State<'_, WebhookState>, root: String, id: String) -> CmdResult<bool> {
    state.hub(&root)?.store().delete(&id).map_err(err)
}

/// Clears one inbox's events (`""` = unrouted) or all of them.
#[tauri::command]
pub async fn wh_clear(state: State<'_, WebhookState>, root: String, inbox: Option<String>) -> CmdResult<usize> {
    state.hub(&root)?.store().clear(inbox.as_deref()).map_err(err)
}

#[tauri::command]
pub async fn wh_retention(state: State<'_, WebhookState>, root: String) -> CmdResult<Retention> {
    state.hub(&root)?.store().retention().map_err(err)
}

/// Saves the retention policy (absent fields = unlimited) and prunes; returns events removed.
#[tauri::command]
pub async fn wh_set_retention(state: State<'_, WebhookState>, root: String, retention: Retention) -> CmdResult<usize> {
    state.hub(&root)?.store().set_retention(retention).map_err(err)
}

// ---------- tunnels ----------

fn has_tunnel_secret(store: &dyn SecretStore, name: &str) -> bool {
    matches!(store.get(&tunnel_config::secret_scope(), name), Ok(Some(v)) if !v.trim().is_empty())
}

/// Installed tunnel tools with version and readiness (runs `--version`/`status`; ~1 s). Uses the
/// binaries configured in Tunnel settings and counts a keychain ngrok authtoken as ready.
#[tauri::command]
pub async fn tunnel_detect(app: State<'_, AppState>) -> CmdResult<Vec<ToolInfo>> {
    let settings = TunnelSettings::load(&app.data_dir);
    let token = has_tunnel_secret(app.secrets.as_ref(), tunnel_config::NGROK_AUTHTOKEN);
    tauri::async_runtime::spawn_blocking(move || TunnelKind::ALL.iter().map(|k| devall_webhooks::tunnels::detect_with(*k, settings.program(*k), token)).collect())
        .await
        .map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelSettingsView {
    pub settings: TunnelSettings,
    /// Which keychain credentials are stored (never their values).
    pub secrets: HashMap<&'static str, bool>,
    pub keychain_available: bool,
}

/// Bring-your-own tunnel settings (`<app data>/tunnels.json`) and which credentials are stored.
#[tauri::command]
pub fn tunnel_settings_get(app: State<'_, AppState>) -> TunnelSettingsView {
    let secrets = tunnel_config::SECRET_NAMES.iter().map(|n| (*n, has_tunnel_secret(app.secrets.as_ref(), n))).collect();
    TunnelSettingsView { settings: TunnelSettings::load(&app.data_dir), secrets, keychain_available: app.keychain_available }
}

#[tauri::command]
pub fn tunnel_settings_save(app: State<'_, AppState>, settings: TunnelSettings) -> CmdResult<()> {
    settings.validate().map_err(err)?;
    settings.save(&app.data_dir).map_err(err)
}

/// Stores (or with an empty value, removes) a tunnel credential in the OS keychain:
/// `ngrok-authtoken`, `ngrok-basic-auth-password`, `cloudflared-token`.
#[tauri::command]
pub fn tunnel_secret_set(app: State<'_, AppState>, name: String, value: Option<String>) -> CmdResult<()> {
    if !tunnel_config::SECRET_NAMES.contains(&name.as_str()) {
        return Err(format!("unknown tunnel credential `{name}`"));
    }
    let scope = tunnel_config::secret_scope();
    match value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        Some(v) => app.secrets.set(&scope, &name, &v).map_err(err),
        None => app.secrets.delete(&scope, &name).map_err(err),
    }
}

/// Starts `kind` briefly with the saved settings to check the setup (authtoken, domain, Funnel).
#[tauri::command]
pub async fn tunnel_test(app: State<'_, AppState>, kind: TunnelKind) -> CmdResult<devall_webhooks::tunnels::TunnelTest> {
    let settings = TunnelSettings::load(&app.data_dir);
    Ok(devall_webhooks::tunnels::test_tool(kind, &settings, app.secrets.as_ref(), std::time::Duration::from_secs(25)).await)
}

/// The command line a tunnel would run for `port` (credentials masked), for display.
#[tauri::command]
pub async fn tunnel_command(app: State<'_, AppState>, kind: TunnelKind, port: u16) -> CmdResult<String> {
    let settings = TunnelSettings::load(&app.data_dir);
    let launch = devall_webhooks::tunnels::configured_launch(kind, port, &settings, app.secrets.as_ref()).await.map_err(err)?;
    Ok(launch.display(kind))
}

/// Starts `kind` exposing `127.0.0.1:port` with the user's tunnel settings and keychain
/// credentials, streaming its status to `on_status`. With `root`, the tunnel's URL becomes the
/// public URL of that project's inboxes while it runs ("Expose"). `program` overrides the binary.
#[tauri::command]
pub async fn tunnel_start(
    state: State<'_, WebhookState>,
    app: State<'_, AppState>,
    kind: TunnelKind,
    port: u16,
    root: Option<String>,
    program: Option<String>,
    on_status: Channel<TunnelStatus>,
) -> CmdResult<TunnelStatus> {
    let hub = match root.as_deref() {
        Some(r) => Some(state.hub(r)?),
        None => None,
    };
    let mut settings = TunnelSettings::load(&app.data_dir);
    if let Some(p) = program.filter(|p| !p.trim().is_empty()) {
        match kind {
            TunnelKind::Ngrok => settings.ngrok.program = Some(p),
            TunnelKind::Cloudflared => settings.cloudflared.program = Some(p),
            TunnelKind::Tailscale => settings.tailscale.program = Some(p),
        }
    }
    let tunnel = devall_webhooks::tunnels::start_configured(kind, port, &settings, app.secrets.as_ref()).await.map_err(err)?;
    let id = tunnel.id().to_string();
    state.tunnels.lock().unwrap_or_else(|e| e.into_inner()).insert(id, TunnelEntry { tunnel: tunnel.clone(), root });
    let mut rx = tunnel.subscribe();
    let first = rx.borrow_and_update().clone();
    tauri::async_runtime::spawn(async move {
        let mut applied: Option<String> = None;
        loop {
            let s = rx.borrow_and_update().clone();
            if let Some(h) = &hub {
                let live = matches!(s.state, TunnelState::Online | TunnelState::Starting);
                match (&s.public_url, live) {
                    (Some(u), true) if applied.as_ref() != Some(u) => {
                        h.set_public_base(Some(u.clone()));
                        applied = Some(u.clone());
                    }
                    (_, false) if applied.is_some() => {
                        if h.public_base() == applied {
                            h.set_public_base(None);
                        }
                        applied = None;
                    }
                    _ => {}
                }
            }
            let _ = on_status.send(s);
            // Ends after the final status once the tunnel's supervisor has finished.
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
    Ok(first)
}

#[tauri::command]
pub async fn tunnel_stop(state: State<'_, WebhookState>, id: String) -> CmdResult<()> {
    let entry = state.tunnels.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    if let Some(e) = entry {
        e.tunnel.stop().await;
        if let Some(root) = e.root
            && let Ok(hub) = state.hub(&root)
            && hub.public_base() == e.tunnel.status().public_url
        {
            hub.set_public_base(None);
        }
    }
    Ok(())
}

/// Every tunnel started this session (including failed/stopped ones until they are stopped).
#[tauri::command]
pub fn tunnel_status(state: State<'_, WebhookState>) -> Vec<TunnelStatus> {
    let map = state.tunnels.lock().unwrap_or_else(|e| e.into_inner());
    let mut list: Vec<TunnelStatus> = map.values().map(|e| e.tunnel.status()).collect();
    list.sort_by_key(|s| s.started_at);
    list
}
