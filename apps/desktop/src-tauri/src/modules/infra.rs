//! Docker, Kubernetes, port-forward manager and GCP commands — a thin layer over
//! `crates/devall-infra`.
//!
//! # Integration (done by whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    `devall-infra = { path = "../../../crates/devall-infra" }`
//!    (or `devall-infra = { path = "crates/devall-infra" }` in the root `[workspace.dependencies]`
//!    and `devall-infra.workspace = true`).
//! 2. `src/modules/mod.rs`: `pub mod infra;`
//! 3. `src/lib.rs`:
//!    * Windows only, first thing in `run()` (before any thread is spawned), so kube-rs starts
//!      exec auth plugins (gke-gcloud-auth-plugin, aws, kubelogin) without flashing a console
//!      window in release builds:
//!      ```text
//!      #[cfg(windows)]
//!      // SAFETY: single-threaded at this point; read later by kube-rs when it spawns plugins.
//!      unsafe { std::env::set_var("KUBE_RS_UNSTABLE_CREATE_NO_WINDOW", "1") };
//!      ```
//!    * in `.setup(|app| …)`: `app.manage(modules::infra::InfraState::new());`
//!    * in `tauri::generate_handler![...]` add every command listed in [`COMMANDS`] below as
//!      `modules::infra::<name>,`.
//! 4. Stop forwards on exit (optional; they die with the process anyway): in `.on_window_event`
//!    / `RunEvent::Exit`, `app.state::<modules::infra::InfraState>().shutdown()`.
//! 5. **Port-forward → environment variables** (the headline synergy). Running forwards write
//!    `{{var}}` overrides into `devall_infra::forward::global_overrides()` (in memory only). Add
//!    them as a resolver layer between captured runtime values and the environment file, in
//!    `commands.rs::resolver_for`, which gains the environment *id*:
//!    ```text
//!    pub(crate) fn resolver_for<'a>(
//!        p: &Project,
//!        runtime: &devall_core::Variables,
//!        env_id: Option<&str>,                       // ← new
//!        env: Option<&EnvironmentFile>,
//!        folder_vars: &[devall_core::Variables],
//!        secrets: &'a SecretResolver<'a>,
//!    ) -> Resolver<'a> {
//!        let mut r = Resolver::new(secrets);
//!        r.push_layer(runtime.clone());
//!        // Running port-forwards: e.g. baseUrl → http://127.0.0.1:18080 while the forward runs.
//!        r.push_layer(devall_infra::forward::global_overrides().variables(&p.root().to_string_lossy(), env_id));
//!        if let Some(e) = env { r.push_layer(e.variables.clone()); }
//!        …
//!    }
//!    ```
//!    Every caller already has the env id it read `env_file` from; pass it as the new 3rd
//!    argument (`env.as_deref()` / `env_id`): `commands.rs` `variable_catalog`; `send.rs`
//!    `prepare_for`, `send_request` (assertions), `oauth2_token`, `oauth2_clear`;
//!    `modules/db.rs`, `modules/import.rs`, `modules/proto.rs` (×2), `modules/webhooks.rs`
//!    (`grep -n "resolver_for(" -r src`). Collection runs go through `prepare_for` already.
//!    In `variable_catalog`, list the overrides first so the UI shows where a value comes from:
//!    ```text
//!    let overrides = devall_infra::forward::global_overrides();
//!    for (k, v) in overrides.variables(&root, env.as_deref()) {
//!        let source = overrides.source_of(&root, env.as_deref(), &k).map(|n| format!("port-forward: {n}"));
//!        push(&k, &v, source.unwrap_or_default(), &mut r);
//!    }
//!    ```
//!    The Infra UI bumps `useVarsVersion` after starting/stopping a
//!    forward, so variable previews refresh.
//!
//! Streams (logs, stats, watches, tails, exec) run as tasks keyed by a client-chosen id and stop
//! with `infra_stream_stop`, when their channel closes, or with `infra_stream_stop_all` (the UI
//! calls it when the Infra page mounts, to drop streams orphaned by a reload). Forwards are not
//! streams: they keep running until stopped, across navigation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devall_infra::docker::{ComposeProject, ContainerRow, DockerClient, DockerLogOptions, EngineInfo, ImageRow, NetworkRow, SocketCandidate, StatsSample, VolumeRow};
use devall_infra::forward::{self, ForwardEntry, ForwardFile, ForwardManager, ForwardSource, ForwardStatus, OverrideEntry};
use devall_infra::gcp::datastore::DsEntity;
use devall_infra::gcp::firestore::FsPage;
use devall_infra::gcp::logging::{LogEntryRow, LoggingTailMsg};
use devall_infra::gcp::pubsub::{PubSubTailMsg, PublishMessage, ReceivedMessage, Subscription, Topic};
use devall_infra::gcp::secretmanager::{self, Secret, SecretVersion};
use devall_infra::gcp::storage::{Bucket, ObjectInfo, ObjectList};
use devall_infra::gcp::{GcpClient, GcpStatus, GcpTarget};
use devall_infra::kube::resources::{self as kres};
use devall_infra::kube::stream as kstream;
use devall_infra::kube::{ClusterInfo, Describe, KindInfo, KindRef, KubeContexts, KubeManager, KubeOptions, PodLogOptions, ResourceList, SecretEntry, WatchMsg};
use devall_infra::stream::{ExecEvent, ExecInput, LogMsg, Sink};
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

