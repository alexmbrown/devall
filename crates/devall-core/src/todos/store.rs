//! The personal to-do store: `<app data>/todos/todos.json`, never inside a project. All edits go
//! through [`TodoOp`]s applied to [`TodoData`] (pure, testable); [`TodoStore`] adds the file and the
//! one-time migration from v1.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};

use super::dates::{self, DateCtx};
use super::filter::{self, EvalCtx};
use super::migrate;
use super::model::*;
use super::quickadd::{self, ParseCtx, QuickAdd};
use crate::error::{Error, IoContext, Result};
use crate::fsutil::atomic_write;

/// The store's file name inside `<app data>/todos/`.
pub const FILE: &str = "todos.json";
/// Completed tasks older than this aren't part of a [`Snapshot`] (see [`TodoData::completed`]).
const SNAPSHOT_DONE_DAYS: i64 = 7;
const MAX_HISTORY: usize = 200;

/// The /dev/all project whose list is in scope (the open project).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub id: String,
    pub name: String,
    pub root: String,
}

impl ProjectRef {
    pub fn new(id: impl Into<String>, name: impl Into<String>, root: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), root: root.into() }
    }

    pub fn of(project: &crate::Project) -> Self {
        Self::new(project.id(), project.manifest.name.clone(), project.root().to_string_lossy().to_string())
    }
}

/// The current time for an operation (injected so tests are deterministic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    /// Unix ms.
    pub ms: i64,
    pub ctx: DateCtx,
}

impl Now {
    pub fn local() -> Self {
        Self { ms: chrono::Utc::now().timestamp_millis(), ctx: DateCtx::local_now() }
    }
}

fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

fn yes() -> bool {
    true
}

/// A task to create. `text` is quick-add text (parsed unless `parse` is false); the other fields
/// are defaults that the text can override (`#list`, `p1`, a date…).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTask {
    pub text: String,
    #[serde(default = "yes")]
    pub parse: bool,
    /// Token texts not to parse (chips the user turned off).
    #[serde(default)]
    pub ignore: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub list_id: Option<String>,
    pub section_id: Option<String>,
    pub parent_id: Option<String>,
    /// Insert right after this sibling (default: at the end).
    pub after_id: Option<String>,
    pub priority: Option<Priority>,
    pub due: Option<Due>,
    /// Natural-language due date (`next fri 5pm`, `every mon`).
    pub due_string: Option<String>,
    /// `YYYY-MM-DD` or natural language.
    pub deadline: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub links: Vec<String>,
}

impl NewTask {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            parse: true,
            ignore: Vec::new(),
            description: String::new(),
            list_id: None,
            section_id: None,
            parent_id: None,
            after_id: None,
            priority: None,
            due: None,
            due_string: None,
            deadline: None,
            labels: Vec::new(),
            links: Vec::new(),
        }
    }

    pub fn in_list(mut self, list_id: impl Into<String>) -> Self {
        self.list_id = Some(list_id.into());
        self
    }
}

/// Partial update. Missing fields stay; `due: null` / `deadline: null` clear.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPatch {
    pub content: Option<String>,
    pub description: Option<String>,
    pub priority: Option<Priority>,
    #[serde(default, deserialize_with = "tri")]
    pub due: Option<Option<Due>>,
    /// Natural language; `""` clears the due date.
    pub due_string: Option<String>,
    /// `YYYY-MM-DD`: moves the date, keeping time and recurrence; `""` clears the due date.
    pub due_date: Option<String>,
    /// `YYYY-MM-DD` or natural language; `null`/`""` clears.
    #[serde(default, deserialize_with = "tri")]
    pub deadline: Option<Option<String>>,
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub add_labels: Vec<String>,
    #[serde(default)]
    pub remove_labels: Vec<String>,
    pub links: Option<Vec<String>>,
    #[serde(default)]
    pub add_links: Vec<String>,
    pub collapsed: Option<bool>,
}

