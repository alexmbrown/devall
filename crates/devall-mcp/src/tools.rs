//! The project operations behind each MCP tool. Plain functions over a [`Project`] so they are
//! easy to test; every string that leaves goes through [`Ctx::clean`] (secret values + credential
//! patterns masked — the calling agent usually runs on a cloud model).

use std::path::{Path, PathBuf};

use devall_ai::Redactor;
use devall_core::secrets::SecretResolver;
use devall_core::tree::TreeNode;
use devall_core::vars::SecretLookup;
use devall_core::{Project, Resolver, SecretStore};
use devall_http::oauth2::TokenStore;
use devall_http::{ClientPool, prepare_authorized};
use serde_json::{Value, json};

use crate::McpOptions;
use crate::approval::{self, ApprovalDecision, ApprovalRequest, Approvals, Gate, send_gate};

pub const MAX_BODY_CHARS: usize = 16_000;

/// Per-call context: the project plus a redactor for its current secrets.
pub struct Ctx<'a> {
    pub project: Project,
    pub store: &'a dyn SecretStore,
    pub options: &'a McpOptions,
    redactor: Redactor,
}

impl<'a> Ctx<'a> {
    pub fn open(root: &Path, store: &'a dyn SecretStore, options: &'a McpOptions) -> Result<Self, String> {
        let project = Project::open(root).map_err(|e| e.to_string())?;
        let redactor = Redactor::for_project(&project, store);
        Ok(Self { project, store, options, redactor })
    }

    /// Masks secrets and credential-looking strings.
    pub fn clean(&self, s: &str) -> String {
        self.redactor.redact(s, true).0
    }