/// Every command of this module, for `generate_handler!` (reference list; lib.rs spells them out).
#[allow(dead_code)]
pub const COMMANDS: &[&str] = &[
    "infra_stream_stop",
    "infra_stream_stop_all",
    "infra_exec_input",
    "infra_exec_resize",
    "docker_candidates",
    "docker_connect",
    "docker_info",
    "docker_containers",
    "docker_images",
    "docker_volumes",
    "docker_networks",
    "docker_compose_projects",
    "docker_inspect",
    "docker_start",
    "docker_stop",
    "docker_restart",
    "docker_remove",
    "docker_compose",
    "docker_logs",
    "docker_stats",
    "docker_exec",
    "kube_options",
    "kube_set_options",
    "kube_contexts",
    "kube_test",
    "kube_namespaces",
    "kube_kinds",
    "kube_list",
    "kube_watch",
    "kube_yaml",
    "kube_secret_values",
    "kube_describe",
    "kube_delete",
    "kube_restart",
    "kube_scale",
    "kube_logs",
    "kube_exec",
    "forward_list",
    "forward_start",
    "forward_stop",
    "forward_save",
    "forward_delete",
    "forward_active_overrides",
    "forward_save_to_environment",
    "gcp_status",
    "gcp_pubsub_topics",
    "gcp_pubsub_subscriptions",
    "gcp_pubsub_create_topic",
    "gcp_pubsub_delete_topic",
    "gcp_pubsub_create_subscription",
    "gcp_pubsub_delete_subscription",
    "gcp_pubsub_publish",
    "gcp_pubsub_pull",
    "gcp_pubsub_tail",
    "gcp_firestore_collections",
    "gcp_firestore_documents",
    "gcp_datastore_kinds",
    "gcp_datastore_entities",
    "gcp_storage_buckets",
    "gcp_storage_objects",
    "gcp_storage_download",
    "gcp_storage_preview",
    "gcp_storage_upload",
    "gcp_logging_list",
    "gcp_logging_tail",
    "gcp_secrets",
    "gcp_secret_versions",
    "gcp_secrets_import",
];

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct ExecHandle {
    input: mpsc::Sender<ExecInput>,
    abort: AbortHandle,
}

pub struct InfraState {
    docker: tokio::sync::Mutex<Option<DockerClient>>,
    /// Engine address chosen in the UI; `None` = auto-detect.
    docker_host: Mutex<Option<String>>,
    kube: KubeManager,
    forwards: ForwardManager,
    gcp: Option<GcpClient>,
    gcp_error: Option<String>,
    streams: Arc<Mutex<HashMap<String, AbortHandle>>>,
    execs: Arc<Mutex<HashMap<String, ExecHandle>>>,
    adhoc: Mutex<u64>,
}

impl Default for InfraState {
    fn default() -> Self {
        Self::new()
    }
}

impl InfraState {
    pub fn new() -> Self {
        // Probing the login-shell PATH can take a moment on macOS; do it off the UI path.
        std::thread::spawn(|| {
            let _ = devall_infra::sys::search_path();
        });
        let (gcp, gcp_error) = match GcpClient::new() {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Self {
            docker: tokio::sync::Mutex::new(None),
            docker_host: Mutex::new(None),
            kube: KubeManager::default(),
            // Shared with the HTTP send path through `global_overrides()`.
            forwards: ForwardManager::new(forward::global_overrides().clone()),
            gcp,
            gcp_error,
            streams: Arc::default(),
            execs: Arc::default(),
            adhoc: Mutex::new(0),
        }
    }

    /// Stops every forward, stream and exec session.
    pub fn shutdown(&self) {
        self.forwards.stop_all();
        for (_, h) in self.streams.lock().unwrap().drain() {
            h.abort();
        }
        for (_, e) in self.execs.lock().unwrap().drain() {
            e.abort.abort();
        }
    }

    async fn docker(&self) -> CmdResult<DockerClient> {
        let mut guard = self.docker.lock().await;
        if let Some(c) = guard.as_ref() {
            return Ok(c.clone());
        }
        let host = self.docker_host.lock().unwrap().clone();
        let c = DockerClient::connect(host.as_deref()).await.map_err(err)?;
        *guard = Some(c.clone());
        Ok(c)
    }

    async fn docker_reset(&self) {
        *self.docker.lock().await = None;
    }

    /// The running Kubernetes forward listening on `port` for `root` (for "explain this failure").
    pub(crate) fn kube_forward_on_port(&self, root: &str, port: u16) -> Option<ForwardStatus> {
        self.forwards.list(Some(root)).into_iter().find(|f| f.source == ForwardSource::Kubernetes && f.local_port == Some(port) && f.backend.is_some())
    }

    pub(crate) async fn kube_client(&self, context: &str) -> CmdResult<devall_infra::kube::Client> {
        self.kube.client(context).await.map_err(err)
    }

    /// The client for `context` plus its default namespace.
    pub(crate) async fn kube_conn(&self, context: &str) -> CmdResult<devall_infra::kube::KubeConn> {
        self.kube.conn(context).await.map_err(err)
    }

    fn gcp(&self) -> CmdResult<&GcpClient> {
        self.gcp.as_ref().ok_or_else(|| self.gcp_error.clone().unwrap_or_else(|| "GCP client unavailable".into()))
    }

    /// Runs `fut` as a stream task under `id` (replacing one with the same id); it is forgotten
    /// when it ends.
    fn spawn_stream<F>(&self, id: String, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let streams = self.streams.clone();
        let key = id.clone();
        let mut map = self.streams.lock().unwrap();
        if let Some(old) = map.remove(&id) {
            old.abort();
        }
        let handle = tokio::spawn(async move {
            fut.await;
            streams.lock().unwrap().remove(&key);
        });
        map.insert(id, handle.abort_handle());
    }

    fn spawn_exec<F>(&self, id: String, make: impl FnOnce(mpsc::Receiver<ExecInput>) -> F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<ExecInput>(256);
        let execs = self.execs.clone();
        let key = id.clone();
        let fut = make(rx);
        // Hold the map while spawning so a session that ends at once can't be re-inserted dead.
        let mut map = self.execs.lock().unwrap();
        let handle = tokio::spawn(async move {
            fut.await;
            execs.lock().unwrap().remove(&key);
        });
        if let Some(old) = map.insert(id, ExecHandle { input: tx, abort: handle.abort_handle() }) {
            old.abort.abort();
        }
    }
}

fn channel_sink<T: tauri::ipc::IpcResponse + Clone + Send + Sync + 'static>(ch: Channel<T>) -> Sink<T> {
    Arc::new(move |m: T| ch.send(m).is_ok())
}

