//! Hosted relay (Group 2) commands — a thin layer over `crates/devall-relay`: account sign-in,
//! reverse tunnels and the hosted webhook inbox, against the hosted relay or a self-hosted one.
//!
//! # Integration (done by whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    ```toml
//!    devall-relay = { path = "../../../crates/devall-relay" }
//!    futures-util = { version = "0.3", default-features = false, features = ["std"] }
//!    ```
//!    (`devall-core`, `serde`, `serde_json`, `tauri`, `tauri-plugin-opener` and `tokio` are already there.)
//! 2. `src/modules/mod.rs`: `pub mod relay;`
//! 3. `src/lib.rs`, inside `.setup(|app| { … })` after `config_dir` is known:
//!    ```ignore
//!    app.manage(modules::relay::RelayState::new(config_dir.clone()));
//!    ```
//!    and in `tauri::generate_handler![…]`:
//!    ```text
//!    modules::relay::relay_status,
//!    modules::relay::relay_sign_in,
//!    modules::relay::relay_sign_out,
//!    modules::relay::relay_account,
//!    modules::relay::relay_tunnel_start,
//!    modules::relay::relay_tunnel_stop,
//!    modules::relay::relay_inbox_connect,
//!    modules::relay::relay_inbox_disconnect,
//!    modules::relay::relay_settings_get,
//!    modules::relay::relay_settings_set,
//!    modules::relay::relay_upgrade_url,
//!    ```
//! 4. Hosted inbox → webhooks module (`modules::webhooks`, `devall-webhooks`). Until this is wired,
//!    events are appended to `<project data>/relay-inbox/<date>.jsonl` (on this device) and
//!    acknowledged after the write. To feed them into the project's webhook hub instead, replace
//!    the `app.manage(…RelayState::new…)` line from step 3 with (needs `devall-webhooks` in
//!    Cargo.toml, which the webhooks module adds anyway):
//!    ```ignore
//!    let relay = modules::relay::RelayState::new(config_dir.clone());
//!    let handle = app.handle().clone();
//!    relay.set_inbox_sink(std::sync::Arc::new(move |root: String, ev: devall_relay::InboxEvent| {
//!        let handle = handle.clone();
//!        Box::pin(async move {
//!            use tauri::Manager as _;
//!            let hub = handle.state::<modules::webhooks::WebhookState>().hub(&root)?;
//!            let incoming = devall_webhooks::IncomingEvent {
//!                method: ev.method.clone(),
//!                url: ev.url.clone(),
//!                path: ev.path.clone(),            // "/stripe?x=1" — ingest splits path + query
//!                headers: ev.headers.clone(),
//!                body: ev.body().map_err(|e| e.to_string())?,
//!                received_at: ev.received_at_ms().unwrap_or(0),
//!                remote_addr: ev.source_ip.clone(),
//!                public_url: Some(ev.url.clone()), // the URL the sender called (Twilio signs it)
//!                external_id: Some(ev.id.clone()), // redeliveries become no-ops
//!                source: devall_webhooks::EventSource::Relay,
//!                ..Default::default()
//!            };
//!            hub.ingest(incoming).await.map(|_| ()).map_err(|e| e.to_string())
//!        })
//!    }));
//!    app.manage(relay);
//!    ```
//!    The relay acknowledges an event only after the sink returns `Ok` and redelivers it
//!    otherwise (at-least-once); `ingest` dedupes by `external_id`, and so does the client.
//!
//! Hosted settings (endpoint, Firebase API key, sign-in/upgrade pages) live in
//! `crates/devall-relay/src/hosted.rs`. Credentials never touch disk: the Firebase refresh token
//! and the self-host token live in the OS keychain; `relay.json` in the app config dir only
//! records the mode and the self-hosted URL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devall_core::{KeychainSecretStore, MemorySecretStore, Project, SecretStore};
use devall_relay::auth::{load_static_token, store_static_token};
use devall_relay::{
    AccountInfo, InboxEvent, InboxHandle, InboxOptions, InboxState, InboxUpdate, RelayAuth, RelayClient, RelayConfig, SignedIn, TcpConnectHandle, TcpTunnelHandle, TunnelEvent, TunnelHandle, TunnelOptions, TunnelStatus, hosted,
    normalize_endpoint,
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Receives hosted-inbox events for a project root; `Ok` acknowledges the event to the relay.
pub type InboxSink = Arc<dyn Fn(String, InboxEvent) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RelayMode {
    #[default]
    Hosted,
    SelfHosted,
}

/// Persisted in `<config dir>/relay.json` (no secrets).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RelaySettings {
    #[serde(default)]
    pub mode: RelayMode,
    #[serde(default)]
    pub self_hosted_endpoint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelaySettingsView {
    pub mode: RelayMode,
    pub self_hosted_endpoint: String,
    /// A self-host token is stored in the keychain.
    pub has_token: bool,
    pub hosted_endpoint: String,
    /// The hosted relay's constants were filled in (`hosted.rs`); otherwise only self-hosting works.
    pub hosted_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxView {
    pub root: String,
    pub state: InboxState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatusView {
    pub mode: RelayMode,
    pub endpoint: String,
    pub signed_in: bool,
    pub hosted_configured: bool,
    pub keychain_available: bool,
    pub tunnel: Option<TunnelStatus>,
    pub inboxes: Vec<InboxView>,
}

pub(crate) struct Running<H> {
    pub(crate) handle: Arc<H>,
    pub(crate) forwarder: tokio::task::JoinHandle<()>,
}

pub struct RelayState {
    config_dir: PathBuf,
    pub(crate) secrets: Arc<dyn SecretStore>,
    keychain_available: bool,
    settings: Mutex<RelaySettings>,
    client: Mutex<Option<Arc<RelayClient>>>,
    tunnel: tokio::sync::Mutex<Option<Running<TunnelHandle>>>,
    inboxes: tokio::sync::Mutex<HashMap<String, Running<InboxHandle>>>,
    sink: Mutex<Option<InboxSink>>,
    /// Published raw TCP tunnel (modules::cloud).
    pub(crate) tcp: tokio::sync::Mutex<Option<Running<TcpTunnelHandle>>>,
    /// Local listeners forwarding into TCP tunnels, by tunnel name (modules::cloud).
    pub(crate) tcp_consumers: tokio::sync::Mutex<HashMap<String, Running<TcpConnectHandle>>>,
}

impl RelayState {
    pub fn new(config_dir: PathBuf) -> Self {
        let keychain_available = KeychainSecretStore::available();
        let secrets: Arc<dyn SecretStore> = if keychain_available { Arc::new(KeychainSecretStore::new()) } else { Arc::new(MemorySecretStore::default()) };
        let settings = std::fs::read_to_string(config_dir.join("relay.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self {
            config_dir,
            secrets,
            keychain_available,
            settings: Mutex::new(settings),
            client: Mutex::new(None),
            tunnel: tokio::sync::Mutex::new(None),
            inboxes: tokio::sync::Mutex::new(HashMap::new()),
            sink: Mutex::new(None),
            tcp: tokio::sync::Mutex::new(None),
            tcp_consumers: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Where hosted-inbox events go (the webhooks module's `ingest`). See the module docs.
    pub fn set_inbox_sink(&self, sink: InboxSink) {
        *self.sink.lock().expect("sink lock") = Some(sink);
    }

    fn settings(&self) -> RelaySettings {
        self.settings.lock().expect("settings lock").clone()
    }

    fn config(&self) -> CmdResult<RelayConfig> {
        let s = self.settings();
        match s.mode {
            RelayMode::Hosted => {
                if !hosted::is_configured() {
                    return Err("The hosted relay isn't available in this build yet. Self-host the relay (free) and enter its address under Self-hosting.".into());
                }
                Ok(RelayConfig::hosted())
            }
            RelayMode::SelfHosted => {
                if s.self_hosted_endpoint.trim().is_empty() {
                    return Err("Enter your self-hosted relay's address first.".into());
                }
                Ok(RelayConfig::self_hosted(s.self_hosted_endpoint.clone(), None))
            }
        }
    }

    pub(crate) fn mode(&self) -> RelayMode {
        self.settings().mode
    }

    pub(crate) fn client(&self) -> CmdResult<Arc<RelayClient>> {
        let mut slot = self.client.lock().expect("client lock");
        if let Some(c) = slot.as_ref() {
            return Ok(c.clone());
        }
        let c = Arc::new(RelayClient::new(self.config()?, self.secrets.clone()).map_err(err)?);
        *slot = Some(c.clone());
        Ok(c)
    }

    fn endpoint(&self) -> String {
        let s = self.settings();
        match s.mode {
            RelayMode::Hosted => hosted::HOSTED_ENDPOINT.to_string(),
            RelayMode::SelfHosted => s.self_hosted_endpoint,
        }
    }

    fn signed_in(&self) -> bool {
        self.client().map(|c| c.is_signed_in()).unwrap_or(false)
    }

    async fn stop_all(&self) {
        if let Some(t) = self.tunnel.lock().await.take() {
            t.handle.stop().await;
            t.forwarder.abort();
        }
        let inboxes: Vec<_> = self.inboxes.lock().await.drain().collect();
        for (_, i) in inboxes {
            i.handle.stop().await;
            i.forwarder.abort();
        }
        if let Some(t) = self.tcp.lock().await.take() {
            t.handle.stop().await;
            t.forwarder.abort();
        }
        let consumers: Vec<_> = self.tcp_consumers.lock().await.drain().collect();
        for (_, c) in consumers {
            c.handle.stop().await;
            c.forwarder.abort();
        }
    }

    fn settings_view(&self) -> RelaySettingsView {
        let s = self.settings();
        RelaySettingsView {
            mode: s.mode,
            self_hosted_endpoint: s.self_hosted_endpoint,
            has_token: matches!(load_static_token(self.secrets.as_ref()), Ok(Some(_))),
            hosted_endpoint: hosted::HOSTED_ENDPOINT.into(),
            hosted_configured: hosted::is_configured(),
        }
    }

    fn sink_for(&self, root: String) -> devall_relay::InboxHandler {
        let sink = self.sink.lock().expect("sink lock").clone();
        match sink {
            Some(sink) => {
                let root = root.clone();
                devall_relay::handler(move |ev| sink(root.clone(), ev))
            }
            None => devall_relay::handler(move |ev| {
                let root = root.clone();
                async move { tokio::task::spawn_blocking(move || append_jsonl(Path::new(&root), &ev)).await.map_err(err)? }
            }),
        }
    }
}

/// Fallback sink: one JSON line per event in `<project data>/relay-inbox/<YYYY-MM-DD>.jsonl`
/// (the project data folder on this device, see `devall_core::local::project_dir`).
fn append_jsonl(root: &Path, ev: &InboxEvent) -> Result<(), String> {
    use std::io::Write as _;
    let day = ev.received_at.get(..10).filter(|d| d.bytes().all(|b| b.is_ascii_digit() || b == b'-')).unwrap_or("unknown");
    let dir = devall_core::local::project_dir(root).join("relay-inbox");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let mut line = serde_json::to_vec(ev).map_err(err)?;
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{day}.jsonl"))).map_err(err)?;
    f.write_all(&line).map_err(err)?;
    f.sync_data().map_err(err)
}

// ---------- commands ----------

#[tauri::command]
pub async fn relay_status(state: State<'_, RelayState>) -> CmdResult<RelayStatusView> {
    let tunnel = state.tunnel.lock().await.as_ref().map(|t| t.handle.status());
    let inboxes = state.inboxes.lock().await.iter().map(|(root, i)| InboxView { root: root.clone(), state: i.handle.status() }).collect();
    Ok(RelayStatusView {
        mode: state.settings().mode,
        endpoint: state.endpoint(),
        signed_in: state.signed_in(),
        hosted_configured: hosted::is_configured(),
        keychain_available: state.keychain_available,
        tunnel,
        inboxes,
    })
}

/// Opens the hosted sign-in page in the browser and waits (up to 10 minutes) for the loopback callback.
#[tauri::command]
pub async fn relay_sign_in(app: AppHandle, state: State<'_, RelayState>) -> CmdResult<SignedIn> {
    if state.settings().mode != RelayMode::Hosted {
        return Err("Self-hosted relays use a token (Self-hosting settings), not browser sign-in.".into());
    }
    let client = state.client()?;
    let opener = app.clone();
    client
        .sign_in(hosted::SIGNIN_URL, move |url| opener.opener().open_url(url, None::<&str>).map_err(|e| e.to_string()), Duration::from_secs(600))
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn relay_sign_out(state: State<'_, RelayState>) -> CmdResult<()> {
    state.stop_all().await;
    if let Ok(c) = state.client()
        && matches!(c.config().auth, RelayAuth::FirebaseRefreshToken { .. })
    {
        c.sign_out().await.map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn relay_account(state: State<'_, RelayState>) -> CmdResult<AccountInfo> {
    state.client()?.account().await.map_err(err)
}

/// Starts (or restarts) the tunnel to `http://127.0.0.1:<port>`; streams status + request log.
#[tauri::command]
pub async fn relay_tunnel_start(
    state: State<'_, RelayState>,
    port: u16,
    subdomain: Option<String>,
    rewrite_host: Option<bool>,
    on_event: Channel<TunnelEvent>,
) -> CmdResult<TunnelStatus> {
    if port == 0 {
        return Err("Pick the local port to expose.".into());
    }
    let client = state.client()?;
    if !client.is_signed_in() {
        return Err(match state.settings().mode {
            RelayMode::Hosted => "Sign in to use the hosted relay.".into(),
            RelayMode::SelfHosted => "Set your self-hosted relay token first.".into(),
        });
    }
    if let Some(old) = state.tunnel.lock().await.take() {
        old.handle.stop().await;
        old.forwarder.abort();
    }
    let mut opts = TunnelOptions::for_port(port);
    opts.subdomain = subdomain.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
    opts.rewrite_host = rewrite_host.unwrap_or(false);
    let handle = Arc::new(client.start_tunnel(opts));
    let mut rx = handle.subscribe();
    let forwarder = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if on_event.send(ev).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    let status = match handle.wait_connected(Duration::from_secs(20)).await {
        Ok(s) => s,
        Err(_) => handle.status(),
    };
    *state.tunnel.lock().await = Some(Running { handle, forwarder });
    Ok(status)
}

#[tauri::command]
pub async fn relay_tunnel_stop(state: State<'_, RelayState>) -> CmdResult<()> {
    if let Some(t) = state.tunnel.lock().await.take() {
        t.handle.stop().await;
        t.forwarder.abort();
    }
    Ok(())
}

/// Connects the project's hosted webhook inbox. Events go to the registered sink (devall-webhooks
/// `ingest`), or to `<project data>/relay-inbox/*.jsonl` until one is registered.
#[tauri::command]
pub async fn relay_inbox_connect(state: State<'_, RelayState>, root: String, on_event: Channel<InboxUpdate>) -> CmdResult<InboxState> {
    let project = Project::open(&root).map_err(err)?;
    let client = state.client()?;
    if !client.is_signed_in() {
        return Err("Sign in (or set a self-hosted token) to use the hosted inbox.".into());
    }
    if let Some(old) = state.inboxes.lock().await.remove(&root) {
        old.handle.stop().await;
        old.forwarder.abort();
    }
    let opts = InboxOptions { project_key: Some(project.manifest.id.clone()), ..InboxOptions::default() };
    let handle = Arc::new(client.connect_inbox(opts, state.sink_for(root.clone())));
    let mut rx = handle.subscribe();
    let forwarder = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if on_event.send(ev).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    let status = match handle.wait_connected(Duration::from_secs(20)).await {
        Ok(s) => s,
        Err(_) => handle.status(),
    };
    state.inboxes.lock().await.insert(root, Running { handle, forwarder });
    Ok(status)
}

#[tauri::command]
pub async fn relay_inbox_disconnect(state: State<'_, RelayState>, root: String) -> CmdResult<()> {
    if let Some(i) = state.inboxes.lock().await.remove(&root) {
        i.handle.stop().await;
        i.forwarder.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn relay_settings_get(state: State<'_, RelayState>) -> RelaySettingsView {
    state.settings_view()
}

/// Switches hosted ↔ self-hosted. `token` (self-host) goes to the OS keychain; `None` keeps the stored one.
#[tauri::command]
pub async fn relay_settings_set(state: State<'_, RelayState>, mode: RelayMode, endpoint: Option<String>, token: Option<String>) -> CmdResult<RelaySettingsView> {
    let mut next = state.settings();
    next.mode = mode;
    if let Some(e) = endpoint {
        let e = e.trim();
        next.self_hosted_endpoint = if e.is_empty() { String::new() } else { normalize_endpoint(e).map_err(err)?.to_string() };
    }
    if mode == RelayMode::SelfHosted && next.self_hosted_endpoint.is_empty() {
        return Err("Enter your relay's address, e.g. wss://relay.example.com".into());
    }
    if let Some(t) = token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        store_static_token(state.secrets.as_ref(), &t).map_err(err)?;
    }
    std::fs::create_dir_all(&state.config_dir).map_err(err)?;
    let json = serde_json::to_vec_pretty(&next).map_err(err)?;
    std::fs::write(state.config_dir.join("relay.json"), json).map_err(err)?;
    state.stop_all().await;
    *state.settings.lock().expect("settings lock") = next;
    *state.client.lock().expect("client lock") = None;
    Ok(state.settings_view())
}

/// Opens the Supporter page (sign-in + Stripe checkout / billing portal) in the browser.
#[tauri::command]
pub fn relay_upgrade_url(app: AppHandle) -> CmdResult<String> {
    let url = hosted::UPGRADE_URL.to_string();
    app.opener().open_url(&url, None::<&str>).map_err(err)?;
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_sink_appends_json_lines() {
        devall_core::local::isolate_for_tests();
        let dir = std::env::temp_dir().join(format!("devall-relay-test-{}", std::process::id()));
        let ev = InboxEvent {
            id: "evt_1".into(),
            seq: 1,
            inbox_id: "abcdefghijklmnopqrst".into(),
            method: "POST".into(),
            url: "https://hooks.devall.dev/abcdefghijklmnopqrst/x".into(),
            path: "/x".into(),
            headers: vec![],
            body_base64: "e30=".into(),
            body_size: 2,
            received_at: "2026-10-21T16:13:24.517Z".into(),
            source_ip: None,
        };
        append_jsonl(&dir, &ev).unwrap();
        append_jsonl(&dir, &ev).unwrap();
        let text = std::fs::read_to_string(devall_core::local::project_dir(&dir).join("relay-inbox/2026-10-21.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