    fn clean_value(&self, v: Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.clean(&s)),
            Value::Array(a) => Value::Array(a.into_iter().map(|x| self.clean_value(x)).collect()),
            Value::Object(o) => Value::Object(o.into_iter().map(|(k, x)| (k, self.clean_value(x))).collect()),
            other => other,
        }
    }

    pub fn todo_base(&self) -> Option<PathBuf> {
        self.options.app_data_dir.clone().or_else(crate::default_app_data_dir)
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn list_requests(ctx: &Ctx) -> Result<Value, String> {
    fn walk(n: &TreeNode, folder: &str, out: &mut Vec<Value>) {
        match n {
            TreeNode::Folder { name, children, path } => {
                let f = if path.is_empty() { String::new() } else if folder.is_empty() { name.clone() } else { format!("{folder}/{name}") };
                for c in children {
                    walk(c, &f, out);
                }
            }
            TreeNode::Request { name, method, path, protocol, .. } => out.push(json!({ "path": path, "name": name, "method": method, "protocol": protocol, "folder": folder })),
        }
    }
    let mut out = Vec::new();
    walk(&ctx.project.tree().map_err(err)?, "", &mut out);
    Ok(ctx.clean_value(json!({ "project": ctx.project.manifest.name, "requests": out })))
}

pub fn read_request(ctx: &Ctx, path: &str) -> Result<Value, String> {
    let req = ctx.project.read_request(path).map_err(err)?;
    let eff = ctx.project.effective_request(path, &req).map_err(err)?;
    let yaml = devall_core::fsutil::to_yaml(&req).map_err(err)?;
    Ok(ctx.clean_value(json!({
        "path": path,
        "name": req.name,
        "method": eff.request.method,
        "url": eff.request.url,
        "inheritsAuth": req.auth.is_inherit(),
        "file": yaml,
        "references": devall_core::vars::references(&serde_json::to_string(&eff.request).unwrap_or_default()),
    })))
}

pub fn list_environments(ctx: &Ctx) -> Result<Value, String> {
    let envs = ctx.project.environments().map_err(err)?;
    let list: Vec<Value> = envs
        .iter()
        .map(|e| {
            let r = SecretResolver::new(ctx.store, ctx.project.id(), Some(e.id.as_str()), ctx.project.root());
            let secrets: Vec<Value> = e.file.secrets.iter().map(|n| json!({ "name": n, "set": r.source_of(n).is_some() })).collect();
            json!({ "id": e.id, "name": e.file.name, "protected": e.file.protected, "variables": e.file.variables, "secrets": secrets })
        })
        .collect();
    Ok(ctx.clean_value(json!({ "projectVariables": ctx.project.manifest.variables, "environments": list, "note": "Secret values are never returned; reference them as {{secret.NAME}}." })))
}

pub fn search_notes(ctx: &Ctx, query: &str, limit: usize) -> Result<Value, String> {
    let hits = ctx.project.search_notes(query, limit.clamp(1, 50)).map_err(err)?;
    Ok(ctx.clean_value(serde_json::to_value(hits).map_err(err)?))
}

/// Meaning-based search over notes, requests, mocks and environment names, using the project's
/// local embedding index (`<project data>/embeddings/` on this machine). Without the index or a reachable embedding
/// model it falls back to keyword ranking and says so.
pub async fn search_project(ctx: &Ctx<'_>, query: &str, limit: usize, embedder: Option<std::sync::Arc<dyn devall_ai::embed::Embed>>) -> Result<Value, String> {
    let r = devall_ai::semantic::search(ctx.project.root(), query, limit.clamp(1, 50), embedder.as_deref(), || devall_ai::semantic::collect_chunks(&ctx.project, &ctx.redactor))
        .await
        .map_err(err)?;
    let hits: Vec<Value> = r
        .hits
        .iter()
        .map(|h| json!({ "kind": h.kind, "source": h.source, "title": h.title, "section": h.anchor, "snippet": h.snippet, "score": h.score, "similarity": h.similarity }))
        .collect();
    Ok(ctx.clean_value(json!({ "mode": r.mode, "note": r.note, "hits": hits })))
}

pub fn read_note(ctx: &Ctx, path: &str) -> Result<String, String> {
    let rel = path.trim_start_matches("notes/");
    Ok(ctx.clean(&ctx.project.read_note(rel).map_err(err)?))
}

pub fn list_notes(ctx: &Ctx) -> Result<Vec<(String, String)>, String> {
    Ok(ctx.project.all_notes().map_err(err)?.into_iter().map(|(p, text)| (devall_core::notes::title_of(&text, &p), p)).collect())
}

/// The project's personal to-do list (only that list: the user's other lists stay private),
/// optionally narrowed by a filter query (`today | overdue`, `p1 & @bug`).
pub fn list_todos(ctx: &Ctx, include_done: bool, filter: Option<&str>) -> Result<Value, String> {
    use devall_core::todos::{Now, Task, TodoStore};
    let Some(base) = ctx.todo_base() else { return Err("no app data directory: to-dos are unavailable".into()) };
    let data = TodoStore::read(&base).map_err(err)?;
    let list = ctx.project.id().to_string();
    let wanted: Option<std::collections::HashSet<String>> = match filter.map(str::trim).filter(|f| !f.is_empty()) {
        Some(q) => Some(data.run_filter(q, Some(std::slice::from_ref(&list)), &Now::local())?.into_iter().flat_map(|g| g.ids).collect()),
        None => None,
    };
    let rows: Vec<(usize, &Task)> = data.ordered(&list, include_done).into_iter().filter(|(_, t)| wanted.as_ref().is_none_or(|w| w.contains(&t.id))).collect();
    let section = |id: &Option<String>| id.as_deref().and_then(|s| data.sections.iter().find(|x| x.id == s)).map(|s| s.name.clone());
    let items: Vec<Value> = rows
        .iter()
        .map(|(depth, t)| {
            json!({
                "id": t.id,
                "content": t.content,
                "description": t.description,
                "done": t.done,
                "priority": t.priority.to_string(),
                "due": t.due.as_ref().map(|d| json!({
                    "date": d.date.format("%Y-%m-%d").to_string(),
                    "time": d.time.map(|x| x.format("%H:%M").to_string()),
                    "timezone": d.timezone,
                    "recurring": d.recurrence.is_some(),
                    "rule": (!d.string.is_empty()).then(|| d.string.clone()),
                })),
                "deadline": t.deadline.map(|d| d.format("%Y-%m-%d").to_string()),
                "labels": t.labels,
                "section": section(&t.section_id),
                "parentId": t.parent_id,
                "depth": depth,
                "links": t.links,
                "attachments": t.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
                "comments": t.comments.iter().map(|c| json!({ "text": c.text, "at": c.at })).collect::<Vec<_>>(),
                "completedAt": t.completed_at,
            })
        })
        .collect();
    let md = devall_core::context::tasks_markdown(&data, &rows);
    let note = if ctx.options.read_only_local {
        "Personal to-do list for this project (read-only here, stored outside the repo). Priorities: p1 = most urgent, p4 = none."
    } else {
        "Personal to-do list for this project (stored outside the repo; change it with add_todo, update_todo, complete_todo, comment_todo). Priorities: p1 = most urgent, p4 = none."
    };
    Ok(ctx.clean_value(json!({ "items": items, "markdown": md, "note": note })))
}

// ---------- writes: shared notes and the personal to-do list ----------

/// `path` relative to `notes/` (`.md` added when missing). Notebooks are edited in the app only:
/// they are validated JSON, and agents have no use for raw cell JSON.
fn note_rel(path: &str) -> Result<String, String> {
    let rel = path.trim().trim_start_matches('/').trim_start_matches("notes/");
    if rel.is_empty() || rel.ends_with('/') {
        return Err("`path` must name a note, e.g. design/auth.md".into());
    }
    if rel.ends_with(".ipynb") {
        return Err("notebooks (.ipynb) can't be edited through MCP; edit Markdown notes instead".into());
    }
    Ok(if rel.ends_with(".md") { rel.to_string() } else { format!("{rel}.md") })
}

fn read_existing_note(ctx: &Ctx, rel: &str) -> Result<Option<String>, String> {
    match ctx.project.read_note(rel) {
        Ok(s) => Ok(Some(s)),
        Err(devall_core::Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(err(e)),
    }
}

/// Saves a note and labels the change "agent" in the project's local file history.
fn save_agent_note(ctx: &Ctx, rel: &str, content: &str) -> Result<(), String> {
    ctx.project.save_note(rel, content).map_err(err)?;
    devall_core::local::mark_write(&ctx.project.notes_root().join(rel), devall_core::local::WriteSource::Agent);
    Ok(())
}

fn note_written(rel: &str, created: bool, content: &str) -> Value {
    json!({ "path": rel, "created": created, "chars": content.chars().count(), "note": "Shared project note (committed with the repo); the user sees it in /dev/all → Notes." })
}

/// Creates a note, or replaces one when `overwrite` is set.
pub fn write_note(ctx: &Ctx, path: &str, content: &str, overwrite: bool) -> Result<Value, String> {
    let rel = note_rel(path)?;
    let existing = read_existing_note(ctx, &rel)?;
    if let Some(old) = &existing {
        if !overwrite {
            return Err(format!("notes/{rel} already exists: pass overwrite: true to replace it, or use edit_note / append_to_note"));
        }
        // The agent only ever saw a masked copy: writing it back would replace the real values.
        if ctx.clean(old) != *old {
            return Err(format!("notes/{rel} contains values that were masked for you (••••••); replacing the whole note would destroy them. Use edit_note or append_to_note."));
        }
    }
    save_agent_note(ctx, &rel, content)?;
    Ok(note_written(&rel, existing.is_none(), content))
}

/// Appends `text` as a new paragraph (creating the note when missing).
pub fn append_to_note(ctx: &Ctx, path: &str, text: &str) -> Result<Value, String> {
    let rel = note_rel(path)?;
    let existing = read_existing_note(ctx, &rel)?;
    let content = match &existing {
        Some(old) if !old.trim().is_empty() => format!("{}\n\n{}\n", old.trim_end(), text.trim_end()),
        _ => format!("{}\n", text.trim_end()),
    };
    save_agent_note(ctx, &rel, &content)?;
    Ok(note_written(&rel, existing.is_none(), &content))
}

/// Replaces an exact snippet of a note (once, or every occurrence with `replace_all`).
pub fn edit_note(ctx: &Ctx, path: &str, old_text: &str, new_text: &str, replace_all: bool) -> Result<Value, String> {
    let rel = note_rel(path)?;
    if old_text.is_empty() {
        return Err("`oldText` is empty: use append_to_note to add text".into());
    }
    let Some(old) = read_existing_note(ctx, &rel)? else { return Err(format!("notes/{rel} doesn't exist: create it with write_note")) };
    // Notes may use CRLF on Windows while agents send LF.
    let (body, crlf) = if old.contains("\r\n") { (old.replace("\r\n", "\n"), true) } else { (old.clone(), false) };
    let (find, with) = (old_text.replace("\r\n", "\n"), new_text.replace("\r\n", "\n"));
    let n = body.matches(&find).count();
    if n == 0 {
        let hint = if find.contains(devall_ai::redact::MASK) { " (it contains masked text ••••••, which never matches the real note)" } else { "" };
        return Err(format!("`oldText` was not found in notes/{rel}{hint}; read_note it again and copy the text exactly"));
    }
    if n > 1 && !replace_all {
        return Err(format!("`oldText` occurs {n} times in notes/{rel}: include more surrounding text, or pass replaceAll: true"));
    }
    let mut next = if replace_all { body.replace(&find, &with) } else { body.replacen(&find, &with, 1) };
    if crlf {
        next = next.replace('\n', "\r\n");
    }
    save_agent_note(ctx, &rel, &next)?;
    Ok(json!({ "path": rel, "replacements": if replace_all { n } else { 1 } }))
}

/// The app data dir and this project's to-do list.
fn todo_scope(ctx: &Ctx) -> Result<(PathBuf, devall_core::todos::ProjectRef), String> {
    let base = ctx.todo_base().ok_or("no app data directory: to-dos are unavailable")?;
    Ok((base, devall_core::todos::ProjectRef::of(&ctx.project)))
}

/// Resolves a (short) task id, but only within this project's list: agents never see or touch
/// the user's other lists.
fn project_task(data: &devall_core::todos::TodoData, list: &str, id: &str) -> Result<String, String> {
    let full = data.find_id(id).map_err(err)?;
    match data.task(&full) {
        Some(t) if t.list_id == list => Ok(full),
        _ => Err(format!("task {id} not found in this project's to-do list")),
    }
}

fn task_json(ctx: &Ctx, data: &devall_core::todos::TodoData, id: &str) -> Value {
    let Some(t) = data.task(id) else { return json!({ "id": id }) };
    ctx.clean_value(json!({
        "id": t.id,
        "content": t.content,
        "description": t.description,
        "done": t.done,
        "priority": t.priority.to_string(),
        "due": t.due.as_ref().map(|d| json!({ "date": d.date.format("%Y-%m-%d").to_string(), "time": d.time.map(|x| x.format("%H:%M").to_string()), "recurring": d.recurrence.is_some(), "rule": (!d.string.is_empty()).then(|| d.string.clone()) })),
        "deadline": t.deadline.map(|d| d.format("%Y-%m-%d").to_string()),
        "labels": t.labels,
        "parentId": t.parent_id,
        "links": t.links,
    }))
}

/// Adds a task to the project's list. `text` uses quick-add syntax unless `parse` is false.
pub fn add_todo(ctx: &Ctx, text: &str, description: Option<&str>, parent: Option<&str>, links: Vec<String>, parse: bool) -> Result<Value, String> {
    use devall_core::todos::{NewTask, Now, TodoOp, TodoStore};
    let (base, project) = todo_scope(ctx)?;
    let mut store = TodoStore::open(&base).map_err(err)?;
    store.data.ensure_lists(Some(&project));
    let mut task = NewTask::new(text.trim()).in_list(project.id.clone());
    task.parse = parse;
    task.description = description.unwrap_or_default().to_string();
    task.links = links;
    if let Some(p) = parent.filter(|p| !p.trim().is_empty()) {
        task.parent_id = Some(project_task(&store.data, &project.id, p)?);
    }
    let out = store.apply(TodoOp::Add { task }, Some(&project), &Now::local()).map_err(err)?;
    let id = out.created.ok_or("the task was not created")?;
    Ok(json!({ "task": task_json(ctx, &store.data, &id), "note": "Added to the user's personal to-do list for this project (shown in /dev/all → To-dos)." }))
}

/// Changes a task: content/description/priority/dates/labels. Missing fields stay as they are.
pub fn update_todo(ctx: &Ctx, id: &str, args: &serde_json::Map<String, Value>) -> Result<Value, String> {
    use devall_core::todos::{Now, Priority, TaskPatch, TodoOp, TodoStore};
    let (base, project) = todo_scope(ctx)?;
    let mut store = TodoStore::open(&base).map_err(err)?;
    let full = project_task(&store.data, &project.id, id)?;
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let list = |k: &str| args.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|x| x.trim_start_matches('@').to_string()).collect::<Vec<_>>());
    let priority = match s("priority") {
        Some(p) => Some(serde_json::from_value::<Priority>(json!(p.trim().to_lowercase())).map_err(|_| format!("priority must be p1, p2, p3 or p4 (got {p})"))?),
        None => None,
    };
    let patch = TaskPatch {
        content: s("content").filter(|c| !c.trim().is_empty()),
        description: s("description"),
        priority,
        due_string: s("due"),
        deadline: args.get("deadline").map(|v| v.as_str().map(str::to_string).filter(|d| !d.is_empty())),
        labels: list("labels"),
        add_labels: list("addLabels").unwrap_or_default(),
        remove_labels: list("removeLabels").unwrap_or_default(),
        add_links: list("addLinks").unwrap_or_default(),
        ..Default::default()
    };
    store.apply(TodoOp::Update { ids: vec![full.clone()], patch }, Some(&project), &Now::local()).map_err(err)?;
    Ok(json!({ "task": task_json(ctx, &store.data, &full) }))
}

