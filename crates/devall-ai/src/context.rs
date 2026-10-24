//! Project knowledge for prompts, built on `devall_core::context`: shared notes, the personal
//! to-do list, the request outline and environments (secret *names* only). Documents are ranked
//! against the question, numbered `S1…Sn`, fenced as untrusted data, and kept within a character
//! budget sized for small local models. Answers cite `[S1]`; [`citations`] maps them back to
//! clickable sources.

use std::path::Path;
use std::sync::OnceLock;

use devall_core::Project;
use devall_core::context::{self as core_ctx, ContextDoc, ContextOptions};
use devall_core::notes::{strip_front_matter, title_of};
use serde::Serialize;

use crate::Result;

/// A numbered source shown to the model and cited in answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    /// `S1`, `S2`…
    pub id: String,
    /// `note`, `todos`, `requests`, `environments`.
    pub kind: String,
    /// Stable reference: `notes/design/auth.md`, `todos (personal)`, `http/`, `environments/`.
    pub source: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_docs: usize,
    pub max_chars: usize,
    pub per_doc: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self { max_docs: 8, max_chars: 14_000, per_doc: 5_000 }
    }
}

const TODO_WORDS: &[&str] = &[
    "todo", "todos", "to-do", "to-dos", "left", "remaining", "pending", "next", "open", "unfinished", "task", "tasks", "backlog", "finish", "priority", "due", "deadline", "doing", "plan",
];

/// Whether the question is about remaining work (then the to-do list is always included).
pub fn wants_todos(question: &str) -> bool {
    let q = question.to_lowercase();
    q.split(|c: char| !(c.is_alphanumeric() || c == '-')).any(|w| TODO_WORDS.contains(&w))
}

/// Ranked context documents for `question`.
pub fn relevant(project: &Project, todo_base: Option<&Path>, question: &str, budget: Budget) -> Result<Vec<ContextDoc>> {
    let docs = core_ctx::collect(project, todo_base, &ContextOptions { max_chars_per_doc: budget.per_doc, ..Default::default() })?;
    let mut picked: Vec<ContextDoc> = core_ctx::rank(&docs, question, budget.max_docs).into_iter().cloned().collect();
    if wants_todos(question)
        && let Some(todos) = docs.iter().find(|d| d.kind == "todos")
    {
        picked.retain(|d| d.kind != "todos");
        picked.insert(0, todos.clone());
    }
    if picked.is_empty() {
        // No keyword overlap: give the model the notes and to-dos rather than nothing.
        picked = docs.iter().filter(|d| d.kind == "note" || d.kind == "todos").take(budget.max_docs).cloned().collect();
    }
    picked.truncate(budget.max_docs);
    Ok(picked)
}

