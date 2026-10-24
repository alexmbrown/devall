//! The log query language.
//!
//! ```text
//! timeout                      free text, case-insensitive, anywhere in the entry
//! "connection reset"           phrase
//! /time(d)?out/i               regex over the raw entry (flags: i m s x)
//! status>=500                  field comparison (numeric when both sides are numbers)
//! user.id=42                   dotted paths into nested JSON
//! path=/api/*                  glob (case-insensitive); `field=*` = field exists
//! msg~"timeout|refused"        regex on a field (case-insensitive unless written /re/)
//! msg!~"health"   level!=debug field negations (also true when the field is missing)
//! host:web                     contains (case-insensitive)
//! level>=warn                  levels: trace < debug < info < warn < error < fatal
//! ts>=now-15m  ts<"2024-01-02T10:00"   time (also -15m, now, dates, epoch ms)
//! a b  /  a AND b  /  a && b   all must match (juxtaposition = AND)
//! a OR b  /  a || b            either
//! NOT a  /  !a  /  -a          negation
//! (a OR b) -c                  grouping
//! ```
//! Special fields: `level`, `msg`/`message`, `ts`/`time`, `raw`, `line`, `source`, `format`,
//! `trace`/`correlation`. Keywords are upper-case so the words "and"/"or"/"not" stay searchable.

use std::cell::OnceCell;
use std::fmt;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::parse::{Level, LogEntry};
use crate::time::{self, TimeCtx};

/// A parse error with the character range it refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryError {
    pub message: String,
    /// Character offsets into the query.
    pub start: usize,
    pub end: usize,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at {})", self.message, self.start + 1)
    }
}

impl std::error::Error for QueryError {}

fn qerr(message: impl Into<String>, start: usize, end: usize) -> QueryError {
    QueryError { message: message.into(), start, end: end.max(start + 1) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Match,
    NotMatch,
    Contains,
    Exists,
}

impl Op {
    fn symbol(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Match => "~",
            Op::NotMatch => "!~",
            Op::Contains => ":",
            Op::Exists => "=*",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldRef {
    Level,
    Message,
    Ts,
    Raw,
    Line,
    Source,
    Format,
    Correlation(String),
    Path(String),
}

#[derive(Debug, Clone)]
pub struct QValue {
    pub text: String,
    lower: String,
    num: Option<f64>,
    level: Option<Level>,
    ts: Option<i64>,
    re: Option<Regex>,
}

#[derive(Debug, Clone)]
pub enum Expr {
    All,
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    /// Case-insensitive literal (compiled to a regex for speed).
    Text(Regex),
    Regex(Regex),
    Field { field: FieldRef, op: Op, value: QValue },
}

/// What a query is evaluated against.
pub trait Subject {
    fn raw(&self) -> &str;
    /// Effective timestamp (carried forward for entries without their own).
    fn ts(&self) -> Option<i64>;
    fn level(&self) -> Level;
    fn line_no(&self) -> u64;
    fn source(&self) -> Option<&str>;
    /// The fully parsed entry (may be parsed lazily).
    fn entry(&self) -> &LogEntry;
}

impl Subject for LogEntry {
    fn raw(&self) -> &str {
        &self.raw
    }
    fn ts(&self) -> Option<i64> {
        self.ts
    }
    fn level(&self) -> Level {
        self.level
    }
    fn line_no(&self) -> u64 {
        self.line_no
    }
    fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
    fn entry(&self) -> &LogEntry {
        self
    }
}

/// A subject that parses its entry only when a field predicate needs it.
pub struct Lazy<'a, F: Fn(&str) -> LogEntry> {
    pub raw: &'a str,
    pub ts: Option<i64>,
    pub level: Level,
    pub line_no: u64,
    pub source: Option<&'a str>,
    make: F,
    cell: OnceCell<LogEntry>,
}

impl<'a, F: Fn(&str) -> LogEntry> Lazy<'a, F> {
    pub fn new(raw: &'a str, ts: Option<i64>, level: Level, line_no: u64, source: Option<&'a str>, make: F) -> Self {
        Self { raw, ts, level, line_no, source, make, cell: OnceCell::new() }
    }
}

impl<F: Fn(&str) -> LogEntry> Subject for Lazy<'_, F> {
    fn raw(&self) -> &str {
        self.raw
    }
    fn ts(&self) -> Option<i64> {
        self.ts
    }
    fn level(&self) -> Level {
        self.level
    }
    fn line_no(&self) -> u64 {
        self.line_no
    }
    fn source(&self) -> Option<&str> {
        self.source
    }
    fn entry(&self) -> &LogEntry {
        self.cell.get_or_init(|| (self.make)(self.raw))
    }
}

/// A parsed query.
#[derive(Debug, Clone)]
pub struct Query {
    pub expr: Expr,
    pub text: String,
}

impl Default for Query {
    fn default() -> Self {
        Self { expr: Expr::All, text: String::new() }
    }
}

impl Query {
    pub fn parse(text: &str) -> Result<Query, QueryError> {
        Self::parse_at(text, crate::now_ms(), &TimeCtx::default())
    }

    /// Parses with an explicit "now" (relative times) and zone context (absolute times).
    pub fn parse_at(text: &str, now: i64, ctx: &TimeCtx) -> Result<Query, QueryError> {
        let toks = lex(text)?;
        let mut p = Parser { toks, pos: 0, now, ctx, len: text.chars().count() };
        let expr = if p.toks.is_empty() { Expr::All } else { p.or()? };
        if let Some(t) = p.toks.get(p.pos) {
            return Err(qerr(
                match t.kind {
                    TokKind::RParen => "unexpected ')' (no matching '(')",
                    _ => "unexpected input",
                },
                t.start,
                t.end,
            ));
        }
        Ok(Query { expr, text: text.to_string() })
    }

    pub fn is_all(&self) -> bool {
        matches!(self.expr, Expr::All)
    }

    pub fn matches(&self, s: &dyn Subject) -> bool {
        eval(&self.expr, s)
    }
}

// ---------- lexer ----------

#[derive(Debug, Clone, PartialEq)]
enum TokKind {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Term { text: String },
    Regex { pat: String, flags: String },
    Pred { field: String, op: Op, value: Val },
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Bare(String),
    Quoted(String),
    Regex(String, String),
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TokKind,
    start: usize,
    end: usize,
}

fn is_field_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '@' || c == '$'
}