/// Completes (or reopens) a task. Completing a recurring task moves it to its next occurrence.
pub fn complete_todo(ctx: &Ctx, id: &str, done: bool) -> Result<Value, String> {
    use devall_core::todos::{Now, TodoOp, TodoStore};
    let (base, project) = todo_scope(ctx)?;
    let mut store = TodoStore::open(&base).map_err(err)?;
    let full = project_task(&store.data, &project.id, id)?;
    let out = store.apply(TodoOp::Complete { ids: vec![full.clone()], done }, Some(&project), &Now::local()).map_err(err)?;
    let next = out.rescheduled.first().map(|r| r.date.format("%Y-%m-%d").to_string());
    Ok(json!({ "task": task_json(ctx, &store.data, &full), "nextOccurrence": next }))
}

/// Adds a comment to a task (e.g. progress notes from the agent).
pub fn comment_todo(ctx: &Ctx, id: &str, text: &str) -> Result<Value, String> {
    use devall_core::todos::{Now, TodoOp, TodoStore};
    let (base, project) = todo_scope(ctx)?;
    let mut store = TodoStore::open(&base).map_err(err)?;
    let full = project_task(&store.data, &project.id, id)?;
    store.apply(TodoOp::AddComment { id: full.clone(), text: text.trim().to_string() }, Some(&project), &Now::local()).map_err(err)?;
    Ok(json!({ "task": task_json(ctx, &store.data, &full), "comments": store.data.task(&full).map(|t| t.comments.len()).unwrap_or(0) }))
}