fn exec_sink(ch: Channel<ExecEvent>) -> Sink<ExecEvent> {
    channel_sink(ch)
}

// ---------- streams & exec sessions ----------

/// Stops a log/stats/watch/tail stream or an exec session.
#[tauri::command]
pub fn infra_stream_stop(state: State<'_, InfraState>, stream_id: String) -> bool {
    let mut stopped = false;
    if let Some(h) = state.streams.lock().unwrap().remove(&stream_id) {
        h.abort();
        stopped = true;
    }
    if let Some(e) = state.execs.lock().unwrap().remove(&stream_id) {
        e.abort.abort();
        stopped = true;
    }
    stopped
}

/// Stops every stream and exec session (forwards keep running). Returns how many were stopped.
#[tauri::command]
pub fn infra_stream_stop_all(state: State<'_, InfraState>) -> usize {
    let mut n = 0;
    for (_, h) in state.streams.lock().unwrap().drain() {
        h.abort();
        n += 1;
    }
    for (_, e) in state.execs.lock().unwrap().drain() {
        e.abort.abort();
        n += 1;
    }
    n
}

/// Keystrokes for an exec session (UTF-8 text, control characters included).
#[tauri::command]
pub async fn infra_exec_input(state: State<'_, InfraState>, exec_id: String, data: String) -> CmdResult<bool> {
    let tx = state.execs.lock().unwrap().get(&exec_id).map(|e| e.input.clone());
    match tx {
        Some(tx) => Ok(tx.send(ExecInput::Data(data.into_bytes())).await.is_ok()),
        None => Ok(false),
    }
}

#[tauri::command]
pub async fn infra_exec_resize(state: State<'_, InfraState>, exec_id: String, cols: u16, rows: u16) -> CmdResult<bool> {
    let tx = state.execs.lock().unwrap().get(&exec_id).map(|e| e.input.clone());
    match tx {
        Some(tx) => Ok(tx.send(ExecInput::Resize { cols: cols.max(1), rows: rows.max(1) }).await.is_ok()),
        None => Ok(false),
    }
}

// ---------- Docker ----------

#[tauri::command]
pub fn docker_candidates() -> Vec<SocketCandidate> {
    devall_infra::docker::candidates()
}

/// Connects to `host` (any DOCKER_HOST form) or auto-detects when empty; remembered for the session.
#[tauri::command]
pub async fn docker_connect(state: State<'_, InfraState>, host: Option<String>) -> CmdResult<EngineInfo> {
    *state.docker_host.lock().unwrap() = host.filter(|h| !h.trim().is_empty());
    state.docker_reset().await;
    let c = state.docker().await?;
    c.info().await.map_err(err)
}

#[tauri::command]
pub async fn docker_info(state: State<'_, InfraState>) -> CmdResult<EngineInfo> {
    let c = state.docker().await?;
    match c.info().await {
        Ok(i) => Ok(i),
        Err(e) => {
            // The engine may have restarted or switched; reconnect next time.
            state.docker_reset().await;
            Err(err(e))
        }
    }
}

#[tauri::command]
pub async fn docker_containers(state: State<'_, InfraState>, all: Option<bool>) -> CmdResult<Vec<ContainerRow>> {
    state.docker().await?.containers(all.unwrap_or(true)).await.map_err(err)
}

