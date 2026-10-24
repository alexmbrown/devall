//! Regex sandbox: the linear-time `regex` crate ("rust" flavor) and the backtracking `fancy-regex`
//! ("pcre-like": look-around, backreferences, atomic groups) behind one interface, plus a
//! deterministic explanation built from the parsed AST.
//!
//! Offsets are reported twice: `start`/`end` in UTF-16 code units (what JavaScript strings use, so
//! the UI can highlight directly) and `byteStart`/`byteEnd` in UTF-8 bytes.

use std::time::Instant;

use regex_syntax::ast::{self, Ast};
use serde_json::{Value, json};

use crate::{Result, arg_str, invalid};

/// Matches returned per run; the count keeps going up to this limit only.
const MAX_MATCHES: usize = 10_000;
/// Backtracking steps before fancy-regex gives up (catastrophic-backtracking guard).
const BACKTRACK_LIMIT: usize = 1_000_000;
/// Compiled program size cap for untrusted patterns.
const SIZE_LIMIT: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Rust,
    Pcre,
}

impl Flavor {
    fn parse(s: &str) -> Flavor {
        if s.eq_ignore_ascii_case("pcre-like") || s.eq_ignore_ascii_case("pcre") || s.eq_ignore_ascii_case("js") { Flavor::Pcre } else { Flavor::Rust }
    }

    fn name(self) -> &'static str {
        match self {
            Flavor::Rust => "rust",
            Flavor::Pcre => "pcre-like",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Flags {
    i: bool,
    m: bool,
    s: bool,
    x: bool,
    swap_greed: bool,
}

fn parse_flags(s: &str) -> Result<Flags> {
    let mut f = Flags::default();
    for c in s.chars() {
        match c {
            'i' => f.i = true,
            'm' => f.m = true,
            's' => f.s = true,
            'x' => f.x = true,
            'U' => f.swap_greed = true,
            // Always global / always Unicode: accepted for copy-paste from JS (`/…/gu`).
            'g' | 'u' | ' ' | ',' => {}
            other => return Err(invalid(format!("unknown flag '{other}' (supported: i m s x U)"))),
        }
    }
    Ok(f)
}

pub struct Opts {
    pub pattern: String,
    pub flags: String,
    pub flavor: Flavor,
}

impl Opts {
    pub fn from_args(args: &Value) -> Opts {
        Opts { pattern: arg_str(args, "pattern").to_string(), flags: arg_str(args, "flags").to_string(), flavor: Flavor::parse(arg_str(args, "flavor")) }
    }
}

enum Engine {
    Rust(regex::Regex),
    Fancy(fancy_regex::Regex),
}

fn compile(o: &Opts) -> Result<Engine> {
    if o.pattern.is_empty() {
        return Err(invalid("enter a pattern"));
    }
    let f = parse_flags(&o.flags)?;
    match o.flavor {
        Flavor::Rust => regex::RegexBuilder::new(&o.pattern)
            .case_insensitive(f.i)
            .multi_line(f.m)
            .dot_matches_new_line(f.s)
            .ignore_whitespace(f.x)
            .swap_greed(f.swap_greed)
            .size_limit(SIZE_LIMIT)
            .build()
            .map(Engine::Rust)
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("look-around") || msg.contains("backreferences") {
                    invalid(format!("{msg}\nTip: switch the flavor to pcre-like for look-around and backreferences."))
                } else {
                    invalid(msg)
                }
            }),
        Flavor::Pcre => {
            // fancy-regex has no swap-greed builder option; an inline group does the same.
            let pattern = if f.swap_greed { format!("(?U:{})", o.pattern) } else { o.pattern.clone() };
            fancy_regex::RegexBuilder::new(&pattern)
                .case_insensitive(f.i)
                .multi_line(f.m)
                .dot_matches_new_line(f.s)
                .ignore_whitespace(f.x)
                .backtrack_limit(BACKTRACK_LIMIT)
                .delegate_size_limit(SIZE_LIMIT)
                .build()
                .map(Engine::Fancy)
                .map_err(|e| invalid(e.to_string()))
        }
    }
}

/// Byte offset → UTF-16 offset for one string.
pub(crate) struct Utf16Map {
    table: Option<Vec<u32>>,
}

impl Utf16Map {
    pub(crate) fn new(s: &str) -> Utf16Map {
        if s.is_ascii() {
            return Utf16Map { table: None };
        }
        let mut table = vec![0u32; s.len() + 1];
        let mut u = 0u32;
        for (b, c) in s.char_indices() {
            table[b] = u;
            u += c.len_utf16() as u32;
        }
        table[s.len()] = u;
        Utf16Map { table: Some(table) }
    }