/// Every change to the store.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TodoOp {
    Add { task: NewTask },
    /// Quick-add text applied to an existing task (inline edit): content plus any attributes typed;
    /// text with only attributes (`tomorrow p2`) keeps the content.
    Edit { id: String, text: String, #[serde(default)] ignore: Vec<String> },
    Update { ids: Vec<String>, patch: TaskPatch },
    /// Completing a recurring task moves it to the next occurrence; un-completing an open recurring
    /// task undoes its last completion.
    Complete { ids: Vec<String>, done: bool },
    Delete { ids: Vec<String> },
    /// Moves tasks (with their sub-tasks) to a list/section/parent, at `index` among the siblings.
    Move { ids: Vec<String>, list_id: String, section_id: Option<String>, parent_id: Option<String>, index: Option<usize> },
    /// Puts tasks back exactly as given (undo).
    Restore { tasks: Vec<Task> },
    AddComment { id: String, text: String },
    DeleteComment { id: String, comment_id: String },
    SaveList { id: Option<String>, name: String, color: Option<String> },
    DeleteList { id: String },
    ReorderLists { ids: Vec<String> },
    SaveSection { id: Option<String>, list_id: String, name: String, collapsed: Option<bool> },
    DeleteSection { id: String },
    ReorderSections { ids: Vec<String> },
    SaveLabel { name: String, new_name: Option<String>, color: Option<String> },
    DeleteLabel { name: String },
    SaveFilter { id: Option<String>, name: String, query: String, color: Option<String> },
    DeleteFilter { id: String },
    ReorderFilters { ids: Vec<String> },
    SaveSettings { settings: TodoSettings },
    /// Deletes completed tasks (of one list, or all).
    ClearCompleted { list_id: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rescheduled {
    pub id: String,
    #[serde(with = "ymd")]
    pub date: NaiveDate,
}

/// What an operation did (for the UI's toasts and the CLI).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    /// Id of a created task/list/section/filter.
    pub created: Option<String>,
    /// Recurring tasks moved to their next occurrence.
    pub rescheduled: Vec<Rescheduled>,
    /// Number of tasks affected.
    pub count: usize,
}

/// A task plus values the UI needs but can't compute itself.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    /// Local calendar day the task is due (differs from `due.date` only for fixed time zones).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_local: Option<String>,
    /// Due instant (Unix ms) when the task has a time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterGroup {
    pub title: String,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterView {
    #[serde(flatten)]
    pub filter: SavedFilter,
    pub count: usize,
    pub groups: Vec<FilterGroup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Everything the UI shows: lists, sections, labels, filters (evaluated), settings and the tasks
/// (open ones, plus completed ones from the last days and done sub-tasks of open tasks).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub lists: Vec<TaskList>,
    pub sections: Vec<Section>,
    pub labels: Vec<Label>,
    pub filters: Vec<FilterView>,
    pub settings: TodoSettings,
    pub tasks: Vec<TaskView>,
    /// The open project's list id.
    pub project_list: Option<String>,
    /// All completed tasks plus recurring completions (for the Completed view's badge).
    pub completed_count: usize,
    pub today: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedEntry {
    pub task: TaskView,
    /// Unix ms.
    pub at: i64,
    /// A completion of a recurring task (the task itself is still open).
    pub recurring: bool,
    #[serde(with = "opt_ymd", skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<NaiveDate>,
}

/// A due desktop notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub task_id: String,
    pub title: String,
    pub body: String,
    /// Unix ms.
    pub at: i64,
}

fn default_filters() -> Vec<SavedFilter> {
    vec![
        SavedFilter { id: new_id(), name: "Priority 1".into(), query: "p1".into(), color: Some("red".into()), order: 0 },
        SavedFilter { id: new_id(), name: "Next 7 days".into(), query: "overdue | 7 days".into(), color: Some("blue".into()), order: 1 },
    ]
}

fn norm_color(c: Option<String>) -> Option<String> {
    c.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty() && c.len() <= 16 && c.chars().all(|x| x.is_ascii_alphanumeric() || x == '-'))
}

