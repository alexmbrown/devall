//! Project knowledge for the local AI and the MCP server: shared notes, personal to-dos, and an
//! outline of requests and environments. Everything stays on the machine; secret *values* are
//! never included (environment files only hold secret names anyway).

use std::path::Path;

use serde::Serialize;

use crate::error::Result;
use crate::notes::{strip_front_matter, title_of};
use crate::project::Project;
use crate::todos::{Priority, Task, TodoData, TodoStore};
use crate::tree::TreeNode;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextDoc {
    /// `note`, `todos`, `requests`, `environments`, `repos`.
    pub kind: String,
    /// Stable reference the AI can cite, e.g. `notes/design/auth.md`.
    pub source: String,
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ContextOptions {
    pub include_notes: bool,
    pub include_todos: bool,
    pub include_requests: bool,
    /// Per-document character cap (long notes are truncated with a marker).
    pub max_chars_per_doc: usize,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self { include_notes: true, include_todos: true, include_requests: true, max_chars_per_doc: 8_000 }
    }
}

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}\n…[truncated]", s.chars().take(n).collect::<String>()) }
}

/// One task as a Markdown checkbox line: `- [ ] Ship refunds (due Sat 2026-10-24 17:00 (every
/// sat); deadline 2026-10-30; p1; @api) → http/payments/refund.req.yaml`.
pub fn task_line(t: &Task) -> String {
    task_line_with(t, None)
}

/// [`task_line`] with the board column (status) first in the details, e.g. `(in progress; due …)`.
pub fn task_line_with(t: &Task, status: Option<&str>) -> String {
    let mut line = format!("- [{}] {}", if t.done { "x" } else { " " }, t.content);
    let mut meta = Vec::new();
    if let Some(s) = status {
        meta.push(s.to_lowercase());
    }
    if let Some(d) = &t.due {
        let mut s = format!("due {}", d.date.format("%a %Y-%m-%d"));
        if let Some(time) = d.time {
            s.push_str(&format!(" {}", time.format("%H:%M")));
        }
        if let Some(z) = &d.timezone {
            s.push_str(&format!(" {z}"));
        }
        if d.is_recurring() {
            s.push_str(&format!(" ({})", if d.string.is_empty() { "recurring" } else { d.string.as_str() }));
        }
        meta.push(s);
    }
    if let Some(dl) = t.deadline {
        meta.push(format!("deadline {}", dl.format("%Y-%m-%d")));
    }
    if t.priority != Priority::P4 {
        meta.push(t.priority.to_string());
    }
    if !t.labels.is_empty() {
        meta.push(t.labels.iter().map(|l| format!("@{l}")).collect::<Vec<_>>().join(" "));
    }
    if !meta.is_empty() {
        line.push_str(&format!(" ({})", meta.join("; ")));
    }
    if !t.links.is_empty() {
        line.push_str(&format!(" → {}", t.links.join(", ")));
    }
    line
}

/// Renders rows from [`TodoData::ordered`] as Markdown: sections as headings, sub-tasks indented,
/// the first lines of descriptions under their task.
pub fn tasks_markdown(data: &TodoData, rows: &[(usize, &Task)]) -> String {
    let mut out = Vec::new();
    let mut section: Option<&str> = None;
    for (depth, t) in rows {
        if *depth == 0 && t.section_id.as_deref() != section {
            section = t.section_id.as_deref();
            if let Some(sec) = section.and_then(|s| data.sections.iter().find(|x| x.id == s)) {
                out.push(format!("### {}", sec.name));
            }
        }
        let indent = "  ".repeat(*depth);
        // The board column, for tasks between the first and the done column ("in progress").
        let s = &data.settings;
        let status = s.status_of(t).filter(|c| !c.done && Some(c.id.as_str()) != s.first_column().map(|f| f.id.as_str())).map(|c| c.name.as_str());
        out.push(format!("{indent}{}", task_line_with(t, status)));
        let desc = t.description.trim();
        if !desc.is_empty() {
            let short: String = desc.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect::<Vec<_>>().join(" / ");
            out.push(format!("{indent}  {}", cap(&short, 300).replace('\n', " ")));
        }
    }
    out.join("\n")
}

/// A list's tasks as Markdown: open ones, plus completed ones since `done_since` (Unix ms).
pub fn todos_markdown(data: &TodoData, list_id: &str, done_since: Option<i64>) -> String {
    let rows: Vec<(usize, &Task)> = data.ordered(list_id, done_since.is_some()).into_iter().filter(|(_, t)| !t.done || done_since.is_some_and(|s| t.completed_at.unwrap_or(0) >= s)).collect();
    tasks_markdown(data, &rows)
}