    pub(crate) fn at(&self, byte: usize) -> usize {
        match &self.table {
            None => byte,
            Some(t) => t.get(byte).copied().unwrap_or(0) as usize,
        }
    }
}

fn span_json(map: &Utf16Map, text: &str, start: usize, end: usize) -> Value {
    json!({ "start": map.at(start), "end": map.at(end), "byteStart": start, "byteEnd": end, "text": &text[start..end] })
}

pub fn test(text: &str, o: &Opts) -> Result<Value> {
    let t0 = Instant::now();
    let engine = compile(o)?;
    let compile_us = t0.elapsed().as_micros() as u64;
    let map = Utf16Map::new(text);
    let names: Vec<Option<String>> = match &engine {
        Engine::Rust(re) => re.capture_names().map(|n| n.map(str::to_string)).collect(),
        Engine::Fancy(re) => re.capture_names().map(|n| n.map(str::to_string)).collect(),
    };
    let mut matches: Vec<Value> = Vec::new();
    let mut error: Option<String> = None;
    let mut truncated = false;
    let mut empty_matches = 0usize;
    let t1 = Instant::now();
    let mut push = |groups: Vec<Option<(usize, usize)>>| -> bool {
        if matches.len() >= MAX_MATCHES {
            truncated = true;
            return false;
        }
        let Some((s, e)) = groups[0] else { return true };
        if s == e {
            empty_matches += 1;
        }
        let mut m = span_json(&map, text, s, e);
        m["index"] = json!(matches.len());
        m["groups"] = Value::Array(
            groups
                .iter()
                .enumerate()
                .skip(1)
                .map(|(i, g)| {
                    let mut v = g.map_or(json!({ "matched": false }), |(gs, ge)| span_json(&map, text, gs, ge));
                    v["index"] = json!(i);
                    v["name"] = json!(names.get(i).cloned().flatten());
                    v
                })
                .collect(),
        );
        matches.push(m);
        true
    };
    match &engine {
        Engine::Rust(re) => {
            for c in re.captures_iter(text) {
                if !push((0..c.len()).map(|i| c.get(i).map(|m| (m.start(), m.end()))).collect()) {
                    break;
                }
            }
        }
        Engine::Fancy(re) => {
            for c in re.captures_iter(text) {
                match c {
                    Ok(c) => {
                        if !push((0..c.len()).map(|i| c.get(i).map(|m| (m.start(), m.end()))).collect()) {
                            break;
                        }
                    }
                    Err(e) => {
                        error = Some(runtime_error(&e));
                        break;
                    }
                }
            }
        }
    }
    let match_us = t1.elapsed().as_micros() as u64;
    let mut warnings: Vec<String> = Vec::new();
    if empty_matches > 0 {
        warnings.push(format!("{empty_matches} empty match(es): the pattern can match the empty string"));
    }
    if truncated {
        warnings.push(format!("Stopped after {MAX_MATCHES} matches"));
    }
    if o.flavor == Flavor::Pcre && regex::Regex::new(&o.pattern).is_ok() {
        warnings.push("No backtracking-only features used: the rust flavor runs this pattern in guaranteed linear time".into());
    }
    let group_names: Vec<Value> = names.iter().enumerate().skip(1).map(|(i, n)| json!({ "index": i, "name": n })).collect();
    Ok(json!({
        "flavor": o.flavor.name(),
        "pattern": o.pattern,
        "flags": o.flags,
        "matchCount": matches.len(),
        "truncated": truncated,
        "groupCount": names.len().saturating_sub(1),
        "groupNames": group_names,
        "compileMicros": compile_us,
        "matchMicros": match_us,
        "matches": matches,
        "warnings": warnings,
        "error": error,
    }))
}

fn runtime_error(e: &fancy_regex::Error) -> String {
    let msg = e.to_string();
    if msg.to_ascii_lowercase().contains("backtrack") {
        format!("Aborted: {msg}. The pattern backtracks catastrophically on this input (look for nested quantifiers like (a+)+).")
    } else {
        msg
    }
}

pub fn replace(text: &str, o: &Opts, replacement: &str, limit: usize) -> Result<String> {
    Ok(match compile(o)? {
        Engine::Rust(re) => re.replacen(text, limit, replacement).into_owned(),
        Engine::Fancy(re) => re.try_replacen(text, limit, replacement).map_err(|e| invalid(runtime_error(&e)))?.into_owned(),
    })
}

