//! Local AI (providers, discovery, managed llama.cpp sidecar, model downloads, project-aware
//! features) and the MCP server for coding agents — a thin layer over `crates/devall-ai` and
//! `crates/devall-mcp`.
//!
//! # Integration (for whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    ```toml
//!    devall-ai = { path = "../../../crates/devall-ai" }
//!    devall-mcp = { path = "../../../crates/devall-mcp" }
//!    ```
//!    (`tauri`, `tokio`, `serde`, `serde_json`, `devall-core` are already dependencies.)
//! 2. `src/modules/mod.rs`: `pub mod ai;`
//! 3. `src/lib.rs`, in `.setup(|app| …)` after `data_dir` is known:
//!    `app.manage(modules::ai::AiState::new(data_dir.clone()));`
//!    and on exit nothing extra is needed: the sidecar child is `kill_on_drop`.
//! 4. `src/lib.rs`, inside `tauri::generate_handler![…]`:
//!    ```text
//!    modules::ai::ai_providers, modules::ai::ai_save_provider, modules::ai::ai_remove_provider,
//!    modules::ai::ai_settings, modules::ai::ai_test_provider, modules::ai::ai_discover,
//!    modules::ai::ai_hardware, modules::ai::ai_catalog, modules::ai::ai_download_model,
//!    modules::ai::ai_install_runtime, modules::ai::ai_cancel_download, modules::ai::ai_downloads, modules::ai::ai_delete_model,
//!    modules::ai::ai_sidecar_start, modules::ai::ai_sidecar_stop, modules::ai::ai_sidecar_status,
//!    modules::ai::ai_preview, modules::ai::ai_ask, modules::ai::ai_cancel, modules::ai::ai_run,
//!    modules::ai::ai_nl_regex, modules::ai::ai_nl_jsonpath, modules::ai::ai_nl_jq, modules::ai::ai_nl_request,
//!    modules::ai::ai_mock_data, modules::ai::ai_explain_response, modules::ai::ai_explain_error,
//!    modules::ai::ai_suggest_tests, modules::ai::ai_summarize_notes, modules::ai::ai_todos_from_text,
//!    modules::ai::ai_failure_context, modules::ai::ai_openapi_names,
//!    modules::ai::ai_embedding_settings, modules::ai::ai_embed_sidecar_start, modules::ai::ai_embed_sidecar_stop,
//!    modules::ai::ai_index_status, modules::ai::ai_index_build, modules::ai::ai_index_clear, modules::ai::ai_search,
//!    (ai_run also runs nlSql / nlMongo / nlKubectl with live checks: EXPLAIN, explain, dry run)
//!    modules::ai::mcp_status, modules::ai::mcp_config_snippet, modules::ai::mcp_http_start,
//!    modules::ai::mcp_http_stop, modules::ai::mcp_approvals_pending, modules::ai::mcp_approval_answer,
//!    ```
//! 5. Release bundles ship `llama-server` as a Tauri `externalBin`: `npm run app:bundle` runs
//!    `scripts/fetch-llama-server.mjs` (pinned build, SHA-256 from `crates/devall-ai/llama-assets.json`)
//!    and builds with the `src-tauri/tauri.bundle.conf.json` overlay — not in `tauri.conf.json`,
//!    because tauri-build copies external binaries on every build and would fail `cargo check` /
//!    `tauri dev` without the file. It is then found next to the app executable; otherwise users
//!    install it from AI → Models. Sign it with the app (docs/RELEASING.md): Windows Defender has
//!    repeatedly flagged unsigned llama.cpp builds.
//! 6. Frontend: route `ai` → `features/ai/AiPage` (default export); wrappers in `src/lib/api/ai.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;
use std::time::Duration;

use devall_ai::catalog::{self, CatalogModel, LlamaAsset, Recommendation};
use devall_ai::config::{self, AiConfig};
use devall_ai::discover::{self, DiscoveredServer};
use devall_ai::download::{CancelFlag, DownloadState, Outcome, Progress, Registry as DownloadRegistry};
use devall_ai::embed::{self, Embed, EmbeddingSettings, MANAGED_EMBED_ID};
use devall_ai::semantic::{self, BuildOutcome, BuildProgress, IndexStatus, SearchHit, SearchMode, SearchResult};
use devall_ai::features::{
    self, AskAnswer, ErrorExplanation, FeatureInput, JqSuggestion, JsonPathSuggestion, MockData, NotesSummary, Preview, RegexSuggestion, RequestDraft, ResponseContext, ResponseExplanation,
    TestSuggestions, TodoSuggestions,
};
use devall_ai::hardware::{self, Hardware, LlamaVariant, Tier, VariantChoice};
use devall_ai::sidecar::{self, Installed, MANAGED_PROVIDER_ID, ModelFileStatus, Sidecar, SidecarInfo, SidecarOptions};
use devall_ai::{AiEnv, FailureContext, FailureSource, Provider, ProviderConfig, Redactor};
use devall_core::Project;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub struct AiState {
    data_dir: PathBuf,
    /// Serialises read-modify-write of `ai.json`.
    config_lock: Mutex<()>,
    sidecar: tokio::sync::Mutex<Option<Sidecar>>,
    /// Active downloads by model id (or `runtime:<asset suffix>`): progress for every view, and
    /// cancelling. Progress is broadcast as `ai://download` events.
    downloads: Arc<DownloadRegistry>,
    /// In-flight AI calls by client id, for cancelling.
    calls: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    hardware: Mutex<Option<Hardware>>,
    mcp_http: tokio::sync::Mutex<Option<(String, devall_mcp::HttpServer)>>,
    /// MCP calls waiting for the user's approval (see [`AppConfirmer`]).
    approvals: Arc<ApprovalHub>,
    /// The managed embedding model (`llama-server --embedding`) with its catalog id.
    embed_sidecar: Arc<tokio::sync::Mutex<Option<(String, Sidecar)>>>,
    /// Semantic-index builds by client id, for cancelling (progress is kept).
    builds: Mutex<HashMap<String, CancelFlag>>,
}

impl AiState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            config_lock: Mutex::new(()),
            sidecar: tokio::sync::Mutex::new(None),
            downloads: Arc::default(),
            calls: Mutex::new(HashMap::new()),
            hardware: Mutex::new(None),
            mcp_http: tokio::sync::Mutex::new(None),
            approvals: Arc::default(),
            embed_sidecar: Arc::default(),
            builds: Mutex::new(HashMap::new()),
        }
    }

    /// The embedder for semantic search.
    async fn embedder(&self, app: &AppState) -> CmdResult<Arc<dyn Embed>> {
        resolve_embedder(&self.data_dir, app.secrets.as_ref(), &self.embed_sidecar).await
    }

    fn config(&self) -> CmdResult<AiConfig> {
        AiConfig::load(&self.data_dir).map_err(err)
    }

    fn log_dir(&self) -> PathBuf {
        sidecar::ai_dir(&self.data_dir).join("logs")
    }

    /// The provider to use: an explicit id, else the configured default, else the first
    /// configured provider, else the running managed sidecar.
    async fn provider(&self, app: &AppState, id: Option<&str>) -> CmdResult<Provider> {
        let cfg = self.config()?;
        let managed = || async { self.sidecar.lock().await.as_ref().map(Sidecar::provider) };
        let want = id.filter(|s| !s.is_empty()).map(str::to_string).or_else(|| cfg.default_provider.clone());
        if want.as_deref() == Some(MANAGED_PROVIDER_ID) {
            return managed().await.ok_or_else(|| "the managed llama.cpp model is not running — start it in AI → Models".to_string());
        }
        match cfg.resolve(want.as_deref()) {
            Ok(p) => config::provider(p, app.secrets.as_ref()).map_err(err),
            Err(devall_ai::AiError::NoProvider) => managed().await.ok_or_else(|| err(devall_ai::AiError::NoProvider)),
            Err(e) => Err(err(e)),
        }
    }
}