fn ymd_text(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn not_found(what: &str, id: &str) -> Error {
    Error::NotFound(format!("{what} {id}"))
}

impl TodoData {
    /// A fresh store with the Inbox and two example filters.
    pub fn new_store() -> Self {
        let mut d = TodoData { filters: default_filters(), ..Default::default() };
        d.ensure_lists(None);
        d
    }

    /// Makes sure the Inbox and the scope project's list exist (and the latter's name is current).
    /// Returns whether anything changed.
    pub fn ensure_lists(&mut self, project: Option<&ProjectRef>) -> bool {
        let mut changed = false;
        if !self.lists.iter().any(|l| l.id == INBOX) {
            self.lists.insert(0, TaskList { id: INBOX.into(), name: "Inbox".into(), kind: ListKind::Inbox, color: None, order: -1, project_root: None });
            changed = true;
        }
        if let Some(p) = project {
            match self.lists.iter_mut().find(|l| l.id == p.id) {
                Some(l) => {
                    if !p.name.is_empty() && l.name != p.name {
                        l.name = p.name.clone();
                        changed = true;
                    }
                    if !p.root.is_empty() && l.project_root.as_deref() != Some(p.root.as_str()) {
                        l.project_root = Some(p.root.clone());
                        changed = true;
                    }
                }
                None => {
                    let order = self.lists.iter().map(|l| l.order).max().unwrap_or(0) + 1;
                    let name = if p.name.is_empty() { "Project".to_string() } else { p.name.clone() };
                    self.lists.push(TaskList { id: p.id.clone(), name, kind: ListKind::Project, color: None, order, project_root: Some(p.root.clone()).filter(|r| !r.is_empty()) });
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn task_mut(&mut self, id: &str) -> Result<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id).ok_or_else(|| not_found("task", id))
    }

    pub fn list(&self, id: &str) -> Option<&TaskList> {
        self.lists.iter().find(|l| l.id == id)
    }

    /// Resolves a task id, or a unique prefix/suffix of one (for the CLI's short ids).
    pub fn find_id(&self, short: &str) -> Result<String> {
        let s = short.trim().to_uppercase();
        if let Some(t) = self.tasks.iter().find(|t| t.id == s) {
            return Ok(t.id.clone());
        }
        let hits: Vec<&Task> = self.tasks.iter().filter(|t| s.len() >= 4 && (t.id.ends_with(&s) || t.id.starts_with(&s))).collect();
        match hits.as_slice() {
            [one] => Ok(one.id.clone()),
            [] => Err(not_found("task", short)),
            _ => Err(Error::Invalid(format!("`{short}` matches {} tasks — use more characters", hits.len()))),
        }
    }

    /// Ids of `id` and all its descendants.
    pub fn subtree(&self, id: &str) -> Vec<String> {
        let mut out = vec![id.to_string()];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            out.extend(self.tasks.iter().filter(|t| t.parent_id.as_deref() == Some(cur.as_str())).map(|t| t.id.clone()));
            i += 1;
        }
        out
    }

    fn ancestors(&self, id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = self.task(id).and_then(|t| t.parent_id.clone());
        while let Some(p) = cur {
            if out.contains(&p) {
                break;
            }
            cur = self.task(&p).and_then(|t| t.parent_id.clone());
            out.push(p);
        }
        out
    }

    /// Canonical names for `names`, creating labels that don't exist yet.
    fn ensure_labels(&mut self, names: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in names {
            let n = n.trim().trim_start_matches('@').trim();
            if n.is_empty() || n.contains(char::is_whitespace) {
                continue;
            }
            let canon = match self.labels.iter().find(|l| l.name.eq_ignore_ascii_case(n) || l.name.to_lowercase() == n.to_lowercase()) {
                Some(l) => l.name.clone(),
                None => {
                    self.labels.push(Label { name: n.to_string(), color: None });
                    n.to_string()
                }
            };
            if !out.iter().any(|x| x.eq_ignore_ascii_case(&canon)) {
                out.push(canon);
            }
        }
        out
    }

    fn next_order(&self, list_id: &str, section_id: Option<&str>, parent_id: Option<&str>) -> i64 {
        self.tasks.iter().filter(|t| t.list_id == list_id && t.section_id.as_deref() == section_id && t.parent_id.as_deref() == parent_id).map(|t| t.order + 1).max().unwrap_or(0)
    }

    /// Renumbers the siblings of a slot, placing `moving` (in order) at `index`.
    fn place(&mut self, list_id: &str, section_id: Option<&str>, parent_id: Option<&str>, moving: &[String], index: Option<usize>) {
        let mut siblings: Vec<(i64, String)> = self
            .tasks
            .iter()
            .filter(|t| t.list_id == list_id && t.section_id.as_deref() == section_id && t.parent_id.as_deref() == parent_id && !moving.contains(&t.id))
            .map(|t| (t.order, t.id.clone()))
            .collect();
        siblings.sort();
        let mut ids: Vec<String> = siblings.into_iter().map(|(_, id)| id).collect();
        let at = index.unwrap_or(ids.len()).min(ids.len());
        for (k, id) in moving.iter().enumerate() {
            ids.insert(at + k, id.clone());
        }
        let pos: HashMap<&str, i64> = ids.iter().enumerate().map(|(i, id)| (id.as_str(), i as i64)).collect();
        for t in &mut self.tasks {
            if let Some(o) = pos.get(t.id.as_str()) {
                t.order = *o;
            }
        }
    }

    /// Quick-add parsing against this store's lists, sections and labels.
    pub fn parse_quick_add(&self, text: &str, default_list: Option<&str>, ignore: &[String], now: &Now) -> QuickAdd {
        let ctx = ParseCtx { dates: now.ctx, lists: &self.lists, sections: &self.sections, labels: &self.labels, default_list, ignore };
        quickadd::parse(text, &ctx)
    }

    fn parse_deadline(&self, s: &str, now: &Now) -> Result<Option<NaiveDate>> {
        dates::parse_date(s, &now.ctx).map_err(Error::Invalid)
    }

    fn check_section(&self, list_id: &str, section_id: Option<String>) -> Option<String> {
        section_id.filter(|s| self.sections.iter().any(|x| x.id == *s && x.list_id == list_id))
    }

    fn add(&mut self, input: NewTask, now: &Now) -> Result<String> {
        let mut q = if input.parse {
            self.parse_quick_add(&input.text, input.list_id.as_deref().or(Some(INBOX)), &input.ignore, now)
        } else {
            QuickAdd { content: input.text.trim().to_string(), ..Default::default() }
        };
        if q.content.trim().is_empty() {
            return Err(Error::Invalid("the task has no content".into()));
        }
        let parent = match input.parent_id.as_deref() {
            Some(p) => Some(self.task(p).cloned().ok_or_else(|| not_found("task", p))?),
            None => None,
        };
        let (list_id, section_id) = match &parent {
            Some(p) => (p.list_id.clone(), p.section_id.clone()),
            None => {
                let list = q.list_id.take().or(input.list_id.clone()).unwrap_or_else(|| INBOX.to_string());
                if self.list(&list).is_none() {
                    return Err(not_found("list", &list));
                }
                let section = q.section_id.take().or_else(|| if input.list_id.as_deref() == Some(list.as_str()) { input.section_id.clone() } else { None });
                let section = self.check_section(&list, section);
                (list, section)
            }
        };
        let due = match (q.due.take(), &input.due_string) {
            (Some(d), _) => Some(d),
            (None, _) if q.no_date => None,
            (None, Some(s)) if !s.trim().is_empty() => dates::parse_due(s, &now.ctx).map_err(Error::Invalid)?,
            _ => input.due.clone(),
        };
        let deadline = match (q.deadline, input.deadline.as_deref()) {
            (Some(d), _) => Some(d),
            (None, Some(s)) => self.parse_deadline(s, now)?,
            _ => None,
        };
        let mut names = input.labels.clone();
        names.extend(q.labels.iter().cloned());
        let labels = self.ensure_labels(&names);
        let mut links: Vec<String> = Vec::new();
        for l in input.links.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
            if !links.iter().any(|x| x == l) {
                links.push(l.to_string());
            }
        }
        let id = new_id();
        let parent_id = parent.map(|p| p.id);
        let task = Task {
            id: id.clone(),
            content: q.content.trim().to_string(),
            description: input.description.trim_end().to_string(),
            list_id: list_id.clone(),
            section_id: section_id.clone(),
            parent_id: parent_id.clone(),
            order: self.next_order(&list_id, section_id.as_deref(), parent_id.as_deref()),
            priority: q.priority.or(input.priority).unwrap_or_default(),
            due,
            deadline,
            labels,
            links,
            done: false,
            completed_at: None,
            created_at: now.ms,
            updated_at: now.ms,
            comments: Vec::new(),
            history: Vec::new(),
            collapsed: false,
        };
        self.tasks.push(task);
        if let Some(after) = input.after_id.as_deref()
            && let Some(a) = self.task(after).filter(|a| a.list_id == list_id && a.section_id == section_id && a.parent_id == parent_id).map(|a| a.order)
        {
            let mut siblings: Vec<(i64, String)> = self
                .tasks
                .iter()
                .filter(|t| t.list_id == list_id && t.section_id == section_id && t.parent_id == parent_id && t.id != id)
                .map(|t| (t.order, t.id.clone()))
                .collect();
            siblings.sort();
            let index = siblings.iter().position(|(o, _)| *o > a).unwrap_or(siblings.len());
            self.place(&list_id, section_id.as_deref(), parent_id.as_deref(), std::slice::from_ref(&id), Some(index));
        }
        Ok(id)
    }

    fn edit(&mut self, id: &str, text: &str, ignore: &[String], now: &Now) -> Result<()> {
        let list = self.task(id).ok_or_else(|| not_found("task", id))?.list_id.clone();
        let q = self.parse_quick_add(text, Some(&list), ignore, now);
        let labels = self.ensure_labels(&q.labels);
        let t = self.task_mut(id)?;
        // Only attributes typed (`tomorrow p2`): the content stays.
        if !q.content.trim().is_empty() {
            t.content = q.content.trim().to_string();
        }
        if let Some(d) = q.due {
            t.due = Some(d);
        } else if q.no_date {
            t.due = None;
        }
        if let Some(p) = q.priority {
            t.priority = p;
        }
        if let Some(d) = q.deadline {
            t.deadline = Some(d);
        }
        for l in labels {
            if !t.labels.iter().any(|x| x.eq_ignore_ascii_case(&l)) {
                t.labels.push(l);
            }
        }
        t.updated_at = now.ms;
        let (cur_list, cur_section, parent) = (t.list_id.clone(), t.section_id.clone(), t.parent_id.clone());
        let new_list = q.list_id.unwrap_or(cur_list.clone());
        let new_section = if q.section_id.is_some() { q.section_id } else if new_list == cur_list { cur_section.clone() } else { None };
        if new_list != cur_list || new_section != cur_section {
            let parent = if new_list == cur_list && new_section == cur_section { parent } else { None };
            self.move_tasks(&[id.to_string()], &new_list, new_section, parent, None, now)?;
        }
        Ok(())
    }

    fn update(&mut self, ids: &[String], patch: TaskPatch, now: &Now) -> Result<usize> {
        let due_string = match patch.due_string.as_deref() {
            Some(s) if !s.trim().is_empty() => Some(dates::parse_due(s, &now.ctx).map_err(Error::Invalid)?),
            Some(_) => Some(None),
            None => None,
        };
        let due_date = match patch.due_date.as_deref() {
            Some(s) if !s.trim().is_empty() => Some(Some(NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| Error::Invalid(format!("expected a YYYY-MM-DD date, got {s}")))?)),
            Some(_) => Some(None),
            None => None,
        };
        let deadline = match &patch.deadline {
            Some(Some(s)) => Some(self.parse_deadline(s, now)?),
            Some(None) => Some(None),
            None => None,
        };
        let set_labels = patch.labels.as_ref().map(|l| self.ensure_labels(l));
        let add_labels = self.ensure_labels(&patch.add_labels);
        for id in ids {
            let t = self.task_mut(id)?;
            if let Some(c) = patch.content.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                t.content = c.to_string();
            }
            if let Some(d) = &patch.description {
                t.description = d.trim_end().to_string();
            }
            if let Some(p) = patch.priority {
                t.priority = p;
            }
            if let Some(d) = &patch.due {
                t.due = d.clone();
            }
            if let Some(d) = &due_string {
                t.due = d.clone();
            }
            match due_date {
                Some(Some(date)) => match &mut t.due {
                    Some(due) => due.date = date,
                    None => t.due = Some(Due::on(date)),
                },
                Some(None) => t.due = None,
                None => {}
            }
            if let Some(d) = deadline {
                t.deadline = d;
            }
            if let Some(l) = &set_labels {
                t.labels = l.clone();
            }
            for l in &add_labels {
                if !t.labels.iter().any(|x| x.eq_ignore_ascii_case(l)) {
                    t.labels.push(l.clone());
                }
            }
            t.labels.retain(|l| !patch.remove_labels.iter().any(|r| r.trim_start_matches('@').eq_ignore_ascii_case(l)));
            if let Some(l) = &patch.links {
                t.links = l.iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            }
            for l in patch.add_links.iter().map(|x| x.trim()).filter(|x| !x.is_empty()) {
                if !t.links.iter().any(|x| x == l) {
                    t.links.push(l.to_string());
                }
            }
            if let Some(c) = patch.collapsed {
                t.collapsed = c;
            }
            t.updated_at = now.ms;
        }
        Ok(ids.len())
    }

    fn complete(&mut self, ids: &[String], done: bool, now: &Now) -> Result<(usize, Vec<Rescheduled>)> {
        let mut rescheduled = Vec::new();
        let mut count = 0;
        for id in ids {
            let t = self.task(id).ok_or_else(|| not_found("task", id))?.clone();
            if done {
                if t.done {
                    continue;
                }
                count += 1;
                if let Some(due) = t.due.as_ref().filter(|d| d.is_recurring())
                    && let Some(next) = dates::next_occurrence(due, now.ctx.today)
                {
                    let sub = self.subtree(id);
                    let t = self.task_mut(id)?;
                    t.history.push(Completion { at: now.ms, due: due.date });
                    if t.history.len() > MAX_HISTORY {
                        t.history.remove(0);
                    }
                    if let Some(d) = &mut t.due {
                        d.date = next;
                    }
                    t.updated_at = now.ms;
                    // Sub-tasks come back for the next occurrence.
                    for s in sub.iter().skip(1) {
                        let st = self.task_mut(s)?;
                        st.done = false;
                        st.completed_at = None;
                    }
                    rescheduled.push(Rescheduled { id: id.clone(), date: next });
                    continue;
                }
                for s in self.subtree(id) {
                    let st = self.task_mut(&s)?;
                    if !st.done {
                        st.done = true;
                        st.completed_at = Some(now.ms);
                        st.updated_at = now.ms;
                    }
                }
            } else if t.done {
                count += 1;
                let mut reopen = self.ancestors(id);
                reopen.push(id.clone());
                for a in reopen {
                    let at = self.task_mut(&a)?;
                    if at.done {
                        at.done = false;
                        at.completed_at = None;
                        at.updated_at = now.ms;
                    }
                }
            } else if let Some(last) = t.history.last().cloned() {
                // Undo the last completion of a recurring task.
                count += 1;
                let at = self.task_mut(id)?;
                at.history.pop();
                if let Some(d) = &mut at.due {
                    d.date = last.due;
                }
                at.updated_at = now.ms;
            }
        }
        Ok((count, rescheduled))
    }

    fn delete(&mut self, ids: &[String]) -> usize {
        let mut gone: HashSet<String> = HashSet::new();
        for id in ids {
            if self.task(id).is_some() {
                gone.extend(self.subtree(id));
            }
        }
        self.tasks.retain(|t| !gone.contains(&t.id));
        gone.len()
    }

    fn move_tasks(&mut self, ids: &[String], list_id: &str, section_id: Option<String>, parent_id: Option<String>, index: Option<usize>, now: &Now) -> Result<usize> {
        let (list_id, section_id) = match parent_id.as_deref() {
            Some(p) => {
                let pt = self.task(p).ok_or_else(|| not_found("task", p))?;
                for id in ids {
                    if self.subtree(id).iter().any(|x| x == p) {
                        return Err(Error::Invalid("a task can't become a sub-task of itself".into()));
                    }
                }
                (pt.list_id.clone(), pt.section_id.clone())
            }
            None => {
                if self.list(list_id).is_none() {
                    return Err(not_found("list", list_id));
                }
                (list_id.to_string(), self.check_section(list_id, section_id))
            }
        };
        // Moving a task and one of its descendants: the descendant follows its ancestor.
        let roots: Vec<String> = ids.iter().filter(|id| !self.ancestors(id).iter().any(|a| ids.contains(a))).cloned().collect();
        for id in &roots {
            self.task(id).ok_or_else(|| not_found("task", id))?;
            let sub = self.subtree(id);
            for s in &sub {
                let t = self.task_mut(s)?;
                t.list_id = list_id.clone();
                t.section_id = section_id.clone();
                if s == id {
                    t.parent_id = parent_id.clone();
                }
                t.updated_at = now.ms;
            }
        }
        self.place(&list_id, section_id.as_deref(), parent_id.as_deref(), &roots, index);
        Ok(roots.len())
    }

    fn restore(&mut self, tasks: Vec<Task>) -> usize {
        let n = tasks.len();
        for mut t in tasks {
            if self.list(&t.list_id).is_none() {
                t.list_id = INBOX.into();
                t.section_id = None;
            }
            t.section_id = self.check_section(&t.list_id, t.section_id.take());
            match self.tasks.iter_mut().find(|x| x.id == t.id) {
                Some(x) => *x = t,
                None => self.tasks.push(t),
            }
        }
        let names: Vec<String> = self.tasks.iter().flat_map(|t| t.labels.clone()).collect();
        self.ensure_labels(&names);
        self.fix_orphans();
        n
    }

    /// Sub-tasks whose parent is gone become top-level.
    fn fix_orphans(&mut self) {
        let ids: HashSet<String> = self.tasks.iter().map(|t| t.id.clone()).collect();
        for t in &mut self.tasks {
            if t.parent_id.as_ref().is_some_and(|p| !ids.contains(p) || *p == t.id) {
                t.parent_id = None;
            }
        }
    }

    /// Applies an operation in memory.
    pub fn apply(&mut self, op: TodoOp, project: Option<&ProjectRef>, now: &Now) -> Result<Applied> {
        self.ensure_lists(project);
        let mut out = Applied::default();
        match op {
            TodoOp::Add { mut task } => {
                // Without an explicit list, a task goes to the open project's list (else the Inbox).
                if task.list_id.is_none() && task.parent_id.is_none() {
                    task.list_id = project.map(|p| p.id.clone());
                }
                out.created = Some(self.add(task, now)?);
                out.count = 1;
            }
            TodoOp::Edit { id, text, ignore } => {
                self.edit(&id, &text, &ignore, now)?;
                out.count = 1;
            }
            TodoOp::Update { ids, patch } => out.count = self.update(&ids, patch, now)?,
            TodoOp::Complete { ids, done } => {
                let (n, r) = self.complete(&ids, done, now)?;
                out.count = n;
                out.rescheduled = r;
            }
            TodoOp::Delete { ids } => out.count = self.delete(&ids),
            TodoOp::Move { ids, list_id, section_id, parent_id, index } => out.count = self.move_tasks(&ids, &list_id, section_id, parent_id, index, now)?,
            TodoOp::Restore { tasks } => out.count = self.restore(tasks),
            TodoOp::AddComment { id, text } => {
                if text.trim().is_empty() {
                    return Err(Error::Invalid("the comment is empty".into()));
                }
                let cid = new_id();
                let t = self.task_mut(&id)?;
                t.comments.push(Comment { id: cid.clone(), text: text.trim_end().to_string(), at: now.ms });
                t.updated_at = now.ms;
                out.created = Some(cid);
            }
            TodoOp::DeleteComment { id, comment_id } => {
                let t = self.task_mut(&id)?;
                t.comments.retain(|c| c.id != comment_id);
                t.updated_at = now.ms;
            }
            TodoOp::SaveList { id, name, color } => {
                let name = name.trim().to_string();
                let color = norm_color(color);
                match id {
                    Some(id) => {
                        let l = self.lists.iter_mut().find(|l| l.id == id).ok_or_else(|| not_found("list", &id))?;
                        if l.kind == ListKind::User && !name.is_empty() {
                            l.name = name;
                        }
                        l.color = color;
                    }
                    None => {
                        if name.is_empty() {
                            return Err(Error::Invalid("the list needs a name".into()));
                        }
                        let id = new_id();
                        let order = self.lists.iter().map(|l| l.order).max().unwrap_or(0) + 1;
                        self.lists.push(TaskList { id: id.clone(), name, kind: ListKind::User, color, order, project_root: None });
                        out.created = Some(id);
                    }
                }
            }
            TodoOp::DeleteList { id } => {
                let l = self.list(&id).ok_or_else(|| not_found("list", &id))?;
                if l.kind == ListKind::Inbox {
                    return Err(Error::Invalid("the Inbox can't be deleted".into()));
                }
                let before = self.tasks.len();
                self.tasks.retain(|t| t.list_id != id);
                self.sections.retain(|s| s.list_id != id);
                self.lists.retain(|l| l.id != id);
                out.count = before - self.tasks.len();
            }
            TodoOp::ReorderLists { ids } => {
                for l in &mut self.lists {
                    if let Some(i) = ids.iter().position(|x| *x == l.id) {
                        l.order = i as i64;
                    }
                }
            }
            TodoOp::SaveSection { id, list_id, name, collapsed } => {
                let name = name.trim().to_string();
                match id {
                    Some(id) => {
                        let s = self.sections.iter_mut().find(|s| s.id == id).ok_or_else(|| not_found("section", &id))?;
                        if !name.is_empty() {
                            s.name = name;
                        }
                        if let Some(c) = collapsed {
                            s.collapsed = c;
                        }
                    }
                    None => {
                        if self.list(&list_id).is_none() {
                            return Err(not_found("list", &list_id));
                        }
                        if name.is_empty() {
                            return Err(Error::Invalid("the section needs a name".into()));
                        }
                        let id = new_id();
                        let order = self.sections.iter().filter(|s| s.list_id == list_id).map(|s| s.order + 1).max().unwrap_or(0);
                        self.sections.push(Section { id: id.clone(), list_id, name, order, collapsed: collapsed.unwrap_or(false) });
                        out.created = Some(id);
                    }
                }
            }
            TodoOp::DeleteSection { id } => {
                if !self.sections.iter().any(|s| s.id == id) {
                    return Err(not_found("section", &id));
                }
                let before = self.tasks.len();
                self.tasks.retain(|t| t.section_id.as_deref() != Some(id.as_str()));
                self.sections.retain(|s| s.id != id);
                self.fix_orphans();
                out.count = before - self.tasks.len();
            }
            TodoOp::ReorderSections { ids } => {
                for s in &mut self.sections {
                    if let Some(i) = ids.iter().position(|x| *x == s.id) {
                        s.order = i as i64;
                    }
                }
            }
            TodoOp::SaveLabel { name, new_name, color } => {
                let canon = self.ensure_labels(std::slice::from_ref(&name)).pop().ok_or_else(|| Error::Invalid("labels are one word".into()))?;
                let color = norm_color(color);
                let new_name = new_name.map(|n| n.trim().trim_start_matches('@').to_string()).filter(|n| !n.is_empty() && *n != canon);
                if let Some(n) = &new_name {
                    if n.contains(char::is_whitespace) {
                        return Err(Error::Invalid("labels are one word".into()));
                    }
                    if self.labels.iter().any(|l| l.name.eq_ignore_ascii_case(n) && !l.name.eq_ignore_ascii_case(&canon)) {
                        return Err(Error::AlreadyExists(format!("label {n}")));
                    }
                    for t in &mut self.tasks {
                        for l in &mut t.labels {
                            if *l == canon {
                                *l = n.clone();
                            }
                        }
                    }
                }
                if let Some(l) = self.labels.iter_mut().find(|l| l.name == canon) {
                    l.color = color;
                    if let Some(n) = new_name {
                        l.name = n;
                    }
                }
            }
            TodoOp::DeleteLabel { name } => {
                self.labels.retain(|l| !l.name.eq_ignore_ascii_case(&name));
                for t in &mut self.tasks {
                    let before = t.labels.len();
                    t.labels.retain(|l| !l.eq_ignore_ascii_case(&name));
                    out.count += before - t.labels.len();
                }
            }
            TodoOp::SaveFilter { id, name, query, color } => {
                filter::parse(&query, &now.ctx).map_err(Error::Invalid)?;
                let name = name.trim().to_string();
                let color = norm_color(color);
                match id {
                    Some(id) => {
                        let f = self.filters.iter_mut().find(|f| f.id == id).ok_or_else(|| not_found("filter", &id))?;
                        if !name.is_empty() {
                            f.name = name;
                        }
                        f.query = query.trim().to_string();
                        f.color = color;
                    }
                    None => {
                        let id = new_id();
                        let order = self.filters.iter().map(|f| f.order + 1).max().unwrap_or(0);
                        let name = if name.is_empty() { query.trim().to_string() } else { name };
                        self.filters.push(SavedFilter { id: id.clone(), name, query: query.trim().to_string(), color, order });
                        out.created = Some(id);
                    }
                }
            }
            TodoOp::DeleteFilter { id } => self.filters.retain(|f| f.id != id),
            TodoOp::ReorderFilters { ids } => {
                for f in &mut self.filters {
                    if let Some(i) = ids.iter().position(|x| *x == f.id) {
                        f.order = i as i64;
                    }
                }
            }
            TodoOp::SaveSettings { settings } => {
                if let Some(t) = settings.all_day_reminder.as_deref().filter(|t| !t.is_empty())
                    && NaiveTime::parse_from_str(t, "%H:%M").is_err()
                {
                    return Err(Error::Invalid(format!("reminder time must be HH:MM, got {t}")));
                }
                self.settings = TodoSettings { all_day_reminder: settings.all_day_reminder.filter(|t| !t.is_empty()), ..settings };
            }
            TodoOp::ClearCompleted { list_id } => {
                let before = self.tasks.len();
                self.tasks.retain(|t| !(t.done && list_id.as_deref().is_none_or(|l| t.list_id == l)));
                self.fix_orphans();
                out.count = before - self.tasks.len();
            }
        }
        Ok(out)
    }

    fn view<Tz: TimeZone>(&self, t: &Task, local: &Tz) -> TaskView {
        let due_local = t.due.as_ref().map(|d| ymd_text(dates::local_due_date(d, local)));
        let due_at = t.due.as_ref().and_then(|d| dates::due_instant(d, local)).map(|i| i.timestamp_millis());
        TaskView { task: t.clone(), due_local, due_at }
    }

    /// Open tasks matching a filter query, grouped like the query (`a, b`), sorted by due date,
    /// time and priority. `lists` restricts the search (MCP: the project's list only).
    pub fn run_filter_in<Tz: TimeZone>(&self, query: &str, lists: Option<&[String]>, now: &Now, local: &Tz) -> std::result::Result<Vec<FilterGroup>, String> {
        let q = filter::parse(query, &now.ctx)?;
        let list_names: HashMap<&str, &str> = self.lists.iter().map(|l| (l.id.as_str(), l.name.as_str())).collect();
        let section_names: HashMap<&str, &str> = self.sections.iter().map(|s| (s.id.as_str(), s.name.as_str())).collect();
        let list_name = |id: &str| list_names.get(id).map(|s| s.to_string());
        let section_name = |id: &str| section_names.get(id).map(|s| s.to_string());
        let due_date = |t: &Task| t.due.as_ref().map(|d| dates::local_due_date(d, local));
        let created = |t: &Task| chrono::DateTime::from_timestamp_millis(t.created_at).map(|d| d.with_timezone(local).date_naive()).unwrap_or(now.ctx.today);
        let cx = EvalCtx { today: now.ctx.today, list_name: &list_name, section_name: &section_name, due_date: &due_date, created: &created };
        let list_order: HashMap<&str, i64> = self.lists.iter().map(|l| (l.id.as_str(), l.order)).collect();
        let open: Vec<&Task> = self.tasks.iter().filter(|t| !t.done && lists.is_none_or(|ls| ls.contains(&t.list_id))).collect();
        Ok(q.groups
            .iter()
            .map(|(title, e)| {
                let mut hits: Vec<&&Task> = open.iter().filter(|t| e.matches(t, &cx)).collect();
                hits.sort_by_key(|t| {
                    let d = due_date(t);
                    (d.is_none(), d, t.due.as_ref().and_then(|d| d.time).is_none(), t.due.as_ref().and_then(|d| d.time), t.priority, list_order.get(t.list_id.as_str()).copied().unwrap_or(0), t.order)
                });
                FilterGroup { title: title.clone(), ids: hits.into_iter().map(|t| t.id.clone()).collect() }
            })
            .collect())
    }

    pub fn snapshot_in<Tz: TimeZone>(&self, project: Option<&ProjectRef>, now: &Now, local: &Tz) -> Snapshot {
        let mut data = self.clone();
        data.ensure_lists(project);
        let open: HashSet<&str> = data.tasks.iter().filter(|t| !t.done).map(|t| t.id.as_str()).collect();
        let cutoff = now.ms - SNAPSHOT_DONE_DAYS * 86_400_000;
        let tasks = data
            .tasks
            .iter()
            .filter(|t| !t.done || t.completed_at.unwrap_or(0) >= cutoff || t.parent_id.as_deref().is_some_and(|p| open.contains(p)))
            .map(|t| data.view(t, local))
            .collect();
        let mut filters: Vec<FilterView> = data
            .filters
            .iter()
            .map(|f| match data.run_filter_in(&f.query, None, now, local) {
                Ok(groups) => FilterView { filter: f.clone(), count: groups.iter().map(|g| g.ids.len()).sum(), groups, error: None },
                Err(e) => FilterView { filter: f.clone(), count: 0, groups: Vec::new(), error: Some(e) },
            })
            .collect();
        filters.sort_by_key(|f| f.filter.order);
        let mut lists = data.lists.clone();
        lists.sort_by_key(|l| (l.kind != ListKind::Inbox, l.order));
        let mut sections = data.sections.clone();
        sections.sort_by_key(|s| s.order);
        let mut labels = data.labels.clone();
        labels.sort_by_key(|l| l.name.to_lowercase());
        let completed_count = data.tasks.iter().filter(|t| t.done).count() + data.tasks.iter().map(|t| t.history.len()).sum::<usize>();
        Snapshot {
            lists,
            sections,
            labels,
            filters,
            settings: data.settings.clone(),
            tasks,
            project_list: project.map(|p| p.id.clone()),
            completed_count,
            today: ymd_text(now.ctx.today),
        }
    }

    pub fn snapshot(&self, project: Option<&ProjectRef>, now: &Now) -> Snapshot {
        self.snapshot_in(project, now, &chrono::Local)
    }

    pub fn run_filter(&self, query: &str, lists: Option<&[String]>, now: &Now) -> std::result::Result<Vec<FilterGroup>, String> {
        self.run_filter_in(query, lists, now, &chrono::Local)
    }

    /// Completion history, newest first: completed tasks and completions of recurring tasks.
    pub fn completed(&self, list_id: Option<&str>, before: Option<i64>, limit: usize) -> Vec<CompletedEntry> {
        let mut out: Vec<CompletedEntry> = Vec::new();
        for t in self.tasks.iter().filter(|t| list_id.is_none_or(|l| t.list_id == l)) {
            if t.done
                && let Some(at) = t.completed_at
            {
                out.push(CompletedEntry { task: self.view(t, &chrono::Local), at, recurring: false, occurrence: t.due.as_ref().map(|d| d.date) });
            }
            for h in &t.history {
                out.push(CompletedEntry { task: self.view(t, &chrono::Local), at: h.at, recurring: true, occurrence: Some(h.due) });
            }
        }
        out.retain(|e| before.is_none_or(|b| e.at < b));
        out.sort_by_key(|e| std::cmp::Reverse(e.at));
        out.truncate(limit);
        out
    }

    /// Tasks of a list in display order (no-section tasks, then each section; sub-tasks after
    /// their parent) with their depth.
    pub fn ordered(&self, list_id: &str, include_done: bool) -> Vec<(usize, &Task)> {
        let mut out = Vec::new();
        let mut sections: Vec<&Section> = self.sections.iter().filter(|s| s.list_id == list_id).collect();
        sections.sort_by_key(|s| s.order);
        let mut slots: Vec<Option<&str>> = vec![None];
        slots.extend(sections.iter().map(|s| Some(s.id.as_str())));
        fn walk<'a>(data: &'a TodoData, list: &str, section: Option<&str>, parent: Option<&str>, depth: usize, include_done: bool, out: &mut Vec<(usize, &'a Task)>) {
            let mut kids: Vec<&Task> = data.tasks.iter().filter(|t| t.list_id == list && t.section_id.as_deref() == section && t.parent_id.as_deref() == parent && (include_done || !t.done)).collect();
            kids.sort_by_key(|t| (t.done, t.order));
            for k in kids {
                out.push((depth, k));
                if depth < 32 {
                    walk(data, list, section, Some(&k.id), depth + 1, include_done, out);
                }
            }
        }
        for s in slots {
            walk(self, list_id, s, None, 0, include_done, &mut out);
        }
        out
    }

    /// Reminders due in `(from_ms, to_ms]`. Long gaps (sleep) only look back 15 minutes, so waking
    /// up doesn't replay a day of notifications.
    pub fn due_reminders<Tz: TimeZone>(&self, from_ms: i64, to_ms: i64, local: &Tz) -> Vec<Reminder> {
        let s = &self.settings;
        if !s.reminders || to_ms <= from_ms {
            return Vec::new();
        }
        let from = from_ms.max(to_ms - 15 * 60_000);
        let in_window = |at: i64| at > from && at <= to_ms;
        let all_day = s.all_day_reminder.as_deref().and_then(|t| NaiveTime::parse_from_str(t, "%H:%M").ok());
        let list_name = |id: &str| self.list(id).map(|l| l.name.clone()).unwrap_or_default();
        let mut out = Vec::new();
        for t in self.tasks.iter().filter(|t| !t.done) {
            let Some(due) = &t.due else { continue };
            let list = list_name(&t.list_id);
            match dates::due_instant(due, local).map(|i| i.timestamp_millis()) {
                Some(at) => {
                    let time = due.time.map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
                    if in_window(at) {
                        out.push(Reminder { task_id: t.id.clone(), title: t.content.clone(), body: format!("Due now ({time}) · {list}"), at });
                    }
                    let before = s.remind_before_minutes as i64 * 60_000;
                    if before > 0 && in_window(at - before) {
                        out.push(Reminder { task_id: t.id.clone(), title: t.content.clone(), body: format!("Due in {} min ({time}) · {list}", s.remind_before_minutes), at: at - before });
                    }
                }
                None => {
                    if let Some(when) = all_day
                        && let Some(at) = dates::resolve_local(local, due.date.and_time(when)).map(|i| i.timestamp_millis())
                        && in_window(at)
                    {
                        out.push(Reminder { task_id: t.id.clone(), title: t.content.clone(), body: format!("Due today · {list}"), at });
                    }
                }
            }
        }
        out.sort_by_key(|r| r.at);
        out
    }
}