/// Saved test cases: input `{"shouldMatch": [...], "shouldNotMatch": [...]}`. A case matches when
/// the pattern finds a match anywhere in it (search, like `regex.test`; anchor with `^…$` for a
/// full match).
pub fn cases(input: &str, o: &Opts) -> Result<Value> {
    let v: Value = serde_json::from_str(input.trim()).map_err(|e| invalid(format!("input must be {{\"shouldMatch\": [...], \"shouldNotMatch\": [...]}}: {e}")))?;
    let list = |k: &str| -> Vec<String> { v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let engine = compile(o)?;
    let mut out = Vec::new();
    let (mut passed, mut failed) = (0usize, 0usize);
    for (text, should) in list("shouldMatch").into_iter().map(|t| (t, true)).chain(list("shouldNotMatch").into_iter().map(|t| (t, false))) {
        let found: std::result::Result<Option<String>, String> = match &engine {
            Engine::Rust(re) => Ok(re.find(&text).map(|m| m.as_str().to_string())),
            Engine::Fancy(re) => re.find(&text).map(|m| m.map(|m| m.as_str().to_string())).map_err(|e| e.to_string()),
        };
        let (matched, error) = match found {
            Ok(m) => (m, None),
            Err(e) => (None, Some(e)),
        };
        let ok = error.is_none() && matched.is_some() == should;
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }
        out.push(json!({ "text": text, "shouldMatch": should, "matched": matched.is_some(), "match": matched, "passed": ok, "error": error }));
    }
    Ok(json!({ "passed": passed, "failed": failed, "cases": out }))
}

pub fn split(text: &str, o: &Opts) -> Result<Value> {
    let parts: Vec<String> = match compile(o)? {
        Engine::Rust(re) => re.split(text).map(str::to_string).collect(),
        Engine::Fancy(re) => re.split(text).map(|r| r.map(str::to_string)).collect::<std::result::Result<_, _>>().map_err(|e| invalid(runtime_error(&e)))?,
    };
    Ok(json!(parts))
}

// ── Explain ──────────────────────────────────────────────────────────────────────────────────────

struct Explainer<'p> {
    pattern: &'p str,
    map: Utf16Map,
}

fn node(kind: &str, text: &str, span: Option<(usize, usize)>, description: String, children: Vec<Value>) -> Value {
    let mut v = json!({ "kind": kind, "text": text, "description": description });
    if let Some((s, e)) = span {
        v["start"] = json!(s);
        v["end"] = json!(e);
    }
    if !children.is_empty() {
        v["children"] = Value::Array(children);
    }
    v
}

/// An alternation branch; a branch that is a single token is folded into one row.
fn alternative(i: usize, text: &str, span: Option<(usize, usize)>, mut children: Vec<Value>) -> Value {
    if children.len() == 1 && children[0]["text"] == text {
        let only = children.remove(0);
        let desc = only["description"].as_str().unwrap_or("");
        let mut lowered = desc.chars();
        let desc = lowered.next().map(|f| f.to_lowercase().collect::<String>() + lowered.as_str()).unwrap_or_default();
        let grand = only.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
        return node("alternative", text, span, format!("Alternative {}: {desc}", i + 1), grand);
    }
    node("alternative", text, span, format!("Alternative {}", i + 1), children)
}

fn quote_char(c: char) -> String {
    match c {
        ' ' => "space".into(),
        '\t' => "tab (U+0009)".into(),
        '\n' => "line feed (U+000A)".into(),
        '\r' => "carriage return (U+000D)".into(),
        c if c.is_control() || c.is_whitespace() => format!("U+{:04X}", c as u32),
        c => format!("'{c}'"),
    }
}

fn perl_class(kind: &ast::ClassPerlKind, negated: bool) -> String {
    let base = match kind {
        ast::ClassPerlKind::Digit => "a digit (0–9, or any Unicode decimal digit)",
        ast::ClassPerlKind::Space => "a whitespace character (space, tab, newline…)",
        ast::ClassPerlKind::Word => "a word character (letter, digit or underscore)",
    };
    if negated { format!("Any character that is NOT {base}") } else { format!("Matches {base}") }
}

fn ascii_class(kind: &ast::ClassAsciiKind) -> &'static str {
    use ast::ClassAsciiKind::*;
    match kind {
        Alnum => "ASCII letters and digits [A-Za-z0-9]",
        Alpha => "ASCII letters [A-Za-z]",
        Ascii => "any ASCII character [\\x00-\\x7F]",
        Blank => "space or tab",
        Cntrl => "ASCII control characters",
        Digit => "digits [0-9]",
        Graph => "visible ASCII characters (no space)",
        Lower => "lowercase letters [a-z]",
        Print => "printable ASCII characters (incl. space)",
        Punct => "ASCII punctuation",
        Space => "ASCII whitespace",
        Upper => "uppercase letters [A-Z]",
        Word => "word characters [A-Za-z0-9_]",
        Xdigit => "hex digits [0-9A-Fa-f]",
    }
}

fn unicode_category_name(c: char) -> &'static str {
    match c {
        'L' => "Letter",
        'M' => "Mark",
        'N' => "Number",
        'P' => "Punctuation",
        'S' => "Symbol",
        'Z' => "Separator",
        'C' => "Other / control",
        _ => "",
    }
}