/// `ai.json` → `embeddings` (a configured local provider, or the managed embedding model); with
/// nothing configured, the managed embedding model when it runs. Cloud endpoints are refused
/// unless "Allow project context for cloud providers" is on.
async fn resolve_embedder(data_dir: &Path, store: &dyn devall_core::SecretStore, managed: &tokio::sync::Mutex<Option<(String, Sidecar)>>) -> CmdResult<Arc<dyn Embed>> {
    let cfg = AiConfig::load(data_dir).map_err(err)?;
    let running = async {
        let mut g = managed.lock().await;
        match g.as_mut() {
            Some((id, s)) => s.running().then(|| Arc::new(s.embedder(id)) as Arc<dyn Embed>),
            None => None,
        }
    };
    match cfg.embeddings.clone() {
        Some(s) if s.provider == MANAGED_EMBED_ID => running.await.ok_or_else(|| "the managed embedding model is not running — start it in AI → Search".to_string()),
        Some(s) => Ok(Arc::new(embed::embedder_for(&cfg, &s, store).map_err(err)?)),
        None => running.await.ok_or_else(|| "choose an embedding model in AI → Search (Ollama, LM Studio, llama.cpp or the managed embedding model)".to_string()),
    }
}

/// Query embeddings for the in-app MCP endpoint's `search_project` (sees the managed model too).
struct AppEmbedderSource {
    data_dir: PathBuf,
    managed: Arc<tokio::sync::Mutex<Option<(String, Sidecar)>>>,
    store: Box<dyn devall_core::SecretStore>,
}

impl devall_mcp::EmbedderSource for AppEmbedderSource {
    fn embedder(&self) -> embed::EmbedderFuture<'_> {
        Box::pin(async move { resolve_embedder(&self.data_dir, self.store.as_ref(), &self.managed).await.ok() })
    }
}

/// Semantic hits that rank Ask's sources, when the project has an index built with the current
/// embedding model (best effort: keyword ranking otherwise).
async fn ask_hits(state: &AiState, app: &AppState, root: &str, question: &str) -> Vec<SearchHit> {
    if !semantic::index_dir(Path::new(root)).join("index.json").is_file() || question.trim().is_empty() {
        return Vec::new();
    }
    let Ok(e) = state.embedder(app).await else { return Vec::new() };
    match semantic::search(Path::new(root), question, 12, Some(e.as_ref()), || Ok(Vec::new())).await {
        Ok(r) if r.mode == SearchMode::Hybrid => r.hits,
        _ => Vec::new(),
    }
}

fn open_project(root: Option<&str>) -> CmdResult<Option<Project>> {
    match root.filter(|r| !r.is_empty()) {
        Some(r) => Project::open(r).map(Some).map_err(err),
        None => Ok(None),
    }
}

fn redactor(app: &AppState, project: Option<&Project>) -> Redactor {
    project.map(|p| Redactor::for_project(p, app.secrets.as_ref())).unwrap_or_default()
}

// ── Providers ─────────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersView {
    pub config: AiConfig,
    /// The managed llama.cpp sidecar, when running (usable as provider id `managed-llama`).
    pub managed: Option<ProviderConfig>,
    pub keychain_available: bool,
}

#[tauri::command]
pub async fn ai_providers(state: State<'_, AiState>, app: State<'_, AppState>) -> CmdResult<ProvidersView> {
    let managed = state.sidecar.lock().await.as_ref().map(|s| s.provider().config);
    Ok(ProvidersView { config: state.config()?, managed, keychain_available: app.keychain_available })
}

/// Saves a provider. `api_key`: `None` keeps the stored key, `""` deletes it, otherwise it is
/// stored in the OS keychain (never in `ai.json`).
#[tauri::command]
pub fn ai_save_provider(state: State<'_, AiState>, app: State<'_, AppState>, provider: ProviderConfig, api_key: Option<String>, make_default: Option<bool>) -> CmdResult<AiConfig> {
    let _g = state.config_lock.lock().unwrap();
    let id = provider.id.clone();
    let mut cfg = config::save_provider(&state.data_dir, app.secrets.as_ref(), provider, api_key).map_err(err)?;
    if make_default == Some(true) {
        cfg.default_provider = Some(id);
        cfg.save(&state.data_dir).map_err(err)?;
    }
    Ok(cfg)
}

#[tauri::command]
pub fn ai_remove_provider(state: State<'_, AiState>, app: State<'_, AppState>, id: String) -> CmdResult<AiConfig> {
    let _g = state.config_lock.lock().unwrap();
    config::remove_provider(&state.data_dir, app.secrets.as_ref(), &id).map_err(err)
}

/// Default provider (may be `managed-llama`) and the cloud-context opt-in.
#[tauri::command]
pub fn ai_settings(state: State<'_, AiState>, default_provider: Option<String>, allow_remote_context: Option<bool>) -> CmdResult<AiConfig> {
    let _g = state.config_lock.lock().unwrap();
    let mut cfg = state.config()?;
    if let Some(d) = default_provider {
        cfg.default_provider = (!d.is_empty()).then_some(d);
    }
    if let Some(a) = allow_remote_context {
        cfg.allow_remote_context = a;
    }
    cfg.save(&state.data_dir).map_err(err)?;
    Ok(cfg)
}

/// Test connection: lists the endpoint's models. Pass an unsaved `provider` (+ `api_key`) to test
/// before saving, or an `id`.
#[tauri::command]
pub async fn ai_test_provider(
    state: State<'_, AiState>,
    app: State<'_, AppState>,
    id: Option<String>,
    provider: Option<ProviderConfig>,
    api_key: Option<String>,
) -> CmdResult<Vec<String>> {
    let p = match provider {
        Some(cfg) => {
            let key = match api_key.filter(|k| !k.is_empty()) {
                Some(k) => Some(k),
                None if cfg.has_api_key => config::load_api_key(app.secrets.as_ref(), &cfg.id).map_err(err)?,
                None => None,
            };
            Provider::new(cfg, key).map_err(err)?
        }
        None => state.provider(&app, id.as_deref()).await?,
    };
    p.list_models().await.map_err(err)
}

#[tauri::command]
pub async fn ai_discover() -> Vec<DiscoveredServer> {
    discover::discover().await
}

// ── Hardware, catalog, downloads ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareView {
    pub hardware: Hardware,
    pub tier: Tier,
    pub tier_label: &'static str,
    pub recommendation: Recommendation,
    pub llama: VariantChoice,
    pub llama_download_bytes: u64,
    /// Download key of the recommended llama.cpp runtime (see `ai_downloads`).
    pub llama_key: String,
}

#[tauri::command]
pub async fn ai_hardware(state: State<'_, AiState>, refresh: Option<bool>) -> CmdResult<HardwareView> {
    let cached = if refresh == Some(true) { None } else { state.hardware.lock().unwrap().clone() };
    let hw = match cached {
        Some(h) => h,
        None => {
            let h = tokio::task::spawn_blocking(hardware::detect).await.map_err(err)?;
            *state.hardware.lock().unwrap() = Some(h.clone());
            h
        }
    };
    let tier = hardware::tier(&hw);
    let llama = hardware::llama_variant(&hw);
    let llama_download_bytes = catalog::llama_asset(llama.variant).map(|a| a.total_bytes()).unwrap_or(0);
    let llama_key = runtime_key(llama.variant);
    Ok(HardwareView { tier, tier_label: tier.label(), recommendation: catalog::recommend(tier), llama, llama_download_bytes, llama_key, hardware: hw })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    #[serde(flatten)]
    pub model: CatalogModel,
    pub status: ModelFileStatus,
    pub downloading: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    pub models: Vec<CatalogEntry>,
    pub llama_build: &'static str,
    pub installed: Vec<Installed>,
    pub runtimes: Vec<LlamaAsset>,
    /// Resolved `llama-server` binary (env override, bundled, or installed), if any.
    pub server: Option<PathBuf>,
    /// Downloads in flight right now.
    pub downloads: Vec<DownloadState>,
}

fn bundled_server() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(sidecar::server_exe_name());
    p.is_file().then_some(p)
}

