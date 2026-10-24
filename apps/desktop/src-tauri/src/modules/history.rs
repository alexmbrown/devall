//! Local history on this device: file versions (local file history), utility runs, recent AI
//! "Ask" answers, the history settings and what each kind uses on disk. Response history commands
//! live in `send.rs`; the event timeline in `modules::logs`.
//!
//! Registration: `pub mod history;`, `.manage(history::HistoryState::default())`, the commands
//! below in `generate_handler!`. `commands::open_project` calls [`project_opened`];
//! `modules::watch` calls [`record_changes`]. Emits `file-history-changed` with `{ root }`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use devall_core::local::{self, HistorySettings};
use devall_history::files::{FileHistory, FileSummary, FileVersion, Limits};
use devall_history::utilities::{NewUtilityRun, UtilityHistory, UtilityRun};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Default)]
pub struct HistoryState {
    utilities: Mutex<Option<Arc<UtilityHistory>>>,
}

impl HistoryState {
    fn utilities(&self) -> CmdResult<Arc<UtilityHistory>> {
        let mut cur = self.utilities.lock().unwrap();
        if let Some(h) = cur.as_ref() {
            return Ok(h.clone());
        }
        let h = Arc::new(UtilityHistory::open_default().map_err(err)?);
        *cur = Some(h.clone());
        Ok(h)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileHistoryChanged {
    root: String,
}

fn files(root: &str) -> CmdResult<Arc<FileHistory>> {
    FileHistory::shared(Path::new(root)).map_err(err)
}

/// A project was opened: records it in its data folder (moving older `.devall/` state once), then
/// prunes and scans its file history in the background.
pub fn project_opened(app: &AppHandle, root: &Path, name: &str) {
    local::touch_project(root, name);
    let settings = local::load_settings();
    if !settings.file_versions {
        return;
    }
    let app = app.clone();
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        let Ok(h) = FileHistory::shared(&root) else { return };
        if let Err(e) = h.prune(&Limits::from(&settings)) {
            eprintln!("/dev/all: pruning file history: {e}");
        }
        match h.scan() {
            Ok(r) if r.recorded + r.deleted > 0 => {
                let _ = app.emit("file-history-changed", FileHistoryChanged { root: root.display().to_string() });
            }
            Ok(_) => {}
            Err(e) => eprintln!("/dev/all: scanning file history: {e}"),
        }
    });
}

/// Records a file-watcher batch (project-relative paths). Called from the watcher thread.
pub fn record_changes(app: &AppHandle, root: &Path, paths: &[String]) {
    if !local::load_settings().file_versions {
        return;
    }
    let Ok(h) = FileHistory::shared(root) else { return };
    if h.record_changes(paths) > 0 {
        let _ = app.emit("file-history-changed", FileHistoryChanged { root: root.display().to_string() });
    }
}

// ---------- file versions ----------

/// Versions of one project file (`path` relative to the project root, e.g. `http/a.req.yaml`).
#[tauri::command]
pub fn fh_versions(root: String, path: String, limit: Option<usize>) -> CmdResult<Vec<FileVersion>> {
    files(&root)?.versions(&path, limit.unwrap_or(200)).map_err(err)
}

/// Versions of every file, newest first.
#[tauri::command]
pub fn fh_recent(root: String, query: Option<String>, offset: Option<usize>, limit: Option<usize>) -> CmdResult<Vec<FileVersion>> {
    files(&root)?.recent(query.as_deref(), offset.unwrap_or(0), limit.unwrap_or(200)).map_err(err)
}

/// Every file with history, most recently changed first.
#[tauri::command]
pub fn fh_files(root: String, query: Option<String>) -> CmdResult<Vec<FileSummary>> {
    files(&root)?.files(query.as_deref()).map_err(err)
}

/// A version's text (`None` for a deletion).
#[tauri::command]
pub fn fh_content(root: String, id: i64) -> CmdResult<Option<String>> {
    Ok(files(&root)?.content(id).map_err(err)?.map(|b| String::from_utf8_lossy(&b).into_owned()))
}

