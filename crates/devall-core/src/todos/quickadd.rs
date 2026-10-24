//! Quick add: one line of text becomes a task, Todoist style.
//!
//! `Ship refunds tomorrow 5pm #Work /Backend @api @urgent p1 {fri}` →
//! content "Ship refunds", due tomorrow 17:00, list Work, section Backend, labels api + urgent,
//! priority 1, deadline Friday.
//!
//! - `#list` / `/section` match existing names (longest match, case-insensitive; quote names with
//!   spaces: `#"Side project"`). Unknown names stay in the text.
//! - `@label` may be new (created when the task is added).
//! - `p1`…`p4` (or `!!1`…`!!4`): priority; the last one wins.
//! - `{date}`: deadline.
//! - The first date expression anywhere in the text is the due date (see [`super::dates`]).
//!
//! Token spans are reported in UTF-16 code units so the UI can highlight them as chips. Tokens whose
//! text is listed in `ignore` (the user clicked a chip away) stay part of the content.

use chrono::NaiveDate;
use serde::Serialize;

use super::dates::{self, DateCtx, Matched};
use super::model::{Due, Label, Priority, Section, TaskList, opt_ymd};

/// What quick add can see: today, existing lists/sections/labels, and the default list.
pub struct ParseCtx<'a> {
    pub dates: DateCtx,
    pub lists: &'a [TaskList],
    pub sections: &'a [Section],
    pub labels: &'a [Label],
    /// The list the task goes to unless `#list` says otherwise (for `/section` lookup).
    pub default_list: Option<&'a str>,
    /// Token texts the user turned off (compared case-insensitively).
    pub ignore: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenKind {
    Date,
    Deadline,
    List,
    Section,
    Label,
    Priority,
}

/// A recognised piece of the input (a chip in the UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickToken {
    pub kind: TokenKind,
    /// UTF-16 offsets into the input.
    pub start: usize,
    pub end: usize,
    /// The raw text of the token.
    pub text: String,
    /// Resolved value: list/section id, label name, `p1`, `YYYY-MM-DD`…
    pub value: String,
    /// A label that doesn't exist yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub new: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickAdd {
    /// The text with every recognised token removed.
    pub content: String,
    pub list_id: Option<String>,
    pub section_id: Option<String>,
    /// Canonical label names (existing labels keep their spelling).
    pub labels: Vec<String>,
    /// Labels among `labels` that don't exist yet.
    pub new_labels: Vec<String>,
    pub priority: Option<Priority>,
    pub due: Option<Due>,
    /// `no date` was typed (clears the due date when editing).
    pub no_date: bool,
    #[serde(with = "opt_ymd", skip_serializing_if = "Option::is_none")]
    pub deadline: Option<NaiveDate>,
    pub tokens: Vec<QuickToken>,
}

struct Span {
    kind: TokenKind,
    start: usize,
    end: usize,
    value: String,
    new: bool,
}

fn is_word_start(text: &str, i: usize) -> bool {
    i == 0 || text[..i].chars().next_back().is_some_and(|c| c.is_whitespace() || c == '(')
}

/// Whether `rest` starts with `name` followed by a word boundary; returns the byte length used.
fn starts_with_name(rest: &str, name: &str) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let mut it = rest.char_indices();
    let mut used = 0;
    for nc in name.chars() {
        let (i, rc) = it.next()?;
        if !rc.to_lowercase().eq(nc.to_lowercase()) {
            return None;
        }
        used = i + rc.len_utf8();
    }
    match rest[used..].chars().next() {
        None => Some(used),
        Some(c) if c.is_whitespace() || matches!(c, ',' | '.' | ';' | '!' | '?' | ')') => Some(used),
        _ => None,
    }
}

/// `#"Quoted name"` or the longest known name; returns (name index, bytes after the sigil).
fn match_name<'n>(rest: &str, names: impl Iterator<Item = (usize, &'n str)>) -> Option<(usize, usize)> {
    if let Some(q) = rest.strip_prefix('"') {
        let close = q.find('"')?;
        let wanted = &q[..close];
        let wanted = wanted.to_lowercase();
        return names.into_iter().find(|(_, n)| n.to_lowercase() == wanted).map(|(i, _)| (i, close + 2));
    }
    names.filter_map(|(i, n)| starts_with_name(rest, n).map(|len| (i, len))).max_by_key(|(_, len)| *len)
}

