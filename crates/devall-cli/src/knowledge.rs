//! `devall notes …` and `devall todo …`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, NaiveDate};
use clap::Subcommand;
use devall_core::notes::NoteNode;
use devall_core::todos::{INBOX, ListKind, NewTask, Now, Priority, ProjectRef, Task, TodoData, TodoOp, TodoStore};

#[derive(Subcommand)]
pub enum NotesCmd {
    /// List notes.
    List {
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
    /// Full-text search.
    Search {
        query: String,
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
    /// Print a note.
    Show {
        path: String,
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
    /// Create a note and print its path.
    New {
        title: String,
        #[arg(long, default_value = "")]
        folder: String,
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
}

/// Which list `devall todo` works on.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct TodoScope {
    /// The Inbox (not tied to a project) instead of the project's list. Outside a project the
    /// Inbox is used anyway.
    #[arg(short = 'i', long, visible_alias = "global", short_alias = 'g')]
    pub inbox: bool,
    /// Project folder (default: the current directory or a parent).
    #[arg(short, long)]
    pub project: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum TodoView {
    /// The project's list (the Inbox outside a project).
    Project,
    Inbox,
    /// Overdue and due today, all lists.
    Today,
    /// The next 14 days, all lists, by day.
    Upcoming,
    /// Every list.
    All,
}

#[derive(Subcommand)]
pub enum TodoCmd {
    /// Add a task with quick-add syntax, e.g.
    /// `devall todo add "Ship refunds tomorrow 5pm #Work /Backend @api p1 {fri}"`.
    Add {
        text: String,
        /// Markdown description.
        #[arg(short, long)]
        description: Option<String>,
        /// Keep the text as is: no dates, #list, /section, @labels or p1–p4.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        scope: TodoScope,
    },
    /// List tasks: the project's list by default, a view, or a filter.
    List {
        #[arg(long, value_enum)]
        view: Option<TodoView>,
        /// Filter across all lists: "today | overdue", "p1 & @bug", "#Work & !@later",
        /// "due before: next week", "search: invoice".
        #[arg(short, long)]
        filter: Option<String>,
        /// Include completed tasks.
        #[arg(long)]
        done: bool,
        /// JSON output.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        scope: TodoScope,
    },
    /// Complete tasks (number from `list`, id or the short id in brackets). Recurring tasks move to
    /// their next date.
    Done {
        #[arg(required = true)]
        which: Vec<String>,
        #[command(flatten)]
        scope: TodoScope,
    },
    /// Reopen completed tasks (for a recurring task: undo its last completion).
    Undo {
        #[arg(required = true)]
        which: Vec<String>,
        #[command(flatten)]
        scope: TodoScope,
    },
    /// Show a task with its description, sub-tasks, comments and links.
    Show {
        which: String,
        #[command(flatten)]
        scope: TodoScope,
    },
    /// Change a task with quick-add syntax, e.g. `devall todo edit 3 "fri p2 @review"`
    /// (text with only attributes keeps the title).
    Edit {
        which: String,
        text: String,
        #[command(flatten)]
        scope: TodoScope,
    },
    /// Delete tasks (with their sub-tasks).
    Rm {
        #[arg(required = true)]
        which: Vec<String>,
        #[command(flatten)]
        scope: TodoScope,
    },
    /// Comment on a task.
    Comment {
        which: String,
        text: String,
        #[command(flatten)]
        scope: TodoScope,
    },
    /// Lists with their open task counts.
    Lists,
    /// Labels with their open task counts.
    Labels,
    /// Saved filters with their match counts.
    Filters,
    /// Show how quick add reads a line, without adding anything.
    Parse {
        text: String,
        #[command(flatten)]
        scope: TodoScope,
    },
}

pub fn notes(cmd: NotesCmd) -> Result<ExitCode> {
    match cmd {
        NotesCmd::List { project } => {
            let p = crate::find_project(project)?;
            fn walk(n: &NoteNode, depth: usize) {
                match n {
                    NoteNode::Folder { name, children, path } => {
                        if !path.is_empty() {
                            println!("{}{name}/", "  ".repeat(depth));
                        }
                        for c in children {
                            walk(c, if path.is_empty() { depth } else { depth + 1 });
                        }
                    }
                    NoteNode::Note { title, path, tags, .. } => {
                        let tags = if tags.is_empty() { String::new() } else { format!("  [{}]", tags.join(", ")) };
                        println!("{}{title}  notes/{path}{tags}", "  ".repeat(depth));
                    }
                }
            }
            walk(&p.notes_tree()?, 0);
        }
        NotesCmd::Search { query, project } => {
            let p = crate::find_project(project)?;
            for h in p.search_notes(&query, 50)? {
                println!("{}  notes/{}\n    {}", h.title, h.path, h.snippet.trim());
            }
        }
        NotesCmd::Show { path, project } => {
            let p = crate::find_project(project)?;
            print!("{}", p.read_note(path.trim_start_matches("notes/"))?);
        }
        NotesCmd::New { title, folder, project } => {
            let p = crate::find_project(project)?;
            println!("notes/{}", p.create_note(&folder, &title)?);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The project in scope (`None` = the Inbox). Outside a project the Inbox is used unless `-p` was
/// given explicitly.
fn project_ref(scope: &TodoScope) -> Result<Option<ProjectRef>> {
    if scope.inbox {
        return Ok(None);
    }
    match crate::find_project(scope.project.clone()) {
        Ok(p) => Ok(Some(ProjectRef::of(&p))),
        Err(e) if scope.project.is_some() => Err(e),
        Err(_) => Ok(None),
    }
}

fn list_of(project: Option<&ProjectRef>) -> String {
    project.map(|p| p.id.clone()).unwrap_or_else(|| INBOX.to_string())
}

fn short_id(id: &str) -> String {
    id[id.len().saturating_sub(6)..].to_lowercase()
}

fn due_label(t: &Task, today: NaiveDate) -> Option<String> {
    let d = t.due.as_ref()?;
    let days = (d.date - today).num_days();
    let mut s = match days {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        -1 => "yesterday".to_string(),
        2..=6 => d.date.format("%a").to_string(),
        _ if d.date.year() == today.year() => d.date.format("%a %d %b").to_string(),
        _ => d.date.format("%a %d %b %Y").to_string(),
    };
    if let Some(time) = d.time {
        s.push_str(&format!(" {}", time.format("%H:%M")));
    }
    if d.is_recurring() {
        s.push_str(&format!(" ({})", if d.string.is_empty() { "recurring" } else { d.string.as_str() }));
    }
    Some(if days < 0 { format!("overdue: {s}") } else { s })
}

/// `  3. [ ] Ship refunds · tomorrow 17:00 · p1 · @api  (k3x9qa)`
fn row(n: Option<usize>, depth: usize, t: &Task, data: &TodoData, today: NaiveDate, show_list: bool) -> String {
    let num = n.map(|n| format!("{n:>3}. ")).unwrap_or_else(|| "     ".into());
    let mut meta = Vec::new();
    // The board column, when it's neither the first nor the done one ("In progress").
    let s = &data.settings;
    if let Some(c) = s.status_of(t).filter(|c| !c.done && s.first_column().is_some_and(|f| f.id != c.id)) {
        meta.push(format!("[{}]", c.name));
    }
    if let Some(d) = due_label(t, today) {
        meta.push(d);
    }
    if let Some(dl) = t.deadline {
        meta.push(format!("deadline {}", dl.format("%a %d %b")));
    }
    if t.priority != Priority::P4 {
        meta.push(t.priority.to_string());
    }
    meta.extend(t.labels.iter().map(|l| format!("@{l}")));
    if show_list && let Some(l) = data.list(&t.list_id) {
        meta.push(format!("#{}", l.name));
    }
    let meta = if meta.is_empty() { String::new() } else { format!(" · {}", meta.join(" · ")) };
    format!("{num}{}[{}] {}{meta}  ({})", "  ".repeat(depth), if t.done { "x" } else { " " }, t.content, short_id(&t.id))
}

/// A task by number (from the plain `list` of this scope), id or short id.
fn resolve(data: &TodoData, project: Option<&ProjectRef>, which: &str) -> Result<String> {
    if let Ok(n) = which.trim().trim_start_matches('#').parse::<usize>() {
        let rows = data.ordered(&list_of(project), false);
        if n >= 1 && n <= rows.len() {
            return Ok(rows[n - 1].1.id.clone());
        }
        if which.len() < 4 {
            bail!("no task #{n} in this list (see `devall todo list`)");
        }
    }
    Ok(data.find_id(which)?)
}

fn print_rows(out: &mut dyn Write, data: &TodoData, rows: &[(usize, &Task)], today: NaiveDate, numbered: bool, show_list: bool) -> Result<()> {
    let mut section: Option<&str> = None;
    let mut n = 0;
    for (depth, t) in rows {
        if !show_list && *depth == 0 && t.section_id.as_deref() != section {
            section = t.section_id.as_deref();
            if let Some(s) = section.and_then(|id| data.sections.iter().find(|s| s.id == id)) {
                writeln!(out, "  ── {} ──", s.name)?;
            }
        }
        let num = (numbered && !t.done).then(|| {
            n += 1;
            n
        });
        writeln!(out, "{}", row(num, *depth, t, data, today, show_list))?;
    }
    Ok(())
}

fn by_due(data: &TodoData, ids: impl Iterator<Item = String>) -> Vec<(usize, &Task)> {
    let mut v: Vec<&Task> = ids.filter_map(|id| data.task(&id)).collect();
    v.sort_by_key(|t| (t.due.as_ref().map(|d| d.date), t.due.as_ref().and_then(|d| d.time).is_none(), t.due.as_ref().and_then(|d| d.time), t.priority));
    v.into_iter().map(|t| (0, t)).collect()
}

pub fn todo(cmd: TodoCmd) -> Result<ExitCode> {
    let base = crate::app_data_dir()?;
    run_todo(&base, cmd, &Now::local(), &mut std::io::stdout())?;
    Ok(ExitCode::SUCCESS)
}

/// Runs a `devall todo` command against the store under `base` (the app data directory).
pub(crate) fn run_todo(base: &Path, cmd: TodoCmd, now: &Now, out: &mut dyn Write) -> Result<()> {
    let today = now.ctx.today;
    let mut store = TodoStore::open(base)?;
    match cmd {
        TodoCmd::Add { text, description, raw, scope } => {
            let project = project_ref(&scope)?;
            let mut task = NewTask::new(text).in_list(list_of(project.as_ref()));
            task.parse = !raw;
            task.description = description.unwrap_or_default();
            let id = store.apply(TodoOp::Add { task }, project.as_ref(), now)?.created.unwrap_or_default();
            let t = store.data.task(&id).context("task vanished")?;
            writeln!(out, "Added{}", row(None, 0, t, &store.data, today, true).trim_start_matches(' ').replacen("[ ]", ":", 1))?;
        }
        TodoCmd::List { view, filter, done, json, scope } => {
            let project = project_ref(&scope)?;
            store.data.ensure_lists(project.as_ref());
            let data = &store.data;
            let mut json_rows: Vec<&Task> = Vec::new();
            if let Some(q) = filter {
                let groups = data.run_filter(&q, None, now).map_err(|e| anyhow!(e))?;
                for g in &groups {
                    let rows = by_due(data, g.ids.iter().cloned());
                    json_rows.extend(rows.iter().map(|(_, t)| *t));
                    if !json {
                        if groups.len() > 1 {
                            writeln!(out, "{} ({})", g.title, rows.len())?;
                        }
                        print_rows(out, data, &rows, today, false, true)?;
                    }
                }
                if !json && groups.iter().all(|g| g.ids.is_empty()) {
                    writeln!(out, "Nothing matches “{q}”.")?;
                }
            } else {
                match view.unwrap_or(if scope.inbox { TodoView::Inbox } else { TodoView::Project }) {
                    v @ (TodoView::Project | TodoView::Inbox) => {
                        let list = if v == TodoView::Inbox { INBOX.to_string() } else { list_of(project.as_ref()) };
                        let name = data.list(&list).map(|l| l.name.clone()).unwrap_or_default();
                        let rows = data.ordered(&list, done);
                        json_rows.extend(rows.iter().map(|(_, t)| *t));
                        if !json {
                            let open = rows.iter().filter(|(_, t)| !t.done).count();
                            writeln!(out, "{name} — {open} open")?;
                            if rows.is_empty() {
                                writeln!(out, "  Nothing to do. Add one: devall todo add \"…\"")?;
                            }
                            print_rows(out, data, &rows, today, list == list_of(project.as_ref()), false)?;
                        }
                    }
                    TodoView::Today => {
                        let open = || data.tasks.iter().filter(|t| !t.done && t.due.is_some());
                        let overdue = by_due(data, open().filter(|t| t.due.as_ref().is_some_and(|d| d.date < today)).map(|t| t.id.clone()));
                        let due_today = by_due(data, open().filter(|t| t.due.as_ref().is_some_and(|d| d.date == today)).map(|t| t.id.clone()));
                        json_rows.extend(overdue.iter().chain(due_today.iter()).map(|(_, t)| *t));
                        if !json {
                            if !overdue.is_empty() {
                                writeln!(out, "Overdue ({})", overdue.len())?;
                                print_rows(out, data, &overdue, today, false, true)?;
                            }
                            writeln!(out, "Today {} ({})", today.format("%a %d %b"), due_today.len())?;
                            print_rows(out, data, &due_today, today, false, true)?;
                        }
                    }
                    TodoView::Upcoming => {
                        let end = today + chrono::Duration::days(14);
                        let rows = by_due(data, data.tasks.iter().filter(|t| !t.done && t.due.as_ref().is_some_and(|d| d.date <= end)).map(|t| t.id.clone()));
                        json_rows.extend(rows.iter().map(|(_, t)| *t));
                        if !json {
                            let mut day: Option<NaiveDate> = None;
                            for r in &rows {
                                let d = r.1.due.as_ref().map(|d| d.date.max(today));
                                if d != day {
                                    day = d;
                                    if let Some(d) = d {
                                        let label = if d == today { "Today".to_string() } else if d == today + chrono::Duration::days(1) { "Tomorrow".into() } else { d.format("%a %d %b").to_string() };
                                        writeln!(out, "{label}")?;
                                    }
                                }
                                print_rows(out, data, std::slice::from_ref(r), today, false, true)?;
                            }
                            if rows.is_empty() {
                                writeln!(out, "Nothing scheduled for the next 14 days.")?;
                            }
                        }
                    }
                    TodoView::All => {
                        let mut lists = data.lists.clone();
                        lists.sort_by_key(|l| (l.kind != ListKind::Inbox, l.order));
                        for l in lists {
                            let rows = data.ordered(&l.id, done);
                            if rows.is_empty() {
                                continue;
                            }
                            json_rows.extend(rows.iter().map(|(_, t)| *t));
                            if !json {
                                writeln!(out, "{} — {} open", l.name, rows.iter().filter(|(_, t)| !t.done).count())?;
                                print_rows(out, data, &rows, today, false, false)?;
                            }
                        }
                    }
                }
            }
            if json {
                writeln!(out, "{}", serde_json::to_string_pretty(&json_rows)?)?;
            }
        }
        TodoCmd::Done { which, scope } => {
            let project = project_ref(&scope)?;
            let ids = which.iter().map(|w| resolve(&store.data, project.as_ref(), w)).collect::<Result<Vec<_>>>()?;
            let applied = store.apply(TodoOp::Complete { ids: ids.clone(), done: true }, project.as_ref(), now)?;
            for id in ids {
                let t = store.data.task(&id).context("task vanished")?;
                match applied.rescheduled.iter().find(|r| r.id == id) {
                    Some(r) => writeln!(out, "Done: {} — next: {}", t.content, r.date.format("%a %d %b"))?,
                    None => writeln!(out, "Done: {}", t.content)?,
                }
            }
        }
        TodoCmd::Undo { which, scope } => {
            let project = project_ref(&scope)?;
            let ids = which.iter().map(|w| resolve(&store.data, project.as_ref(), w)).collect::<Result<Vec<_>>>()?;
            store.apply(TodoOp::Complete { ids: ids.clone(), done: false }, project.as_ref(), now)?;
            for id in ids {
                let t = store.data.task(&id).context("task vanished")?;
                writeln!(out, "Reopened{}", row(None, 0, t, &store.data, today, true).trim_start_matches(' ').replacen("[ ]", ":", 1))?;
            }
        }
        TodoCmd::Show { which, scope } => {
            let project = project_ref(&scope)?;
            let id = resolve(&store.data, project.as_ref(), &which)?;
            let data = &store.data;
            let t = data.task(&id).context("task vanished")?;
            writeln!(out, "{}{}", if t.done { "[x] " } else { "" }, t.content)?;
            let list = data.list(&t.list_id).map(|l| l.name.clone()).unwrap_or_default();
            let section = t.section_id.as_deref().and_then(|s| data.sections.iter().find(|x| x.id == s)).map(|s| format!(" / {}", s.name)).unwrap_or_default();
            writeln!(out, "  list:     {list}{section}")?;
            if let Some(c) = data.settings.status_of(t) {
                writeln!(out, "  status:   {}", c.name)?;
            }
            if let Some(d) = due_label(t, today) {
                writeln!(out, "  due:      {d}")?;
            }
            if let Some(dl) = t.deadline {
                writeln!(out, "  deadline: {}", dl.format("%a %d %b %Y"))?;
            }
            writeln!(out, "  priority: {}", t.priority)?;
            if !t.labels.is_empty() {
                writeln!(out, "  labels:   {}", t.labels.iter().map(|l| format!("@{l}")).collect::<Vec<_>>().join(" "))?;
            }
            for l in &t.links {
                writeln!(out, "  link:     {l}")?;
            }
            writeln!(out, "  id:       {}", t.id)?;
            if !t.description.trim().is_empty() {
                writeln!(out, "\n{}", t.description.trim())?;
            }
            let sub: Vec<(usize, &Task)> = data.ordered(&t.list_id, true).into_iter().filter(|(_, x)| x.id != t.id && data.subtree(&t.id).contains(&x.id)).collect();
            if !sub.is_empty() {
                writeln!(out, "\nSub-tasks:")?;
                print_rows(out, data, &sub, today, false, false)?;
            }
            if !t.comments.is_empty() {
                writeln!(out, "\nComments:")?;
                for c in &t.comments {
                    let when = chrono::DateTime::from_timestamp_millis(c.at).map(|d| d.with_timezone(&chrono::Local).format("%d %b %H:%M").to_string()).unwrap_or_default();
                    writeln!(out, "  {when}  {}", c.text.replace('\n', "\n    "))?;
                }
            }
            if !t.history.is_empty() {
                writeln!(out, "\nCompleted {} time(s), last for {}", t.history.len(), t.history.last().map(|h| h.due.format("%a %d %b").to_string()).unwrap_or_default())?;
            }
        }
        TodoCmd::Edit { which, text, scope } => {
            let project = project_ref(&scope)?;
            let id = resolve(&store.data, project.as_ref(), &which)?;
            store.apply(TodoOp::Edit { id: id.clone(), text, ignore: Vec::new() }, project.as_ref(), now)?;
            let t = store.data.task(&id).context("task vanished")?;
            writeln!(out, "Updated{}", row(None, 0, t, &store.data, today, true).trim_start_matches(' ').replacen("[ ]", ":", 1))?;
        }
        TodoCmd::Rm { which, scope } => {
            let project = project_ref(&scope)?;
            let ids = which.iter().map(|w| resolve(&store.data, project.as_ref(), w)).collect::<Result<Vec<_>>>()?;
            let n = store.apply(TodoOp::Delete { ids }, project.as_ref(), now)?.count;
            writeln!(out, "Deleted {n} task{}", if n == 1 { "" } else { "s" })?;
        }
        TodoCmd::Comment { which, text, scope } => {
            let project = project_ref(&scope)?;
            let id = resolve(&store.data, project.as_ref(), &which)?;
            store.apply(TodoOp::AddComment { id: id.clone(), text }, project.as_ref(), now)?;
            writeln!(out, "Commented on: {}", store.data.task(&id).map(|t| t.content.as_str()).unwrap_or_default())?;
        }
        TodoCmd::Lists => {
            let mut lists = store.data.lists.clone();
            lists.sort_by_key(|l| (l.kind != ListKind::Inbox, l.order));
            for l in lists {
                let open = store.data.tasks.iter().filter(|t| t.list_id == l.id && !t.done).count();
                let kind = match l.kind {
                    ListKind::Project => format!("  (project{})", l.project_root.as_deref().map(|r| format!(": {r}")).unwrap_or_default()),
                    _ => String::new(),
                };
                writeln!(out, "{} ({open}){kind}", l.name)?;
            }
        }
        TodoCmd::Labels => {
            let mut labels = store.data.labels.clone();
            labels.sort_by_key(|l| l.name.to_lowercase());
            for l in labels {
                let open = store.data.tasks.iter().filter(|t| !t.done && t.labels.iter().any(|x| x == &l.name)).count();
                writeln!(out, "@{} ({open})", l.name)?;
            }
        }
        TodoCmd::Filters => {
            let mut filters = store.data.filters.clone();
            filters.sort_by_key(|f| f.order);
            for f in filters {
                match store.data.run_filter(&f.query, None, now) {
                    Ok(g) => writeln!(out, "{}: {} ({})", f.name, f.query, g.iter().map(|g| g.ids.len()).sum::<usize>())?,
                    Err(e) => writeln!(out, "{}: {} (error: {e})", f.name, f.query)?,
                }
            }
        }
        TodoCmd::Parse { text, scope } => {
            let project = project_ref(&scope)?;
            store.data.ensure_lists(project.as_ref());
            let list = list_of(project.as_ref());
            let q = store.data.parse_quick_add(&text, Some(&list), &[], now);
            writeln!(out, "content:  {}", q.content)?;
            let list_id = q.list_id.clone().unwrap_or(list);
            writeln!(out, "list:     {}", store.data.list(&list_id).map(|l| l.name.clone()).unwrap_or_default())?;
            if let Some(s) = q.section_id.as_deref().and_then(|s| store.data.sections.iter().find(|x| x.id == s)) {
                writeln!(out, "section:  {}", s.name)?;
            }
            if let Some(d) = &q.due {
                let mut s = d.date.format("%a %Y-%m-%d").to_string();
                if let Some(t) = d.time {
                    s.push_str(&format!(" {}", t.format("%H:%M")));
                }
                if d.is_recurring() {
                    s.push_str(&format!(" ({})", d.string));
                }
                writeln!(out, "due:      {s}")?;
            }
            if q.no_date {
                writeln!(out, "due:      (cleared)")?;
            }
            if let Some(dl) = q.deadline {
                writeln!(out, "deadline: {}", dl.format("%a %Y-%m-%d"))?;
            }
            if let Some(p) = q.priority {
                writeln!(out, "priority: {p}")?;
            }
            if !q.labels.is_empty() {
                let labels: Vec<String> = q.labels.iter().map(|l| if q.new_labels.contains(l) { format!("@{l} (new)") } else { format!("@{l}") }).collect();
                writeln!(out, "labels:   {}", labels.join(" "))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Command};
    use devall_core::Project;
    use clap::Parser;
    use devall_core::todos::dates::DateCtx;

    /// Friday 2026-10-23, 14:30.
    fn now() -> Now {
        let today = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        Now { ms: 1_792_758_600_000, ctx: DateCtx::new(today, chrono::NaiveTime::from_hms_opt(14, 30, 0).unwrap()) }
    }

    fn run(base: &Path, args: &[&str]) -> String {
        let mut argv = vec!["devall", "todo"];
        argv.extend_from_slice(args);
        let cli = Cli::try_parse_from(argv).unwrap();
        let Command::Todo(cmd) = cli.command else { panic!("not a todo command") };
        let mut out = Vec::new();
        run_todo(base, cmd, &now(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn run_err(base: &Path, args: &[&str]) -> String {
        let mut argv = vec!["devall", "todo"];
        argv.extend_from_slice(args);
        let Command::Todo(cmd) = Cli::try_parse_from(argv).unwrap().command else { panic!() };
        run_todo(base, cmd, &now(), &mut Vec::new()).unwrap_err().to_string()
    }

    #[test]
    fn todo_cli_round_trip() {
        let base = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path(), "Shop").unwrap();
        let p = dir.path().to_str().unwrap();
        let b = base.path();

        let out = run(b, &["add", "Ship refunds tomorrow 5pm @api p1", "-p", p]);
        assert!(out.starts_with("Added: Ship refunds · tomorrow 17:00 · p1 · @api · #Shop"), "{out}");
        run(b, &["add", "Write tests", "-p", p, "-d", "Cover **refunds**"]);
        run(b, &["add", "Renew domain every year on nov 1", "-i"]);
        run(b, &["add", "Pay invoice today p2", "--inbox"]);
        let out = run(b, &["add", "Call #Work tomorrow", "--raw", "-g"]);
        assert!(out.starts_with("Added: Call #Work tomorrow · #Inbox"), "{out}");

        let out = run(b, &["list", "-p", p]);
        assert!(out.starts_with("Shop — 2 open\n"), "{out}");
        assert!(out.contains("  1. [ ] Ship refunds · tomorrow 17:00 · p1 · @api  ("), "{out}");
        assert!(out.contains("  2. [ ] Write tests  ("), "{out}");
        let out = run(b, &["list", "--view", "today"]);
        assert!(out.contains("Today Fri 23 Oct (1)") && out.contains("Pay invoice · today · p2 · #Inbox"), "{out}");
        let out = run(b, &["list", "--view", "upcoming"]);
        assert!(out.contains("Tomorrow\n") && out.contains("Sun 01 Nov\n"), "{out}");
        let out = run(b, &["list", "--filter", "p1 | p2"]);
        assert!(out.contains("Ship refunds") && out.contains("Pay invoice") && !out.contains("Write tests"), "{out}");
        let out = run(b, &["list", "--filter", "overdue, today"]);
        assert!(out.contains("overdue (0)") && out.contains("today (1)"), "{out}");
        let json: serde_json::Value = serde_json::from_str(&run(b, &["list", "--json", "-p", p])).unwrap();
        assert_eq!(json.as_array().map(Vec::len), Some(2));
        assert_eq!(json[0]["priority"], 1);
        assert!(run_err(b, &["list", "--filter", "p1 &"]).contains("ends too early"));

        let out = run(b, &["done", "1", "-p", p]);
        assert_eq!(out, "Done: Ship refunds\n");
        let out = run(b, &["list", "-p", p, "--done"]);
        assert!(out.contains("  1. [ ] Write tests") && out.contains("     [x] Ship refunds"), "{out}");
        let id = {
            let data = TodoStore::read(b).unwrap();
            data.tasks.iter().find(|t| t.content == "Ship refunds").unwrap().id.clone()
        };
        let out = run(b, &["undo", &short_id(&id), "-p", p]);
        assert!(out.starts_with("Reopened: Ship refunds"), "{out}");

        let out = run(b, &["edit", "2", "fri p3 @review", "-p", p]);
        assert!(out.starts_with("Updated: Write tests · Fri 30 Oct"), "{out}");
        run(b, &["comment", "2", "Checked the CI logs", "-p", p]);
        let out = run(b, &["show", "2", "-p", p]);
        assert!(out.starts_with("Write tests\n  list:     Shop\n  status:   To do\n"), "{out}");
        assert!(out.contains("priority: p3") && out.contains("labels:   @review") && out.contains("Cover **refunds**") && out.contains("Checked the CI logs"), "{out}");

        // Recurring: completing moves it to the next date.
        let out = run(b, &["done", "1", "-i"]);
        assert_eq!(out, "Done: Renew domain — next: Mon 01 Nov\n");

        let out = run(b, &["lists"]);
        assert!(out.contains("Inbox (3)") && out.contains("Shop (2)  (project"), "{out}");
        let out = run(b, &["labels"]);
        assert!(out.contains("@api (1)") && out.contains("@review (1)"), "{out}");
        let out = run(b, &["filters"]);
        assert!(out.contains("Priority 1: p1 (1)"), "{out}");
        let out = run(b, &["parse", "Call mom tomorrow 5pm @family p2 {fri}", "-i"]);
        assert!(out.contains("content:  Call mom") && out.contains("list:     Inbox") && out.contains("due:      Sat 2026-10-24 17:00"), "{out}");
        assert!(out.contains("deadline: Fri 2026-10-30") && out.contains("priority: p2") && out.contains("@family (new)"), "{out}");

        let out = run(b, &["rm", "1", "2", "-p", p]);
        assert_eq!(out, "Deleted 2 tasks\n");
        assert!(run_err(b, &["done", "9", "-p", p]).contains("no task #9"));
        assert!(run_err(b, &["show", "zzzzzz", "-p", p]).contains("not found"));
    }
}