/// Like [`relevant`], when a semantic index exists: the keyword rank (`devall_core::context::rank`)
/// and the vector hits for the question are fused with reciprocal-rank fusion. Hits map onto the
/// collected documents (a note chunk → its note, a request chunk → the request outline, an
/// environment → the environments list); request and mock hits also come in as documents of
/// their own (the matched chunk), so an answer can cite the exact request or stub.
pub fn relevant_hybrid(project: &Project, todo_base: Option<&Path>, question: &str, budget: Budget, hits: &[crate::semantic::SearchHit]) -> Result<Vec<ContextDoc>> {
    let mut docs = core_ctx::collect(project, todo_base, &ContextOptions { max_chars_per_doc: budget.per_doc, ..Default::default() })?;
    for h in hits.iter().filter(|h| h.kind == "request" || h.kind == "mock").take(4) {
        if !docs.iter().any(|d| d.source == h.source) {
            docs.push(ContextDoc { kind: h.kind.clone(), source: h.source.clone(), title: h.title.clone(), text: h.text.clone() });
        }
    }
    let doc_of = |h: &crate::semantic::SearchHit| -> Option<usize> {
        let target = match h.kind.as_str() {
            "note" | "request" | "mock" => h.source.clone(),
            "environment" => "environments/".to_string(),
            _ => return None,
        };
        docs.iter().position(|d| d.source == target).or_else(|| (h.kind == "request").then(|| docs.iter().position(|d| d.source == "http/")).flatten())
    };
    const K: f32 = 60.0;
    let mut score = vec![0f32; docs.len()];
    for (r, d) in core_ctx::rank(&docs, question, docs.len()).into_iter().enumerate() {
        if let Some(i) = docs.iter().position(|x| std::ptr::eq(x, d)) {
            score[i] += 1.0 / (K + r as f32 + 1.0);
        }
    }
    // Several chunks of one note: only its best rank counts.
    let mut voted = vec![false; docs.len()];
    for (r, h) in hits.iter().enumerate() {
        if let Some(i) = doc_of(h)
            && !voted[i]
        {
            voted[i] = true;
            score[i] += 1.0 / (K + r as f32 + 1.0);
        }
    }
    let mut order: Vec<usize> = (0..docs.len()).filter(|i| score[*i] > 0.0).collect();
    order.sort_by(|a, b| score[*b].total_cmp(&score[*a]).then(a.cmp(b)));
    let mut picked: Vec<ContextDoc> = order.into_iter().map(|i| docs[i].clone()).collect();
    if wants_todos(question)
        && let Some(todos) = docs.iter().find(|d| d.kind == "todos")
    {
        picked.retain(|d| d.kind != "todos");
        picked.insert(0, todos.clone());
    }
    if picked.is_empty() {
        picked = docs.iter().filter(|d| d.kind == "note" || d.kind == "todos").take(budget.max_docs).cloned().collect();
    }
    picked.truncate(budget.max_docs);
    Ok(picked)
}

/// Specific notes (paths relative to `notes/`) as context documents; all notes when empty.
pub fn notes_docs(project: &Project, paths: &[String], per_doc: usize) -> Result<Vec<ContextDoc>> {
    let cap = |s: &str| if s.chars().count() <= per_doc { s.to_string() } else { format!("{}\n…[truncated]", s.chars().take(per_doc).collect::<String>()) };
    let mut out = Vec::new();
    if paths.is_empty() {
        for (path, text) in project.all_notes()? {
            let (_, body) = strip_front_matter(&text);
            out.push(ContextDoc { kind: "note".into(), source: format!("notes/{path}"), title: title_of(&text, &path), text: cap(body) });
        }
    } else {
        for p in paths {
            let rel = p.trim_start_matches("notes/");
            let text = project.read_note(rel)?;
            let (_, body) = strip_front_matter(&text);
            out.push(ContextDoc { kind: "note".into(), source: format!("notes/{rel}"), title: title_of(&text, rel), text: cap(body) });
        }
    }
    Ok(out)
}

fn attr(s: &str) -> String {
    s.replace('"', "'").replace(['\n', '\r'], " ")
}

/// Formats documents as numbered `<source>` blocks within `max_chars`.
pub fn format_sources(docs: &[ContextDoc], max_chars: usize) -> (String, Vec<SourceRef>) {
    let mut out = String::new();
    let mut refs = Vec::new();
    for d in docs {
        let id = format!("S{}", refs.len() + 1);
        let block = format!(
            "<source id=\"{id}\" kind=\"{}\" path=\"{}\" title=\"{}\">\n{}\n</source>\n\n",
            d.kind,
            attr(&d.source),
            attr(&d.title),
            d.text.replace("</source>", "<\\/source>")
        );
        if !refs.is_empty() && out.len() + block.len() > max_chars {
            break;
        }
        out.push_str(&block);
        refs.push(SourceRef { id, kind: d.kind.clone(), source: d.source.clone(), title: d.title.clone() });
    }
    (out, refs)
}