pub fn get_context(ctx: &Ctx, query: Option<&str>, limit: usize) -> Result<Value, String> {
    let base = ctx.todo_base();
    let q = query.unwrap_or("").trim();
    let budget = devall_ai::context::Budget { max_docs: limit.clamp(1, 20), ..Default::default() };
    let docs = if q.is_empty() {
        devall_core::context::collect(&ctx.project, base.as_deref(), &Default::default()).map_err(err)?.into_iter().take(budget.max_docs).collect()
    } else {
        devall_ai::context::relevant(&ctx.project, base.as_deref(), q, budget).map_err(err)?
    };
    Ok(ctx.clean_value(json!({ "documents": docs })))
}

pub fn run_utility(op: &str, input: &str, args: &Value) -> Result<String, String> {
    if op == "list" {
        let ops: Vec<Value> = devall_utils::list().into_iter().map(|o| json!({ "id": o.id, "category": o.category, "description": o.description, "network": o.network })).collect();
        return serde_json::to_string_pretty(&ops).map_err(err);
    }
    devall_utils::info(op).ok_or_else(|| format!("unknown utility `{op}` — call run_utility with op \"list\""))?;
    devall_utils::run(op, input, args).map_err(err)
}

/// The project's repositories on this machine (listed in devall.yaml, the one containing the
/// project, discovered nested ones). The branch comes from `HEAD` (no git process); remotes are
/// shown without credentials.
pub fn list_repos(ctx: &Ctx) -> Result<Value, String> {
    use devall_core::repos::{self, RepoOrigin};
    let list: Vec<Value> = ctx
        .project
        .repos()
        .iter()
        .map(|r| {
            json!({
                "name": r.name,
                "group": r.group,
                "branch": r.is_found().then(|| repos::head_branch(&r.path)).flatten(),
                "path": r.path.display().to_string(),
                "relPath": r.rel_path,
                "remote": r.remote.as_deref().map(repos::strip_credentials),
                "state": r.state,
                "origin": r.origin,
                "listed": r.origin == RepoOrigin::Manifest,
                "projectPrefix": r.project_prefix,
            })
        })
        .collect();
    Ok(ctx.clean_value(json!({ "repos": list, "linkSyntax": "repo:<name>/<path>" })))
}