#[tauri::command]
pub fn ai_catalog(state: State<'_, AiState>) -> CatalogView {
    let models = catalog::models()
        .into_iter()
        .map(|m| CatalogEntry { status: sidecar::model_status(&state.data_dir, &m), downloading: state.downloads.is_active(m.id), model: m })
        .collect();
    let preferred = state.hardware.lock().unwrap().as_ref().map(|h| hardware::llama_variant(h).variant);
    CatalogView {
        models,
        llama_build: catalog::LLAMA_CPP_BUILD,
        installed: sidecar::installed(&state.data_dir),
        runtimes: catalog::llama_assets(),
        server: sidecar::resolve_server(&state.data_dir, None, bundled_server().as_deref(), preferred),
        downloads: state.downloads.snapshot(),
    }
}

/// Event carrying a [`DownloadState`] whenever a download progresses or ends.
pub const DOWNLOAD_EVENT: &str = "ai://download";

fn runtime_key(variant: LlamaVariant) -> String {
    format!("runtime:{}", variant.asset_suffix())
}

/// Every running download (model and runtime), for views that mount while one is in flight.
#[tauri::command]
pub fn ai_downloads(state: State<'_, AiState>) -> Vec<DownloadState> {
    state.downloads.snapshot()
}

type ProgressSink = Box<dyn FnMut(&Progress) + Send>;

/// Runs `job` in the background, registered under `key`, broadcasting its progress. Returns the
/// initial state at once; when `key` is already downloading, returns its live state instead of
/// starting a second copy (e.g. "Resume" clicked in a view that mounted mid-download).
fn spawn_download<T, F, Fut>(app: &AppHandle, registry: &Arc<DownloadRegistry>, key: String, already: u64, total: Option<u64>, job: F, path: fn(&T) -> String) -> DownloadState
where
    T: Send + 'static,
    F: FnOnce(CancelFlag, ProgressSink) -> Fut,
    Fut: std::future::Future<Output = devall_ai::Result<T>> + Send + 'static,
{
    let (cancel, first) = match registry.begin(&key, already, total) {
        Ok(v) => v,
        Err(live) => return live,
    };
    let _ = app.emit(DOWNLOAD_EVENT, &first);
    let (reg, app2, key2) = (registry.clone(), app.clone(), key.clone());
    let on_progress: ProgressSink = Box::new(move |p: &Progress| {
        if let Some(st) = reg.progress(&key2, p) {
            let _ = app2.emit(DOWNLOAD_EVENT, &st);
        }
    });
    let fut = job(cancel, on_progress);
    let (reg, app) = (registry.clone(), app.clone());
    tauri::async_runtime::spawn(async move {
        let outcome = match fut.await {
            Ok(v) => Outcome::Done(path(&v)),
            Err(e) => Outcome::Failed(e),
        };
        let end = reg.finish(&key, outcome);
        let _ = app.emit(DOWNLOAD_EVENT, &end);
    });
    first
}

/// Starts (or resumes) downloading a catalog model in the background (explicit user action only).
/// Progress arrives as `ai://download` events; see also `ai_downloads`.
#[tauri::command]
pub fn ai_download_model(app: AppHandle, state: State<'_, AiState>, model: String) -> CmdResult<DownloadState> {
    let m = catalog::find(&model).ok_or_else(|| format!("unknown model `{model}`"))?;
    let already = sidecar::model_status(&state.data_dir, &m).partial_bytes;
    let data_dir = state.data_dir.clone();
    let size = m.size_bytes;
    Ok(spawn_download(
        &app,
        &state.downloads,
        model,
        already,
        Some(size),
        move |cancel, mut on_progress| async move { sidecar::download_model(&data_dir, &m, &cancel, |p| on_progress(p)).await },
        |p: &PathBuf| p.display().to_string(),
    ))
}

/// Downloads and installs the pinned llama.cpp build in the background (`variant` defaults to the
/// one detected for this machine). Progress arrives as `ai://download` events.
#[tauri::command]
pub async fn ai_install_runtime(app: AppHandle, state: State<'_, AiState>, variant: Option<LlamaVariant>) -> CmdResult<DownloadState> {
    let variant = match variant {
        Some(v) => v,
        None => {
            let cached = state.hardware.lock().unwrap().clone();
            let hw = match cached {
                Some(h) => h,
                None => tokio::task::spawn_blocking(hardware::detect).await.map_err(err)?,
            };
            hardware::llama_variant(&hw).variant
        }
    };
    let total = catalog::llama_asset(variant).map(|a| a.total_bytes());
    let data_dir = state.data_dir.clone();
    Ok(spawn_download(
        &app,
        &state.downloads,
        runtime_key(variant),
        0,
        total,
        move |cancel, mut on_progress| async move { sidecar::install(&data_dir, variant, &cancel, |p| on_progress(p)).await },
        |i: &Installed| i.server.display().to_string(),
    ))
}

/// Pauses a download (`key` = model id or a runtime key); the partial file is kept.
#[tauri::command]
pub fn ai_cancel_download(state: State<'_, AiState>, model: String) -> bool {
    state.downloads.cancel(&model)
}

/// Moves a downloaded model to the OS trash.
#[tauri::command]
pub async fn ai_delete_model(state: State<'_, AiState>, model: String) -> CmdResult<()> {
    let m = catalog::find(&model).ok_or_else(|| format!("unknown model `{model}`"))?;
    if state.downloads.is_active(&model) {
        return Err(format!("{} is downloading — pause it first", m.name));
    }
    if let Some(s) = state.sidecar.lock().await.as_ref()
        && s.info.model_path == sidecar::model_path(&state.data_dir, &m)
    {
        return Err("stop the managed model before deleting it".into());
    }
    if state.embed_sidecar.lock().await.as_ref().is_some_and(|(_, s)| s.info.model_path == sidecar::model_path(&state.data_dir, &m)) {
        return Err("stop the managed embedding model before deleting it".into());
    }
    sidecar::delete_model(&state.data_dir, &m).map_err(err)
}

// ── Managed llama.cpp sidecar ─────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStatus {
    pub running: bool,
    pub info: Option<SidecarInfo>,
    pub server: Option<PathBuf>,
}

/// Starts `llama-server` with a catalog model id (or an absolute `.gguf` path).
#[tauri::command]
pub async fn ai_sidecar_start(state: State<'_, AiState>, model: String, options: Option<SidecarOptions>) -> CmdResult<SidecarStatus> {
    let (model_path, mut opts) = match catalog::find(&model) {
        Some(m) => {
            let st = sidecar::model_status(&state.data_dir, &m);
            if !st.downloaded {
                return Err(format!("{} isn't downloaded yet", m.name));
            }
            let mut o = options.unwrap_or_default();
            o.context = o.context.min(m.context.max(4096)).max(2048);
            o.alias = o.alias.or_else(|| Some(m.id.to_string()));
            (st.path, o)
        }
        None if model.to_lowercase().ends_with(".gguf") && Path::new(&model).is_file() => (PathBuf::from(&model), options.unwrap_or_default()),
        None => return Err(format!("unknown model `{model}`")),
    };
    let preferred = state.hardware.lock().unwrap().as_ref().map(|h| hardware::llama_variant(h).variant);
    let server = sidecar::resolve_server(&state.data_dir, None, bundled_server().as_deref(), preferred)
        .ok_or("llama.cpp isn't installed — install the runtime in AI → Models (or connect Ollama / LM Studio)")?;
    if let Some(inst) = sidecar::installed(&state.data_dir).into_iter().find(|i| i.server == server) {
        opts.gpu = inst.variant.gpu();
    }
    let mut guard = state.sidecar.lock().await;
    if let Some(mut old) = guard.take() {
        old.stop().await;
    }
    let s = Sidecar::start(&server, &model_path, &opts, &state.log_dir(), Duration::from_secs(300)).await.map_err(err)?;
    let info = s.info.clone();
    *guard = Some(s);
    Ok(SidecarStatus { running: true, info: Some(info), server: Some(server) })
}