fn is_field_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '$' | '-')
}

/// Reads a quoted string starting at `chars[i] == '"'`. Only `\"` and `\\` are escapes; other
/// backslashes stay (so regexes don't need double escaping).
fn read_quoted(chars: &[char], i: usize) -> Result<(String, usize), QueryError> {
    let mut out = String::new();
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '\\' if j + 1 < chars.len() && (chars[j + 1] == '"' || chars[j + 1] == '\\') => {
                out.push(chars[j + 1]);
                j += 2;
            }
            '"' => return Ok((out, j + 1)),
            c => {
                out.push(c);
                j += 1;
            }
        }
    }
    Err(qerr("unterminated string (missing closing \")", i, chars.len()))
}

/// `/pattern/flags` at `chars[i] == '/'`; `None` when it isn't a well-formed regex literal.
fn read_regex(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    let mut pat = String::new();
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '\\' if j + 1 < chars.len() && chars[j + 1] == '/' => {
                pat.push('/');
                j += 2;
            }
            '\\' if j + 1 < chars.len() => {
                pat.push('\\');
                pat.push(chars[j + 1]);
                j += 2;
            }
            '/' => {
                let mut k = j + 1;
                let mut flags = String::new();
                while k < chars.len() && matches!(chars[k], 'i' | 'm' | 's' | 'x') {
                    flags.push(chars[k]);
                    k += 1;
                }
                if k < chars.len() && !chars[k].is_whitespace() && chars[k] != ')' {
                    return None;
                }
                if pat.is_empty() {
                    return None;
                }
                return Some((pat, flags, k));
            }
            c => {
                pat.push(c);
                j += 1;
            }
        }
    }
    None
}