/// Collects context documents. `todo_base` is the app data directory holding personal to-dos.
pub fn collect(project: &Project, todo_base: Option<&Path>, opts: &ContextOptions) -> Result<Vec<ContextDoc>> {
    let mut docs = Vec::new();
    if opts.include_notes {
        for (path, text) in project.all_notes()? {
            let (_, body) = strip_front_matter(&text);
            docs.push(ContextDoc { kind: "note".into(), source: format!("notes/{path}"), title: title_of(&text, &path), text: cap(body, opts.max_chars_per_doc) });
        }
    }
    if opts.include_todos
        && let Some(base) = todo_base
        && let Ok(data) = TodoStore::read(base)
    {
        // The open project's list: open tasks and what was completed in the last two weeks.
        let since = chrono::Utc::now().timestamp_millis() - 14 * 86_400_000;
        let text = todos_markdown(&data, project.id(), Some(since));
        if !text.is_empty() {
            docs.push(ContextDoc { kind: "todos".into(), source: "todos (personal)".into(), title: "My to-dos".into(), text: cap(&text, opts.max_chars_per_doc) });
        }
    }
    if opts.include_requests {
        let mut lines = Vec::new();
        fn walk(n: &TreeNode, depth: usize, out: &mut Vec<String>) {
            match n {
                TreeNode::Folder { name, children, path } => {
                    if !path.is_empty() {
                        out.push(format!("{}- {name}/", "  ".repeat(depth)));
                    }
                    for c in children {
                        walk(c, if path.is_empty() { depth } else { depth + 1 }, out);
                    }
                }
                TreeNode::Request { name, method, path, .. } => out.push(format!("{}- {method} {name} (http/{path})", "  ".repeat(depth))),
            }
        }
        walk(&project.tree()?, 0, &mut lines);
        if !lines.is_empty() {
            docs.push(ContextDoc { kind: "requests".into(), source: "http/".into(), title: "Saved requests".into(), text: cap(&lines.join("\n"), opts.max_chars_per_doc) });
        }
        let envs = project.environments()?;
        if !envs.is_empty() {
            let text = envs
                .iter()
                .map(|e| {
                    let vars = e.file.variables.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ");
                    let secrets = if e.file.secrets.is_empty() { String::new() } else { format!("; secrets: {}", e.file.secrets.join(", ")) };
                    format!("- {} ({}): {vars}{secrets}", e.file.name, e.id)
                })
                .collect::<Vec<_>>()
                .join("\n");
            docs.push(ContextDoc { kind: "environments".into(), source: "environments/".into(), title: "Environments".into(), text });
        }
        // Multi-repo projects: which repos the shared requests/environments are for.
        let repos = project.repos();
        if !repos.is_empty() {
            docs.push(ContextDoc {
                kind: "repos".into(),
                source: "devall.yaml#repos".into(),
                title: "Repositories".into(),
                text: format!("Files in a repo are referenced as repo:<name>/<path>.\n{}", crate::repos::outline(&repos)),
            });
        }
    }
    Ok(docs)
}

/// Simple keyword relevance ranking (until local embeddings land): documents sharing more query
/// words rank higher; ties keep their original order.
pub fn rank<'a>(docs: &'a [ContextDoc], query: &str, limit: usize) -> Vec<&'a ContextDoc> {
    let words: Vec<String> = query.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).map(str::to_string).collect();
    let mut scored: Vec<(usize, usize, &ContextDoc)> = docs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let hay = format!("{} {}", d.title, d.text).to_lowercase();
            (words.iter().map(|w| hay.matches(w.as_str()).count()).sum::<usize>(), i, d)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().filter(|(s, _, _)| *s > 0 || words.is_empty()).take(limit).map(|(_, _, d)| d).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::{ProjectRef, TodoStore};

    #[test]
    fn collects_notes_todos_requests() {
        let dir = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Ctx").unwrap();
        let n = p.create_note("", "Payments").unwrap();
        p.save_note(&n, "# Payments\n\nStripe webhooks are verified with STRIPE_SECRET.\n").unwrap();
        p.create_request("", crate::HttpRequestFile::new("Create charge")).unwrap();
        let mut st = TodoStore::open(base.path()).unwrap();
        let task = st.add_text(Some(&ProjectRef::of(&p)), "Re-sign stripe webhook replays tomorrow 9am @stripe p2").unwrap();
        st.add_text(None, "Personal errand").unwrap();
        let mut sub = crate::todos::NewTask::new("Check the replay log");
        sub.parent_id = Some(task.id.clone());
        sub.description = "Look for 400s\nthen retry".into();
        st.apply(crate::todos::TodoOp::Add { task: sub }, None, &crate::todos::Now::local()).unwrap();
        let mut wip = crate::todos::NewTask::new("Rotate the signing key").in_list(p.id());
        wip.status = Some("in progress".into());
        st.apply(crate::todos::TodoOp::Add { task: wip }, None, &crate::todos::Now::local()).unwrap();

        let docs = collect(&p, Some(base.path()), &ContextOptions::default()).unwrap();
        let kinds: Vec<_> = docs.iter().map(|d| d.kind.as_str()).collect();
        assert_eq!(kinds, ["note", "todos", "requests", "environments"]);
        let todo = &docs[1].text;
        assert!(todo.starts_with("- [ ] Re-sign stripe webhook replays (due "), "{todo}");
        assert!(todo.contains(" 09:00; p2; @stripe)"), "{todo}");
        assert!(todo.contains("\n  - [ ] Check the replay log\n    Look for 400s / then retry"), "{todo}");
        assert!(todo.contains("\n- [ ] Rotate the signing key (in progress)"), "{todo}");
        assert!(!todo.contains("Personal errand"), "only the project's list");
        assert!(docs[2].text.contains("GET Create charge"));

        let top = rank(&docs, "how do we verify stripe webhooks?", 2);
        assert_eq!(top[0].source, "notes/payments.md");
    }
}
