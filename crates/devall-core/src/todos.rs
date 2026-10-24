//! Personal to-dos, Todoist style. User-specific: one file in the app's data directory
//! (`<app data>/todos/todos.json`), never in a project, so they're never committed and follow the
//! user across clones.
//!
//! - [`model`]: tasks (content, Markdown description, priority p1–p4, due date/time/zone/recurrence,
//!   deadline, labels, links, sub-tasks, comments, completion history), lists (the Inbox, one per
//!   /dev/all project, and the user's own), sections, labels, saved filters, reminder settings.
//! - [`dates`]: natural-language dates and recurrence rules (`tomorrow 5pm`, `every mon, wed`).
//! - [`quickadd`]: `Ship it tomorrow #Work /Backend @api p1 {fri}` → a task.
//! - [`filter`]: `today | overdue`, `p1 & @bug`, `due before: next week`…
//! - [`store`]: the file, every operation ([`TodoOp`]), the UI snapshot, history and reminders.
//!
//! Schema v2; v1 files (`todos/<project id>.json`, `todos/_global.json`) are migrated on the first
//! save and kept in `todos/v1-backup/`.

pub mod dates;
pub mod filter;
mod migrate;
pub mod model;
pub mod quickadd;
pub mod store;

use std::path::Path;

pub use model::*;
pub use store::*;

use crate::error::Result;

/// Tasks of one list (e.g. a project's), read without creating or migrating anything on disk.
pub fn read_list(base_dir: &Path, list_id: &str) -> Result<Vec<Task>> {
    Ok(TodoStore::read(base_dir)?.tasks.into_iter().filter(|t| t.list_id == list_id).collect())
}