/// A YAML version parsed to JSON (environment and manifest diffs); `None` for a deletion or
/// content that isn't YAML.
#[tauri::command]
pub fn fh_yaml(root: String, id: i64) -> CmdResult<Option<serde_json::Value>> {
    let Some(data) = files(&root)?.content(id).map_err(err)? else { return Ok(None) };
    Ok(serde_yaml_ng::from_slice::<serde_json::Value>(&data).ok())
}

/// Writes a version back to its file (a new "restored" version). Returns the path.
#[tauri::command]
pub fn fh_restore(app: AppHandle, root: String, id: i64) -> CmdResult<String> {
    let path = files(&root)?.restore(id).map_err(err)?;
    let _ = app.emit("file-history-changed", FileHistoryChanged { root });
    Ok(path)
}

#[tauri::command]
pub fn fh_delete(root: String, id: i64) -> CmdResult<bool> {
    files(&root)?.delete_version(id).map_err(err)
}

/// Forgets one file's versions, or all of the project's.
#[tauri::command]
pub fn fh_clear(root: String, path: Option<String>) -> CmdResult<usize> {
    files(&root)?.clear(path.as_deref()).map_err(err)
}

// ---------- utility runs ----------

/// Records a utility run (device-wide). `None` when utility history is off.
#[tauri::command]
pub fn uh_record(state: State<'_, HistoryState>, run: NewUtilityRun) -> CmdResult<Option<String>> {
    let settings = local::load_settings();
    if !settings.utilities {
        return Ok(None);
    }
    state.utilities()?.record(run, settings.utilities_max).map(Some).map_err(err)
}

#[tauri::command]
pub fn uh_list(state: State<'_, HistoryState>, op: Option<String>, query: Option<String>, offset: Option<usize>, limit: Option<usize>) -> CmdResult<Vec<UtilityRun>> {
    state.utilities()?.list(op.as_deref(), query.as_deref(), offset.unwrap_or(0), limit.unwrap_or(200)).map_err(err)
}

#[tauri::command]
pub fn uh_delete(state: State<'_, HistoryState>, id: String) -> CmdResult<bool> {
    state.utilities()?.delete(&id).map_err(err)
}

#[tauri::command]
pub fn uh_clear(state: State<'_, HistoryState>, op: Option<String>) -> CmdResult<usize> {
    state.utilities()?.clear(op.as_deref()).map_err(err)
}

// ---------- AI "Ask" answers ----------

/// Questions and answers kept per project (`<project data>/ai-ask.json`, newest last).
const ASK_FILE: &str = "ai-ask.json";
const ASK_KEEP: usize = 50;

fn ask_path(root: &str) -> std::path::PathBuf {
    local::project_dir(Path::new(root)).join(ASK_FILE)
}