#[tauri::command]
pub async fn docker_images(state: State<'_, InfraState>) -> CmdResult<Vec<ImageRow>> {
    state.docker().await?.images().await.map_err(err)
}

#[tauri::command]
pub async fn docker_volumes(state: State<'_, InfraState>) -> CmdResult<Vec<VolumeRow>> {
    state.docker().await?.volumes().await.map_err(err)
}

#[tauri::command]
pub async fn docker_networks(state: State<'_, InfraState>) -> CmdResult<Vec<NetworkRow>> {
    state.docker().await?.networks().await.map_err(err)
}

#[tauri::command]
pub async fn docker_compose_projects(state: State<'_, InfraState>) -> CmdResult<Vec<ComposeProject>> {
    state.docker().await?.compose_projects().await.map_err(err)
}

#[tauri::command]
pub async fn docker_inspect(state: State<'_, InfraState>, id: String) -> CmdResult<serde_json::Value> {
    state.docker().await?.inspect(&id).await.map_err(err)
}

#[tauri::command]
pub async fn docker_start(state: State<'_, InfraState>, id: String) -> CmdResult<()> {
    state.docker().await?.start(&id).await.map_err(err)
}

#[tauri::command]
pub async fn docker_stop(state: State<'_, InfraState>, id: String) -> CmdResult<()> {
    state.docker().await?.stop(&id).await.map_err(err)
}

#[tauri::command]
pub async fn docker_restart(state: State<'_, InfraState>, id: String) -> CmdResult<()> {
    state.docker().await?.restart(&id).await.map_err(err)
}

/// Removes a container. The UI confirms first.
#[tauri::command]
pub async fn docker_remove(state: State<'_, InfraState>, id: String, force: Option<bool>) -> CmdResult<()> {
    state.docker().await?.remove(&id, force.unwrap_or(false)).await.map_err(err)
}

/// `docker compose -p <project> <action>`: `up` (detached), `down`, `stop`, `start`, `restart`,
/// `pull`. Returns the CLI output. `down` is destructive; the UI confirms.
#[tauri::command]
pub async fn docker_compose(state: State<'_, InfraState>, project: ComposeProject, action: String) -> CmdResult<String> {
    let args: &[&str] = match action.as_str() {
        "up" => &["up", "-d"],
        "down" => &["down"],
        "stop" => &["stop"],
        "start" => &["start"],
        "restart" => &["restart"],
        "pull" => &["pull"],
        other => return Err(format!("unknown compose action {other}")),
    };
    state.docker().await?.compose(&project, args).await.map_err(err)
}

#[tauri::command]
pub async fn docker_logs(state: State<'_, InfraState>, stream_id: String, id: String, options: Option<DockerLogOptions>, on_event: Channel<LogMsg>) -> CmdResult<()> {
    let client = state.docker().await?;
    let opts = options.unwrap_or_default();
    let sink = channel_sink(on_event);
    state.spawn_stream(stream_id, async move { client.stream_logs(&id, &opts, sink).await });
    Ok(())
}

#[tauri::command]
pub async fn docker_stats(state: State<'_, InfraState>, stream_id: String, id: String, on_event: Channel<StatsSample>) -> CmdResult<()> {
    let client = state.docker().await?;
    let sink = channel_sink(on_event);
    state.spawn_stream(stream_id, async move {
        let _ = client.stream_stats(&id, sink).await;
    });
    Ok(())
}

/// Starts an interactive process (a shell when `command` is empty) in a running container.
#[tauri::command]
pub async fn docker_exec(
    state: State<'_, InfraState>,
    exec_id: String,
    id: String,
    command: Option<Vec<String>>,
    tty: Option<bool>,
    on_event: Channel<ExecEvent>,
) -> CmdResult<()> {
    let client = state.docker().await?;
    let sink = exec_sink(on_event);
    state.spawn_exec(exec_id, move |rx| async move {
        if let Err(e) = client.exec(&id, command.unwrap_or_default(), tty.unwrap_or(true), rx, sink.clone()).await {
            sink(ExecEvent::Exit { code: None, error: Some(e.to_string()) });
        }
    });
    Ok(())
}

// ---------- Kubernetes ----------

#[tauri::command]
pub fn kube_options(state: State<'_, InfraState>) -> KubeOptions {
    state.kube.options()
}

/// Kubeconfig files to merge (empty = KUBECONFIG / ~/.kube/config) and the WSL plugin fallback.
#[tauri::command]
pub fn kube_set_options(state: State<'_, InfraState>, options: KubeOptions) {
    state.kube.set_options(options);
}

#[tauri::command]
pub async fn kube_contexts(state: State<'_, InfraState>) -> CmdResult<KubeContexts> {
    state.kube.contexts().await.map_err(err)
}

/// Connects (running the auth plugin) and returns the server version.
#[tauri::command]
pub async fn kube_test(state: State<'_, InfraState>, context: String) -> CmdResult<ClusterInfo> {
    state.kube.test(&context).await.map_err(err)
}

#[tauri::command]
pub async fn kube_namespaces(state: State<'_, InfraState>, context: String) -> CmdResult<Vec<String>> {
    let conn = state.kube.conn(&context).await.map_err(err)?;
    kres::namespaces(&conn.client, &conn.default_namespace).await.map_err(err)
}