fn read_op(chars: &[char], i: usize) -> Option<(Op, usize)> {
    let c = *chars.get(i)?;
    let n = chars.get(i + 1).copied();
    Some(match (c, n) {
        ('>', Some('=')) => (Op::Ge, 2),
        ('<', Some('=')) => (Op::Le, 2),
        ('!', Some('=')) => (Op::Ne, 2),
        ('!', Some('~')) => (Op::NotMatch, 2),
        ('=', Some('~')) => (Op::Match, 2),
        ('=', Some('=')) => (Op::Eq, 2),
        ('=', _) => (Op::Eq, 1),
        ('>', _) => (Op::Gt, 1),
        ('<', _) => (Op::Lt, 1),
        ('~', _) => (Op::Match, 1),
        (':', _) => (Op::Contains, 1),
        _ => return None,
    })
}

fn bare_end(chars: &[char], mut j: usize) -> usize {
    while j < chars.len() && !chars[j].is_whitespace() && chars[j] != ')' && chars[j] != '(' {
        j += 1;
    }
    j
}

fn lex(text: &str) -> Result<Vec<Tok>, QueryError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let push = |out: &mut Vec<Tok>, kind, end| out.push(Tok { kind, start, end });
        match c {
            '(' => {
                push(&mut out, TokKind::LParen, i + 1);
                i += 1;
            }
            ')' => {
                push(&mut out, TokKind::RParen, i + 1);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                push(&mut out, TokKind::And, i + 2);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                push(&mut out, TokKind::Or, i + 2);
                i += 2;
            }
            '!' if chars.get(i + 1).is_some_and(|n| !n.is_whitespace() && *n != '=' && *n != '~') => {
                push(&mut out, TokKind::Not, i + 1);
                i += 1;
            }
            '-' if chars.get(i + 1).is_some_and(|n| !n.is_whitespace() && !n.is_ascii_digit() && *n != '-') => {
                push(&mut out, TokKind::Not, i + 1);
                i += 1;
            }
            '"' => {
                let (s, j) = read_quoted(&chars, i)?;
                push(&mut out, TokKind::Term { text: s }, j);
                i = j;
            }
            '/' if read_regex(&chars, i).is_some() => {
                let (pat, flags, j) = read_regex(&chars, i).unwrap();
                push(&mut out, TokKind::Regex { pat, flags }, j);
                i = j;
            }
            _ => {
                // Field predicate?
                if is_field_start(c) {
                    let mut j = i;
                    while j < chars.len() && is_field_char(chars[j]) {
                        j += 1;
                    }
                    if let Some((op, n)) = read_op(&chars, j) {
                        let field: String = chars[i..j].iter().collect();
                        let vstart = j + n;
                        let next = chars.get(vstart).copied();
                        // `http://…` and `C:\…` are text, not `field:value`.
                        let texty = op == Op::Contains && matches!(next, Some('/') | Some('\\'));
                        if !texty {
                            let (value, vend) = match next {
                                None => return Err(qerr(format!("expected a value after '{field}{}'", op.symbol()), i, vstart)),
                                Some(ch) if ch.is_whitespace() || ch == ')' => {
                                    return Err(qerr(format!("expected a value after '{field}{}'", op.symbol()), i, vstart));
                                }
                                Some('"') => {
                                    let (s, e) = read_quoted(&chars, vstart)?;
                                    (Val::Quoted(s), e)
                                }
                                Some('/') if matches!(op, Op::Match | Op::NotMatch) && read_regex(&chars, vstart).is_some() => {
                                    let (p, f, e) = read_regex(&chars, vstart).unwrap();
                                    (Val::Regex(p, f), e)
                                }
                                Some(_) => {
                                    let e = bare_end(&chars, vstart);
                                    (Val::Bare(chars[vstart..e].iter().collect()), e)
                                }
                            };
                            push(&mut out, TokKind::Pred { field, op, value }, vend);
                            i = vend;
                            continue;
                        }
                    }
                }
                let j = bare_end(&chars, i);
                let word: String = chars[i..j].iter().collect();
                let kind = match word.as_str() {
                    "AND" => TokKind::And,
                    "OR" => TokKind::Or,
                    "NOT" => TokKind::Not,
                    _ => TokKind::Term { text: word },
                };
                push(&mut out, kind, j);
                i = j;
            }
        }
    }
    Ok(out)
}

// ---------- parser ----------