#[tauri::command]
pub async fn ai_sidecar_stop(state: State<'_, AiState>) -> CmdResult<()> {
    if let Some(mut s) = state.sidecar.lock().await.take() {
        s.stop().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn ai_sidecar_status(state: State<'_, AiState>) -> CmdResult<SidecarStatus> {
    let mut guard = state.sidecar.lock().await;
    if guard.as_mut().is_some_and(|s| !s.running()) {
        guard.take();
    }
    let preferred = state.hardware.lock().unwrap().as_ref().map(|h| hardware::llama_variant(h).variant);
    Ok(SidecarStatus {
        running: guard.is_some(),
        info: guard.as_ref().map(|s| s.info.clone()),
        server: sidecar::resolve_server(&state.data_dir, None, bundled_server().as_deref(), preferred),
    })
}

// ── Features ──────────────────────────────────────────────────────────────────────────────────

/// "What will be sent" for any feature: the exact payload after redaction.
#[tauri::command]
pub async fn ai_preview(state: State<'_, AiState>, app: State<'_, AppState>, root: Option<String>, feature: String, input: Value, provider: Option<String>) -> CmdResult<Preview> {
    let p = state.provider(&app, provider.as_deref()).await?;
    let project = open_project(root.as_deref())?;
    let cfg = state.config()?;
    let input = FeatureInput::from_parts(&feature, input).map_err(err)?;
    let hits = match (&input, root.as_deref()) {
        (FeatureInput::Ask { question }, Some(r)) => ask_hits(&state, &app, r, question).await,
        _ => Vec::new(),
    };
    let env = AiEnv::new(project.as_ref(), Some(&state.data_dir), redactor(&app, project.as_ref()), cfg.allow_remote_context).with_semantic(hits);
    features::preview(&input, &env, &p).map_err(err)
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AskEvent {
    Token { text: String },
}

/// Streams an answer over notes, to-dos and requests. `call_id` lets the UI cancel it.
#[tauri::command]
pub async fn ai_ask(
    state: State<'_, AiState>,
    app: State<'_, AppState>,
    root: String,
    question: String,
    provider: Option<String>,
    call_id: Option<String>,
    on_token: Channel<AskEvent>,
) -> CmdResult<AskAnswer> {
    let p = state.provider(&app, provider.as_deref()).await?;
    let project = Project::open(&root).map_err(err)?;
    let red = redactor(&app, Some(&project));
    let allow_remote = state.config()?.allow_remote_context;
    let data_dir = state.data_dir.clone();
    let hits = ask_hits(&state, &app, &root, &question).await;
    let task = tokio::spawn(async move {
        let env = AiEnv::new(Some(&project), Some(&data_dir), red, allow_remote).with_semantic(hits);
        features::ask_stream(&env, &p, &question, |t| {
            let _ = on_token.send(AskEvent::Token { text: t.to_string() });
        })
        .await
    });
    run_cancellable(&state, call_id, task).await
}

async fn run_cancellable<T>(state: &AiState, call_id: Option<String>, task: tokio::task::JoinHandle<devall_ai::Result<T>>) -> CmdResult<T> {
    if let Some(id) = &call_id {
        state.calls.lock().unwrap().insert(id.clone(), task.abort_handle());
    }
    let res = task.await;
    if let Some(id) = &call_id {
        state.calls.lock().unwrap().remove(id);
    }
    match res {
        Ok(r) => r.map_err(err),
        Err(e) if e.is_cancelled() => Err("cancelled".into()),
        Err(e) => Err(err(e)),
    }
}

#[tauri::command]
pub fn ai_cancel(state: State<'_, AiState>, call_id: String) -> bool {
    if let Some(flag) = state.builds.lock().unwrap().get(&call_id) {
        flag.store(true, Ordering::SeqCst);
        return true;
    }
    match state.calls.lock().unwrap().remove(&call_id) {
        Some(h) => {
            h.abort();
            true
        }
        None => false,
    }
}

// ── Semantic search ───────────────────────────────────────────────────────────────────────────

/// Sets the embedding model (`provider` = a configured provider id or `managed-embed`; an empty
/// provider clears the choice).
#[tauri::command]
pub fn ai_embedding_settings(state: State<'_, AiState>, provider: String, model: Option<String>) -> CmdResult<AiConfig> {
    let _g = state.config_lock.lock().unwrap();
    let mut cfg = state.config()?;
    cfg.embeddings = (!provider.trim().is_empty()).then(|| EmbeddingSettings { provider: provider.trim().to_string(), model: model.unwrap_or_default().trim().to_string() });
    cfg.save(&state.data_dir).map_err(err)?;
    Ok(cfg)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedSidecarStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<SidecarInfo>,
}

async fn embed_status(state: &AiState) -> EmbedSidecarStatus {
    let mut g = state.embed_sidecar.lock().await;
    if g.as_mut().is_some_and(|(_, s)| !s.running()) {
        g.take();
    }
    EmbedSidecarStatus { running: g.is_some(), model: g.as_ref().map(|(m, _)| m.clone()), info: g.as_ref().map(|(_, s)| s.info.clone()) }
}

/// Starts `llama-server --embedding` with a downloaded catalog embedding model (EmbeddingGemma 2,
/// Qwen3-Embedding) next to the chat model.
#[tauri::command]
pub async fn ai_embed_sidecar_start(state: State<'_, AiState>, model: String) -> CmdResult<EmbedSidecarStatus> {
    let m = catalog::find(&model).filter(|m| m.role == catalog::ModelRole::Embedding).ok_or_else(|| format!("`{model}` is not an embedding model in the catalog"))?;
    let st = sidecar::model_status(&state.data_dir, &m);
    if !st.downloaded {
        return Err(format!("{} isn't downloaded yet", m.name));
    }
    let preferred = state.hardware.lock().unwrap().as_ref().map(|h| hardware::llama_variant(h).variant);
    let server = sidecar::resolve_server(&state.data_dir, None, bundled_server().as_deref(), preferred)
        .ok_or("llama.cpp isn't installed — install the runtime in AI → Models (or use Ollama / LM Studio for embeddings)")?;
    let gpu = sidecar::installed(&state.data_dir).into_iter().find(|i| i.server == server).map(|i| i.variant.gpu()).unwrap_or(true);
    let opts = SidecarOptions { context: 8192, alias: Some(m.id.to_string()), gpu, embedding: true, ..Default::default() };
    let mut guard = state.embed_sidecar.lock().await;
    if let Some((_, mut old)) = guard.take() {
        old.stop().await;
    }
    let s = Sidecar::start(&server, &st.path, &opts, &state.log_dir().join("embeddings"), Duration::from_secs(120)).await.map_err(err)?;
    *guard = Some((m.id.to_string(), s));
    drop(guard);
    Ok(embed_status(&state).await)
}

#[tauri::command]
pub async fn ai_embed_sidecar_stop(state: State<'_, AiState>) -> CmdResult<()> {
    if let Some((_, mut s)) = state.embed_sidecar.lock().await.take() {
        s.stop().await;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexView {
    pub status: IndexStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<EmbeddingSettings>,
    /// The embedding model that would be used now (`model @ endpoint`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedder: Option<String>,
    pub embedder_local: bool,
    /// Whether the index was built with that model (otherwise search falls back to keywords).
    pub embedder_matches: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedder_error: Option<String>,
    pub managed: EmbedSidecarStatus,
}

fn collect_project_chunks(app: &AppState, root: &str) -> CmdResult<Vec<semantic::Chunk>> {
    let project = Project::open(root).map_err(err)?;
    let red = redactor(app, Some(&project));
    semantic::collect_chunks(&project, &red).map_err(err)
}

/// Index state for the project (and how many chunks changed since the last build).
#[tauri::command]
pub async fn ai_index_status(state: State<'_, AiState>, app: State<'_, AppState>, root: String) -> CmdResult<IndexView> {
    let chunks = collect_project_chunks(&app, &root)?;
    let status = semantic::status(Path::new(&root), Some(&chunks));
    let emb = state.embedder(&app).await;
    let cfg = state.config()?;
    Ok(IndexView {
        embedder_matches: emb.as_ref().is_ok_and(|e| status.embedder.as_deref() == Some(e.id().as_str())),
        embedder: emb.as_ref().ok().map(|e| e.label()),
        embedder_local: emb.as_ref().is_ok_and(|e| e.is_local()),
        embedder_error: emb.err(),
        settings: cfg.embeddings,
        managed: embed_status(&state).await,
        status,
    })
}

/// Builds or refreshes the index (only new and changed chunks are embedded). `call_id` cancels
/// through `ai_cancel`; what was embedded so far is kept.
#[tauri::command]
pub async fn ai_index_build(state: State<'_, AiState>, app: State<'_, AppState>, root: String, call_id: Option<String>, on_progress: Channel<BuildProgress>) -> CmdResult<BuildOutcome> {
    let embedder = state.embedder(&app).await?;
    let chunks = collect_project_chunks(&app, &root)?;
    let flag = CancelFlag::default();
    if let Some(id) = &call_id {
        state.builds.lock().unwrap().insert(id.clone(), flag.clone());
    }
    let res = semantic::build(Path::new(&root), chunks, embedder.as_ref(), &flag, |p| {
        let _ = on_progress.send(p);
    })
    .await;
    if let Some(id) = &call_id {
        state.builds.lock().unwrap().remove(id);
    }
    res.map_err(err)
}

/// Deletes the project's index (`.devall/embeddings/`; a cache, rebuilt on demand).
#[tauri::command]
pub fn ai_index_clear(root: String) -> CmdResult<()> {
    semantic::clear(Path::new(&root)).map_err(err)
}

/// Semantic search over notes, requests, mocks and environment names (keyword fallback).
#[tauri::command]
pub async fn ai_search(state: State<'_, AiState>, app: State<'_, AppState>, root: String, query: String, limit: Option<usize>) -> CmdResult<SearchResult> {
    let embedder = state.embedder(&app).await.ok();
    semantic::search(Path::new(&root), &query, limit.unwrap_or(20), embedder.as_deref(), || {
        let project = Project::open(&root)?;
        semantic::collect_chunks(&project, &redactor(&app, Some(&project)))
    })
    .await
    .map_err(err)
}

/// Runs any feature by name (`ask` runs non-streaming). Used by `AiAssistButton`. NL → SQL /
/// MongoDB / kubectl get live checks here (read-only `EXPLAIN`, `explain`, server-side dry
/// run); nothing they suggest is executed.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ai_run(
    handle: AppHandle,
    state: State<'_, AiState>,
    app: State<'_, AppState>,
    root: Option<String>,
    feature: String,
    input: Value,
    provider: Option<String>,
    call_id: Option<String>,
) -> CmdResult<Value> {
    let input = FeatureInput::from_parts(&feature, input).map_err(err)?;
    let checker = live_checker(&handle, &input).await?;
    let p = state.provider(&app, provider.as_deref()).await?;
    let project = open_project(root.as_deref())?;
    let red = redactor(&app, project.as_ref());
    let allow_remote = state.config()?.allow_remote_context;
    let data_dir = state.data_dir.clone();
    let task = tokio::spawn(async move {
        let env = AiEnv::new(project.as_ref(), Some(&data_dir), red, allow_remote);
        let to = |r: std::result::Result<Value, serde_json::Error>| r.map_err(|e| devall_ai::AiError::Protocol(e.to_string()));
        match (&input, checker) {
            (FeatureInput::NlSql { .. }, Some(c)) => to(serde_json::to_value(features::nl_to_sql(&env, &p, &input, c.as_ref()).await?)),
            (FeatureInput::NlMongo { .. }, Some(c)) => to(serde_json::to_value(features::nl_to_mongo(&env, &p, &input, c.as_ref()).await?)),
            (FeatureInput::NlKubectl { .. }, Some(c)) => to(serde_json::to_value(features::nl_to_kubectl(&env, &p, &input, c.as_ref()).await?)),
            _ => features::run(&input, &env, &p).await,
        }
    });
    run_cancellable(&state, call_id, task).await
}

// ── Live checks for NL → SQL / MongoDB / kubectl ──────────────────────────────────────────────

fn clip_plan(s: &str) -> String {
    if s.chars().count() <= 1500 { s.to_string() } else { format!("{}…", s.chars().take(1500).collect::<String>()) }
}

/// `EXPLAIN` (never ANALYZE) of the suggested statement on the user's session.
struct SqlChecker {
    session: Arc<devall_db::Session>,
    allow_writes: bool,
}

impl devall_ai::structured::Checker for SqlChecker {
    fn check<'a>(&'a self, candidate: &'a Value) -> devall_ai::structured::CheckFuture<'a> {
        Box::pin(async move {
            let sql = devall_ai::nlquery::clean_sql(candidate["sql"].as_str().unwrap_or_default());
            let conn = self.session.sql().map_err(err)?;
            let stmts = devall_db::sql::split(&sql, conn.dialect());
            let [stmt] = stmts.as_slice() else { return Err(format!("give exactly one statement (got {})", stmts.len())) };
            let c = devall_db::sql::classify(&stmt.text, conn.dialect());
            if !c.read_only && (!self.allow_writes || self.session.is_read_only()) {
                return Err(format!(
                    "{} changes data; only a read-only query is allowed here{}.",
                    c.keyword,
                    if self.session.is_read_only() { " (the connection is read-only)" } else { "" }
                ));
            }
            let plan = conn.explain(&stmt.text, false).await.map_err(|e| format!("the database rejected it: {}", self.session.redact(&e)))?;
            let text = plan.text.clone().unwrap_or_else(|| serde_json::to_string(&plan.plan).unwrap_or_default());
            Ok(json!({ "readOnly": c.read_only, "keyword": c.keyword, "plan": clip_plan(&text) }))
        })
    }
}

/// The server's `explain` of the suggested find / aggregate.
struct MongoChecker {
    session: Arc<devall_db::Session>,
    database: String,
    collection: String,
}

impl devall_ai::structured::Checker for MongoChecker {
    fn check<'a>(&'a self, candidate: &'a Value) -> devall_ai::structured::CheckFuture<'a> {
        Box::pin(async move {
            let m = self.session.mongo().map_err(err)?;
            let (kind, filter, projection, sort, limit, pipeline) = devall_ai::nlquery::mongo_from(candidate);
            let opt = |s: String| Some(s).filter(|x| !x.trim().is_empty());
            let res = if kind == "aggregate" {
                let req = devall_db::mongo::AggregateRequest { database: self.database.clone(), collection: self.collection.clone(), pipeline, limit: None };
                m.explain_aggregate(&req, false).await
            } else {
                let req = devall_db::mongo::FindRequest {
                    database: self.database.clone(),
                    collection: self.collection.clone(),
                    filter: opt(filter),
                    projection: opt(projection),
                    sort: opt(sort),
                    skip: None,
                    limit: (limit > 0).then_some(limit),
                };
                m.explain_find(&req, false).await
            };
            let plan = res.map_err(|e| format!("the server rejected it: {}", self.session.redact(&e)))?;
            let winning = plan.pointer("/queryPlanner/winningPlan").or_else(|| plan.pointer("/stages/0/$cursor/queryPlanner/winningPlan")).cloned().unwrap_or(plan);
            Ok(json!({ "plan": clip_plan(&serde_json::to_string(&winning).unwrap_or_default()) }))
        })
    }
}

/// Parses the command and dry-runs changes on the cluster (nothing is applied).
struct KubectlChecker {
    client: devall_infra::kube::Client,
    custom: Vec<devall_infra::kube::KindRef>,
    namespace: String,
}

impl devall_ai::structured::Checker for KubectlChecker {
    fn check<'a>(&'a self, candidate: &'a Value) -> devall_ai::structured::CheckFuture<'a> {
        Box::pin(async move {
            let cmd = devall_infra::kube::kubectl::parse(candidate["command"].as_str().unwrap_or_default())?;
            devall_infra::kube::kubectl::validate(&self.client, &cmd, &self.custom, &self.namespace).await
        })
    }
}

/// The live checker for NL features that have one (`None` = run without, e.g. no session).
async fn live_checker(handle: &AppHandle, input: &FeatureInput) -> CmdResult<Option<Box<dyn devall_ai::structured::Checker>>> {
    Ok(match input {
        FeatureInput::NlSql { session: Some(id), allow_writes, .. } => {
            let session = handle.state::<crate::modules::db::DbState>().session(id)?;
            Some(Box::new(SqlChecker { session, allow_writes: *allow_writes }))
        }
        FeatureInput::NlMongo { session: Some(id), database, collection, .. } => {
            let session = handle.state::<crate::modules::db::DbState>().session(id)?;
            Some(Box::new(MongoChecker { session, database: database.clone(), collection: collection.clone() }))
        }
        FeatureInput::NlKubectl { context, namespace, .. } => {
            let infra = handle.state::<crate::modules::infra::InfraState>();
            let conn = infra.kube_conn(context).await?;
            let custom = match tokio::time::timeout(Duration::from_secs(5), devall_infra::kube::resources::custom_kinds(&conn.client)).await {
                Ok(Ok(k)) => k.into_iter().map(|k| k.kind).collect(),
                _ => Vec::new(),
            };
            let namespace = if namespace.trim().is_empty() { conn.default_namespace.clone() } else { namespace.clone() };
            Some(Box::new(KubectlChecker { client: conn.client, custom, namespace }))
        }
        _ => None,
    })
}

/// Provider + project + redactor for one feature call.
struct Call {
    provider: Provider,
    project: Option<Project>,
    redactor: Redactor,
    allow_remote: bool,
}

impl Call {
    async fn new(state: &AiState, app: &AppState, root: Option<&str>, provider: Option<&str>) -> CmdResult<Self> {
        let provider = state.provider(app, provider).await?;
        let project = open_project(root)?;
        let redactor = redactor(app, project.as_ref());
        Ok(Self { provider, project, redactor, allow_remote: state.config()?.allow_remote_context })
    }

    fn env<'a>(&'a self, data_dir: &'a Path) -> AiEnv<'a> {
        AiEnv::new(self.project.as_ref(), Some(data_dir), self.redactor.clone(), self.allow_remote)
    }
}

#[tauri::command]
pub async fn ai_nl_regex(
    state: State<'_, AiState>,
    app: State<'_, AppState>,
    provider: Option<String>,
    description: String,
    positive: Option<Vec<String>>,
    negative: Option<Vec<String>>,
) -> CmdResult<RegexSuggestion> {
    let c = Call::new(&state, &app, None, provider.as_deref()).await?;
    features::nl_to_regex(&c.env(&state.data_dir), &c.provider, &description, &positive.unwrap_or_default(), &negative.unwrap_or_default()).await.map_err(err)
}

#[tauri::command]
pub async fn ai_nl_jsonpath(state: State<'_, AiState>, app: State<'_, AppState>, provider: Option<String>, description: String, sample_json: String) -> CmdResult<JsonPathSuggestion> {
    let c = Call::new(&state, &app, None, provider.as_deref()).await?;
    features::nl_to_jsonpath(&c.env(&state.data_dir), &c.provider, &description, &sample_json).await.map_err(err)
}

#[tauri::command]
pub async fn ai_nl_jq(state: State<'_, AiState>, app: State<'_, AppState>, provider: Option<String>, description: String, sample_json: String) -> CmdResult<JqSuggestion> {
    let c = Call::new(&state, &app, None, provider.as_deref()).await?;
    features::nl_to_jq(&c.env(&state.data_dir), &c.provider, &description, &sample_json).await.map_err(err)
}

/// Drafts a request using the project's variable/secret names (never their values).
#[tauri::command]
pub async fn ai_nl_request(state: State<'_, AiState>, app: State<'_, AppState>, root: Option<String>, provider: Option<String>, description: String) -> CmdResult<RequestDraft> {
    let c = Call::new(&state, &app, root.as_deref(), provider.as_deref()).await?;
    features::nl_to_request(&c.env(&state.data_dir), &c.provider, &description).await.map_err(err)
}

#[tauri::command]
pub async fn ai_mock_data(state: State<'_, AiState>, app: State<'_, AppState>, provider: Option<String>, schema_or_sample: String, count: Option<usize>) -> CmdResult<MockData> {
    let c = Call::new(&state, &app, None, provider.as_deref()).await?;
    features::generate_mock_data(&c.env(&state.data_dir), &c.provider, &schema_or_sample, count.unwrap_or(10)).await.map_err(err)
}

#[tauri::command]
pub async fn ai_explain_response(state: State<'_, AiState>, app: State<'_, AppState>, root: Option<String>, provider: Option<String>, context: ResponseContext) -> CmdResult<ResponseExplanation> {
    let c = Call::new(&state, &app, root.as_deref(), provider.as_deref()).await?;
    features::explain_response(&c.env(&state.data_dir), &c.provider, &context).await.map_err(err)
}

#[tauri::command]
pub async fn ai_explain_error(state: State<'_, AiState>, app: State<'_, AppState>, root: Option<String>, provider: Option<String>, text: String) -> CmdResult<ErrorExplanation> {
    let c = Call::new(&state, &app, root.as_deref(), provider.as_deref()).await?;
    features::explain_error(&c.env(&state.data_dir), &c.provider, &text).await.map_err(err)
}

#[tauri::command]
pub async fn ai_suggest_tests(state: State<'_, AiState>, app: State<'_, AppState>, root: Option<String>, provider: Option<String>, sample: ResponseContext) -> CmdResult<TestSuggestions> {
    let c = Call::new(&state, &app, root.as_deref(), provider.as_deref()).await?;
    features::suggest_tests(&c.env(&state.data_dir), &c.provider, &sample).await.map_err(err)
}

#[tauri::command]
pub async fn ai_summarize_notes(state: State<'_, AiState>, app: State<'_, AppState>, root: String, provider: Option<String>, paths: Option<Vec<String>>) -> CmdResult<NotesSummary> {
    let c = Call::new(&state, &app, Some(&root), provider.as_deref()).await?;
    features::summarize_notes(&c.env(&state.data_dir), &c.provider, &paths.unwrap_or_default()).await.map_err(err)
}

/// Suggested to-dos only — the UI adds the ones the user keeps with `todos_apply` (`add`).
#[tauri::command]
pub async fn ai_todos_from_text(state: State<'_, AiState>, app: State<'_, AppState>, root: Option<String>, provider: Option<String>, text: String) -> CmdResult<TodoSuggestions> {
    let c = Call::new(&state, &app, root.as_deref(), provider.as_deref()).await?;
    features::todos_from_text(&c.env(&state.data_dir), &c.provider, &text).await.map_err(err)
}

// ── OpenAPI naming ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiNamingView {
    #[serde(flatten)]
    pub naming: features::OpenApiNaming,
    /// The improved document as YAML (what the dialog saves once the user accepts it).
    pub improved: String,
    /// Lint of the spec as sent and of the improved one.
    pub lint_before: devall_openapi::LintReport,
    pub lint_after: devall_openapi::LintReport,
}

/// "Improve names with AI" for a traffic-inferred OpenAPI draft: proposes operationIds,
/// summaries, tags and component schema names (validated, one repair retry), applies them as a
/// patch, and lints both versions. Nothing is saved.
#[tauri::command]
pub async fn ai_openapi_names(
    state: State<'_, AiState>,
    app: State<'_, AppState>,
    root: Option<String>,
    provider: Option<String>,
    spec: String,
    call_id: Option<String>,
) -> CmdResult<OpenApiNamingView> {
    let p = state.provider(&app, provider.as_deref()).await?;
    let project = open_project(root.as_deref())?;
    let red = redactor(&app, project.as_ref());
    let allow_remote = state.config()?.allow_remote_context;
    let data_dir = state.data_dir.clone();
    let text = spec.clone();
    let task = tokio::spawn(async move {
        let env = AiEnv::new(project.as_ref(), Some(&data_dir), red, allow_remote);
        features::improve_openapi_names(&env, &p, &text).await
    });
    let naming = run_cancellable(&state, call_id, task).await?;
    let improved = serde_yaml_ng::to_string(&naming.spec).map_err(err)?;
    let (before, after) = tokio::task::spawn_blocking({
        let improved = improved.clone();
        move || (devall_openapi::lint_text(&spec), devall_openapi::lint_text(&improved))
    })
    .await
    .map_err(err)?;
    Ok(OpenApiNamingView { naming, improved, lint_before: before, lint_after: after })
}

// ── Explain this failure: context gathering ──────────────────────────────────────────────────

/// `http://host:port/path?q` → (host, port, path?query).
fn split_url(url: &str) -> Option<(String, Option<u16>, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let end = stripped.find(']')?;
        (stripped[..end].to_string(), stripped[end + 1..].strip_prefix(':').and_then(|p| p.parse().ok()))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().ok()),
            None => (authority.to_string(), None),
        }
    };
    let port = port.or(match scheme.to_ascii_lowercase().as_str() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    });
    let path = path.split('#').next().unwrap_or("/");
    Some((host.to_ascii_lowercase(), port, if path.starts_with('/') { path.to_string() } else { format!("/{path}") }))
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") || host.ends_with(".localhost") || host.starts_with("127.")
}