/// `mocks/mappings/**/*.json` (WireMock layout) summarised.
pub fn list_mocks(ctx: &Ctx) -> Result<Value, String> {
    let dir = ctx.project.root().join("mocks").join("mappings");
    let mut files = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")) {
                out.push(p);
            }
        }
    }
    walk(&dir, &mut files);
    files.sort();
    let mut stubs = Vec::new();
    let mut errors = Vec::new();
    for f in files {
        let rel = f.strip_prefix(ctx.project.root()).unwrap_or(&f).to_string_lossy().replace('\\', "/");
        let v: Value = match std::fs::read(&f).map_err(err).and_then(|b| serde_json::from_slice(&b).map_err(err)) {
            Ok(v) => v,
            Err(e) => {
                errors.push(json!({ "file": rel, "error": e }));
                continue;
            }
        };
        let mappings = match v.get("mappings").and_then(Value::as_array) {
            Some(list) => list.clone(),
            None => vec![v],
        };
        for m in mappings {
            let rq = &m["request"];
            let url = ["url", "urlPath", "urlPattern", "urlPathPattern", "urlPathTemplate"].iter().find_map(|k| rq[*k].as_str().map(|u| format!("{u} ({k})")));
            stubs.push(json!({
                "file": rel,
                "id": m["id"].as_str().or(m["uuid"].as_str()),
                "name": m["name"],
                "method": rq["method"].as_str().unwrap_or("ANY"),
                "url": url,
                "status": m["response"]["status"].as_u64().unwrap_or(200),
                "priority": m["priority"],
                "scenario": m["scenarioName"],
            }));
        }
    }
    Ok(ctx.clean_value(json!({ "mappings": stubs, "errors": errors })))
}