struct Parser<'c> {
    toks: Vec<Tok>,
    pos: usize,
    now: i64,
    ctx: &'c TimeCtx,
    len: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn or(&mut self) -> Result<Expr, QueryError> {
        let mut items = vec![self.and()?];
        while let Some(t) = self.peek() {
            if t.kind != TokKind::Or {
                break;
            }
            let (s, e) = (t.start, t.end);
            self.pos += 1;
            if self.peek().is_none_or(|t| matches!(t.kind, TokKind::RParen | TokKind::Or | TokKind::And)) {
                return Err(qerr("OR needs a term on its right", s, e));
            }
            items.push(self.and()?);
        }
        Ok(if items.len() == 1 { items.pop().unwrap() } else { Expr::Or(items) })
    }

    fn and(&mut self) -> Result<Expr, QueryError> {
        let mut items = vec![self.not()?];
        while let Some(t) = self.peek() {
            match t.kind {
                TokKind::And => {
                    let (s, e) = (t.start, t.end);
                    self.pos += 1;
                    if self.peek().is_none_or(|t| matches!(t.kind, TokKind::RParen | TokKind::Or | TokKind::And)) {
                        return Err(qerr("AND needs a term on its right", s, e));
                    }
                    items.push(self.not()?);
                }
                TokKind::Or | TokKind::RParen => break,
                _ => items.push(self.not()?),
            }
        }
        Ok(if items.len() == 1 { items.pop().unwrap() } else { Expr::And(items) })
    }

    fn not(&mut self) -> Result<Expr, QueryError> {
        if let Some(t) = self.peek()
            && t.kind == TokKind::Not
        {
            let (s, e) = (t.start, t.end);
            self.pos += 1;
            if self.peek().is_none_or(|t| matches!(t.kind, TokKind::RParen | TokKind::Or | TokKind::And)) {
                return Err(qerr("NOT needs a term after it", s, e));
            }
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, QueryError> {
        let Some(t) = self.toks.get(self.pos).cloned() else {
            return Err(qerr("expected a search term", self.len, self.len));
        };
        self.pos += 1;
        match t.kind {
            TokKind::LParen => {
                if self.peek().is_some_and(|t| t.kind == TokKind::RParen) {
                    return Err(qerr("empty parentheses", t.start, self.peek().unwrap().end));
                }
                let e = self.or()?;
                match self.peek() {
                    Some(r) if r.kind == TokKind::RParen => {
                        self.pos += 1;
                        Ok(e)
                    }
                    _ => Err(qerr("missing ')' for this '('", t.start, t.end)),
                }
            }
            TokKind::RParen => Err(qerr("unexpected ')'", t.start, t.end)),
            TokKind::And => Err(qerr("AND needs a term on its left", t.start, t.end)),
            TokKind::Or => Err(qerr("OR needs a term on its left", t.start, t.end)),
            TokKind::Not => unreachable!("handled by not()"),
            TokKind::Term { text } => text_expr(&text).map_err(|m| qerr(m, t.start, t.end)),
            TokKind::Regex { pat, flags } => compile_regex(&pat, &flags).map(Expr::Regex).map_err(|m| qerr(m, t.start, t.end)),
            TokKind::Pred { field, op, value } => self.pred(&field, op, value).map_err(|m| qerr(m, t.start, t.end)),
        }
    }

    fn pred(&self, field: &str, op: Op, value: Val) -> Result<Expr, String> {
        let lower = field.to_ascii_lowercase();
        let fref = match lower.as_str() {
            "level" | "lvl" => FieldRef::Level,
            "msg" | "message" => FieldRef::Message,
            "ts" | "time" | "timestamp" | "@timestamp" => FieldRef::Ts,
            "raw" | "_raw" => FieldRef::Raw,
            "line" | "line_no" | "lineno" => FieldRef::Line,
            "source" | "_source" => FieldRef::Source,
            "format" => FieldRef::Format,
            "trace" | "trace_id" | "traceid" | "correlation" | "correlation_id" | "correlationid" => FieldRef::Correlation(field.to_string()),
            _ => FieldRef::Path(field.to_string()),
        };
        let (text, explicit_re) = match value {
            Val::Bare(s) | Val::Quoted(s) => (s, None),
            Val::Regex(p, f) => (p.clone(), Some((p, f))),
        };
        let mut op = op;
        let mut q = QValue { lower: text.to_lowercase(), num: text.trim().parse::<f64>().ok(), text, level: None, ts: None, re: None };
        match op {
            Op::Match | Op::NotMatch => {
                q.re = Some(match explicit_re {
                    Some((p, f)) => compile_regex(&p, &f)?,
                    None => compile_regex(&q.text, "i")?,
                });
            }
            Op::Eq | Op::Ne if q.text == "*" => {
                if op == Op::Ne {
                    return Ok(Expr::Not(Box::new(Expr::Field { field: fref, op: Op::Exists, value: q })));
                }
                op = Op::Exists;
            }
            Op::Eq | Op::Ne if q.text.contains('*') && fref != FieldRef::Level => {
                let pat = format!("^{}$", q.text.split('*').map(regex::escape).collect::<Vec<_>>().join(".*"));
                q.re = Some(compile_regex(&pat, "is")?);
            }
            _ => {}
        }
        match &fref {
            FieldRef::Level if !matches!(op, Op::Match | Op::NotMatch | Op::Exists) => {
                q.level = Some(if q.lower == "unknown" { Level::Unknown } else { Level::parse(&q.text).ok_or_else(|| format!("unknown level '{}' (use trace, debug, info, warn, error, fatal)", q.text))? });
                if op == Op::Contains {
                    op = Op::Eq;
                }
            }
            FieldRef::Ts if !matches!(op, Op::Match | Op::NotMatch | Op::Exists | Op::Contains) => {
                q.ts = Some(time::parse_query_time(&q.text, self.now, self.ctx).ok_or_else(|| format!("can't read '{}' as a time (try now-15m, 2024-01-02T10:00, epoch ms)", q.text))?);
            }
            FieldRef::Line if matches!(op, Op::Gt | Op::Ge | Op::Lt | Op::Le) && q.num.is_none() => {
                return Err(format!("line numbers are numeric, got '{}'", q.text));
            }
            _ => {}
        }
        Ok(Expr::Field { field: fref, op, value: q })
    }
}

fn compile_regex(pat: &str, flags: &str) -> Result<Regex, String> {
    let full = if flags.is_empty() { pat.to_string() } else { format!("(?{flags}){pat}") };
    regex::RegexBuilder::new(&full).size_limit(10 << 20).build().map_err(|e| {
        let msg = e.to_string();
        let last = msg.lines().rev().find(|l| l.starts_with("error:")).unwrap_or("invalid regex").trim_start_matches("error: ");
        format!("invalid regex: {last}")
    })
}

fn text_expr(text: &str) -> Result<Expr, String> {
    if text.is_empty() {
        return Ok(Expr::All);
    }
    compile_regex(&regex::escape(text), "i").map(Expr::Text)
}

// ---------- evaluation ----------

fn eval(e: &Expr, s: &dyn Subject) -> bool {
    match e {
        Expr::All => true,
        Expr::And(items) => items.iter().all(|x| eval(x, s)),
        Expr::Or(items) => items.iter().any(|x| eval(x, s)),
        Expr::Not(x) => !eval(x, s),
        Expr::Text(re) | Expr::Regex(re) => re.is_match(s.raw()),
        Expr::Field { field, op, value } => eval_field(field, *op, value, s),
    }
}

fn eval_field(field: &FieldRef, op: Op, q: &QValue, s: &dyn Subject) -> bool {
    match field {
        FieldRef::Level => {
            let l = s.level();
            match op {
                Op::Exists => l != Level::Unknown,
                Op::Match => q.re.as_ref().is_some_and(|r| r.is_match(l.as_str())),
                Op::NotMatch => !q.re.as_ref().is_some_and(|r| r.is_match(l.as_str())),
                Op::Eq => Some(l) == q.level,
                Op::Ne => Some(l) != q.level,
                _ => match (l.rank(), q.level.and_then(Level::rank)) {
                    (Some(a), Some(b)) => cmp_ord(op, a.cmp(&b)),
                    _ => false,
                },
            }
        }
        FieldRef::Ts => match (op, s.ts()) {
            (Op::Exists, t) => t.is_some(),
            (Op::Ne, None) => true,
            (_, None) => false,
            (Op::Match | Op::NotMatch | Op::Contains, Some(t)) => str_op(op, &time::format_iso(t), q),
            (_, Some(t)) => q.ts.is_some_and(|v| cmp_ord(op, t.cmp(&v))),
        },
        FieldRef::Line => num_or_str(op, Some(&Value::from(s.line_no())), q),
        FieldRef::Raw => str_op_present(op, Some(s.raw()), q),
        FieldRef::Source => str_op_present(op, s.source(), q),
        FieldRef::Message => str_op_present(op, Some(&s.entry().message), q),
        FieldRef::Format => str_op_present(op, Some(s.entry().format.as_str()), q),
        FieldRef::Correlation(name) => {
            let e = s.entry();
            match &e.correlation_id {
                Some(c) => str_op_present(op, Some(c), q),
                None => num_or_str(op, e.field(name), q),
            }
        }
        FieldRef::Path(name) => num_or_str(op, s.entry().field(name), q),
    }
}

fn cmp_ord(op: Op, o: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        Op::Eq => o == Equal,
        Op::Ne => o != Equal,
        Op::Gt => o == Greater,
        Op::Ge => o != Less,
        Op::Lt => o == Less,
        Op::Le => o != Greater,
        _ => false,
    }
}