fn clip_text(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

fn ms_to_iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.format("%Y-%m-%d %H:%M:%S%.3fZ").to_string()).unwrap_or_default()
}

/// The stub that answered (or the closest stubs) when the request went to the project's mock.
fn mock_sources(mock: &crate::modules::mock::MockState, root: &str, host: &str, port: Option<u16>, method: &str, path: &str) -> Vec<FailureSource> {
    let Some(h) = crate::modules::mock::running(mock, root) else { return Vec::new() };
    if !is_loopback(host) || port != Some(h.port()) {
        return Vec::new();
    }
    let m = h.explain_request(method, path);
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap_or_default();
    if let Some(stub) = &m.stub {
        let label = m.stub_name.clone().or_else(|| m.stub_id.clone()).unwrap_or_else(|| "stub".into());
        return vec![FailureSource {
            kind: "mockStub".into(),
            source: format!("{}#{}", m.stub_file.clone().unwrap_or_default(), m.stub_id.clone().unwrap_or_default()),
            title: format!("Mock stub that answered: {label}"),
            text: clip_text(&pretty(stub), 3500),
        }];
    }
    m.near_misses
        .iter()
        .take(2)
        .enumerate()
        .map(|(i, nm)| {
            let stub = &nm["stubMapping"];
            let id = stub["id"].as_str().unwrap_or_default();
            let label = stub["name"].as_str().unwrap_or(id).to_string();
            let fields: Vec<String> = nm["matchResult"]["fields"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|f| f["matched"] == false)
                        .map(|f| format!("- {}: expected {} but got {}", f["field"].as_str().unwrap_or("?"), f["expected"], f["actual"]))
                        .collect()
                })
                .unwrap_or_default();
            FailureSource {
                kind: "mockNearMiss".into(),
                source: format!("{}#{id}", nm["file"].as_str().unwrap_or_default()),
                title: format!("{} closest mock stub (no stub matched): {label}", if i == 0 { "Closest" } else { "Next" }),
                text: clip_text(&format!("Mismatched fields:\n{}\n\nStub:\n{}", fields.join("\n"), pretty(stub)), 3500),
            }
        })
        .collect()
}