fn unicode_class(c: &ast::ClassUnicode) -> String {
    let what = match &c.kind {
        ast::ClassUnicodeKind::OneLetter(l) => format!("Unicode category {l} ({})", unicode_category_name(*l)),
        ast::ClassUnicodeKind::Named(n) => format!("Unicode property/script \"{n}\""),
        ast::ClassUnicodeKind::NamedValue { name, value, .. } => format!("Unicode property {name} = {value}"),
    };
    if c.is_negated() { format!("Any character NOT in {what}") } else { format!("Any character in {what}") }
}

fn assertion(kind: &ast::AssertionKind) -> &'static str {
    use ast::AssertionKind::*;
    match kind {
        StartLine => "Start of the text (start of each line with the m flag)",
        EndLine => "End of the text (end of each line with the m flag)",
        StartText => "Start of the text (\\A), regardless of flags",
        EndText => "End of the text (\\z), regardless of flags",
        WordBoundary => "Word boundary: between a word and a non-word character",
        NotWordBoundary => "Not a word boundary",
        WordBoundaryStart | WordBoundaryStartAngle => "Start of a word",
        WordBoundaryEnd | WordBoundaryEndAngle => "End of a word",
        WordBoundaryStartHalf => "Start-of-word half boundary",
        WordBoundaryEndHalf => "End-of-word half boundary",
    }
}

fn flag_name(f: &ast::Flag) -> &'static str {
    match f {
        ast::Flag::CaseInsensitive => "case-insensitive (i)",
        ast::Flag::MultiLine => "multi-line: ^ and $ match at line breaks (m)",
        ast::Flag::DotMatchesNewLine => "dot matches newline (s)",
        ast::Flag::SwapGreed => "swap greedy and lazy quantifiers (U)",
        ast::Flag::Unicode => "Unicode mode (u)",
        ast::Flag::CRLF => "CRLF line terminators (R)",
        ast::Flag::IgnoreWhitespace => "ignore whitespace and allow # comments (x)",
    }
}

fn describe_flags(flags: &ast::Flags) -> String {
    let mut on = Vec::new();
    let mut off = Vec::new();
    let mut negate = false;
    for item in &flags.items {
        match &item.kind {
            ast::FlagsItemKind::Negation => negate = true,
            ast::FlagsItemKind::Flag(f) => {
                if negate {
                    off.push(flag_name(f))
                } else {
                    on.push(flag_name(f))
                }
            }
        }
    }
    let mut parts = Vec::new();
    if !on.is_empty() {
        parts.push(format!("enable {}", on.join(", ")));
    }
    if !off.is_empty() {
        parts.push(format!("disable {}", off.join(", ")));
    }
    parts.join("; ")
}

pub(crate) fn quantifier(lo: usize, hi: Option<usize>, greedy: bool) -> String {
    let times = match (lo, hi) {
        (0, Some(1)) => "Optional (0 or 1 times)".to_string(),
        (0, None) => "0 or more times".to_string(),
        (1, None) => "1 or more times".to_string(),
        (n, None) => format!("{n} or more times"),
        (a, Some(b)) if a == b => format!("Exactly {a} time{}", if a == 1 { "" } else { "s" }),
        (a, Some(b)) => format!("Between {a} and {b} times"),
    };
    if lo == hi.unwrap_or(usize::MAX) {
        times
    } else if greedy {
        format!("{times}, as many as possible (greedy)")
    } else {
        format!("{times}, as few as possible (lazy)")
    }
}

