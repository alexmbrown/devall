//! Load testing commands — a thin layer over `crates/devall-load`.
//!
//! # Integration (done by whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    `devall-load = { path = "../../../crates/devall-load" }`
//!    (or add `devall-load = { path = "crates/devall-load" }` to the root `[workspace.dependencies]`
//!    and use `devall-load.workspace = true`).
//! 2. `src/modules/mod.rs`: `pub mod load;`
//! 3. `src/lib.rs`: `mod modules;` (if not yet declared), then on the builder
//!    `.manage(modules::load::LoadState::new())` and in `tauri::generate_handler![...]`:
//!    ```text
//!    modules::load::load_list,
//!    modules::load::load_read,
//!    modules::load::load_save,
//!    modules::load::load_validate,
//!    modules::load::load_create,
//!    modules::load::load_delete,
//!    modules::load::load_preflight,
//!    modules::load::load_run,
//!    modules::load::load_stop,
//!    modules::load::load_running,
//!    modules::load::load_runs,
//!    modules::load::load_read_run,
//!    modules::load::load_delete_run,
//!    modules::load::load_compare,
//!    modules::load::load_schema,
//!    modules::load::load_run_distributed,
//!    modules::load::load_agents_check,
//!    modules::load::load_agents_token,
//!    modules::load::load_cloud_prepare,
//!    modules::load::load_cloud_info,
//!    modules::load::load_cloud_target,
//!    modules::load::load_cloud_verify,
//!    modules::load::load_cloud_remove,
//!    modules::load::load_cloud_run,
//!    ```
//! 4. Optionally copy `crates/devall-load/schemas/*.json` to the repo's `schemas/`.
//!
//! Secrets: the module keeps its own store (OS keychain when available, else in-memory), the same
//! way `state::AppState` does, so it needs nothing from the rest of the app. Keychain entries are
//! shared, so values set on the Environments page are visible here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use devall_core::secrets::SecretScope;
use devall_core::{MemorySecretStore, Project, SecretStore};
use devall_load::distributed::{AgentInfo, AgentTarget, DistributedOptions};
use devall_load::compare::{CompareOptions, Comparison};
use devall_load::{
    Confirmation, Issue, LoadSnapshot, LoadSummary, LoadTarget, LoadTestEntry, LoadTestFile, Preflight, RunControl, RunEntry, RunOptions,
    build_plan, effective_environment, store,
};
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use crate::modules::mock::MockState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn open(root: &str) -> CmdResult<Project> {
    Project::open(root).map_err(err)
}

pub struct LoadState {
    secrets: Box<dyn SecretStore>,
    /// Runs in progress by caller-chosen id, so the UI can stop them.
    running: Mutex<HashMap<String, RunControl>>,
}

impl LoadState {
    pub fn new() -> Self {
        let secrets: Box<dyn SecretStore> = if devall_core::KeychainSecretStore::available() {
            Box::new(devall_core::KeychainSecretStore::new())
        } else {
            Box::new(MemorySecretStore::default())
        };
        Self { secrets, running: Mutex::new(HashMap::new()) }
    }
}

impl Default for LoadState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------- test files ----------

/// Every `load/**/*.load.yaml` with its run count (unparseable files carry `error`).
#[tauri::command]
pub fn load_list(root: String) -> CmdResult<Vec<LoadTestEntry>> {
    let p = open(&root)?;
    store::list_tests(p.root()).map_err(err)
}

#[tauri::command]
pub fn load_read(root: String, path: String) -> CmdResult<LoadTestFile> {
    let p = open(&root)?;
    store::read_test(p.root(), &path).map_err(err)
}

/// Saves the test (even if it has problems, so drafts persist) and returns its validation issues.
#[tauri::command]
pub fn load_save(root: String, path: String, test: LoadTestFile) -> CmdResult<Vec<Issue>> {
    let p = open(&root)?;
    store::save_test(p.root(), &path, &test).map_err(err)?;
    Ok(devall_load::validate(&test))
}

#[tauri::command]
pub fn load_validate(test: LoadTestFile) -> Vec<Issue> {
    devall_load::validate(&test)
}

/// Creates `load/<slug>.load.yaml`, optionally load-testing `request` (a path under `http/`).
#[tauri::command]
pub fn load_create(root: String, name: String, request: Option<String>) -> CmdResult<String> {
    let p = open(&root)?;
    let template = LoadTestFile::template(name.trim(), request.filter(|r| !r.is_empty()));
    store::create_test(p.root(), &name, Some(template)).map_err(err)
}