/// Sources cited in `answer` (`[S2]`, `[S1, S3]`, or a bracketed path), in order of appearance.
pub fn citations(answer: &str, refs: &[SourceRef]) -> Vec<SourceRef> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\[([^\[\]\n]{1,200})\]").unwrap());
    let mut out: Vec<SourceRef> = Vec::new();
    for cap in re.captures_iter(answer) {
        for part in cap[1].split([',', ';']) {
            let p = part.trim();
            let hit = refs.iter().find(|r| r.id.eq_ignore_ascii_case(p) || r.source == p);
            if let Some(r) = hit
                && !out.contains(r)
            {
                out.push(r.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use devall_core::todos::{ProjectRef, TodoStore};

    #[test]
    fn todo_questions_include_the_todo_list() {
        let dir = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Shop").unwrap();
        let n = p.create_note("", "Payments").unwrap();
        p.save_note(&n, "# Payments\n\nWe use Stripe. Refunds go through the /refunds endpoint.\n").unwrap();
        let a = p.create_note("", "Auth").unwrap();
        p.save_note(&a, "# Auth\n\nPKCE for the SPA.\n").unwrap();
        let mut l = TodoStore::open(base.path()).unwrap();
        l.add_text(Some(&ProjectRef::of(&p)), "Re-sign stripe webhook replays").unwrap();
        l.add_text(Some(&ProjectRef::of(&p)), "Add idempotency keys to charges").unwrap();

        assert!(wants_todos("what's left on payments?"));
        assert!(!wants_todos("how do refunds work?"));
        let docs = relevant(&p, Some(base.path()), "what's left on payments?", Budget::default()).unwrap();
        assert_eq!(docs[0].kind, "todos");
        assert!(docs.iter().any(|d| d.source == "notes/payments.md"));

        let (text, refs) = format_sources(&docs, 14_000);
        assert!(text.contains("<source id=\"S1\" kind=\"todos\""));
        assert!(text.contains("- [ ] Add idempotency keys to charges"));
        let cited = citations("Two items remain [S1]. Stripe handles it [S2, S1]. See [notes/payments.md].", &refs);
        assert_eq!(cited[0].kind, "todos");
        assert!(cited.iter().any(|r| r.source == "notes/payments.md"));
        assert_eq!(cited.len(), 2);

        let all = notes_docs(&p, &[], 1000).unwrap();
        assert_eq!(all.len(), 2);
        let one = notes_docs(&p, &["notes/auth.md".into()], 1000).unwrap();
        assert_eq!(one[0].title, "Auth");
        assert!(notes_docs(&p, &["../devall.yaml".into()], 1000).is_err());
    }

    #[test]
    fn semantic_hits_join_the_keyword_rank() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Shop").unwrap();
        let a = p.create_note("", "Billing").unwrap();
        p.save_note(&a, "# Billing\n\nInvoices are sent monthly.\n").unwrap();
        let b = p.create_note("", "Auth").unwrap();
        p.save_note(&b, "# Auth\n\nPKCE.\n").unwrap();
        let hit = |kind: &str, source: &str, text: &str| crate::semantic::SearchHit {
            kind: kind.into(),
            source: source.into(),
            title: source.into(),
            anchor: None,
            snippet: text.into(),
            text: text.into(),
            score: 1.0,
            similarity: Some(0.8),
        };
        // No keyword overlap with "money owed", but the vectors found the billing note and a stub.
        let hits = vec![hit("note", "notes/billing.md", "Invoices are sent monthly."), hit("mock", "mocks/mappings/invoices.json", "Mock stub: GET /invoices → 200")];
        let docs = relevant_hybrid(&p, None, "where is the money owed", Budget::default(), &hits).unwrap();
        assert_eq!(docs[0].source, "notes/billing.md");
        assert!(docs.iter().any(|d| d.kind == "mock" && d.source == "mocks/mappings/invoices.json"));
        assert!(docs.iter().position(|d| d.source == "notes/auth.md").is_none_or(|i| i > 0));
    }

    #[test]
    fn budget_limits_sources() {
        let docs: Vec<ContextDoc> = (0..5).map(|i| ContextDoc { kind: "note".into(), source: format!("notes/{i}.md"), title: format!("N{i}"), text: "x".repeat(400) }).collect();
        let (_, refs) = format_sources(&docs, 1000);
        assert_eq!(refs.len(), 2);
        // A single oversized doc is still included.
        let (_, refs) = format_sources(&docs[..1], 10);
        assert_eq!(refs.len(), 1);
    }
}