/// Positive string operators on a present value.
fn str_op(op: Op, v: &str, q: &QValue) -> bool {
    match op {
        Op::Exists => true,
        Op::Match => q.re.as_ref().is_some_and(|r| r.is_match(v)),
        Op::NotMatch => !q.re.as_ref().is_some_and(|r| r.is_match(v)),
        Op::Contains => v.to_lowercase().contains(&q.lower),
        Op::Eq => match &q.re {
            Some(r) => r.is_match(v),
            None => v == q.text || v.to_lowercase() == q.lower,
        },
        Op::Ne => !str_op(Op::Eq, v, q),
        Op::Gt | Op::Ge | Op::Lt | Op::Le => match (q.num, v.trim().parse::<f64>().ok()) {
            (Some(b), Some(a)) => a.partial_cmp(&b).is_some_and(|o| cmp_ord(op, o)),
            _ => cmp_ord(op, v.cmp(q.text.as_str())),
        },
    }
}

fn str_op_present(op: Op, v: Option<&str>, q: &QValue) -> bool {
    match v {
        Some(v) => str_op(op, v, q),
        None => matches!(op, Op::Ne | Op::NotMatch),
    }
}

fn num_or_str(op: Op, v: Option<&Value>, q: &QValue) -> bool {
    let negative = matches!(op, Op::Ne | Op::NotMatch);
    let Some(v) = v.filter(|v| !v.is_null()) else { return negative };
    if negative {
        // Negations hold when no element matches.
        let pos = if op == Op::Ne { Op::Eq } else { Op::Match };
        return !num_or_str(pos, Some(v), q);
    }
    match v {
        Value::Array(items) => items.iter().any(|x| num_or_str(op, Some(x), q)),
        Value::Number(n) => match (q.num, n.as_f64()) {
            (Some(b), Some(a)) if !matches!(op, Op::Match | Op::Contains | Op::Exists) && q.re.is_none() => {
                a.partial_cmp(&b).is_some_and(|o| cmp_ord(op, o))
            }
            _ => str_op(op, &n.to_string(), q),
        },
        Value::String(s) => str_op(op, s, q),
        Value::Bool(b) => str_op(op, if *b { "true" } else { "false" }, q),
        Value::Object(_) => match op {
            Op::Exists => true,
            Op::Match | Op::Contains => str_op(op, &v.to_string(), q),
            _ => false,
        },
        Value::Null => false,
    }
}

