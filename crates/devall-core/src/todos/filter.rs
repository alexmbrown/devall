//! A small Todoist-like filter language for saved filters, `devall todo list --filter` and MCP.
//!
//! ```text
//! today | overdue            p1 & @bug              #Work & !@later
//! due before: next week      no date                search: invoice
//! (p1 | p2) & 7 days         @review*               overdue, today      (`,` = separate groups)
//! status: in progress        !status: to do
//! ```
//!
//! Terms: `today`, `tomorrow`, `yesterday`, `overdue`/`od`, `no date`, `no time`, `recurring`,
//! `no labels`, `no deadline`, `no section`, `subtask`, `all`, `p1`…`p4`, `no priority`, `@label`,
//! `#list`, `/section` (`*` wildcards), `N days`/`next N days` (today and the following N-1 days),
//! `due: <date>`, `due before: <date>`, `due after: <date>` (also `date…`), `deadline: <date>`,
//! `deadline before:`/`after:`, `created before:`/`after:`, `search: <text>`, `status: <column>`
//! (board column name, `*` wildcards), or a bare date (`jan 15`, `next fri`). Operators: `&`, `|`,
//! `!`, parentheses. Filters only match open tasks (so never the done column).

use chrono::NaiveDate;

use super::dates::{self, DateCtx};
use super::model::{Priority, Task};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Today,
    Tomorrow,
    Yesterday,
    Overdue,
    NoDate,
    NoTime,
    Recurring,
    NoLabels,
    NoDeadline,
    NoSection,
    Subtask,
    All,
    Priority(Priority),
    Label(String),
    List(String),
    Section(String),
    NextDays(i64),
    DueOn(NaiveDate),
    DueBefore(NaiveDate),
    DueAfter(NaiveDate),
    DeadlineOn(NaiveDate),
    DeadlineBefore(NaiveDate),
    DeadlineAfter(NaiveDate),
    CreatedBefore(NaiveDate),
    CreatedAfter(NaiveDate),
    Search(String),
    /// Board column name (lowercase, `*` wildcards).
    Status(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Term(Term),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

/// A parsed query: one expression per comma-separated group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub groups: Vec<(String, Expr)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lex {
    And,
    Or,
    Not,
    Open,
    Close,
    Comma,
    Text(String),
}

fn lex(q: &str) -> Vec<(Lex, usize)> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut text_at = 0;
    let mut chars = q.char_indices().peekable();
    let flush = |text: &mut String, at: usize, out: &mut Vec<(Lex, usize)>| {
        if !text.trim().is_empty() {
            out.push((Lex::Text(text.trim().to_string()), at));
        }
        text.clear();
    };
    while let Some((i, c)) = chars.next() {
        let at_term_start = text.trim().is_empty();
        match c {
            '\\' => {
                if let Some((_, n)) = chars.next() {
                    if text.trim().is_empty() {
                        text_at = i;
                    }
                    text.push(n);
                }
            }
            '&' | '|' | ')' | ',' => {
                flush(&mut text, text_at, &mut out);
                out.push((
                    match c {
                        '&' => Lex::And,
                        '|' => Lex::Or,
                        ')' => Lex::Close,
                        _ => Lex::Comma,
                    },
                    i,
                ));
            }
            '(' if at_term_start => {
                flush(&mut text, text_at, &mut out);
                out.push((Lex::Open, i));
            }
            '!' if at_term_start => {
                flush(&mut text, text_at, &mut out);
                out.push((Lex::Not, i));
            }
            _ => {
                if text.trim().is_empty() {
                    text_at = i;
                }
                text.push(c);
            }
        }
    }
    flush(&mut text, text_at, &mut out);
    out
}

struct P<'a> {
    toks: Vec<(Lex, usize)>,
    pos: usize,
    ctx: &'a DateCtx,
}

impl P<'_> {
    fn peek(&self) -> Option<&Lex> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut e = self.and()?;
        while self.peek() == Some(&Lex::Or) {
            self.pos += 1;
            e = Expr::Or(Box::new(e), Box::new(self.and()?));
        }
        Ok(e)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut e = self.unary()?;
        while self.peek() == Some(&Lex::And) {
            self.pos += 1;
            e = Expr::And(Box::new(e), Box::new(self.unary()?));
        }
        Ok(e)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.toks.get(self.pos).cloned() {
            Some((Lex::Not, _)) => {
                self.pos += 1;
                Ok(Expr::Not(Box::new(self.unary()?)))
            }
            Some((Lex::Open, _)) => {
                self.pos += 1;
                let e = self.or()?;
                if self.peek() != Some(&Lex::Close) {
                    return Err("missing `)`".into());
                }
                self.pos += 1;
                Ok(e)
            }
            Some((Lex::Text(s), _)) => {
                self.pos += 1;
                term(&s, self.ctx).map(Expr::Term)
            }
            Some((t, _)) => Err(format!("unexpected `{}`", lex_str(&t))),
            None => Err("the filter ends too early".into()),
        }
    }
}