fn label_chars(rest: &str) -> usize {
    let mut end = 0;
    for (i, c) in rest.char_indices() {
        if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':') {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    // A trailing `.`/`:` ends the sentence, not the label.
    rest[..end].trim_end_matches(['.', ':', '/']).len()
}

/// Parses quick-add text.
pub fn parse(text: &str, ctx: &ParseCtx) -> QuickAdd {
    let ignored = |raw: &str| ctx.ignore.iter().any(|i| i.trim().eq_ignore_ascii_case(raw.trim()));
    let mut spans: Vec<Span> = Vec::new();
    let mut out = QuickAdd::default();

    // Word-level sigils: #list, @label, p1, {deadline}. Sections need the final list: second pass.
    let mut section_starts: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        if !is_word_start(text, i) || c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let rest = &text[i + c.len_utf8()..];
        let mut consumed = 0;
        match c {
            '#' => {
                if let Some((li, len)) = match_name(rest, ctx.lists.iter().enumerate().map(|(k, l)| (k, l.name.as_str())))
                    && !ignored(&text[i..i + 1 + len])
                {
                    let l = &ctx.lists[li];
                    spans.push(Span { kind: TokenKind::List, start: i, end: i + 1 + len, value: l.id.clone(), new: false });
                    out.list_id = Some(l.id.clone());
                    consumed = 1 + len;
                }
            }
            '/' => section_starts.push(i),
            '@' => {
                let len = label_chars(rest);
                if len > 0 && !ignored(&text[i..i + 1 + len]) {
                    let raw = &rest[..len];
                    let existing = ctx.labels.iter().find(|l| l.name.to_lowercase() == raw.to_lowercase());
                    let name = existing.map(|l| l.name.clone()).unwrap_or_else(|| raw.to_string());
                    let new = existing.is_none();
                    if !out.labels.iter().any(|l| l.eq_ignore_ascii_case(&name)) {
                        out.labels.push(name.clone());
                        if new {
                            out.new_labels.push(name.clone());
                        }
                    }
                    spans.push(Span { kind: TokenKind::Label, start: i, end: i + 1 + len, value: name, new });
                    consumed = 1 + len;
                }
            }
            '{' => {
                if let Some(close) = rest.find('}')
                    && !rest[..close].contains('{')
                    && let Ok(Some(d)) = dates::parse_date(&rest[..close], &ctx.dates)
                    && !ignored(&text[i..i + close + 2])
                {
                    spans.push(Span { kind: TokenKind::Deadline, start: i, end: i + close + 2, value: d.format("%Y-%m-%d").to_string(), new: false });
                    out.deadline = Some(d);
                    consumed = close + 2;
                }
            }
            'p' | 'P' | '!' => {
                let word_end = text[i..].find(char::is_whitespace).map(|e| i + e).unwrap_or(text.len());
                let word = text[i..word_end].trim_end_matches([',', '.', ';', ')']);
                if let Some(p) = Priority::parse(word).filter(|_| word.len() <= 3 && word.len() >= 2)
                    && (word.starts_with(['p', 'P']) || word.starts_with("!!"))
                    && !ignored(word)
                {
                    spans.push(Span { kind: TokenKind::Priority, start: i, end: i + word.len(), value: p.to_string(), new: false });
                    out.priority = Some(p);
                    consumed = word.len();
                }
            }
            _ => {}
        }
        i += consumed.max(c.len_utf8());
    }

    let target = out.list_id.clone().or_else(|| ctx.default_list.map(str::to_string));
    for start in section_starts {
        if spans.iter().any(|s| start >= s.start && start < s.end) {
            continue;
        }
        let Some(list) = target.as_deref() else { break };
        let candidates: Vec<&Section> = ctx.sections.iter().filter(|s| s.list_id == list).collect();
        if let Some((si, len)) = match_name(&text[start + 1..], candidates.iter().enumerate().map(|(k, s)| (k, s.name.as_str())))
            && !ignored(&text[start..start + 1 + len])
        {
            let s = candidates[si];
            spans.push(Span { kind: TokenKind::Section, start, end: start + 1 + len, value: s.id.clone(), new: false });
            out.section_id = Some(s.id.clone());
        }
    }

    // The first date expression in a stretch of words without sigils.
    let toks = dates::tokenize(text);
    let blocked = |t: &dates::Tok| spans.iter().any(|s| t.start < s.end && t.end > s.start);
    let mut segments: Vec<Vec<dates::Tok>> = vec![Vec::new()];
    for t in toks {
        if blocked(&t) {
            segments.push(Vec::new());
        } else if let Some(seg) = segments.last_mut() {
            seg.push(t);
        }
    }
    'outer: for seg in &segments {
        for k in 0..seg.len() {
            if seg[k].norm == "," {
                continue;
            }
            if let Some((m, j)) = dates::match_at(seg, k, &ctx.dates) {
                let (start, end) = (seg[k].start, seg[j - 1].end);
                if ignored(&text[start..end]) {
                    continue;
                }
                let value = match m {
                    Matched::NoDate => {
                        out.no_date = true;
                        String::new()
                    }
                    Matched::Due(spec) => {
                        let value = spec.date.format("%Y-%m-%d").to_string();
                        out.due = Some(spec.into_due(&text[start..end]));
                        value
                    }
                };
                spans.push(Span { kind: TokenKind::Date, start, end, value, new: false });
                break 'outer;
            }
        }
    }

    spans.sort_by_key(|s| s.start);
    // Content: the text without the spans.
    let mut content = String::new();
    let mut at = 0;
    for s in &spans {
        if s.start >= at {
            content.push_str(&text[at..s.start]);
            content.push(' ');
            at = s.end;
        }
    }
    content.push_str(&text[at.min(text.len())..]);
    out.content = tidy(&content);

    let u16_at = |byte: usize| text[..byte].encode_utf16().count();
    out.tokens = spans
        .into_iter()
        .map(|s| QuickToken { kind: s.kind, start: u16_at(s.start), end: u16_at(s.end), text: text[s.start..s.end].to_string(), value: s.value, new: s.new })
        .collect();
    out
}

/// Collapses whitespace, removes spaces before punctuation and dangling separators.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        let punct_only = word.chars().all(|c| matches!(c, ',' | '.' | ';' | ':' | '!' | '?'));
        if punct_only && !out.is_empty() {
            out.push_str(word);
        } else {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    out.trim_end_matches([',', ';', ':', ' ']).trim_start_matches([',', ';', ':', ' ']).to_string()
}

#[cfg(test)]
mod tests;