/// Moves the test file to the OS trash (its run history stays).
#[tauri::command]
pub fn load_delete(root: String, path: String) -> CmdResult<()> {
    let p = open(&root)?;
    store::delete_test(p.root(), &path).map_err(err)
}

#[tauri::command]
pub fn load_schema() -> serde_json::Value {
    devall_load::schema::load_test_schema()
}

// ---------- running ----------

/// Warnings, target hosts and whether confirmation is needed, without sending anything.
/// `test` is the editor's current state (defaults to the file on disk); `env` is the app's
/// active environment, used when the test doesn't pin one. `against_mock` (or `target: mock`)
/// shows the run as it would go to the project's mock server (without starting it).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn load_preflight(
    state: State<'_, LoadState>,
    mock: State<'_, MockState>,
    root: String,
    path: String,
    test: Option<LoadTestFile>,
    env: Option<String>,
    against_mock: Option<bool>,
) -> CmdResult<Preflight> {
    let p = open(&root)?;
    let test = match test {
        Some(t) => t,
        None => store::read_test(p.root(), &path).map_err(err)?,
    };
    let env = effective_environment(&test, None, env.as_deref());
    let mut plan = build_plan(&p, &path, &test, env.as_deref(), state.secrets.as_ref()).map_err(err)?;
    if against_mock.unwrap_or(false) || test.target == LoadTarget::Mock {
        let url = crate::modules::mock::mock_status(mock, root.clone()).url.unwrap_or_else(|| "http://127.0.0.1".into());
        plan.target_mock(&url).map_err(err)?;
    }
    Ok(plan.preflight())
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[allow(clippy::large_enum_variant)] // returned once per run
pub enum LoadRunResult {
    /// The run finished (possibly stopped early — see `summary.aborted`).
    Completed {
        summary: LoadSummary,
        /// Run id of the saved summary (`load-runs/<test>/<timestamp>.json`, in the project data folder).
        file: Option<String>,
        /// Set when the summary could not be written.
        write_error: Option<String>,
    },
    /// Nothing was sent: ask the user, then call again with `confirm: true`.
    RequiresConfirmation { confirmation: Confirmation, preflight: Preflight },
}

/// Runs the saved test at `path`, streaming snapshots (~4/s) to `on_progress`, then writes the
/// summary in the project data folder (`load-runs/`). `run_id` is chosen by the caller so it can call [`load_stop`].
/// `against_mock` (or `target: mock` in the test) starts the project's mock server if needed,
/// sends the load there and saves the run as a mock baseline.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn load_run(
    state: State<'_, LoadState>,
    mock: State<'_, MockState>,
    root: String,
    path: String,
    run_id: String,
    env: Option<String>,
    confirm: bool,
    against_mock: Option<bool>,
    on_progress: Channel<LoadSnapshot>,
) -> CmdResult<LoadRunResult> {
    let p = open(&root)?;
    let test = store::read_test(p.root(), &path).map_err(err)?;
    let env = effective_environment(&test, None, env.as_deref());
    let mut plan = build_plan(&p, &path, &test, env.as_deref(), state.secrets.as_ref()).map_err(err)?;
    if against_mock.unwrap_or(false) || test.target == LoadTarget::Mock {
        let status = crate::modules::mock::ensure_running(&mock, &root).await?;
        let url = status.url.ok_or("the mock server did not start")?;
        plan.target_mock(&url).map_err(err)?;
    }
    let preflight = plan.preflight();
    if let Some(confirmation) = preflight.requires_confirmation.clone()
        && !confirm
    {
        return Ok(LoadRunResult::RequiresConfirmation { confirmation, preflight });
    }
    let secret_values = plan.secret_values.clone();
    let control = RunControl::new();
    state.running.lock().unwrap().insert(run_id.clone(), control.clone());
    let progress: devall_load::ProgressFn = Arc::new(move |snap: LoadSnapshot| {
        let _ = on_progress.send(snap);
    });
    let result = devall_load::run(plan, RunOptions { run_id: run_id.clone(), ..Default::default() }, Some(progress), control).await;
    state.running.lock().unwrap().remove(&run_id);
    let summary = result.map_err(err)?;
    crate::modules::logs::record_load_run(&root, &summary);
    let (file, write_error) = match store::write_run(p.root(), &summary, &secret_values) {
        Ok(f) => (Some(f), None),
        Err(e) => (None, Some(e.to_string())),
    };
    Ok(LoadRunResult::Completed { summary, file, write_error })
}

// ---------- distributed (load agents) ----------

