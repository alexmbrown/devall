//! Runnable notebooks: run `.ipynb` cells and Markdown code blocks on the user's machine — a thin
//! layer over `crates/devall-notebook`.
//!
//! # Integration (done by whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    `devall-notebook = { path = "../../../crates/devall-notebook" }`
//!    (or add `devall-notebook = { path = "crates/devall-notebook" }` to the root
//!    `[workspace.dependencies]` and use `devall-notebook.workspace = true`).
//! 2. `src/modules/mod.rs`: `pub mod notebooks;`
//! 3. `src/lib.rs`, in `.setup(|app| { … })` next to the notes state:
//!    `app.manage(modules::notebooks::NotebookState::new(data_dir.clone()));`
//!    and in `tauri::generate_handler![...]`:
//!    ```text
//!    modules::notebooks::nb_run_cell,
//!    modules::notebooks::nb_run_snippet,
//!    modules::notebooks::nb_interrupt,
//!    modules::notebooks::nb_restart,
//!    modules::notebooks::nb_shutdown,
//!    modules::notebooks::nb_kernels,
//!    modules::notebooks::nb_interpreters,
//!    modules::notebooks::nb_set_interpreter,
//!    modules::notebooks::nb_trust_status,
//!    modules::notebooks::nb_trust,
//!    modules::notebooks::nb_allow_secrets,
//!    ```
//! 4. Optional, recommended: stop kernels when the app exits. Replace `.run(tauri::generate_context!())`
//!    with `.build(tauri::generate_context!())?.run(|app, e| if let tauri::RunEvent::Exit = e {
//!    tauri::async_runtime::block_on(app.state::<modules::notebooks::NotebookState>().shutdown_all()) })`.
//!    (Without it, kernels still exit: on Windows they live in kill-on-close job objects, and every
//!    kernel quits when its stdin — the app — goes away.)
//!
//! Trust: `nb_run_cell` refuses to run a notebook until `nb_trust_status` found it trusted or the
//! user chose `nb_trust` in this session; `nb_run_snippet` checks the block's hash against the
//! user-level trust store every time. Secrets reach a cell only with a per-cell grant
//! (`nb_allow_secrets`) pinned to that cell's code. The trust store lives in the app data dir
//! (`notebook-trust.json`), never in the project.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devall_core::fsutil::{normalize_rel, safe_join};
use devall_core::providers::ProviderCache;
use devall_core::secrets::SecretResolver;
use devall_core::{MemorySecretStore, Project, SecretStore};
use devall_notebook::env::{PrepareOptions, prepare};
use devall_notebook::trust::{code_hash, notebook_hash};
use devall_notebook::{CellEvent, CellResult, Engine, InterpreterStatus, Language, RunOptions, TrustStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;
use tauri::ipc::Channel;

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn open(root: &str) -> R<Project> {
    Project::open(root).map_err(err)
}

const INTERPRETERS_FILE: &str = "notebook-interpreters.json";

pub struct NotebookState {
    engine: Arc<Engine>,
    trust: TrustStore,
    secrets: Box<dyn SecretStore>,
    providers: ProviderCache,
    /// Kernel keys of notebooks verified/trusted in this session.
    session_trusted: Mutex<HashSet<String>>,
    data_dir: PathBuf,
}

impl NotebookState {
    pub fn new(data_dir: PathBuf) -> Self {
        let engine = Engine::new(data_dir.join("notebook-tmp"));
        let overrides: BTreeMap<Language, PathBuf> =
            std::fs::read_to_string(data_dir.join(INTERPRETERS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        for (lang, path) in overrides {
            engine.interpreters().set_override(lang, Some(path));
        }
        let secrets: Box<dyn SecretStore> = if devall_core::KeychainSecretStore::available() {
            Box::new(devall_core::KeychainSecretStore::new())
        } else {
            Box::new(MemorySecretStore::default())
        };
        Self {
            engine: Arc::new(engine),
            trust: TrustStore::open(&data_dir),
            secrets,
            providers: ProviderCache::default(),
            session_trusted: Mutex::new(HashSet::new()),
            data_dir,
        }
    }

    /// Stops every kernel and running cell (call on app exit).
    pub async fn shutdown_all(&self) {
        self.engine.shutdown_all().await;
    }
}

/// One kernel scope per notebook (or Markdown note).
fn kernel_key(root: &str, path: &str) -> String {
    format!("{root}|{}", normalize_rel(path))
}

fn language(name: &str) -> R<Language> {
    Language::parse(name).ok_or_else(|| format!("“{name}” cells can't run here (supported: shell, powershell, cmd, javascript, python)"))
}

fn is_notebook(path: &str) -> bool {
    path.ends_with(".ipynb")
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CellRunOptions {
    /// Caller-chosen id, so the UI can interrupt before the run finishes.
    pub run_id: Option<String>,
    /// Substitute `{{variables}}` in the code (`metadata.devall.templating`).
    #[serde(default)]
    pub templating: bool,
    /// Inject secrets (requires `nb_allow_secrets` for this exact cell code).
    #[serde(default)]
    pub secrets: bool,
    pub timeout_ms: Option<u64>,
    /// Working directory relative to the project root (`metadata.devall.cwd`).
    pub cwd: Option<String>,
    pub execution_count: Option<u32>,
}

/// Runs one notebook cell, streaming [`CellEvent`]s to `on_event`. `env` is the active environment
/// id; its non-secret variables arrive as `DEVALL_<name>` environment variables.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn nb_run_cell(
    state: State<'_, NotebookState>,
    root: String,
    notebook_path: String,
    cell_id: String,
    code: String,
    language: String,
    env: Option<String>,
    options: Option<CellRunOptions>,
    on_event: Channel<CellEvent>,
) -> R<CellResult> {
    let p = open(&root)?;
    let lang = self::language(&language)?;
    let opts = options.unwrap_or_default();
    let key = kernel_key(&root, &notebook_path);
    if !state.session_trusted.lock().unwrap().contains(&key) {
        return Err("This notebook isn't trusted on this machine yet. Review its code, then choose “Trust & run”.".into());
    }
    if opts.secrets && !state.trust.secrets_allowed(p.id(), &notebook_path, &cell_id, &code_hash(lang.as_str(), &code)) {
        return Err("Secrets aren't enabled for this cell (or its code changed since you enabled them).".into());
    }
    let cwd = match opts.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty() && *c != ".") {
        Some(rel) => safe_join(p.root(), rel).map_err(err)?,
        None => p.root().to_path_buf(),
    };
    let env_id = env.as_deref().filter(|e| !e.is_empty());
    let env_file = env_id.and_then(|id| p.read_environment(id).ok());
    let resolver = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env_id, p.root(), env_file.as_ref(), &state.providers);
    let prepared = prepare(
        &p,
        &code,
        PrepareOptions {
            environment: env_id,
            templating: opts.templating,
            secrets: opts.secrets.then_some(&resolver as &dyn devall_core::SecretLookup),
            notebook: Some(&notebook_path),
        },
    )
    .map_err(err)?;
    if opts.templating && !prepared.unresolved.is_empty() {
        let names: Vec<String> = prepared.unresolved.iter().map(|u| format!("{{{{{u}}}}}")).collect();
        let _ = on_event.send(CellEvent::Notice { text: format!("Unresolved: {}", names.join(", ")), at: now_ms() });
    }
    let run = RunOptions {
        run_id: opts.run_id,
        cwd,
        env: prepared.env,
        timeout: opts.timeout_ms.filter(|t| *t > 0).map(Duration::from_millis),
        kernel_key: key,
        redact: prepared.secret_values,
        execution_count: opts.execution_count,
    };
    let handle = state.engine.start(&prepared.code, lang, run).await.map_err(err)?;
    let result = handle
        .finish(|e| {
            let _ = on_event.send(e.clone());
        })
        .await;
    crate::modules::logs::record_notebook_run(
        &root,
        &notebook_path,
        Some(&cell_id),
        result.ok,
        result.duration_ms as f64,
        serde_json::json!({ "language": result.language, "exitCode": result.exit_code, "interrupted": result.interrupted, "timedOut": result.timed_out }),
    );
    Ok(result)
}

/// Runs a fenced code block from a Markdown note (trust-gated per block). JavaScript and Python
/// blocks of one note share a kernel.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn nb_run_snippet(
    state: State<'_, NotebookState>,
    root: String,
    note_path: String,
    language: String,
    code: String,
    env: Option<String>,
    run_id: Option<String>,
    on_event: Channel<CellEvent>,
) -> R<CellResult> {
    let p = open(&root)?;
    let lang = self::language(&language)?;
    if !state.trust.is_snippet_trusted(p.id(), &note_path, &code_hash(lang.as_str(), &code)) {
        return Err("This code block isn't trusted yet. Review it, then choose “Trust & run”.".into());
    }
    let env_id = env.as_deref().filter(|e| !e.is_empty());
    let prepared = prepare(&p, &code, PrepareOptions { environment: env_id, notebook: Some(&note_path), ..Default::default() }).map_err(err)?;
    let run = RunOptions {
        run_id,
        cwd: p.root().to_path_buf(),
        env: prepared.env,
        kernel_key: kernel_key(&root, &note_path),
        ..Default::default()
    };
    let handle = state.engine.start(&prepared.code, lang, run).await.map_err(err)?;
    Ok(handle
        .finish(|e| {
            let _ = on_event.send(e.clone());
        })
        .await)
}