impl Explainer<'_> {
    fn span(&self, s: &ast::Span) -> Option<(usize, usize)> {
        Some((self.map.at(s.start.offset), self.map.at(s.end.offset)))
    }

    fn text(&self, s: &ast::Span) -> &str {
        self.pattern.get(s.start.offset..s.end.offset).unwrap_or("")
    }

    fn literal(&self, l: &ast::Literal) -> Value {
        let desc = match &l.kind {
            ast::LiteralKind::Verbatim => format!("Matches {}", quote_char(l.c)),
            ast::LiteralKind::Meta | ast::LiteralKind::Superfluous => format!("Matches {} literally (escaped)", quote_char(l.c)),
            ast::LiteralKind::Octal => format!("Octal escape: {}", quote_char(l.c)),
            ast::LiteralKind::HexFixed(_) | ast::LiteralKind::HexBrace(_) => format!("Character {} (U+{:04X}) by hex code", quote_char(l.c), l.c as u32),
            ast::LiteralKind::Special(_) => format!("Matches {}", quote_char(l.c)),
        };
        node("literal", self.text(&l.span), self.span(&l.span), desc, vec![])
    }

    fn class_item(&self, item: &ast::ClassSetItem, out: &mut Vec<Value>) {
        match item {
            ast::ClassSetItem::Empty(_) => {}
            ast::ClassSetItem::Literal(l) => out.push(node("literal", self.text(&l.span), self.span(&l.span), format!("The character {}", quote_char(l.c)), vec![])),
            ast::ClassSetItem::Range(r) => out.push(node(
                "range",
                self.text(&r.span),
                self.span(&r.span),
                format!("A character in the range {} to {} (U+{:04X}–U+{:04X})", quote_char(r.start.c), quote_char(r.end.c), r.start.c as u32, r.end.c as u32),
                vec![],
            )),
            ast::ClassSetItem::Ascii(a) => out.push(node(
                "class",
                self.text(&a.span),
                self.span(&a.span),
                format!("{} {}", if a.negated { "Not" } else { "POSIX class:" }, ascii_class(&a.kind)),
                vec![],
            )),
            ast::ClassSetItem::Unicode(u) => out.push(node("class", self.text(&u.span), self.span(&u.span), unicode_class(u), vec![])),
            ast::ClassSetItem::Perl(p) => out.push(node("class", self.text(&p.span), self.span(&p.span), perl_class(&p.kind, p.negated), vec![])),
            ast::ClassSetItem::Bracketed(b) => out.push(self.bracketed(b)),
            ast::ClassSetItem::Union(u) => {
                for i in &u.items {
                    self.class_item(i, out);
                }
            }
        }
    }

    fn class_set(&self, set: &ast::ClassSet, out: &mut Vec<Value>) {
        match set {
            ast::ClassSet::Item(i) => self.class_item(i, out),
            ast::ClassSet::BinaryOp(op) => {
                let mut l = Vec::new();
                let mut r = Vec::new();
                self.class_set(&op.lhs, &mut l);
                self.class_set(&op.rhs, &mut r);
                let what = match op.kind {
                    ast::ClassSetBinaryOpKind::Intersection => "Intersection (&&): characters in both sets",
                    ast::ClassSetBinaryOpKind::Difference => "Difference (--): characters in the left set but not the right",
                    ast::ClassSetBinaryOpKind::SymmetricDifference => "Symmetric difference (~~): characters in exactly one set",
                };
                let lhs = node("set", self.text(op.lhs.span()), self.span(op.lhs.span()), "Left set".into(), l);
                let rhs = node("set", self.text(op.rhs.span()), self.span(op.rhs.span()), "Right set".into(), r);
                out.push(node("setop", self.text(&op.span), self.span(&op.span), what.into(), vec![lhs, rhs]));
            }
        }
    }

    fn bracketed(&self, b: &ast::ClassBracketed) -> Value {
        let mut children = Vec::new();
        self.class_set(&b.kind, &mut children);
        let desc = if b.negated { "Any single character NOT listed below" } else { "Any single character listed below" };
        node("class", self.text(&b.span), self.span(&b.span), desc.into(), children)
    }

    fn sequence(&self, asts: &[Ast], out: &mut Vec<Value>) {
        let mut run: Vec<&ast::Literal> = Vec::new();
        let flush = |run: &mut Vec<&ast::Literal>, out: &mut Vec<Value>| {
            match run.len() {
                0 => {}
                1 => out.push(self.literal(run[0])),
                _ => {
                    let (s, e) = (run[0].span.start.offset, run[run.len() - 1].span.end.offset);
                    let chars: String = run.iter().map(|l| l.c).collect();
                    let text = self.pattern.get(s..e).unwrap_or("");
                    out.push(node("literal", text, Some((self.map.at(s), self.map.at(e))), format!("Matches the characters \"{chars}\" literally"), vec![]));
                }
            }
            run.clear();
        };
        for a in asts {
            match a {
                Ast::Literal(l) if matches!(l.kind, ast::LiteralKind::Verbatim | ast::LiteralKind::Meta | ast::LiteralKind::Superfluous) => run.push(l),
                other => {
                    flush(&mut run, out);
                    self.ast(other, out);
                }
            }
        }
        flush(&mut run, out);
    }

    fn ast(&self, a: &Ast, out: &mut Vec<Value>) {
        match a {
            Ast::Empty(_) => {}
            Ast::Flags(f) => out.push(node("flags", self.text(&f.span), self.span(&f.span), format!("Set flags for the rest of the group: {}", describe_flags(&f.flags)), vec![])),
            Ast::Literal(l) => out.push(self.literal(l)),
            Ast::Dot(s) => out.push(node("dot", self.text(s), self.span(s), "Any character except a line break (including line breaks with the s flag)".into(), vec![])),
            Ast::Assertion(x) => out.push(node("assertion", self.text(&x.span), self.span(&x.span), assertion(&x.kind).into(), vec![])),
            Ast::ClassUnicode(c) => out.push(node("class", self.text(&c.span), self.span(&c.span), unicode_class(c), vec![])),
            Ast::ClassPerl(c) => out.push(node("class", self.text(&c.span), self.span(&c.span), perl_class(&c.kind, c.negated), vec![])),
            Ast::ClassBracketed(b) => out.push(self.bracketed(b)),
            Ast::Repetition(r) => {
                let (lo, hi) = match &r.op.kind {
                    ast::RepetitionKind::ZeroOrOne => (0, Some(1)),
                    ast::RepetitionKind::ZeroOrMore => (0, None),
                    ast::RepetitionKind::OneOrMore => (1, None),
                    ast::RepetitionKind::Range(ast::RepetitionRange::Exactly(n)) => (*n as usize, Some(*n as usize)),
                    ast::RepetitionKind::Range(ast::RepetitionRange::AtLeast(n)) => (*n as usize, None),
                    ast::RepetitionKind::Range(ast::RepetitionRange::Bounded(a, b)) => (*a as usize, Some(*b as usize)),
                };
                let mut children = Vec::new();
                self.ast(&r.ast, &mut children);
                let quant = node("quantifier", self.text(&r.op.span), self.span(&r.op.span), quantifier(lo, hi, r.greedy), vec![]);
                children.push(quant);
                out.push(node("repeat", self.text(&r.span), self.span(&r.span), format!("Repeat: {}", quantifier(lo, hi, r.greedy)), children));
            }
            Ast::Group(g) => {
                let mut children = Vec::new();
                self.ast(&g.ast, &mut children);
                let desc = match &g.kind {
                    ast::GroupKind::CaptureIndex(i) => format!("Capturing group #{i}"),
                    ast::GroupKind::CaptureName { name, .. } => format!("Named capturing group \"{}\" (#{})", name.name, name.index),
                    ast::GroupKind::NonCapturing(f) if f.items.is_empty() => "Non-capturing group".into(),
                    ast::GroupKind::NonCapturing(f) => format!("Non-capturing group with flags: {}", describe_flags(f)),
                };
                out.push(node("group", self.text(&g.span), self.span(&g.span), desc, children));
            }
            Ast::Alternation(alt) => {
                let children = alt
                    .asts
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let mut c = Vec::new();
                        self.ast(a, &mut c);
                        alternative(i, self.text(a.span()), self.span(a.span()), c)
                    })
                    .collect();
                out.push(node("alternation", self.text(&alt.span), self.span(&alt.span), format!("Matches one of {} alternatives (tried left to right)", alt.asts.len()), children));
            }
            Ast::Concat(c) => self.sequence(&c.asts, out),
        }
    }
}