/// Project events with the same correlation id, or near the request in time and mentioning its
/// path.
fn event_sources(root: &str, ctx: &ResponseContext, path: &str, at: i64) -> Vec<FailureSource> {
    let Ok(store) = devall_logs::EventStore::for_project(Path::new(root)) else { return Vec::new() };
    let headers = ctx.request_headers.iter().chain(ctx.headers.iter()).map(|(k, v)| (k.as_str(), v.as_str()));
    let corr = devall_logs::correlation::extract(headers, Some(&ctx.body));
    let mut events: Vec<devall_logs::Event> = Vec::new();
    if let Some(c) = &corr
        && let Ok(list) = store.correlated(c)
    {
        events.extend(list.into_iter().rev().take(6));
    }
    let bare_path = path.split('?').next().unwrap_or(path);
    if bare_path.len() > 1 {
        let filter = devall_logs::EventFilter {
            since: Some(at - 60_000),
            until: Some(at + 60_000),
            text: Some(bare_path.to_string()),
            limit: Some(20),
            newest_first: true,
            ..Default::default()
        };
        if let Ok(list) = store.query(&filter) {
            for e in list {
                // The send itself is already S1.
                if e.source == "http" && e.kind.starts_with("http.") && corr.is_none() {
                    continue;
                }
                if !events.iter().any(|x| x.id == e.id) {
                    events.push(e);
                }
            }
        }
    }
    events.truncate(8);
    events
        .into_iter()
        .map(|e| {
            let data = if e.data.is_null() { String::new() } else { clip_text(&e.data.to_string(), 700) };
            FailureSource {
                kind: "event".into(),
                source: format!("event:{}", e.id),
                title: format!("Event: {}", clip_text(&e.title, 80)),
                text: format!(
                    "{} {} {} [{}]{}{}\n{}",
                    ms_to_iso(e.at),
                    e.level.as_str().to_uppercase(),
                    e.kind,
                    e.source,
                    e.correlation_id.as_deref().map(|c| format!(" correlation={c}")).unwrap_or_default(),
                    e.duration_ms.map(|d| format!(" {d:.0} ms")).unwrap_or_default(),
                    if data.is_empty() { e.title.clone() } else { format!("{}\n{data}", e.title) }
                ),
            }
        })
        .collect()
}