/// Ctrl-C for a running cell; false when it already finished.
#[tauri::command]
pub fn nb_interrupt(state: State<'_, NotebookState>, run_id: String) -> bool {
    state.engine.interrupt(&run_id)
}

/// Stops the notebook's kernels (variables are cleared); the next cell starts fresh ones.
#[tauri::command]
pub async fn nb_restart(state: State<'_, NotebookState>, root: String, notebook_path: String) -> R<usize> {
    Ok(state.engine.restart(&kernel_key(&root, &notebook_path)).await)
}

/// Stops the notebook's kernels (closing a notebook).
#[tauri::command]
pub async fn nb_shutdown(state: State<'_, NotebookState>, root: String, notebook_path: String) -> R<usize> {
    Ok(state.engine.shutdown(&kernel_key(&root, &notebook_path)).await)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelInfo {
    pub language: Language,
    pub label: String,
}

/// The notebook's live kernels.
#[tauri::command]
pub async fn nb_kernels(state: State<'_, NotebookState>, root: String, notebook_path: String) -> R<Vec<KernelInfo>> {
    Ok(state.engine.kernels(&kernel_key(&root, &notebook_path)).await.into_iter().map(|(language, label)| KernelInfo { language, label }).collect())
}

/// Interpreter for every language (or why it's missing). `refresh` re-detects.
#[tauri::command]
pub async fn nb_interpreters(state: State<'_, NotebookState>, refresh: Option<bool>) -> R<Vec<InterpreterStatus>> {
    if refresh.unwrap_or(false) {
        state.engine.interpreters().refresh();
    }
    Ok(state.engine.interpreters().statuses().await)
}

/// Pins (or with `path: null`, unpins) the interpreter for a language; remembered across restarts.
#[tauri::command]
pub async fn nb_set_interpreter(state: State<'_, NotebookState>, language: String, path: Option<String>) -> R<InterpreterStatus> {
    let lang = self::language(&language)?;
    let path = path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).map(PathBuf::from);
    state.engine.interpreters().set_override(lang, path);
    let all: BTreeMap<Language, PathBuf> = state.engine.interpreters().overrides().into_iter().collect();
    let text = serde_json::to_string_pretty(&all).map_err(err)?;
    devall_core::fsutil::atomic_write(&state.data_dir.join(INTERPRETERS_FILE), text.as_bytes()).map_err(err)?;
    let statuses = state.engine.interpreters().statuses().await;
    statuses.into_iter().find(|s| s.language == lang).ok_or_else(|| "unknown language".into())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustStatus {
    pub trusted: bool,
    pub hash: String,
    /// Notebook cells (by id) whose secrets grant matches their current code.
    pub secret_cells: Vec<String>,
}

fn secret_cells(state: &NotebookState, project_id: &str, path: &str, content: &str) -> Vec<String> {
    let Ok(nb) = serde_json::from_str::<Value>(content) else { return Vec::new() };
    nb.get("cells")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("cell_type").and_then(Value::as_str) == Some("code"))
        .filter_map(|c| {
            let id = c.get("id").and_then(Value::as_str)?;
            let (lang, code) = devall_core::notebook::cell_language(c, &nb);
            let h = code_hash(lang.unwrap_or(""), &code);
            state.trust.secrets_allowed(project_id, path, id, &h).then(|| id.to_string())
        })
        .collect()
}