fn lex_str(t: &Lex) -> &str {
    match t {
        Lex::And => "&",
        Lex::Or => "|",
        Lex::Not => "!",
        Lex::Open => "(",
        Lex::Close => ")",
        Lex::Comma => ",",
        Lex::Text(s) => s,
    }
}

fn date_arg(s: &str, ctx: &DateCtx) -> Result<NaiveDate, String> {
    dates::parse_date(s, ctx).and_then(|d| d.ok_or_else(|| "a date is missing".to_string()))
}

fn term(raw: &str, ctx: &DateCtx) -> Result<Term, String> {
    let s = raw.trim();
    let l = s.to_lowercase();
    let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(match l.as_str() {
        "today" | "tod" => Term::Today,
        "tomorrow" | "tom" => Term::Tomorrow,
        "yesterday" => Term::Yesterday,
        "overdue" | "od" => Term::Overdue,
        "no date" | "no due date" => Term::NoDate,
        "no time" => Term::NoTime,
        "recurring" => Term::Recurring,
        "no labels" | "no label" => Term::NoLabels,
        "no deadline" => Term::NoDeadline,
        "no section" => Term::NoSection,
        "subtask" | "subtasks" => Term::Subtask,
        "all" | "*" | "view all" => Term::All,
        "no priority" => Term::Priority(Priority::P4),
        _ => {
            if let Some(p) = Priority::parse(&l).filter(|_| l.starts_with('p') && l.len() == 2) {
                return Ok(Term::Priority(p));
            }
            if let Some(rest) = s.strip_prefix('@') {
                return Ok(Term::Label(rest.trim().to_lowercase()));
            }
            if let Some(rest) = s.strip_prefix('#') {
                return Ok(Term::List(rest.trim().trim_matches('"').to_lowercase()));
            }
            if let Some(rest) = s.strip_prefix('/') {
                return Ok(Term::Section(rest.trim().trim_matches('"').to_lowercase()));
            }
            if let Some((key, val)) = l.split_once(':') {
                let val = val.trim();
                return match key.trim() {
                    "due" | "date" => Ok(Term::DueOn(date_arg(val, ctx)?)),
                    "due before" | "date before" => Ok(Term::DueBefore(date_arg(val, ctx)?)),
                    "due after" | "date after" => Ok(Term::DueAfter(date_arg(val, ctx)?)),
                    "deadline" => Ok(Term::DeadlineOn(date_arg(val, ctx)?)),
                    "deadline before" => Ok(Term::DeadlineBefore(date_arg(val, ctx)?)),
                    "deadline after" => Ok(Term::DeadlineAfter(date_arg(val, ctx)?)),
                    "created before" => Ok(Term::CreatedBefore(date_arg(val, ctx)?)),
                    "created after" => Ok(Term::CreatedAfter(date_arg(val, ctx)?)),
                    "search" => {
                        // Keep the original case-insensitive text after the first colon.
                        let text = s.split_once(':').map(|(_, v)| v.trim().to_lowercase()).unwrap_or_default();
                        if text.is_empty() { Err("`search:` needs some text".into()) } else { Ok(Term::Search(text)) }
                    }
                    "status" => {
                        let name = val.trim_matches('"').trim();
                        if name.is_empty() { Err("`status:` needs a column name, e.g. status: in progress".into()) } else { Ok(Term::Status(name.to_string())) }
                    }
                    other => Err(format!("unknown filter `{other}:`")),
                };
            }
            let words: Vec<&str> = l.split(' ').collect();
            let days = match words.as_slice() {
                [n, "days" | "day"] | ["next", n, "days" | "day"] => n.parse::<i64>().ok(),
                _ => None,
            };
            if let Some(n) = days.filter(|n| *n > 0) {
                return Ok(Term::NextDays(n));
            }
            if let Ok(Some(d)) = dates::parse_date(&l, ctx) {
                return Ok(Term::DueOn(d));
            }
            return Err(format!("unknown filter `{s}`"));
        }
    })
}