/// Built-in kinds, plus the cluster's custom resources when `custom`.
#[tauri::command]
pub async fn kube_kinds(state: State<'_, InfraState>, context: String, custom: Option<bool>) -> CmdResult<Vec<KindInfo>> {
    let mut kinds = kres::builtin_kinds();
    if custom.unwrap_or(false) {
        let client = state.kube.client(&context).await.map_err(err)?;
        kinds.extend(kres::custom_kinds(&client).await.map_err(err)?);
    }
    Ok(kinds)
}

#[tauri::command]
pub async fn kube_list(state: State<'_, InfraState>, context: String, kind: KindRef, namespace: Option<String>, label_selector: Option<String>) -> CmdResult<ResourceList> {
    let client = state.kube.client(&context).await.map_err(err)?;
    kres::list(&client, &kind, namespace.as_deref(), label_selector.as_deref()).await.map_err(err)
}

/// Live table updates for `kind` (Reset on every (re)list, then Upsert/Delete).
#[tauri::command]
pub async fn kube_watch(
    state: State<'_, InfraState>,
    stream_id: String,
    context: String,
    kind: KindRef,
    namespace: Option<String>,
    label_selector: Option<String>,
    on_event: Channel<WatchMsg>,
) -> CmdResult<()> {
    let client = state.kube.client(&context).await.map_err(err)?;
    let sink = channel_sink(on_event);
    state.spawn_stream(stream_id, kstream::watch(client, kind, namespace.filter(|n| !n.is_empty()), label_selector, sink));
    Ok(())
}

/// YAML of one object. Secret values are masked unless `reveal`.
#[tauri::command]
pub async fn kube_yaml(state: State<'_, InfraState>, context: String, kind: KindRef, namespace: Option<String>, name: String, reveal: Option<bool>) -> CmdResult<String> {
    let client = state.kube.client(&context).await.map_err(err)?;
    kres::get_yaml(&client, &kind, namespace.as_deref(), &name, reveal.unwrap_or(false)).await.map_err(err)
}

/// Decoded Secret values (explicit reveal only).
#[tauri::command]
pub async fn kube_secret_values(state: State<'_, InfraState>, context: String, namespace: String, name: String) -> CmdResult<Vec<SecretEntry>> {
    let client = state.kube.client(&context).await.map_err(err)?;
    kres::secret_values(&client, &namespace, &name).await.map_err(err)
}

#[tauri::command]
pub async fn kube_describe(state: State<'_, InfraState>, context: String, kind: KindRef, namespace: Option<String>, name: String) -> CmdResult<Describe> {
    let client = state.kube.client(&context).await.map_err(err)?;
    kres::describe(&client, &kind, namespace.as_deref(), &name).await.map_err(err)
}

/// Deletes an object. The UI confirms first.
#[tauri::command]
pub async fn kube_delete(state: State<'_, InfraState>, context: String, kind: KindRef, namespace: Option<String>, name: String) -> CmdResult<()> {
    let client = state.kube.client(&context).await.map_err(err)?;
    kres::delete(&client, &kind, namespace.as_deref(), &name).await.map_err(err)
}

/// Rollout restart (Deployment/StatefulSet/DaemonSet) or pod delete. The UI confirms first.
#[tauri::command]
pub async fn kube_restart(state: State<'_, InfraState>, context: String, kind: KindRef, namespace: Option<String>, name: String) -> CmdResult<()> {
    let client = state.kube.client(&context).await.map_err(err)?;
    kres::restart(&client, &kind, namespace.as_deref(), &name).await.map_err(err)
}