/// Pod events and the last log lines when the URL is a running Kubernetes port-forward.
async fn pod_sources(infra: &crate::modules::infra::InfraState, root: &str, host: &str, port: Option<u16>) -> Vec<FailureSource> {
    let Some(port) = port.filter(|_| is_loopback(host)) else { return Vec::new() };
    let Some(fwd) = infra.kube_forward_on_port(root, port) else { return Vec::new() };
    let (Some(pod), Some(ns)) = (fwd.backend.clone(), fwd.namespace.clone()) else { return Vec::new() };
    let context = fwd.context.clone().unwrap_or_default();
    let Ok(client) = infra.kube_client(&context).await else { return Vec::new() };
    let reference = format!("pod:{context}/{ns}/{pod}");
    let mut out = Vec::new();
    let pod_kind = devall_infra::kube::KindRef::new("", "v1", "Pod", "pods", true);
    if let Ok(d) = devall_infra::kube::resources::describe(&client, &pod_kind, Some(&ns), &pod).await {
        let mut text = String::new();
        for c in &d.containers {
            text.push_str(&format!(
                "container {}: {}{} ready={} restarts={}{}\n",
                c.name,
                c.state,
                c.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default(),
                c.ready,
                c.restarts,
                c.message.as_deref().map(|m| format!(" — {}", clip_text(m, 200))).unwrap_or_default()
            ));
        }
        for e in d.events.iter().take(15) {
            text.push_str(&format!("{} {} {} (x{}) {}\n", e.last_seen.as_deref().unwrap_or("-"), e.type_, e.reason, e.count, clip_text(&e.message, 300)));
        }
        if d.events.is_empty() {
            text.push_str("(no recent events)\n");
        }
        out.push(FailureSource { kind: "podEvents".into(), source: reference.clone(), title: format!("Pod {pod} ({ns}) status and events — forward {}", fwd.name), text });
    }
    if let Ok(logs) = devall_infra::kube::stream::pod_log_tail(client, &ns, &pod, None, 40).await {
        let tail: Vec<&str> = logs.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
        if !tail.is_empty() {
            out.push(FailureSource { kind: "podLogs".into(), source: reference, title: format!("Last log lines of pod {pod}"), text: clip_text(&tail.join("\n"), 3500) });
        }
    }
    out
}

/// Gathers context for "explain this failure": the mock stub that answered (or the closest ones)
/// when the URL is the project's mock server, related project events (same correlation id, or
/// the same path within ±60 s of `at`), and pod events + log tail when the URL is a running
/// Kubernetes port-forward. Each step is bounded (5 s) and best-effort. The result is the input
/// of the `explainFailure` feature, so the UI can show it ("what will be sent") before running.
#[tauri::command]
pub async fn ai_failure_context(
    mock: State<'_, crate::modules::mock::MockState>,
    infra: State<'_, crate::modules::infra::InfraState>,
    root: Option<String>,
    context: ResponseContext,
    error: Option<String>,
    at: Option<i64>,
) -> CmdResult<FailureContext> {
    let mut out = FailureContext { exchange: context.clone(), error, sources: Vec::new() };
    let Some(root) = root.filter(|r| !r.is_empty()) else { return Ok(out) };
    let Some((host, port, path)) = split_url(&context.url) else { return Ok(out) };
    let at = at.unwrap_or_else(devall_logs::now_ms);
    out.sources.extend(mock_sources(&mock, &root, &host, port, &context.method, &path));
    let (r2, ctx2, p2) = (root.clone(), context.clone(), path.clone());
    if let Ok(Ok(evs)) = tokio::time::timeout(Duration::from_secs(5), tokio::task::spawn_blocking(move || event_sources(&r2, &ctx2, &p2, at))).await {
        out.sources.extend(evs);
    }
    if let Ok(pods) = tokio::time::timeout(Duration::from_secs(8), pod_sources(&infra, &root, &host, port)).await {
        out.sources.extend(pods);
    }
    Ok(out)
}