/// The agents' bearer token lives in the OS keychain under this scope (never in project files).
fn agents_token_scope() -> SecretScope {
    SecretScope::new("_devall-load", "agents")
}

fn stored_agents_token(state: &LoadState) -> Option<String> {
    state.secrets.get(&agents_token_scope(), "token").ok().flatten().filter(|t| !t.trim().is_empty())
}

/// `token`: `Some("")` forgets the stored token, `Some(t)` stores it, `None` only reports.
/// Returns whether a token is stored.
#[tauri::command]
pub fn load_agents_token(state: State<'_, LoadState>, token: Option<String>) -> CmdResult<bool> {
    match token.as_deref().map(str::trim) {
        Some("") => {
            let _ = state.secrets.delete(&agents_token_scope(), "token");
        }
        Some(t) => state.secrets.set(&agents_token_scope(), "token", t).map_err(err)?,
        None => {}
    }
    Ok(stored_agents_token(&state).is_some())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCheck {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<AgentInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn agents_token(state: &LoadState, token: Option<String>) -> CmdResult<String> {
    token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).or_else(|| stored_agents_token(state)).ok_or_else(|| "enter the agents' token (the DEVALL_AGENT_TOKEN they run with)".to_string())
}

/// `GET /v1/info` on every agent (reachable, token accepted, idle, same protocol).
#[tauri::command]
pub async fn load_agents_check(state: State<'_, LoadState>, agents: Vec<AgentTarget>, token: Option<String>) -> CmdResult<Vec<AgentCheck>> {
    let token = agents_token(&state, token)?;
    let client = devall_load::distributed::agent_client(None).map_err(err)?;
    let agents = devall_load::distributed::resolve_agents(agents).await.map_err(err)?;
    let mut out = Vec::new();
    for a in agents {
        match devall_load::distributed::agent_info(&client, &a.address, &token).await {
            Ok(info) => out.push(AgentCheck { address: a.address, info: Some(info), error: None }),
            Err(e) => out.push(AgentCheck { address: a.address, info: None, error: Some(e.to_string()) }),
        }
    }
    Ok(out)
}

/// Runs the saved test across load agents (`devall load agent`), merging their results.
/// `allow_secrets` is the user's explicit consent to send resolved `{{secret.X}}` values to the
/// agents; without it, tests that use secrets are refused. `token` (else the keychain's) is the
/// agents' bearer token; `remember_token` stores it in the OS keychain.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn load_run_distributed(
    state: State<'_, LoadState>,
    root: String,
    path: String,
    run_id: String,
    env: Option<String>,
    confirm: bool,
    agents: Vec<AgentTarget>,
    token: Option<String>,
    remember_token: Option<bool>,
    allow_secrets: bool,
    on_progress: Channel<LoadSnapshot>,
) -> CmdResult<LoadRunResult> {
    let token = agents_token(&state, token)?;
    if remember_token == Some(true) {
        state.secrets.set(&agents_token_scope(), "token", &token).map_err(err)?;
    }
    let p = open(&root)?;
    let test = store::read_test(p.root(), &path).map_err(err)?;
    if test.target == LoadTarget::Mock {
        return Err("this test targets the project's mock server, which runs on this machine — run it locally".into());
    }
    let env = effective_environment(&test, None, env.as_deref());
    let plan = build_plan(&p, &path, &test, env.as_deref(), state.secrets.as_ref()).map_err(err)?;
    let preflight = plan.preflight();
    if let Some(confirmation) = preflight.requires_confirmation.clone()
        && !confirm
    {
        return Ok(LoadRunResult::RequiresConfirmation { confirmation, preflight });
    }
    let secret_values = plan.secret_values.clone();
    let control = RunControl::new();
    state.running.lock().unwrap().insert(run_id.clone(), control.clone());
    let progress: devall_load::ProgressFn = Arc::new(move |snap: LoadSnapshot| {
        let _ = on_progress.send(snap);
    });
    let opts = DistributedOptions { agents, token, allow_secrets, run_id: run_id.clone(), ..Default::default() };
    let result = devall_load::run_distributed(plan, opts, Some(progress), control).await;
    state.running.lock().unwrap().remove(&run_id);
    let summary = result.map_err(err)?;
    crate::modules::logs::record_load_run(&root, &summary);
    let (file, write_error) = match store::write_run(p.root(), &summary, &secret_values) {
        Ok(f) => (Some(f), None),
        Err(e) => (None, Some(e.to_string())),
    };
    Ok(LoadRunResult::Completed { summary, file, write_error })
}