/// Explains via the `regex-syntax` AST (exact source spans). Patterns that need backtracking
/// features fall back to fancy-regex's parse tree (no spans) in the pcre-like flavor.
pub fn explain(pattern: &str, flags: &str, flavor: &str) -> Result<Value> {
    if pattern.is_empty() {
        return Err(invalid("enter a pattern to explain"));
    }
    let f = parse_flags(flags)?;
    let flavor = Flavor::parse(flavor);
    let parsed = ast::parse::ParserBuilder::new().ignore_whitespace(f.x).build().parse(pattern);
    match parsed {
        Ok(a) => {
            let ex = Explainer { pattern, map: Utf16Map::new(pattern) };
            let mut nodes = Vec::new();
            ex.ast(&a, &mut nodes);
            let mut warnings = Vec::new();
            if let Err(e) = regex::RegexBuilder::new(pattern).ignore_whitespace(f.x).build() {
                warnings.push(format!("Parses, but does not compile: {e}"));
            }
            Ok(json!({ "flavor": flavor.name(), "source": "ast", "nodes": nodes, "warnings": warnings }))
        }
        Err(e) => {
            let unsupported = matches!(e.kind(), ast::ErrorKind::UnsupportedLookAround | ast::ErrorKind::UnsupportedBackreference);
            if flavor == Flavor::Rust || !unsupported {
                if flavor == Flavor::Pcre {
                    // Might still be valid fancy syntax (atomic groups, possessive quantifiers…).
                    if let Ok((expr, names)) = fancy_tree(pattern, f.x) {
                        return Ok(fancy_explain(&expr, &names));
                    }
                }
                let hint = if unsupported { "\nTip: switch the flavor to pcre-like for look-around and backreferences." } else { "" };
                return Err(invalid(format!("{e}{hint}")));
            }
            let (expr, names) = fancy_tree(pattern, f.x).map_err(|e| invalid(e.to_string()))?;
            Ok(fancy_explain(&expr, &names))
        }
    }
}