// ---------- filter ----------

/// What the UI sends: a query plus level toggles and a time range.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    pub query: String,
    /// Empty = all levels.
    pub levels: Vec<Level>,
    /// Inclusive Unix ms.
    pub from: Option<i64>,
    /// Exclusive Unix ms.
    pub to: Option<i64>,
}

impl Filter {
    pub fn query(q: &str) -> Self {
        Self { query: q.to_string(), ..Default::default() }
    }

    pub fn compile(&self) -> Result<Compiled, QueryError> {
        self.compile_at(crate::now_ms(), &TimeCtx::default())
    }

    pub fn compile_at(&self, now: i64, ctx: &TimeCtx) -> Result<Compiled, QueryError> {
        let query = Query::parse_at(&self.query, now, ctx)?;
        let levels = if self.levels.is_empty() || self.levels.len() >= Level::ALL.len() {
            None
        } else {
            let mut set = [false; 7];
            for l in &self.levels {
                set[*l as usize] = true;
            }
            Some(set)
        };
        Ok(Compiled { query, levels, from: self.from, to: self.to })
    }

    pub fn is_trivial(&self) -> bool {
        self.query.trim().is_empty() && (self.levels.is_empty() || self.levels.len() >= Level::ALL.len()) && self.from.is_none() && self.to.is_none()
    }
}