/// OAuth 2.0 tokens cached in the keychain like the desktop app does.
struct KeychainTokens<'a> {
    store: &'a dyn SecretStore,
    project_id: String,
}

impl TokenStore for KeychainTokens<'_> {
    fn get(&self, key: &str) -> Option<String> {
        self.store.get(&devall_core::SecretScope::new(self.project_id.clone(), "__oauth2"), key).ok().flatten()
    }
    fn set(&self, key: &str, value: &str) {
        let _ = self.store.set(&devall_core::SecretScope::new(self.project_id.clone(), "__oauth2"), key, value);
    }
    fn delete(&self, key: &str) {
        let _ = self.store.delete(&devall_core::SecretScope::new(self.project_id.clone(), "__oauth2"), key);
    }
}

/// Why a send is refused when no approvals are possible (the stdio CLI).
#[cfg(test)]
fn send_policy(method: &str, environment: Option<(&str, bool)>, allow_writes: bool) -> Result<(), String> {
    let mode = if allow_writes { approval::WriteMode::Allow } else { approval::WriteMode::ReadOnly };
    match send_gate(method, environment, mode, false) {
        Gate::Allow => Ok(()),
        Gate::Ask(r) | Gate::Refuse(r) => Err(r),
    }
}

const SENSITIVE_HEADERS: &[&str] = &["authorization", "proxy-authorization", "cookie", "x-api-key", "api-key", "x-auth-token", "x-access-token"];