fn read_asks(root: &str) -> Vec<serde_json::Value> {
    std::fs::read(ask_path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The project's recent AI questions and answers (oldest first).
#[tauri::command]
pub fn ai_ask_history(root: String) -> Vec<serde_json::Value> {
    read_asks(&root)
}

/// Appends a question + answer (`{ id, at, question, answer, citations }`), keeping the last 50.
#[tauri::command]
pub fn ai_ask_history_add(root: String, entry: serde_json::Value) -> CmdResult<()> {
    let mut list = read_asks(&root);
    list.push(entry);
    let drop = list.len().saturating_sub(ASK_KEEP);
    list.drain(..drop);
    let path = ask_path(&root);
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(err)?;
    devall_core::fsutil::atomic_write(&path, &serde_json::to_vec(&list).map_err(err)?).map_err(err)
}

#[tauri::command]
pub fn ai_ask_history_clear(root: String) -> CmdResult<()> {
    match std::fs::remove_file(ask_path(&root)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
        _ => Ok(()),
    }
}

// ---------- settings & usage ----------

#[tauri::command]
pub fn hist_settings() -> HistorySettings {
    local::load_settings()
}

#[tauri::command]
pub fn hist_set_settings(app_state: State<'_, AppState>, settings: HistorySettings) -> CmdResult<HistorySettings> {
    local::save_settings(&settings).map_err(err)?;
    let s = local::load_settings();
    FileHistory::set_max_versions_all(s.file_max_versions);
    app_state.set_history_retention(s.responses_per_request);
    Ok(s)
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KindUsage {
    /// Entries (responses, versions, runs, events); `None` when not counted.
    count: Option<usize>,
    bytes: u64,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryUsage {
    /// `<app data>`.
    data_dir: String,
    /// The open project's data folder, when a project is given.
    project_dir: Option<String>,
    responses: KindUsage,
    files: KindUsage,
    utilities: KindUsage,
    events: KindUsage,
    webhooks: KindUsage,
    load_runs: KindUsage,
    /// Semantic search index and saved proxy captures.
    caches: KindUsage,
}

#[tauri::command]
pub fn hist_usage(state: State<'_, HistoryState>, app_state: State<'_, AppState>, root: Option<String>) -> CmdResult<HistoryUsage> {
    let mut u = HistoryUsage { data_dir: local::app_data_dir().map(|d| d.display().to_string()).unwrap_or_default(), ..Default::default() };
    if let Ok(h) = state.utilities()
        && let Ok((n, bytes)) = h.stats()
    {
        u.utilities = KindUsage { count: Some(n), bytes };
    }
    if let Some(root) = root.as_deref().filter(|r| !r.is_empty()) {
        let dir = local::project_dir(Path::new(root));
        u.project_dir = Some(dir.display().to_string());
        if let Ok(h) = app_state.history(root)
            && let Ok((n, bytes)) = h.stats()
        {
            u.responses = KindUsage { count: Some(n), bytes };
        }
        if let Ok(s) = files(root).and_then(|h| h.stats().map_err(err)) {
            u.files = KindUsage { count: Some(s.versions), bytes: s.disk_bytes };
        }
        u.events = KindUsage { count: None, bytes: devall_history::db_size(&dir.join("events.sqlite")) };
        u.webhooks = KindUsage { count: None, bytes: devall_history::db_size(&dir.join("webhooks.sqlite")) };
        u.load_runs = KindUsage { count: None, bytes: local::dir_size(&dir.join(devall_load::store::LOCAL_RUNS_DIR)) };
        u.caches = KindUsage { count: None, bytes: local::dir_size(&dir.join("embeddings")) + local::dir_size(&dir.join("captures")) };
    }
    Ok(u)
}

/// Clears one kind of history: `responses`, `files` or `events` (need `root`), or `utilities`.
#[tauri::command]
pub fn hist_clear(app: AppHandle, state: State<'_, HistoryState>, app_state: State<'_, AppState>, kind: String, root: Option<String>) -> CmdResult<usize> {
    let need_root = || root.as_deref().filter(|r| !r.is_empty()).ok_or_else(|| "open a project first".to_string());
    match kind.as_str() {
        "utilities" => state.utilities()?.clear(None).map_err(err),
        "responses" => app_state.history(need_root()?)?.clear(None).map_err(err),
        "files" => {
            let root = need_root()?;
            let n = files(root)?.clear(None).map_err(err)?;
            let _ = app.emit("file-history-changed", FileHistoryChanged { root: root.to_string() });
            Ok(n)
        }
        "events" => devall_logs::events::EventStore::for_project(Path::new(need_root()?)).and_then(|s| s.clear()).map_err(err),
        other => Err(format!("unknown history kind `{other}`")),
    }
}

/// The folder to show for "Show data folder": the project's data folder (created if needed), or
/// the app data folder without a project.
#[tauri::command]
pub fn hist_data_dir(root: Option<String>) -> CmdResult<String> {
    let dir = match root.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => local::project_dir(Path::new(r)),
        None => local::app_data_dir().ok_or("no user data directory on this system")?,
    };
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir.display().to_string())
}
