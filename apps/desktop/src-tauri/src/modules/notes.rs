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
use devall_core::todos::{attachments, Applied, Attachment, CompletedEntry, Due, FilterGroup, Now, ProjectRef, Snapshot, TodoOp, TodoStore};
use devall_core::Project;
use serde::{Deserialize, Serialize};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::{Action, ActionType, NotificationExt};
use tauri_plugin_opener::OpenerExt;

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoAttachResult {
    pub added: Vec<Attachment>,
    /// Files that couldn't be attached, with the reason (too large, unreadable…).
    pub errors: Vec<String>,
    pub snapshot: Snapshot,
}

/// Attaches files to a task: the native file dialog is opened here, so the UI never hands over
/// paths; copies go to `<app data>/todos/attachments/<task id>/`. `None` when cancelled.
#[tauri::command]
pub async fn todos_attachment_pick(app: AppHandle, state: State<'_, NotesState>, root: Option<String>, task_id: String) -> R<Option<TodoAttachResult>> {
    let project = project_ref(root.as_deref())?;
    let mut dialog = app.dialog().file().set_title("Attach files");
    if let Some(w) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&w);
    }
    let Some(files) = dialog.blocking_pick_files() else { return Ok(None) };
    let now = Now::local();
    // Copy first, without the lock (files can be large: sync to-do commands wait for it), then record.
    let mut existing = existing_attachments(&state, &task_id)?;
    let (mut copied, mut errors) = (Vec::new(), Vec::new());
    for f in files {
        match f.into_path().map_err(e).and_then(|p| attachments::store_file(&state.data_dir, &task_id, &p, &existing, now.ms).map_err(e)) {
            Ok(a) => {
                existing.push(a.clone());
                copied.push(a);
            }
            Err(err) => errors.push(err),
        }
    }
    let _g = state.lock.lock().unwrap_or_else(|p| p.into_inner());
    let mut store = TodoStore::open(&state.data_dir).map_err(e)?;
    let mut added = Vec::new();
    for a in copied {
        match store.record_attachment(&task_id, a, project.as_ref(), &now) {
            Ok(a) => added.push(a),
            Err(err) => errors.push(err.to_string()),
        }
    }
    Ok(Some(TodoAttachResult { added, errors, snapshot: store.data.snapshot(project.as_ref(), &now) }))
}

/// The task's attachments right now (for unique file names), or an error when the task is gone.
fn existing_attachments(state: &NotesState, task_id: &str) -> R<Vec<Attachment>> {
    let data = TodoStore::read(&state.data_dir).map_err(e)?;
    Ok(data.task(task_id).ok_or_else(|| format!("task {task_id} not found"))?.attachments.clone())
}

/// What `todos_attachment_upload` stores (the `x-meta` header: JSON with non-ASCII `\u` escaped).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadMeta {
    root: Option<String>,
    task_id: String,
    name: String,
    mime: Option<String>,
}

/// Attaches dropped or pasted bytes (the raw request body) to a task; the name is sanitized and
/// the size capped (`attachments::MAX_BYTES`) in `devall_core`.
#[tauri::command]
pub async fn todos_attachment_upload(state: State<'_, NotesState>, request: Request<'_>) -> R<TodoAttachResult> {
    let InvokeBody::Raw(bytes) = request.body() else { return Err("expected the file's bytes".into()) };
    let meta = request.headers().get("x-meta").and_then(|v| v.to_str().ok()).ok_or("missing x-meta header")?;
    let meta: UploadMeta = serde_json::from_str(meta).map_err(e)?;
    let project = project_ref(meta.root.as_deref())?;
    let now = Now::local();
    let existing = existing_attachments(&state, &meta.task_id)?;
    let a = attachments::store_bytes(&state.data_dir, &meta.task_id, &meta.name, meta.mime.as_deref(), bytes, &existing, now.ms).map_err(e)?;
    let _g = state.lock.lock().unwrap_or_else(|p| p.into_inner());
    let mut store = TodoStore::open(&state.data_dir).map_err(e)?;
    let a = store.record_attachment(&meta.task_id, a, project.as_ref(), &now).map_err(e)?;
    Ok(TodoAttachResult { added: vec![a], errors: Vec::new(), snapshot: store.data.snapshot(project.as_ref(), &now) })
}

/// The validated file of an attachment (never a path from the UI).
fn attachment_file(state: &NotesState, task_id: &str, attachment_id: &str) -> R<PathBuf> {
    let path = TodoStore::read(&state.data_dir).map_err(e)?.attachment_path(&state.data_dir, task_id, attachment_id).map_err(e)?;
    if !path.is_file() {
        return Err(format!("{} is missing from this device", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    }
    Ok(path)
}

/// An attachment's bytes (image previews), as a raw response.
#[tauri::command]
pub async fn todos_attachment_read(state: State<'_, NotesState>, task_id: String, attachment_id: String) -> R<Response> {
    let path = attachment_file(&state, &task_id, &attachment_id)?;
    if std::fs::metadata(&path).map_err(e)?.len() > attachments::MAX_BYTES {
        return Err("too large to preview".into());
    }
    Ok(Response::new(std::fs::read(&path).map_err(e)?))
}

/// Opens an attachment with its default app. Programs and scripts are revealed in the file
/// manager instead (returns `false`), so a click never runs code.
#[tauri::command]
pub async fn todos_attachment_open(app: AppHandle, state: State<'_, NotesState>, task_id: String, attachment_id: String) -> R<bool> {
    let path = attachment_file(&state, &task_id, &attachment_id)?;
    if path.file_name().is_some_and(|n| attachments::opens_as_program(&n.to_string_lossy())) {
        app.opener().reveal_item_in_dir(&path).map_err(e)?;
        return Ok(false);
    }
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(e)?;
    Ok(true)
}

/// Shows an attachment in Explorer / Finder.
#[tauri::command]
pub async fn todos_attachment_reveal(app: AppHandle, state: State<'_, NotesState>, task_id: String, attachment_id: String) -> R<()> {
    let path = attachment_file(&state, &task_id, &attachment_id)?;
    app.opener().reveal_item_in_dir(&path).map_err(e)
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
    // Attachment files nothing has referenced for a week (deleted tasks, removed attachments).
    {
        let dir = app.state::<NotesState>().data_dir.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = TodoStore::gc_attachments(&dir, chrono::Utc::now().timestamp_millis());
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