/// Parses a filter query.
pub fn parse(query: &str, ctx: &DateCtx) -> Result<Query, String> {
    let toks = lex(query);
    if toks.is_empty() {
        return Err("the filter is empty".into());
    }
    // Split at top-level commas.
    let mut groups = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut start_byte = 0;
    for k in 0..=toks.len() {
        let end_here = k == toks.len() || (depth == 0 && toks[k].0 == Lex::Comma);
        if k < toks.len() {
            match toks[k].0 {
                Lex::Open => depth += 1,
                Lex::Close => depth -= 1,
                _ => {}
            }
        }
        if end_here {
            let end_byte = toks.get(k).map(|(_, b)| *b).unwrap_or(query.len());
            let slice = toks[start..k].to_vec();
            let mut p = P { toks: slice, pos: 0, ctx };
            let e = p.or()?;
            if p.pos < p.toks.len() {
                return Err(format!("unexpected `{}`", lex_str(&p.toks[p.pos].0)));
            }
            groups.push((query[start_byte..end_byte].trim().to_string(), e));
            start = k + 1;
            start_byte = (end_byte + 1).min(query.len());
        }
    }
    Ok(Query { groups })
}

/// `*` wildcard match, case-insensitive (`pattern` is lowercase).
fn wild(pattern: &str, value: &str) -> bool {
    let v = value.to_lowercase();
    if !pattern.contains('*') {
        return v == pattern;
    }
    let re = format!("^{}$", pattern.split('*').map(regex::escape).collect::<Vec<_>>().join(".*"));
    regex::Regex::new(&re).is_ok_and(|r| r.is_match(&v))
}

/// Lookups the evaluator needs.
pub struct EvalCtx<'a> {
    pub today: NaiveDate,
    pub list_name: &'a dyn Fn(&str) -> Option<String>,
    pub section_name: &'a dyn Fn(&str) -> Option<String>,
    /// The local calendar day a task is due (see [`dates::local_due_date`]).
    pub due_date: &'a dyn Fn(&Task) -> Option<NaiveDate>,
    /// Creation day of a task (from `createdAt`).
    pub created: &'a dyn Fn(&Task) -> NaiveDate,
    /// Name of the board column a task is in (see [`super::model::TodoSettings::status_of`]).
    pub status_name: &'a dyn Fn(&Task) -> Option<String>,
}

impl Expr {
    pub fn matches(&self, t: &Task, cx: &EvalCtx) -> bool {
        match self {
            Expr::Not(e) => !e.matches(t, cx),
            Expr::And(a, b) => a.matches(t, cx) && b.matches(t, cx),
            Expr::Or(a, b) => a.matches(t, cx) || b.matches(t, cx),
            Expr::Term(term) => {
                let due = (cx.due_date)(t);
                let dd = |f: &dyn Fn(NaiveDate) -> bool| due.is_some_and(f);
                let dl = |f: &dyn Fn(NaiveDate) -> bool| t.deadline.is_some_and(f);
                match term {
                    Term::Today => dd(&|d| d == cx.today),
                    Term::Tomorrow => dd(&|d| d == cx.today + chrono::Duration::days(1)),
                    Term::Yesterday => dd(&|d| d == cx.today - chrono::Duration::days(1)),
                    Term::Overdue => dd(&|d| d < cx.today),
                    Term::NoDate => due.is_none(),
                    Term::NoTime => t.due.as_ref().is_none_or(|d| d.time.is_none()),
                    Term::Recurring => t.due.as_ref().is_some_and(|d| d.recurrence.is_some()),
                    Term::NoLabels => t.labels.is_empty(),
                    Term::NoDeadline => t.deadline.is_none(),
                    Term::NoSection => t.section_id.is_none(),
                    Term::Subtask => t.parent_id.is_some(),
                    Term::All => true,
                    Term::Priority(p) => t.priority == *p,
                    Term::Label(p) => t.labels.iter().any(|l| wild(p, l)),
                    Term::List(p) => (cx.list_name)(&t.list_id).is_some_and(|n| wild(p, &n)),
                    Term::Section(p) => t.section_id.as_deref().and_then(|s| (cx.section_name)(s)).is_some_and(|n| wild(p, &n)),
                    Term::NextDays(n) => dd(&|d| d >= cx.today && d < cx.today + chrono::Duration::days(*n)),
                    Term::DueOn(x) => dd(&|d| d == *x),
                    Term::DueBefore(x) => dd(&|d| d < *x),
                    Term::DueAfter(x) => dd(&|d| d > *x),
                    Term::DeadlineOn(x) => dl(&|d| d == *x),
                    Term::DeadlineBefore(x) => dl(&|d| d < *x),
                    Term::DeadlineAfter(x) => dl(&|d| d > *x),
                    Term::CreatedBefore(x) => (cx.created)(t) < *x,
                    Term::CreatedAfter(x) => (cx.created)(t) > *x,
                    Term::Search(s) => t.content.to_lowercase().contains(s) || t.description.to_lowercase().contains(s),
                    Term::Status(p) => (cx.status_name)(t).is_some_and(|n| wild(p, n.trim())),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