/// Stops a run early; it still produces (and saves) a summary marked `aborted`.
#[tauri::command]
pub fn load_stop(state: State<'_, LoadState>, run_id: String) -> bool {
    match state.running.lock().unwrap().get(&run_id) {
        Some(c) => {
            c.stop();
            true
        }
        None => false,
    }
}

/// Ids of runs in progress.
#[tauri::command]
pub fn load_running(state: State<'_, LoadState>) -> Vec<String> {
    state.running.lock().unwrap().keys().cloned().collect()
}

// ---------- history ----------

/// Saved runs of the test at `path`, newest first.
#[tauri::command]
pub fn load_runs(root: String, path: String) -> CmdResult<Vec<RunEntry>> {
    let p = open(&root)?;
    store::list_runs(p.root(), &path).map_err(err)
}

/// `file` is relative to the project root (as returned by `load_runs`).
#[tauri::command]
pub fn load_read_run(root: String, file: String) -> CmdResult<LoadSummary> {
    let p = open(&root)?;
    store::read_run(p.root(), &file).map_err(err)
}

/// Moves a run summary to the OS trash.
#[tauri::command]
pub fn load_delete_run(root: String, file: String) -> CmdResult<()> {
    let p = open(&root)?;
    store::delete_run(p.root(), &file).map_err(err)
}

/// Compares run `b` (target) against run `a` (base). `tolerance` is relative (0.1 = 10%).
#[tauri::command]
pub fn load_compare(root: String, a: String, b: String, tolerance: Option<f64>) -> CmdResult<Comparison> {
    let p = open(&root)?;
    let base = store::read_run(p.root(), &a).map_err(err)?;
    let target = store::read_run(p.root(), &b).map_err(err)?;
    let opts = CompareOptions { tolerance: tolerance.filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(CompareOptions::default().tolerance) };
    Ok(devall_load::compare(&base, &target, opts))
}

// ---------- cloud regions (the relay's load generators) ----------

/// What a cloud run of this test would send, before anything is sent: the origins that must be
/// verified on the relay, requests needing secrets, and warnings (dynamic values, TLS…).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPrepareView {
    pub origins: Vec<String>,
    pub secret_requests: Vec<String>,
    pub warnings: Vec<String>,
    /// Why this test can't run from cloud regions at all (closed-model executor, mock target…).
    pub error: Option<String>,
    pub planned_duration_ms: u64,
}

fn cloud_plan_for(state: &LoadState, root: &str, path: &str, env: Option<&str>) -> CmdResult<devall_load::Plan> {
    let p = open(root)?;
    let test = store::read_test(p.root(), path).map_err(err)?;
    let env = effective_environment(&test, None, env);
    build_plan(&p, path, &test, env.as_deref(), state.secrets.as_ref()).map_err(err)
}

#[tauri::command]
pub fn load_cloud_prepare(state: State<'_, LoadState>, root: String, path: String, env: Option<String>) -> CmdResult<CloudPrepareView> {
    let plan = cloud_plan_for(&state, &root, &path, env.as_deref())?;
    let secret_requests = plan.secret_request_paths();
    // Secrets allowed here only to list every warning; nothing is sent.
    let opts = devall_load::cloud::CloudOptions { regions: vec!["weur".into()], allow_secrets: true, run_id: "prepare".into() };
    let planned_duration_ms = plan.test.executor.planned_duration().as_millis() as u64;
    Ok(match plan.cloud_plan(&opts) {
        Ok(p) => CloudPrepareView { origins: p.origins, secret_requests, warnings: p.warnings, error: None, planned_duration_ms },
        Err(e) => CloudPrepareView { origins: plan.request_origins(), secret_requests, warnings: Vec::new(), error: Some(e.to_string()), planned_duration_ms },
    })
}

/// `GET /v1/load`: whether cloud load is on, regions, plan limits and the account's targets.
#[tauri::command]
pub async fn load_cloud_info(relay: State<'_, crate::modules::relay::RelayState>) -> CmdResult<serde_json::Value> {
    relay.client()?.api(devall_relay::ApiMethod::GET, "/v1/load", None).await.map_err(err)
}

/// Adds a target (or returns it): its verification token, file URL and DNS name.
#[tauri::command]
pub async fn load_cloud_target(relay: State<'_, crate::modules::relay::RelayState>, target: String) -> CmdResult<serde_json::Value> {
    relay.client()?.api(devall_relay::ApiMethod::POST, "/v1/load/targets", Some(&serde_json::json!({ "target": target }))).await.map_err(err)
}