type GroupNames = std::collections::HashMap<usize, String>;

/// fancy-regex's parse tree plus group index → name (its `ExprTree` type isn't exported).
fn fancy_tree(pattern: &str, x: bool) -> fancy_regex::Result<(fancy_regex::Expr, GroupNames)> {
    let flags = fancy_regex::internal::FLAG_UNICODE | if x { fancy_regex::internal::FLAG_IGNORE_SPACE } else { 0 };
    let tree = fancy_regex::Expr::parse_tree_with_flags(pattern, flags)?;
    let names = tree.named_groups.iter().map(|(k, v)| (*v, k.clone())).collect();
    Ok((tree.expr, names))
}

fn fancy_explain(expr: &fancy_regex::Expr, names: &GroupNames) -> Value {
    let mut group = 0usize;
    let mut nodes = Vec::new();
    fancy_node(expr, &mut group, names, &mut nodes);
    json!({
        "flavor": "pcre-like",
        "source": "backtracking-parser",
        "nodes": nodes,
        "warnings": ["Uses backtracking-only features; explanation built from the fancy-regex parse tree (no source offsets)"],
    })
}

fn render(e: &fancy_regex::Expr) -> String {
    use fancy_regex::{Expr, LookAround};
    match e {
        Expr::Empty => String::new(),
        Expr::Any { newline: true, .. } => "(?s:.)".into(),
        Expr::Any { .. } => ".".into(),
        Expr::Assertion(a) => assertion_text(a).0.into(),
        Expr::Literal { val, .. } => regex::escape(val),
        Expr::Concat(v) => v.iter().map(render).collect(),
        Expr::Alt(v) => v.iter().map(render).collect::<Vec<_>>().join("|"),
        Expr::Group(inner) => format!("({})", render(inner)),
        Expr::LookAround(inner, kind) => {
            let open = match kind {
                LookAround::LookAhead => "(?=",
                LookAround::LookAheadNeg => "(?!",
                LookAround::LookBehind => "(?<=",
                LookAround::LookBehindNeg => "(?<!",
            };
            format!("{open}{})", render(inner))
        }
        Expr::Repeat { child, lo, hi, greedy } => {
            let q = match (*lo, *hi) {
                (0, 1) => "?".to_string(),
                (0, usize::MAX) => "*".into(),
                (1, usize::MAX) => "+".into(),
                (a, usize::MAX) => format!("{{{a},}}"),
                (a, b) if a == b => format!("{{{a}}}"),
                (a, b) => format!("{{{a},{b}}}"),
            };
            format!("{}{q}{}", render(child), if *greedy { "" } else { "?" })
        }
        Expr::Delegate { inner, .. } => inner.clone(),
        Expr::Backref { group, .. } => format!("\\{group}"),
        Expr::AtomicGroup(inner) => format!("(?>{})", render(inner)),
        Expr::KeepOut => "\\K".into(),
        Expr::ContinueFromPreviousMatchEnd => "\\G".into(),
        Expr::SubroutineCall(n) => format!("\\g<{n}>"),
        _ => "…".into(),
    }
}

fn assertion_text(a: &fancy_regex::Assertion) -> (&'static str, &'static str) {
    let dbg = format!("{a:?}");
    let name = dbg.split([' ', '{', '(']).next().unwrap_or("");
    match name {
        "StartText" => ("^", "Start of the text"),
        "EndText" => ("$", "End of the text"),
        "EndTextIgnoreTrailingNewlines" => ("\\Z", "End of the text (ignoring trailing newlines)"),
        "StartLine" => ("^", "Start of a line"),
        "EndLine" => ("$", "End of a line"),
        "LeftWordBoundary" => ("\\<", "Start of a word"),
        "RightWordBoundary" => ("\\>", "End of a word"),
        "WordBoundary" => ("\\b", "Word boundary"),
        "NotWordBoundary" => ("\\B", "Not a word boundary"),
        _ => ("", "Assertion"),
    }
}

