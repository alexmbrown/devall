//! Shared notes (`notes/**/*.md`, committed) and personal to-dos (app data dir, never committed),
//! plus the AI/MCP context built from them.
//!
//! Registration: `mod notes;` in modules/mod.rs, `.manage(notes::NotesState::new(app_data_dir))`,
//! `notes::start_reminders(app.handle())` in setup (needs `tauri_plugin_notification::init()`), and
//! the `notes_*`, `todos_*`, `context_docs` commands in `generate_handler!`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use devall_core::context::{self, ContextDoc, ContextOptions};
use devall_core::notes::{NoteHit, NoteNode};
use devall_core::todos::dates::{self, DateCtx};
use devall_core::todos::quickadd::QuickAdd;
use devall_core::todos::{Applied, CompletedEntry, Due, FilterGroup, Now, ProjectRef, Snapshot, TodoOp, TodoStore};
use devall_core::Project;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::{Action, ActionType, NotificationExt};

pub struct NotesState {
    data_dir: PathBuf,
    /// Serializes read-modify-write of to-do files.
    lock: Mutex<()>,
}

impl NotesState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir, lock: Mutex::new(()) }
    }
}

type R<T> = Result<T, String>;

fn open(root: &str) -> R<Project> {
    Project::open(root).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn notes_tree(root: String) -> R<NoteNode> {
    open(&root)?.notes_tree().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn notes_read(root: String, path: String) -> R<String> {
    open(&root)?.read_note(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn notes_save(root: String, path: String, content: String) -> R<()> {
    open(&root)?.save_note(&path, &content).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn notes_create(root: String, folder: String, title: String) -> R<String> {
    open(&root)?.create_note(&folder, &title).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn notes_create_folder(root: String, parent: String, name: String) -> R<String> {
    open(&root)?.create_note_folder(&parent, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn notes_rename(root: String, path: String, name: String) -> R<String> {
    open(&root)?.rename_note(&path, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn notes_delete(root: String, path: String) -> R<()> {
    open(&root)?.delete_note(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn notes_search(root: String, query: String) -> R<Vec<NoteHit>> {
    open(&root)?.search_notes(&query, 50).map_err(|e| e.to_string())
}

/// The open project's identity for its to-do list (`None` = no project: Inbox and user lists only).
fn project_ref(root: Option<&str>) -> R<Option<ProjectRef>> {
    match root.filter(|r| !r.is_empty()) {
        Some(r) => Ok(Some(ProjectRef::of(&open(r)?))),
        None => Ok(None),
    }
}

fn e(err: impl std::fmt::Display) -> String {
    err.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoApplyResult {
    pub applied: Applied,
    pub snapshot: Snapshot,
}

/// Everything the to-do page shows (lists, sections, labels, evaluated filters, tasks).
#[tauri::command]
pub fn todos_snapshot(state: State<'_, NotesState>, root: Option<String>) -> R<Snapshot> {
    let project = project_ref(root.as_deref())?;
    let _g = state.lock.lock().unwrap_or_else(|p| p.into_inner());
    let data = TodoStore::read(&state.data_dir).map_err(e)?;
    Ok(data.snapshot(project.as_ref(), &Now::local()))
}

/// Applies one change and returns the new snapshot.
#[tauri::command]
pub fn todos_apply(state: State<'_, NotesState>, root: Option<String>, op: TodoOp) -> R<TodoApplyResult> {
    let project = project_ref(root.as_deref())?;
    let _g = state.lock.lock().unwrap_or_else(|p| p.into_inner());
    let mut store = TodoStore::open(&state.data_dir).map_err(e)?;
    let now = Now::local();
    let applied = store.apply(op, project.as_ref(), &now).map_err(e)?;
    Ok(TodoApplyResult { applied, snapshot: store.data.snapshot(project.as_ref(), &now) })
}

/// Live quick-add parsing for the chips under the input (nothing is written).
#[tauri::command]
pub fn todos_parse(state: State<'_, NotesState>, root: Option<String>, text: String, list_id: Option<String>, ignore: Option<Vec<String>>) -> R<QuickAdd> {
    let project = project_ref(root.as_deref())?;
    let mut data = TodoStore::read(&state.data_dir).map_err(e)?;
    data.ensure_lists(project.as_ref());
    Ok(data.parse_quick_add(&text, list_id.as_deref(), &ignore.unwrap_or_default(), &Now::local()))
}

/// Natural-language date for the due/deadline pickers (`dateOnly` for deadlines).
#[tauri::command]
pub fn todos_parse_date(text: String, date_only: Option<bool>) -> R<Option<Due>> {
    let ctx = DateCtx::local_now();
    if date_only.unwrap_or(false) { dates::parse_date(&text, &ctx).map(|d| d.map(Due::on)) } else { dates::parse_due(&text, &ctx) }
}

/// Runs an ad-hoc filter query (the filter editor's preview, search).
#[tauri::command]
pub fn todos_filter(state: State<'_, NotesState>, query: String) -> R<Vec<FilterGroup>> {
    TodoStore::read(&state.data_dir).map_err(e)?.run_filter(&query, None, &Now::local())
}

/// Completion history, newest first (`before`: Unix ms cursor for paging).
#[tauri::command]
pub fn todos_completed(state: State<'_, NotesState>, list_id: Option<String>, before: Option<i64>, limit: Option<usize>) -> R<Vec<CompletedEntry>> {
    Ok(TodoStore::read(&state.data_dir).map_err(e)?.completed(list_id.as_deref(), before, limit.unwrap_or(100).min(500)))
}

/// Shows a sample reminder so the user can check notifications work (and grant them on macOS).
#[tauri::command]
pub fn todos_notify_test(app: AppHandle) -> R<()> {
    let n = app.notification();
    let _ = n.request_permission();
    n.builder().title("Reminders are on").body("You'll get a notification like this when a task is due.").show().map_err(e)
}

/// Desktop reminders for due tasks while the app is running: the store is checked every 20 s for
/// reminders in the elapsed window (see `TodoData::due_reminders`). Clicking a notification opens
/// the task (`todos://open`); its "Complete" button completes it (`todos://changed`).
pub fn start_reminders(app: &AppHandle) {
    let shown: Arc<Mutex<HashMap<i32, String>>> = Arc::default();
    let n = app.notification();
    let _ = n.register_action_types(vec![ActionType::builder("todo").actions(vec![Action::builder("complete", "Complete").build()]).build()]);
    {
        let app = app.clone();
        let shown = shown.clone();
        let _ = n.on_action(move |a| {
            let task = a.notification().and_then(|n| shown.lock().ok()?.get(&n.id()).cloned());
            let Some(task) = task else { return };
            if a.action_id() == "complete" {
                let state = app.state::<NotesState>();
                let _g = state.lock.lock().unwrap_or_else(|p| p.into_inner());
                if let Ok(mut store) = TodoStore::open(&state.data_dir) {
                    let _ = store.apply(TodoOp::Complete { ids: vec![task], done: true }, None, &Now::local());
                }
                let _ = app.emit("todos://changed", ());
            } else {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                let _ = app.emit("todos://open", task);
            }
        });
    }
    // To-dos changed by another process (an agent over MCP, the `devall` CLI): refresh the UI.
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let file = app.state::<NotesState>().data_dir.join("todos").join(devall_core::todos::FILE);
            let stamp = |f: &std::path::Path| std::fs::metadata(f).and_then(|m| m.modified()).ok();
            let mut seen = stamp(&file);
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let now = stamp(&file);
                if now != seen {
                    seen = now;
                    let _ = app.emit("todos://changed", ());
                }
            }
        });
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last = chrono::Utc::now().timestamp_millis();
        let mut next_id = (last % 1_000_000) as i32;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            let now = chrono::Utc::now().timestamp_millis();
            let data_dir = app.state::<NotesState>().data_dir.clone();
            if let Ok(data) = TodoStore::read(&data_dir) {
                for r in data.due_reminders(last, now, &chrono::Local) {
                    next_id = next_id.wrapping_add(1);
                    if let Ok(mut m) = shown.lock() {
                        if m.len() > 500 {
                            m.clear();
                        }
                        m.insert(next_id, r.task_id.clone());
                    }
                    let _ = app.notification().builder().id(next_id).title(&r.title).body(&r.body).action_type_id("todo").show();
                }
            }
            last = now;
        }
    });
}

/// Context documents for the local AI (notes, personal to-dos, request outline), optionally ranked
/// by relevance to `query`.
#[tauri::command]
pub fn context_docs(state: State<'_, NotesState>, root: String, query: Option<String>, limit: Option<usize>) -> R<Vec<ContextDoc>> {
    let p = open(&root)?;
    let docs = context::collect(&p, Some(&state.data_dir), &ContextOptions::default()).map_err(|e| e.to_string())?;
    Ok(match query.filter(|q| !q.trim().is_empty()) {
        Some(q) => context::rank(&docs, &q, limit.unwrap_or(8)).into_iter().cloned().collect(),
        None => docs,
    })
}

#[tauri::command]
pub fn notes_create_notebook(root: String, folder: String, title: String) -> R<String> {
    open(&root)?.create_notebook(&folder, &title).map_err(|e| e.to_string())
}