/// Checks the target's token (HTTP file, then DNS TXT).
#[tauri::command]
pub async fn load_cloud_verify(relay: State<'_, crate::modules::relay::RelayState>, target: String) -> CmdResult<serde_json::Value> {
    relay.client()?.api(devall_relay::ApiMethod::POST, "/v1/load/targets/verify", Some(&serde_json::json!({ "target": target }))).await.map_err(err)
}

#[tauri::command]
pub async fn load_cloud_remove(relay: State<'_, crate::modules::relay::RelayState>, target: String) -> CmdResult<()> {
    let q = url::form_urlencoded::Serializer::new(String::new()).append_pair("target", &target).finish();
    relay.client()?.api(devall_relay::ApiMethod::DELETE, &format!("/v1/load/targets?{q}"), None).await.map(|_| ()).map_err(err)
}

/// Runs the test from the relay's cloud regions (Supporter / self-hosted relay). The relay
/// answers when the run is over; meanwhile `on_progress` gets elapsed-time snapshots, and
/// `load_stop(run_id)` asks the relay to stop at its next segment. The summary is saved like a
/// local run (regions are listed as `agents`).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn load_cloud_run(
    state: State<'_, LoadState>,
    relay: State<'_, crate::modules::relay::RelayState>,
    root: String,
    path: String,
    run_id: String,
    env: Option<String>,
    regions: Vec<String>,
    allow_secrets: bool,
    on_progress: Channel<LoadSnapshot>,
) -> CmdResult<LoadRunResult> {
    let p = open(&root)?;
    let plan = cloud_plan_for(&state, &root, &path, env.as_deref())?;
    let relay_run_id: String = run_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(40).collect();
    let relay_run_id = if relay_run_id.len() >= 8 { relay_run_id } else { String::new() };
    let prepared = plan.cloud_plan(&devall_load::cloud::CloudOptions { regions, allow_secrets, run_id: relay_run_id }).map_err(err)?;
    let client = relay.client()?;
    let body = serde_json::to_value(&prepared.plan).map_err(err)?;
    let planned = plan.test.executor.planned_duration();
    let control = RunControl::new();
    state.running.lock().unwrap().insert(run_id.clone(), control.clone());
    // Progress (elapsed time only — the edge reports once, at the end) and the stop button.
    let ticker = {
        let (client, control, run_id, cloud_id) = (client.clone(), control.clone(), run_id.clone(), prepared.plan.run_id.clone());
        let planned_ms = planned.as_millis() as u64;
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let mut stop_sent = false;
            loop {
                if control.is_stopped() && !stop_sent {
                    stop_sent = true;
                    let _ = client.api(devall_relay::ApiMethod::POST, &format!("/v1/load/runs/{cloud_id}/stop"), None).await;
                }
                let elapsed = started.elapsed().as_millis() as u64;
                let phase = if stop_sent || elapsed >= planned_ms { devall_load::RunPhase::Stopping } else { devall_load::RunPhase::Running };
                let _ = on_progress.send(cloud_snapshot(&run_id, phase, elapsed, planned_ms));
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        })
    };
    let result = client.api_with_timeout(devall_relay::ApiMethod::POST, "/v1/load/runs", Some(&body), planned + std::time::Duration::from_secs(120)).await;
    ticker.abort();
    state.running.lock().unwrap().remove(&run_id);
    let value = result.map_err(err)?;
    let cloud: devall_load::cloud::CloudResult = serde_json::from_value(value).map_err(|e| format!("unexpected answer from the relay: {e}"))?;
    let secret_values = plan.secret_values.clone();
    let summary = plan.cloud_summary(cloud, &prepared.warnings);
    crate::modules::logs::record_load_run(&root, &summary);
    let (file, write_error) = match store::write_run(p.root(), &summary, &secret_values) {
        Ok(f) => (Some(f), None),
        Err(e) => (None, Some(e.to_string())),
    };
    Ok(LoadRunResult::Completed { summary, file, write_error })
}

fn cloud_snapshot(run_id: &str, phase: devall_load::RunPhase, elapsed_ms: u64, planned_ms: u64) -> LoadSnapshot {
    LoadSnapshot {
        run_id: run_id.to_string(),
        phase,
        elapsed_ms,
        planned_duration_ms: planned_ms,
        vus: 0,
        vus_allocated: 0,
        requests: 0,
        failed: 0,
        iterations: 0,
        dropped_iterations: 0,
        rps: 0.0,
        latency: Default::default(),
        bytes_in: 0,
        bytes_out: 0,
        status_codes: Default::default(),
        error_kinds: Default::default(),
        points: Vec::new(),
        bucket_seconds: 1,
    }
}