fn fancy_node(e: &fancy_regex::Expr, group: &mut usize, names: &GroupNames, out: &mut Vec<Value>) {
    use fancy_regex::{Expr, LookAround};
    let text = render(e);
    let kids = |inner: &Expr, group: &mut usize| {
        let mut c = Vec::new();
        fancy_node(inner, group, names, &mut c);
        c
    };
    match e {
        Expr::Empty => {}
        Expr::Any { newline, .. } => out.push(node(
            "dot",
            &text,
            None,
            if *newline { "Any character, including line breaks" } else { "Any character except a line break" }.into(),
            vec![],
        )),
        Expr::Assertion(a) => out.push(node("assertion", &text, None, assertion_text(a).1.into(), vec![])),
        Expr::Literal { val, casei } => out.push(node(
            "literal",
            &text,
            None,
            format!("Matches \"{val}\" literally{}", if *casei { " (case-insensitive)" } else { "" }),
            vec![],
        )),
        Expr::Concat(v) => {
            // Merge runs of single-character literals into one node.
            let mut buf = String::new();
            let flush = |buf: &mut String, out: &mut Vec<Value>| {
                if !buf.is_empty() {
                    out.push(node("literal", &regex::escape(buf), None, format!("Matches the characters \"{buf}\" literally"), vec![]));
                    buf.clear();
                }
            };
            for c in v {
                if let Expr::Literal { val, casei: false } = c {
                    buf.push_str(val);
                } else {
                    flush(&mut buf, out);
                    fancy_node(c, group, names, out);
                }
            }
            flush(&mut buf, out);
        }
        Expr::Alt(v) => {
            let children = v
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let c = kids(a, group);
                    alternative(i, &render(a), None, c)
                })
                .collect();
            out.push(node("alternation", &text, None, format!("Matches one of {} alternatives (tried left to right)", v.len()), children));
        }
        Expr::Group(inner) => {
            *group += 1;
            let n = *group;
            let c = kids(inner, group);
            let desc = match names.get(&n) {
                Some(name) => format!("Named capturing group \"{name}\" (#{n})"),
                None => format!("Capturing group #{n}"),
            };
            out.push(node("group", &text, None, desc, c));
        }
        Expr::LookAround(inner, kind) => {
            let c = kids(inner, group);
            let desc = match kind {
                LookAround::LookAhead => "Positive lookahead: what follows must match (consumes nothing)",
                LookAround::LookAheadNeg => "Negative lookahead: what follows must NOT match",
                LookAround::LookBehind => "Positive lookbehind: what precedes must match",
                LookAround::LookBehindNeg => "Negative lookbehind: what precedes must NOT match",
            };
            out.push(node("lookaround", &text, None, desc.into(), c));
        }
        Expr::Repeat { child, lo, hi, greedy } => {
            let c = kids(child, group);
            let hi = (*hi != usize::MAX).then_some(*hi);
            out.push(node("repeat", &text, None, format!("Repeat: {}", quantifier(*lo, hi, *greedy)), c));
        }
        Expr::Delegate { inner, casei } => {
            // Plain-regex fragment (classes, escapes): explain it with the exact AST walker.
            let mut c = Vec::new();
            if let Ok(a) = ast::parse::Parser::new().parse(inner) {
                let ex = Explainer { pattern: inner, map: Utf16Map::new(inner) };
                ex.ast(&a, &mut c);
                for n in &mut c {
                    strip_spans(n);
                }
            }
            if c.len() == 1 && !casei {
                out.push(c.remove(0));
            } else {
                out.push(node("fragment", inner, None, format!("Regular sub-pattern{}", if *casei { " (case-insensitive)" } else { "" }), c));
            }
        }
        Expr::Backref { group: g, casei } => out.push(node(
            "backref",
            &text,
            None,
            format!(
                "Backreference: the same text captured by group #{g}{}{}",
                names.get(g).map(|n| format!(" (\"{n}\")")).unwrap_or_default(),
                if *casei { ", case-insensitive" } else { "" }
            ),
            vec![],
        )),
        Expr::AtomicGroup(inner) => {
            let c = kids(inner, group);
            out.push(node("group", &text, None, "Atomic group: once matched, never backtracked into".into(), c));
        }
        Expr::KeepOut => out.push(node("assertion", &text, None, "\\K: drop everything matched so far from the reported match".into(), vec![])),
        Expr::ContinueFromPreviousMatchEnd => out.push(node("assertion", &text, None, "\\G: position where the previous match ended".into(), vec![])),
        Expr::SubroutineCall(n) => out.push(node("call", &text, None, format!("Subroutine call: re-run the pattern of group #{n}"), vec![])),
        Expr::Conditional { condition, true_branch, false_branch } => {
            let c = vec![
                node("condition", &render(condition), None, "Condition".into(), kids(condition, group)),
                node("branch", &render(true_branch), None, "If the condition holds".into(), kids(true_branch, group)),
                node("branch", &render(false_branch), None, "Otherwise".into(), kids(false_branch, group)),
            ];
            out.push(node("conditional", &text, None, "Conditional".into(), c));
        }
        Expr::BackrefExistsCondition { group: g, .. } => out.push(node("condition", &text, None, format!("True if group #{g} has matched"), vec![])),
        other => out.push(node("other", &text, None, format!("{other:?}"), vec![])),
    }
}

fn strip_spans(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        o.remove("start");
        o.remove("end");
        if let Some(Value::Array(c)) = o.get_mut("children") {
            c.iter_mut().for_each(strip_spans);
        }
    }
}