fn clip(s: &str, n: usize) -> (String, bool) {
    if s.chars().count() <= n { (s.to_string(), false) } else { (s.chars().take(n).collect(), true) }
}

/// A redacted, size-capped view of the body for the approval prompt.
fn body_preview(body: &devall_http::PreparedBody, mask: &dyn Fn(&str) -> String) -> (Option<String>, bool) {
    use devall_http::PreparedBody;
    let text = match body {
        PreparedBody::None => return (None, false),
        PreparedBody::Bytes(b) if b.is_empty() => return (None, false),
        PreparedBody::Bytes(b) => match std::str::from_utf8(b) {
            Ok(t) => t.to_string(),
            Err(_) => format!("<{} bytes of binary data>", b.len()),
        },
        PreparedBody::Form(f) => f.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"),
        PreparedBody::Multipart(parts) => parts
            .iter()
            .map(|p| match &p.file {
                Some(f) => format!("{}: <file {}>", p.name, f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                None => format!("{}: {}", p.name, p.value),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let (t, cut) = clip(&mask(&text), approval::BODY_PREVIEW_CHARS);
    (Some(t), cut)
}

pub async fn send_request(ctx: &Ctx<'_>, pool: &ClientPool, path: &str, environment: Option<&str>, approvals: Option<&Approvals>) -> Result<Value, String> {
    let p = &ctx.project;
    let req = p.read_request(path).map_err(err)?;
    let eff = p.effective_request(path, &req).map_err(err)?;
    let env_id = environment.map(str::to_string).or_else(|| ctx.options.environment.clone());
    let env_file = env_id.as_deref().map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let env_info = env_file.as_ref().map(|e| (e.name.as_str(), e.protected));
    let mode = ctx.options.write_mode();
    // Refuse what can never be sent before resolving secrets or fetching tokens.
    if let Gate::Refuse(msg) = send_gate(&eff.request.method, env_info, mode, approvals.is_some()) {
        return Err(msg);
    }

    let secrets = SecretResolver::new(ctx.store, p.id(), env_id.as_deref(), p.root());
    let lookup: &dyn SecretLookup = &secrets;
    let mut r = Resolver::new(lookup);
    if let Some(e) = &env_file {
        r.push_layer(e.variables.clone());
    }
    for fv in &eff.folder_variables {
        r.push_layer(fv.clone());
    }
    r.push_layer(p.manifest.variables.clone());
    let tokens = KeychainTokens { store: ctx.store, project_id: p.id().to_string() };
    let prepared = prepare_authorized(&eff.request, &mut r, p.root(), &tokens, None).await.map_err(|e| ctx.clean(&e.redacted(&r.secret_values).to_string()))?;
    let gate = send_gate(&prepared.method, env_info, mode, approvals.is_some());
    if !r.unresolved.is_empty() {
        let list: Vec<String> = r.unresolved.iter().map(|u| format!("{{{{{u}}}}}")).collect();
        return Err(format!("unresolved variables: {} (pass an `environment`, or set the secrets in the app)", list.join(", ")));
    }
    match gate {
        Gate::Allow => {}
        Gate::Refuse(msg) => return Err(msg),
        Gate::Ask(reason) => {
            let Some(approvals) = approvals else { return Err(reason) };
            let mask = |t: &str| ctx.clean(&r.redact(t));
            let (body_preview, body_truncated) = body_preview(&prepared.body, &mask);
            let headers = prepared
                .headers
                .iter()
                .map(|(k, v)| {
                    let sensitive = SENSITIVE_HEADERS.contains(&k.to_ascii_lowercase().as_str());
                    (k.clone(), if sensitive { "••••••".to_string() } else { mask(v) })
                })
                .collect();
            let request = ApprovalRequest {
                id: devall_core::new_id(),
                tool: "send_request".into(),
                request_path: path.to_string(),
                request_name: req.name.clone(),
                method: prepared.method.clone(),
                url: mask(&prepared.url),
                environment_id: env_id.clone(),
                environment: env_file.as_ref().map(|e| e.name.clone()),
                protected: env_file.as_ref().is_some_and(|e| e.protected),
                reason,
                headers,
                body_preview,
                body_truncated,
                timeout_secs: approvals.timeout().as_secs(),
                created_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0),
            };
            if approvals.check(request).await == ApprovalDecision::Deny {
                return Err(format!(
                    "The user denied this {} request in /dev/all (or did not answer within {} s). Do not retry it automatically; ask the user what to do.",
                    prepared.method,
                    approvals.timeout().as_secs()
                ));
            }
        }
    }
    let resp = pool.execute(&prepared).await.map_err(|e| ctx.clean(&e.redacted(&r.secret_values).to_string()))?;
    // Snapshots are compared but never rewritten from an agent.
    let cx = devall_http::checks::CheckContext { project_root: Some(p.root()), request_path: Some(path), update_snapshots: false };
    let tests = devall_http::checks::evaluate_in(&eff.request.tests, &resp, &mut r, &cx);
    let mask = |s: &str| ctx.clean(&r.redact(s));
    let text = String::from_utf8_lossy(&resp.body);
    let truncated = text.chars().count() > MAX_BODY_CHARS;
    let body: String = text.chars().take(MAX_BODY_CHARS).collect();
    let headers: Vec<Value> = resp
        .headers
        .iter()
        .map(|(k, v)| {
            let sensitive = ["set-cookie", "authorization", "proxy-authenticate"].contains(&k.to_ascii_lowercase().as_str());
            json!([k, if sensitive { "••••••".to_string() } else { mask(v) }])
        })
        .collect();
    Ok(json!({
        "request": { "method": prepared.method, "url": mask(&prepared.url), "environment": env_id },
        "status": resp.status,
        "statusText": resp.status_text,
        "durationMs": resp.timings.total_ms,
        "size": resp.body_size,
        "headers": headers,
        "body": mask(&body),
        "bodyTruncated": truncated || resp.body_truncated,
        "tests": tests.iter().map(|t| json!({ "label": mask(&t.label), "passed": t.passed, "actual": t.actual.as_deref().map(mask), "message": t.message })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy() {
        assert!(send_policy("get", None, false).is_ok());
        assert!(send_policy("HEAD", Some(("Dev", false)), false).is_ok());
        assert!(send_policy("POST", None, false).unwrap_err().contains("--allow-writes"));
        assert!(send_policy("POST", None, true).is_ok());
        assert!(send_policy("GET", Some(("Prod", true)), true).unwrap_err().contains("protected"));
    }

    #[test]
    fn utilities() {
        assert!(run_utility("list", "", &json!({})).unwrap().contains("base64.encode"));
        assert_eq!(run_utility("base64.encode", "hi", &json!({})).unwrap(), "aGk=");
        assert!(run_utility("nope", "", &json!({})).is_err());
    }
}