/// A compiled [`Filter`].
#[derive(Debug, Clone, Default)]
pub struct Compiled {
    pub query: Query,
    levels: Option<[bool; 7]>,
    pub from: Option<i64>,
    pub to: Option<i64>,
}

impl Compiled {
    /// Level toggles and time range (no text needed).
    pub fn meta_pass(&self, ts: Option<i64>, level: Level) -> bool {
        if let Some(set) = &self.levels
            && !set[level as usize]
        {
            return false;
        }
        if self.from.is_some() || self.to.is_some() {
            let Some(t) = ts else { return false };
            if self.from.is_some_and(|f| t < f) || self.to.is_some_and(|to| t >= to) {
                return false;
            }
        }
        true
    }

    pub fn is_trivial(&self) -> bool {
        self.levels.is_none() && self.from.is_none() && self.to.is_none() && self.query.is_all()
    }

    pub fn matches(&self, s: &dyn Subject) -> bool {
        self.meta_pass(s.ts(), s.level()) && self.query.matches(s)
    }
}

/// Quotes a value for use in a query (`"` and `\` escaped).
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Format, parse_text};
    use crate::time::NaiveTz;

    fn ctx() -> TimeCtx {
        TimeCtx::new(NaiveTz::Utc, 2024)
    }

    const NOW: i64 = 1_704_164_645_000; // 2024-01-02T03:04:05Z

    fn q(s: &str) -> Query {
        Query::parse_at(s, NOW, &ctx()).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn entries() -> Vec<LogEntry> {
        let text = [
            r#"{"ts":"2024-01-02T03:00:00Z","level":"info","msg":"GET /api/users done","status":200,"user":{"id":42},"path":"/api/users","tags":["a","b"]}"#,
            r#"{"ts":"2024-01-02T03:01:00Z","level":"warn","msg":"slow request timeout soon","status":200,"dur":1.5,"path":"/api/orders"}"#,
            r#"{"ts":"2024-01-02T03:02:00Z","level":"error","msg":"upstream timeout","status":504,"user":{"id":7},"path":"/api/orders","trace_id":"t-9"}"#,
            r#"{"ts":"2024-01-02T03:03:00Z","level":"debug","msg":"health check","status":"200"}"#,
        ]
        .join("\n");
        let (det, e) = parse_text(&text, &ctx());
        assert_eq!(det.format, Format::Json);
        e
    }

    fn hits(s: &str) -> Vec<u64> {
        let qq = q(s);
        entries().iter().filter(|e| qq.matches(*e)).map(|e| e.index).collect()
    }

    #[test]
    fn evaluation() {
        assert_eq!(hits(""), vec![0, 1, 2, 3]);
        assert_eq!(hits("TIMEOUT"), vec![1, 2]);
        assert_eq!(hits("\"upstream timeout\""), vec![2]);
        assert_eq!(hits("/time(d)?out$/"), Vec::<u64>::new()); // raw ends with }
        assert_eq!(hits(r#"/"upstream \w+"/"#), vec![2]);
        assert_eq!(hits("status>=500"), vec![2]);
        assert_eq!(hits("status=200"), vec![0, 1, 3]); // "200" string compares numerically too
        assert_eq!(hits("status!=200"), vec![2]);
        assert_eq!(hits("user.id=42"), vec![0]);
        assert_eq!(hits("user.id>5"), vec![0, 2]);
        assert_eq!(hits("user=*"), vec![0, 2]);
        assert_eq!(hits("user!=*"), vec![1, 3]);
        assert_eq!(hits("msg~\"timeout\""), vec![1, 2]);
        assert_eq!(hits("msg~/^upstream/"), vec![2]);
        assert_eq!(hits("msg!~health"), vec![0, 1, 2]);
        assert_eq!(hits("path=/api/*"), vec![0, 1, 2]);
        assert_eq!(hits("path:ORDERS"), vec![1, 2]);
        assert_eq!(hits("level>=warn"), vec![1, 2]);
        assert_eq!(hits("level:error"), vec![2]);
        assert_eq!(hits("level<info"), vec![3]);
        assert_eq!(hits("timeout AND level=error"), vec![2]);
        assert_eq!(hits("timeout level=warn"), vec![1]);
        assert_eq!(hits("timeout OR health"), vec![1, 2, 3]);
        assert_eq!(hits("timeout || health"), vec![1, 2, 3]);
        assert_eq!(hits("NOT timeout"), vec![0, 3]);
        assert_eq!(hits("-timeout !health"), vec![0]);
        assert_eq!(hits("(timeout OR health) AND NOT level=error"), vec![1, 3]);
        assert_eq!(hits("tags=b"), vec![0]);
        assert_eq!(hits("trace=t-9"), vec![2]);
        assert_eq!(hits("dur>1"), vec![1]);
        assert_eq!(hits("ts>=2024-01-02T03:01:00Z ts<\"2024-01-02T03:03:00Z\""), vec![1, 2]);
        assert_eq!(hits("ts>=now-3m"), vec![2, 3]);
        assert_eq!(hits("ts>-186s"), vec![1, 2, 3]);
        assert_eq!(hits("line>2"), vec![2, 3]);
        assert_eq!(hits("format=json"), vec![0, 1, 2, 3]);
        assert_eq!(hits("and or not"), Vec::<u64>::new()); // lower-case keywords are words
        assert_eq!(hits("-1"), Vec::<u64>::new()); // a negative number is text, not NOT 1
        assert_eq!(hits("http://x"), Vec::<u64>::new()); // text, not field `http`
    }

    #[test]
    fn errors() {
        let err = |s: &str| Query::parse_at(s, NOW, &ctx()).unwrap_err();
        let e = err("status>=");
        assert!(e.message.contains("expected a value"), "{e}");
        assert_eq!(e.start, 0);
        let e = err("(a OR b");
        assert!(e.message.contains("missing ')'"));
        assert_eq!((e.start, e.end), (0, 1));
        let e = err("a OR b)");
        assert!(e.message.contains("unexpected ')'"));
        assert_eq!(e.start, 6);
        assert!(err("\"open").message.contains("unterminated"));
        assert!(err("level=loud").message.contains("unknown level"));
        assert!(err("ts>yesterdayish").message.contains("as a time"));
        assert!(err("msg~\"(\"").message.contains("invalid regex"));
        assert!(err("a AND").message.contains("AND needs"));
        assert!(err("OR a").message.contains("OR needs a term on its left"));
        assert!(err("NOT").message.contains("NOT needs"));
        assert!(err("()").message.contains("empty"));
        let e = err("x  status>");
        assert_eq!(e.start, 3);
    }

    #[test]
    fn filter_meta_and_quote() {
        let f = Filter { query: String::new(), levels: vec![Level::Error, Level::Warn], from: Some(10), to: Some(20) };
        let c = f.compile_at(NOW, &ctx()).unwrap();
        assert!(c.meta_pass(Some(10), Level::Error));
        assert!(!c.meta_pass(Some(20), Level::Error));
        assert!(!c.meta_pass(None, Level::Warn));
        assert!(!c.meta_pass(Some(15), Level::Info));
        assert!(Filter::default().is_trivial());
        let quoted = quote(r#"say "hi" \d"#);
        assert_eq!(quoted, r#""say \"hi\" \\d""#);
        let parsed = q(&format!("msg={quoted}"));
        let Expr::Field { value, .. } = parsed.expr else { panic!() };
        assert_eq!(value.text, r#"say "hi" \d"#);
    }
}