#[tauri::command]
pub async fn kube_scale(state: State<'_, InfraState>, context: String, kind: KindRef, namespace: Option<String>, name: String, replicas: i32) -> CmdResult<()> {
    let client = state.kube.client(&context).await.map_err(err)?;
    kres::scale(&client, &kind, namespace.as_deref(), &name, replicas).await.map_err(err)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn kube_logs(
    state: State<'_, InfraState>,
    stream_id: String,
    context: String,
    namespace: String,
    pod: String,
    options: Option<PodLogOptions>,
    on_event: Channel<LogMsg>,
) -> CmdResult<()> {
    let client = state.kube.client(&context).await.map_err(err)?;
    let opts = options.unwrap_or_default();
    let sink = channel_sink(on_event);
    state.spawn_stream(stream_id, async move { kstream::pod_logs(client, &namespace, &pod, &opts, sink).await });
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn kube_exec(
    state: State<'_, InfraState>,
    exec_id: String,
    context: String,
    namespace: String,
    pod: String,
    container: Option<String>,
    command: Option<Vec<String>>,
    tty: Option<bool>,
    on_event: Channel<ExecEvent>,
) -> CmdResult<()> {
    let client = state.kube.client(&context).await.map_err(err)?;
    let sink = exec_sink(on_event);
    state.spawn_exec(exec_id, move |rx| async move {
        if let Err(e) = kstream::pod_exec(client, &namespace, &pod, container, command.unwrap_or_default(), tty.unwrap_or(true), rx, sink.clone()).await {
            sink(ExecEvent::Exit { code: None, error: Some(e.to_string()) });
        }
    });
    Ok(())
}

// ---------- port forwards ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardListing {
    /// `connections/*.forward.yaml` of the project.
    pub saved: Vec<ForwardEntry>,
    /// Running forwards of the project (saved and ad-hoc).
    pub running: Vec<ForwardStatus>,
}

#[tauri::command]
pub fn forward_list(state: State<'_, InfraState>, root: String) -> ForwardListing {
    ForwardListing { saved: forward::list(&PathBuf::from(&root)), running: state.forwards.list(Some(&root)) }
}

/// Starts a forward: a saved one by `id`, or an ad-hoc `spec` (from the Kubernetes/Docker views).
/// With both, the saved forward runs with the given (edited) spec. Restarts it when running.
#[tauri::command]
pub async fn forward_start(state: State<'_, InfraState>, root: String, id: Option<String>, spec: Option<ForwardFile>) -> CmdResult<ForwardStatus> {
    let (id, spec, saved) = match (id.filter(|i| !i.is_empty()), spec) {
        (Some(id), Some(spec)) => {
            let saved = forward::forward_path(&PathBuf::from(&root), &id).map(|p| p.is_file()).unwrap_or(false);
            (id, spec, saved)
        }
        (Some(id), None) => {
            let spec = forward::read(&PathBuf::from(&root), &id).map_err(err)?;
            (id, spec, true)
        }
        (None, Some(spec)) => {
            let mut n = state.adhoc.lock().unwrap();
            *n += 1;
            (format!("adhoc-{}-{}", devall_core::fsutil::slugify(&spec.name), *n), spec, false)
        }
        (None, None) => return Err("forward_start needs an id or a spec".into()),
    };
    let status = match spec.source {
        ForwardSource::Kubernetes => {
            let conn = state.kube.conn(spec.context.as_deref().unwrap_or("")).await.map_err(err)?;
            state.forwards.start_kube(&root, &id, spec, conn, saved).await.map_err(err)?
        }
        ForwardSource::Docker => {
            let docker = state.docker().await?;
            state.forwards.start_docker(&root, &id, spec, docker, saved).await.map_err(err)?
        }
    };
    record_forward_event(&root, true, &status);
    Ok(status)
}

/// `forward.started` / `forward.stopped` in the project event log (best-effort).
fn record_forward_event(root: &str, started: bool, s: &ForwardStatus) {
    let target = s.backend.clone().unwrap_or_else(|| s.name.clone());
    let remote = [s.context.as_deref(), s.namespace.as_deref()].into_iter().flatten().collect::<Vec<_>>().join("/");
    crate::modules::logs::record_forward(root, started, &target, s.local_port.unwrap_or(0), &remote);
}

#[tauri::command]
pub fn forward_stop(state: State<'_, InfraState>, root: String, id: String) -> bool {
    let before = state.forwards.status(&root, &id);
    let stopped = state.forwards.stop(&root, &id);
    if let (true, Some(s)) = (stopped, before) {
        record_forward_event(&root, false, &s);
    }
    stopped
}

/// Saves a definition to `connections/`: creates a new file when `id` is empty (returns the new
/// id), else overwrites. `runningId` re-keys a running ad-hoc forward to the saved id.
#[tauri::command]
pub fn forward_save(state: State<'_, InfraState>, root: String, id: Option<String>, spec: ForwardFile, running_id: Option<String>) -> CmdResult<String> {
    let dir = PathBuf::from(&root);
    let id = match id.filter(|i| !i.is_empty()) {
        Some(id) => {
            forward::save(&dir, &id, &spec).map_err(err)?;
            id
        }
        None => forward::create(&dir, &spec).map_err(err)?,
    };
    if let Some(old) = running_id.filter(|r| r != &id) {
        state.forwards.adopt(&root, &old, &id, Some(spec));
    }
    Ok(id)
}

/// Stops the forward and moves its definition to the OS trash. The UI confirms first.
#[tauri::command]
pub fn forward_delete(state: State<'_, InfraState>, root: String, id: String) -> CmdResult<()> {
    state.forwards.stop(&root, &id);
    forward::delete(&PathBuf::from(&root), &id).map_err(err)
}

/// Runtime variable overrides of the project (all, or only those applying to `env`).
#[tauri::command]
pub fn forward_active_overrides(state: State<'_, InfraState>, root: String, env: Option<String>) -> Vec<OverrideEntry> {
    let all = state.forwards.overrides().list(Some(&root));
    match env.filter(|e| !e.is_empty()) {
        None => all,
        Some(env) => all.into_iter().filter(|e| e.environment.is_none() || e.environment.as_deref() == Some(env.as_str())).collect(),
    }
}

/// "Save to environment": writes the forward's current bound value into the environment file
/// (`envId` overrides the binding's environment, needed when it is unbound). Returns the env id.
#[tauri::command]
pub fn forward_save_to_environment(state: State<'_, InfraState>, root: String, id: String, env_id: Option<String>) -> CmdResult<String> {
    let entry = state.forwards.overrides().get(&root, &id).ok_or("this forward isn't running or binds no variable")?;
    let env = env_id.filter(|e| !e.is_empty()).or(entry.environment.clone()).ok_or("pick the environment to save into")?;
    forward::write_env_variable(&PathBuf::from(&root), &env, &entry.variable, &entry.value).map_err(err)?;
    Ok(env)
}

// ---------- GCP ----------

#[tauri::command]
pub async fn gcp_status(state: State<'_, InfraState>) -> CmdResult<GcpStatus> {
    Ok(state.gcp()?.status().await)
}

// ---------- GCP Secret Manager ----------

/// Secrets of a real project (metadata only; there is no emulator).
#[tauri::command]
pub async fn gcp_secrets(state: State<'_, InfraState>, target: GcpTarget) -> CmdResult<Vec<Secret>> {
    state.gcp()?.secrets(&target).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_secret_versions(state: State<'_, InfraState>, target: GcpTarget, secret: String) -> CmdResult<Vec<SecretVersion>> {
    state.gcp()?.secret_versions(&target, &secret).await.map_err(err)
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretImportItem {
    /// Secret Manager id.
    pub secret: String,
    /// Version id; empty / `latest` = latest.
    #[serde(default)]
    pub version: Option<String>,
    /// Name in the environment (`{{secret.NAME}}`).
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretImportMode {
    /// Copy the value into the OS keychain under the environment's scope.
    Keychain,
    /// Add a `gcp-sm://` reference to `secretSources` (resolved with gcloud at send time).
    Reference,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretImportResult {
    pub environment: String,
    /// Names imported (values are never returned).
    pub imported: Vec<String>,
    /// `(name, reason)` for items that were not imported.
    pub skipped: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

fn valid_secret_name(n: &str) -> bool {
    let mut chars = n.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// "Import into environment…": Secret Manager secrets become environment secrets — either the
/// values go into the OS keychain under that environment (never into files), or a `gcp-sm://`
/// provider reference is added to `secretSources` (nothing stored). Either way the names are
/// added to the environment's `secrets:` list. Nothing is written to Google Cloud.
#[tauri::command]
pub async fn gcp_secrets_import(
    state: State<'_, InfraState>,
    app: State<'_, crate::state::AppState>,
    root: String,
    env_id: String,
    target: GcpTarget,
    items: Vec<SecretImportItem>,
    mode: SecretImportMode,
) -> CmdResult<SecretImportResult> {
    let p = devall_core::Project::open(&root).map_err(err)?;
    let mut env = p.read_environment(&env_id).map_err(err)?;
    if mode == SecretImportMode::Keychain && !app.keychain_available {
        return Err("the OS keychain is not available, so values can't be stored safely — import as gcp-sm:// references instead".into());
    }
    let project = target.project.trim().to_string();
    let mut out = SecretImportResult { environment: env.name.clone(), ..Default::default() };
    for item in items {
        let name = item.name.trim().to_string();
        if !valid_secret_name(&name) {
            out.skipped.push((name.clone(), "invalid name (letters, digits, _ - . ; must start with a letter or _)".into()));
            continue;
        }
        match mode {
            SecretImportMode::Keychain => {
                let value = match state.gcp()?.access_secret(&target, &item.secret, item.version.as_deref()).await {
                    Ok(v) => v,
                    Err(e) => {
                        out.skipped.push((name.clone(), e.to_string()));
                        continue;
                    }
                };
                app.secrets.set(&devall_core::SecretScope::new(p.id(), env_id.as_str()), &name, &value).map_err(err)?;
                if env.secret_sources.contains_key(&name) {
                    out.warnings.push(format!("{name} also has a provider reference in secretSources; the keychain value now takes precedence"));
                }
            }
            SecretImportMode::Reference => {
                if project.is_empty() {
                    return Err("a GCP project id is required".into());
                }
                env.secret_sources.insert(name.clone(), secretmanager::reference(&project, &item.secret, item.version.as_deref()));
            }
        }
        if !env.secrets.contains(&name) {
            env.secrets.push(name.clone());
        }
        out.imported.push(name);
    }
    if !out.imported.is_empty() {
        p.save_environment(&env_id, &env).map_err(err)?;
    }
    Ok(out)
}

#[tauri::command]
pub async fn gcp_pubsub_topics(state: State<'_, InfraState>, target: GcpTarget) -> CmdResult<Vec<Topic>> {
    state.gcp()?.pubsub_topics(&target).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_pubsub_subscriptions(state: State<'_, InfraState>, target: GcpTarget) -> CmdResult<Vec<Subscription>> {
    state.gcp()?.pubsub_subscriptions(&target).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_pubsub_create_topic(state: State<'_, InfraState>, target: GcpTarget, name: String) -> CmdResult<Topic> {
    state.gcp()?.pubsub_create_topic(&target, name.trim()).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_pubsub_delete_topic(state: State<'_, InfraState>, target: GcpTarget, name: String) -> CmdResult<()> {
    state.gcp()?.pubsub_delete_topic(&target, &name).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_pubsub_create_subscription(state: State<'_, InfraState>, target: GcpTarget, name: String, topic: String, ack_deadline_seconds: Option<i64>) -> CmdResult<Subscription> {
    state.gcp()?.pubsub_create_subscription(&target, name.trim(), &topic, ack_deadline_seconds).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_pubsub_delete_subscription(state: State<'_, InfraState>, target: GcpTarget, name: String) -> CmdResult<()> {
    state.gcp()?.pubsub_delete_subscription(&target, &name).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_pubsub_publish(state: State<'_, InfraState>, target: GcpTarget, topic: String, messages: Vec<PublishMessage>) -> CmdResult<Vec<String>> {
    state.gcp()?.pubsub_publish(&target, &topic, &messages).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_pubsub_pull(state: State<'_, InfraState>, target: GcpTarget, subscription: String, max: Option<u32>, ack: Option<bool>) -> CmdResult<Vec<ReceivedMessage>> {
    state.gcp()?.pubsub_pull(&target, &subscription, max.unwrap_or(20), ack.unwrap_or(false)).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_pubsub_tail(
    state: State<'_, InfraState>,
    stream_id: String,
    target: GcpTarget,
    subscription: String,
    ack: Option<bool>,
    on_event: Channel<PubSubTailMsg>,
) -> CmdResult<()> {
    let client = state.gcp()?.clone();
    let sink = channel_sink(on_event);
    let ack = ack.unwrap_or(true);
    state.spawn_stream(stream_id, async move { client.pubsub_tail(&target, &subscription, ack, sink).await });
    Ok(())
}

#[tauri::command]
pub async fn gcp_firestore_collections(state: State<'_, InfraState>, target: GcpTarget, database: Option<String>, parent: Option<String>) -> CmdResult<Vec<String>> {
    state.gcp()?.firestore_collections(&target, database.as_deref(), parent.as_deref().unwrap_or("")).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_firestore_documents(
    state: State<'_, InfraState>,
    target: GcpTarget,
    database: Option<String>,
    collection: String,
    page_size: Option<u32>,
    page_token: Option<String>,
) -> CmdResult<FsPage> {
    state.gcp()?.firestore_documents(&target, database.as_deref(), &collection, page_size.unwrap_or(50), page_token.as_deref()).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_datastore_kinds(state: State<'_, InfraState>, target: GcpTarget, namespace: Option<String>) -> CmdResult<Vec<String>> {
    state.gcp()?.datastore_kinds(&target, namespace.as_deref()).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_datastore_entities(state: State<'_, InfraState>, target: GcpTarget, namespace: Option<String>, kind: String, limit: Option<u32>) -> CmdResult<Vec<DsEntity>> {
    state.gcp()?.datastore_entities(&target, namespace.as_deref(), &kind, limit.unwrap_or(100)).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_storage_buckets(state: State<'_, InfraState>, target: GcpTarget) -> CmdResult<Vec<Bucket>> {
    state.gcp()?.storage_buckets(&target).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_storage_objects(state: State<'_, InfraState>, target: GcpTarget, bucket: String, prefix: Option<String>, page_token: Option<String>) -> CmdResult<ObjectList> {
    state.gcp()?.storage_objects(&target, &bucket, prefix.as_deref().unwrap_or(""), page_token.as_deref()).await.map_err(err)
}

/// Downloads an object to `dest` (a path the user picked in a save dialog). Returns bytes written.
#[tauri::command]
pub async fn gcp_storage_download(state: State<'_, InfraState>, target: GcpTarget, bucket: String, object: String, dest: String) -> CmdResult<u64> {
    state.gcp()?.storage_download(&target, &bucket, &object, &PathBuf::from(dest)).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_storage_preview(state: State<'_, InfraState>, target: GcpTarget, bucket: String, object: String) -> CmdResult<String> {
    state.gcp()?.storage_preview(&target, &bucket, &object, 256 * 1024).await.map_err(err)
}

/// Uploads `src` (a file the user picked) as `object`. Emulators, or real buckets with writes on.
#[tauri::command]
pub async fn gcp_storage_upload(state: State<'_, InfraState>, target: GcpTarget, bucket: String, object: String, src: String) -> CmdResult<ObjectInfo> {
    state.gcp()?.storage_upload(&target, &bucket, &object, &PathBuf::from(src), None).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_logging_list(state: State<'_, InfraState>, target: GcpTarget, filter: Option<String>, limit: Option<u32>) -> CmdResult<Vec<LogEntryRow>> {
    state.gcp()?.logging_list(&target, filter.as_deref().unwrap_or(""), limit.unwrap_or(200)).await.map_err(err)
}

#[tauri::command]
pub async fn gcp_logging_tail(
    state: State<'_, InfraState>,
    stream_id: String,
    target: GcpTarget,
    filter: Option<String>,
    every_secs: Option<u64>,
    on_event: Channel<LoggingTailMsg>,
) -> CmdResult<()> {
    let client = state.gcp()?.clone();
    let sink = channel_sink(on_event);
    let filter = filter.unwrap_or_default();
    let every = Duration::from_secs(every_secs.unwrap_or(3).clamp(1, 60));
    state.spawn_stream(stream_id, async move { client.logging_tail(&target, &filter, every, Duration::from_secs(60), sink).await });
    Ok(())
}
