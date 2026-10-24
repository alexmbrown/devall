//! Migration from schema v1 (one file per list: `todos/<project id>.json` and `todos/_global.json`,
//! items with `text`/`done`/`priority: high|normal|low`/`due`/`tags`/`links`/`notes`).
//!
//! Mapping: `_global` → Inbox; `<project id>` → that project's list (named after `projectName`);
//! `text` → content, `notes` → description, `tags` → labels, `doneAt` → completedAt, file order →
//! order. Priorities: `high` → p1 (the only emphasis v1 had), `normal` → p4 (the default),
//! `low` → p4 plus the label `low` so nothing is lost.
//! Migrated v1 files are moved to `todos/v1-backup/` after the new file is written.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Deserialize;

use super::model::{Due, INBOX, Label, ListKind, Priority, SCHEMA_VERSION, Task, TaskList, TodoData};

pub(crate) const V1_GLOBAL: &str = "_global";
pub(crate) const BACKUP_DIR: &str = "v1-backup";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1File {
    #[serde(default)]
    project_name: String,
    #[serde(default)]
    items: Vec<V1Todo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1Todo {
    id: String,
    text: String,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    priority: String,
    #[serde(default)]
    due: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    links: Vec<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    done_at: Option<i64>,
}

/// The v1 list files in `dir` (`todos/`), sorted for a stable result.
fn v1_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json") && p.file_name().is_some_and(|n| n != super::store::FILE))
        .collect();
    files.sort();
    files
}

/// Converts the v1 files in `dir` into a v2 store. Returns `None` when there are none. Files that
/// can't be read are skipped (and left in place); the converted ones are returned for the backup.
pub(crate) fn from_v1(dir: &Path) -> Option<(TodoData, Vec<PathBuf>)> {
    let files = v1_files(dir);
    if files.is_empty() {
        return None;
    }
    let mut data = TodoData { version: SCHEMA_VERSION, ..Default::default() };
    data.lists.push(TaskList { id: INBOX.into(), name: "Inbox".into(), kind: ListKind::Inbox, color: None, order: 0, project_root: None });
    let mut converted = Vec::new();
    for path in files {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        let Ok(file) = std::fs::read(&path).map_err(|_| ()).and_then(|b| serde_json::from_slice::<V1File>(&b).map_err(|_| ())) else { continue };
        let list_id = if stem == V1_GLOBAL { INBOX.to_string() } else { stem.clone() };
        if list_id != INBOX {
            let name = if file.project_name.trim().is_empty() { stem.clone() } else { file.project_name.clone() };
            let order = data.lists.len() as i64;
            data.lists.push(TaskList { id: list_id.clone(), name, kind: ListKind::Project, color: None, order, project_root: None });
        }
        for (i, t) in file.items.into_iter().enumerate() {
            let mut labels: Vec<String> = t.tags.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let priority = match t.priority.as_str() {
                "high" => Priority::P1,
                "low" => {
                    labels.push("low".into());
                    Priority::P4
                }
                _ => Priority::P4,
            };
            labels.dedup();
            for l in &labels {
                if !data.labels.iter().any(|x| x.name.eq_ignore_ascii_case(l)) {
                    data.labels.push(Label { name: l.clone(), color: (l == "low").then(|| "gray".to_string()) });
                }
            }
            let due = t.due.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map(Due::on);
            let completed_at = if t.done { t.done_at.or(Some(t.created_at)) } else { None };
            data.tasks.push(Task {
                id: t.id,
                content: t.text,
                description: t.notes.unwrap_or_default(),
                list_id: list_id.clone(),
                section_id: None,
                parent_id: None,
                order: i as i64,
                priority,
                due,
                deadline: None,
                labels,
                links: t.links,
                done: t.done,
                completed_at,
                created_at: t.created_at,
                updated_at: completed_at.unwrap_or(t.created_at),
                comments: Vec::new(),
                attachments: Vec::new(),
                history: Vec::new(),
                collapsed: false,
            });
        }
        converted.push(path);
    }
    Some((data, converted))
}

/// Moves migrated v1 files into `todos/v1-backup/` (kept, never deleted).
pub(crate) fn backup(dir: &Path, files: &[PathBuf]) {
    let backup = dir.join(BACKUP_DIR);
    if files.is_empty() || std::fs::create_dir_all(&backup).is_err() {
        return;
    }
    for f in files {
        if let Some(name) = f.file_name() {
            let _ = std::fs::rename(f, backup.join(name));
        }
    }
}