/// The store file plus pending migration bookkeeping.
pub struct TodoStore {
    dir: PathBuf,
    pub data: TodoData,
    migrated: Vec<PathBuf>,
}

impl TodoStore {
    fn file_path(base_dir: &Path) -> PathBuf {
        base_dir.join("todos").join(FILE)
    }

    fn load(base_dir: &Path) -> Result<(TodoData, Vec<PathBuf>)> {
        let path = Self::file_path(base_dir);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let data: TodoData = serde_json::from_slice(&bytes).map_err(|e| Error::Parse { path: path.clone(), message: e.to_string() })?;
                if data.version > SCHEMA_VERSION {
                    return Err(Error::Invalid(format!("{} was written by a newer /dev/all (schema v{}); update the app", path.display(), data.version)));
                }
                Ok((TodoData { version: SCHEMA_VERSION, ..data }, Vec::new()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match migrate::from_v1(&base_dir.join("todos")) {
                Some((mut data, files)) => {
                    data.filters = default_filters();
                    Ok((data, files))
                }
                None => Ok((TodoData::new_store(), Vec::new())),
            },
            Err(e) => Err(e).at(&path),
        }
    }

    /// Opens the store under the app data directory `base_dir` (migrating v1 lists in memory; they
    /// are converted on disk at the first save).
    pub fn open(base_dir: &Path) -> Result<Self> {
        let (data, migrated) = Self::load(base_dir)?;
        Ok(Self { dir: base_dir.join("todos"), data, migrated })
    }

    /// Reads the store without writing anything (AI context, MCP).
    pub fn read(base_dir: &Path) -> Result<TodoData> {
        Self::load(base_dir).map(|(d, _)| d)
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    pub fn save(&mut self) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.data).map_err(|e| Error::Invalid(e.to_string()))?;
        atomic_write(&self.path(), &json)?;
        if !self.migrated.is_empty() {
            migrate::backup(&self.dir, &std::mem::take(&mut self.migrated));
        }
        Ok(())
    }

    /// Applies an operation and saves.
    pub fn apply(&mut self, op: TodoOp, project: Option<&ProjectRef>, now: &Now) -> Result<Applied> {
        let out = self.data.apply(op, project, now)?;
        self.save()?;
        Ok(out)
    }

    /// Convenience: adds quick-add `text` to a list (default: the project's list, else the Inbox).
    pub fn add_text(&mut self, project: Option<&ProjectRef>, text: &str) -> Result<Task> {
        let list = project.map(|p| p.id.clone()).unwrap_or_else(|| INBOX.to_string());
        let out = self.apply(TodoOp::Add { task: NewTask::new(text).in_list(list) }, project, &Now::local())?;
        let id = out.created.unwrap_or_default();
        self.data.task(&id).cloned().ok_or_else(|| not_found("task", &id))
    }
}

#[cfg(test)]
mod tests;