// ── MCP ───────────────────────────────────────────────────────────────────────────────────────

/// MCP calls waiting for the user. The app-hosted MCP server's [`AppConfirmer`] adds one and emits
/// `mcp-approval` (the request, secrets already masked); the global approval prompt in the UI
/// answers with `mcp_approval_answer`; `mcp-approval-closed` (`{ id, decision }`) tells every
/// window it is settled (answered, timed out → denied, or the server stopped).
#[derive(Default)]
pub struct ApprovalHub {
    pending: Mutex<HashMap<String, (devall_mcp::ApprovalRequest, tokio::sync::oneshot::Sender<devall_mcp::ApprovalDecision>)>>,
}

impl ApprovalHub {
    fn list(&self) -> Vec<devall_mcp::ApprovalRequest> {
        let mut v: Vec<devall_mcp::ApprovalRequest> = self.pending.lock().unwrap().values().map(|(r, _)| r.clone()).collect();
        v.sort_by_key(|r| r.created_at);
        v
    }

    fn answer(&self, id: &str, decision: devall_mcp::ApprovalDecision) -> bool {
        match self.pending.lock().unwrap().remove(id) {
            Some((_, tx)) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    /// Denies everything still waiting (the server is stopping).
    fn deny_all(&self) {
        for (_, (_, tx)) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(devall_mcp::ApprovalDecision::Deny);
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalClosed {
    id: String,
    decision: devall_mcp::ApprovalDecision,
}

/// Removes the pending entry and announces the outcome even when the waiting call is dropped
/// (the MCP layer's own timeout, a client disconnect).
struct PendingGuard {
    app: AppHandle,
    hub: Arc<ApprovalHub>,
    id: String,
    decision: devall_mcp::ApprovalDecision,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.hub.pending.lock().unwrap().remove(&self.id);
        let _ = self.app.emit("mcp-approval-closed", ApprovalClosed { id: self.id.clone(), decision: self.decision });
    }
}

/// Asks the user in the app (any page) to approve an MCP `send_request`.
struct AppConfirmer {
    app: AppHandle,
    hub: Arc<ApprovalHub>,
}

impl devall_mcp::Confirmer for AppConfirmer {
    fn confirm(&self, request: devall_mcp::ApprovalRequest) -> devall_mcp::ApprovalFuture<'_> {
        Box::pin(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let id = request.id.clone();
            let timeout = Duration::from_secs(request.timeout_secs.max(1));
            self.hub.pending.lock().unwrap().insert(id.clone(), (request.clone(), tx));
            let mut guard = PendingGuard { app: self.app.clone(), hub: self.hub.clone(), id, decision: devall_mcp::ApprovalDecision::Deny };
            let _ = self.app.emit("mcp-approval", &request);
            if let Some(w) = self.app.get_webview_window("main") {
                let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
            }
            let decision = match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(d)) => d,
                _ => devall_mcp::ApprovalDecision::Deny,
            };
            guard.decision = decision;
            decision
        })
    }
}

/// Approvals still waiting (the UI shows them after a reload).
#[tauri::command]
pub fn mcp_approvals_pending(state: State<'_, AiState>) -> Vec<devall_mcp::ApprovalRequest> {
    state.approvals.list()
}

/// Answers a pending approval. `false` when it is gone (answered elsewhere or timed out).
#[tauri::command]
pub fn mcp_approval_answer(state: State<'_, AiState>, id: String, decision: devall_mcp::ApprovalDecision) -> bool {
    state.approvals.answer(&id, decision)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    /// `devall` CLI found on PATH or next to the app.
    pub cli: Option<PathBuf>,
    pub snippets: devall_mcp::ConfigSnippets,
    pub http: Option<devall_mcp::HttpInfo>,
}

fn find_cli() -> Option<PathBuf> {
    let name = if cfg!(windows) { "devall.exe" } else { "devall" };
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf))
        && dir.join(name).is_file()
    {
        return Some(dir.join(name));
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

fn snippet_exe(cli: Option<&Path>) -> Option<String> {
    // On PATH → plain `devall`; next to the app → absolute path; missing → `devall` (install it).
    let cli = cli?;
    let on_path = std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(cli.file_name().unwrap_or_default()) == cli));
    (!on_path).then(|| cli.display().to_string())
}

#[tauri::command]
pub async fn mcp_status(state: State<'_, AiState>, root: String, allow_writes: Option<bool>, local_writes: Option<bool>) -> CmdResult<McpStatus> {
    let cli = find_cli();
    let snippets = devall_mcp::config_snippets(Path::new(&root), snippet_exe(cli.as_deref()).as_deref(), allow_writes.unwrap_or(false), local_writes.unwrap_or(true));
    let http = state.mcp_http.lock().await.as_ref().filter(|(r, _)| *r == root).map(|(_, s)| s.info());
    Ok(McpStatus { cli, snippets, http })
}

#[tauri::command]
pub fn mcp_config_snippet(root: String, allow_writes: Option<bool>, exe: Option<String>, local_writes: Option<bool>) -> devall_mcp::ConfigSnippets {
    let exe = exe.or_else(|| snippet_exe(find_cli().as_deref()));
    devall_mcp::config_snippets(Path::new(&root), exe.as_deref(), allow_writes.unwrap_or(false), local_writes.unwrap_or(true))
}

/// Starts the optional Streamable HTTP endpoint (127.0.0.1, random port, bearer token).
/// `confirm_writes`: methods other than GET/HEAD/OPTIONS wait for the user's approval in the app;
/// requests in protected environments always do (whatever the mode). `local_writes` (default on)
/// lets agents write shared notes and the project's personal to-dos.
#[tauri::command]
pub async fn mcp_http_start(
    app: AppHandle,
    state: State<'_, AiState>,
    root: String,
    allow_writes: Option<bool>,
    confirm_writes: Option<bool>,
    local_writes: Option<bool>,
) -> CmdResult<devall_mcp::HttpInfo> {
    let mut guard = state.mcp_http.lock().await;
    if let Some((_, old)) = guard.take() {
        old.stop();
        state.approvals.deny_all();
    }
    let opts = devall_mcp::McpOptions {
        allow_writes: allow_writes.unwrap_or(false),
        confirm_writes: confirm_writes.unwrap_or(false),
        read_only_local: !local_writes.unwrap_or(true),
        app_data_dir: Some(state.data_dir.clone()),
        ..Default::default()
    };
    let keychain = app.state::<AppState>().keychain_available;
    let confirmer: Arc<dyn devall_mcp::Confirmer> = Arc::new(AppConfirmer { app, hub: state.approvals.clone() });
    let server = devall_mcp::DevallMcp::new_with_confirmer(&root, opts, Some(confirmer)).map_err(err)?;
    let store: Box<dyn devall_core::SecretStore> = if keychain { Box::new(devall_core::KeychainSecretStore::new()) } else { Box::new(devall_core::MemorySecretStore::default()) };
    server.set_embedder_source(Some(Arc::new(AppEmbedderSource { data_dir: state.data_dir.clone(), managed: state.embed_sidecar.clone(), store })));
    let srv = devall_mcp::serve_server(server, 0).await.map_err(err)?;
    let info = srv.info();
    *guard = Some((root, srv));
    Ok(info)
}

#[tauri::command]
pub async fn mcp_http_stop(state: State<'_, AiState>) -> CmdResult<()> {
    if let Some((_, s)) = state.mcp_http.lock().await.take() {
        s.stop();
    }
    state.approvals.deny_all();
    Ok(())
}