fn status(state: &NotebookState, root: &str, path: &str, content: &str, language: Option<&str>, trust: bool) -> R<TrustStatus> {
    let p = open(root)?;
    if is_notebook(path) {
        let hash = notebook_hash(content).map_err(err)?;
        if trust {
            state.trust.trust(p.id(), path, &hash).map_err(err)?;
        }
        let trusted = state.trust.is_trusted(p.id(), path, &hash);
        let key = kernel_key(root, path);
        let mut session = state.session_trusted.lock().unwrap();
        if trusted {
            session.insert(key);
        } else {
            session.remove(&key);
        }
        Ok(TrustStatus { trusted, secret_cells: secret_cells(state, p.id(), path, content), hash })
    } else {
        let lang = self::language(language.ok_or("language is required for a code block")?)?;
        let hash = code_hash(lang.as_str(), content);
        if trust {
            state.trust.trust_snippet(p.id(), path, &hash).map_err(err)?;
        }
        Ok(TrustStatus { trusted: state.trust.is_snippet_trusted(p.id(), path, &hash), hash, secret_cells: Vec::new() })
    }
}

/// Whether a notebook (`path` ends with `.ipynb`, `content` = its JSON) or a Markdown code block
/// (`content` = the block's code, `language` required) is trusted on this machine.
#[tauri::command]
pub fn nb_trust_status(state: State<'_, NotebookState>, root: String, path: String, content: String, language: Option<String>) -> R<TrustStatus> {
    status(&state, &root, &path, &content, language.as_deref(), false)
}

/// The user's consent: trusts this exact notebook version / code block.
#[tauri::command]
pub fn nb_trust(state: State<'_, NotebookState>, root: String, path: String, content: String, language: Option<String>) -> R<TrustStatus> {
    status(&state, &root, &path, &content, language.as_deref(), true)
}

/// Enables (or disables) secret injection for one notebook cell, pinned to its current code.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn nb_allow_secrets(
    state: State<'_, NotebookState>,
    root: String,
    path: String,
    cell_id: String,
    code: String,
    language: String,
    allow: bool,
) -> R<()> {
    let p = open(&root)?;
    let lang = self::language(&language)?;
    let hash = code_hash(lang.as_str(), &code);
    state.trust.set_secrets_allowed(p.id(), &path, &cell_id, allow.then_some(hash.as_str())).map_err(err)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
