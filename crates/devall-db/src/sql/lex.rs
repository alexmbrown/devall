//! A small, dialect-aware SQL lexer used to split scripts into statements (respecting strings,
//! quoted identifiers, comments, Postgres dollar quotes, MySQL `DELIMITER`, `BEGIN … END`
//! bodies of triggers/procedures, and T-SQL `GO` batch separators) and to classify statements as
//! read-only or mutating.
//!
//! Classification is deliberately conservative: anything not known to be read-only counts as a
//! write. It is a first line of defence with clear messages; the database session itself is also
//! read-only (see the engines), which catches what text analysis can't (e.g. `SELECT nextval()`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    Postgres,
    Mysql,
    Sqlite,
    /// T-SQL (SQL Server, Azure SQL).
    Sqlserver,
}

impl Dialect {
    pub fn data_dialect(self) -> devall_data::SqlDialect {
        match self {
            Dialect::Postgres => devall_data::SqlDialect::Postgres,
            Dialect::Mysql => devall_data::SqlDialect::Mysql,
            Dialect::Sqlite => devall_data::SqlDialect::Sqlite,
            Dialect::Sqlserver => devall_data::SqlDialect::Sqlserver,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokKind {
    Word,
    Number,
    /// String literal (incl. dollar-quoted bodies and MySQL double-quoted strings).
    Str,
    QuotedIdent,
    Comment,
    /// Statement terminator (`;` or the current MySQL `DELIMITER`).
    Semi,
    Punct,
    /// `$1`, `?`, `:name`, `@var`.
    Param,
    /// A MySQL client `DELIMITER x` line.
    Delimiter,
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
}

fn is_ident_char(c: u8, d: Dialect) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 || (c == b'$' && d != Dialect::Sqlite)
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn eol(b: &[u8], i: usize) -> usize {
    b[i..].iter().position(|c| *c == b'\n').map(|p| i + p).unwrap_or(b.len())
}

/// Index just past a quoted run starting at `i` (which holds the quote). Doubled quotes escape;
/// with `backslash`, `\x` escapes too. Unterminated runs end at the end of input.
fn skip_quoted(b: &[u8], i: usize, q: u8, backslash: bool) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        if backslash && b[j] == b'\\' {
            j += 2;
            continue;
        }
        if b[j] == q {
            if b.get(j + 1) == Some(&q) {
                j += 2;
                continue;
            }
            return j + 1;
        }
        j += 1;
    }
    b.len()
}

fn starts_with_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && hay[..needle.len()].eq_ignore_ascii_case(needle)
}

/// Tokenizes `sql`. Whitespace is dropped; everything else (including comments) is a token.
pub fn lex(sql: &str, d: Dialect) -> Vec<Token> {
    let b = sql.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    let mut delim: Vec<u8> = b";".to_vec();
    let mut line_start = true;
    let push = |out: &mut Vec<Token>, kind, start, end| out.push(Token { kind, start, end });
    while i < n {
        let c = b[i];
        if c == b'\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if d == Dialect::Mysql && line_start && starts_with_ci(&b[i..], b"DELIMITER") && matches!(b.get(i + 9), Some(b' ' | b'\t')) {
            let e = eol(b, i);
            let arg = sql[i + 9..e].trim();
            if !arg.is_empty() {
                delim = arg.as_bytes().to_vec();
            }
            push(&mut out, TokKind::Delimiter, i, e);
            i = e;
            continue;
        }
        line_start = false;
        let start = i;
        if delim != b";" && b[i..].starts_with(&delim) {
            i += delim.len();
            push(&mut out, TokKind::Semi, start, i);
            continue;
        }
        let kind = match c {
            b'-' if b.get(i + 1) == Some(&b'-') => {
                i = eol(b, i);
                TokKind::Comment
            }
            b'#' if d == Dialect::Mysql => {
                i = eol(b, i);
                TokKind::Comment
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                // Postgres and SQL Server block comments nest.
                let nest = matches!(d, Dialect::Postgres | Dialect::Sqlserver);
                let mut depth = 1;
                let mut j = i + 2;
                while j < n {
                    if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            break;
                        }
                    } else if nest && b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                        depth += 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                i = j.min(n);
                TokKind::Comment
            }
            b'\'' => {
                i = skip_quoted(b, i, b'\'', d == Dialect::Mysql);
                TokKind::Str
            }
            b'"' => {
                i = skip_quoted(b, i, b'"', d == Dialect::Mysql);
                if d == Dialect::Mysql { TokKind::Str } else { TokKind::QuotedIdent }
            }
            b'`' if d != Dialect::Postgres => {
                i = skip_quoted(b, i, b'`', false);
                TokKind::QuotedIdent
            }
            b'[' if d == Dialect::Sqlite => {
                i = b[i..].iter().position(|c| *c == b']').map(|p| i + p + 1).unwrap_or(n);
                TokKind::QuotedIdent
            }
            // T-SQL [identifiers] escape `]` by doubling it.
            b'[' if d == Dialect::Sqlserver => {
                i = skip_quoted(b, i, b']', false);
                TokKind::QuotedIdent
            }
            // T-SQL #temp and ##global temp tables.
            b'#' if d == Dialect::Sqlserver && b.get(i + 1).is_some_and(|c| is_ident_start(*c) || *c == b'#') => {
                i += 1;
                while i < n && (is_ident_char(b[i], d) || b[i] == b'#') {
                    i += 1;
                }
                TokKind::Word
            }
            b'$' if d == Dialect::Postgres => {
                if b.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
                    i += 1;
                    while i < n && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    TokKind::Param
                } else {
                    // $tag$ … $tag$ (tag may be empty).
                    let mut j = i + 1;
                    while j < n && b[j] != b'$' && is_ident_char(b[j], Dialect::Sqlite) {
                        j += 1;
                    }
                    if j < n && b[j] == b'$' {
                        let tag = &sql[i..=j];
                        i = match sql[j + 1..].find(tag) {
                            Some(p) => j + 1 + p + tag.len(),
                            None => n,
                        };
                        TokKind::Str
                    } else {
                        i += 1;
                        TokKind::Punct
                    }
                }
            }
            b'$' | b':' | b'@' if b.get(i + 1).is_some_and(|c| is_ident_start(*c)) => {
                i += 1;
                while i < n && is_ident_char(b[i], Dialect::Sqlite) {
                    i += 1;
                }
                TokKind::Param
            }
            b'?' => {
                i += 1;
                while i < n && b[i].is_ascii_digit() {
                    i += 1;
                }
                TokKind::Param
            }
            b';' if delim == b";" => {
                i += 1;
                TokKind::Semi
            }
            c if is_ident_start(c) => {
                while i < n && is_ident_char(b[i], d) {
                    i += 1;
                }
                // E'…' (Postgres escape string): backslashes escape.
                if d == Dialect::Postgres && i - start == 1 && matches!(c, b'E' | b'e') && b.get(i) == Some(&b'\'') {
                    i = skip_quoted(b, i, b'\'', true);
                    TokKind::Str
                } else {
                    TokKind::Word
                }
            }
            c if c.is_ascii_digit() => {
                while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                    i += 1;
                }
                TokKind::Number
            }
            _ => {
                // One (possibly multi-byte) character.
                i += sql[i..].chars().next().map(char::len_utf8).unwrap_or(1);
                TokKind::Punct
            }
        };
        push(&mut out, kind, start, i);
    }
    out
}

/// One statement of a script. `start`/`end` are byte offsets into the script; `from`/`to` are the
/// same range in UTF-16 code units (what editors such as Monaco use).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Statement {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub from: usize,
    pub to: usize,
}

const COMPOUND_OBJECTS: &[&str] = &["TRIGGER", "PROCEDURE", "FUNCTION", "EVENT"];
const PLAIN_OBJECTS: &[&str] = &["TABLE", "INDEX", "VIEW", "SCHEMA", "DATABASE", "SEQUENCE", "TYPE", "EXTENSION", "ROLE", "USER"];

/// Splits a script into statements. Comment-only fragments are dropped; leading comments are not
/// part of a statement's text. T-SQL scripts are split into `GO` batches first (see
/// [`split_tsql`]).
pub fn split(sql: &str, d: Dialect) -> Vec<Statement> {
    if d == Dialect::Sqlserver {
        return split_tsql(sql);
    }
    let toks = lex(sql, d);
    let mut out = Vec::new();
    let mut u16pos = Utf16Cursor::default();
    let mut first: Option<usize> = None;
    let mut last_end = 0;
    let mut words = 0usize;
    let mut leading: Vec<String> = Vec::new();
    let mut compound = false;
    let mut depth: i32 = 0;
    let mut after_end = false;
    let mut flush = |first: &mut Option<usize>, last_end: usize, out: &mut Vec<Statement>| {
        if let Some(s) = first.take() {
            let from = u16pos.advance(sql, s);
            let to = u16pos.advance(sql, last_end);
            out.push(Statement { text: sql[s..last_end].to_string(), start: s, end: last_end, from, to });
        }
    };
    for t in &toks {
        match t.kind {
            TokKind::Comment | TokKind::Delimiter => continue,
            TokKind::Semi if !(compound && depth > 0) => {
                flush(&mut first, last_end, &mut out);
                words = 0;
                leading.clear();
                compound = false;
                depth = 0;
                after_end = false;
                continue;
            }
            _ => {}
        }
        if first.is_none() {
            first = Some(t.start);
        }
        last_end = t.end;
        if t.kind != TokKind::Word {
            after_end = false;
            continue;
        }
        let w = sql[t.start..t.end].to_ascii_uppercase();
        if words < 8 {
            words += 1;
            if leading.first().map(String::as_str) == Some("CREATE")
                && !compound
                && COMPOUND_OBJECTS.contains(&w.as_str())
                && !leading.iter().any(|x| PLAIN_OBJECTS.contains(&x.as_str()))
            {
                compound = true;
            }
            leading.push(w.clone());
        }
        if compound {
            if after_end {
                after_end = false;
                // END IF / END LOOP / END WHILE / END REPEAT close blocks we don't count.
                if matches!(w.as_str(), "IF" | "LOOP" | "WHILE" | "REPEAT") {
                    depth += 1;
                    continue;
                }
                if w == "CASE" {
                    continue;
                }
            }
            match w.as_str() {
                "BEGIN" | "CASE" => depth += 1,
                "END" => {
                    depth -= 1;
                    after_end = true;
                }
                _ => {}
            }
        }
    }
    flush(&mut first, last_end, &mut out);
    out
}

/// Whether `t` is a `GO` batch separator: alone on its line, optionally followed by a repeat
/// count and a `--` comment (sqlcmd / SSMS syntax).
fn is_go(sql: &str, t: &Token) -> bool {
    if t.kind != TokKind::Word || !sql[t.start..t.end].eq_ignore_ascii_case("GO") {
        return false;
    }
    let line_start = sql[..t.start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    if !sql[line_start..t.start].trim().is_empty() {
        return false;
    }
    let line_end = sql[t.end..].find('\n').map(|p| t.end + p).unwrap_or(sql.len());
    let rest = sql[t.end..line_end].trim();
    let rest = rest.split("--").next().unwrap_or("").trim();
    rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit())
}

/// Statement keywords that need the rest of their batch (variables, control flow, and objects
/// whose body runs to the end of the batch).
fn tsql_batch_is_whole(sql: &str, toks: &[Token]) -> bool {
    let words: Vec<String> = toks.iter().filter(|t| t.kind == TokKind::Word).take(4).map(|t| sql[t.start..t.end].to_ascii_uppercase()).collect();
    if matches!(words.first().map(String::as_str), Some("CREATE" | "ALTER")) {
        let obj = words.iter().skip(1).find(|w| !matches!(w.as_str(), "OR" | "ALTER"));
        if matches!(obj.map(String::as_str), Some("PROC" | "PROCEDURE" | "FUNCTION" | "TRIGGER" | "VIEW" | "SCHEMA")) {
            return true;
        }
    }
    let b = sql.as_bytes();
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            // @variables (not @@functions) only live for the batch.
            TokKind::Param if b[t.start] == b'@' && (t.start == 0 || b[t.start - 1] != b'@') => return true,
            TokKind::Word => {
                let w = sql[t.start..t.end].to_ascii_uppercase();
                match w.as_str() {
                    "DECLARE" | "IF" | "WHILE" | "GOTO" | "RETURN" | "TRY" | "CATCH" | "BREAK" | "CONTINUE" => return true,
                    "BEGIN" => {
                        let next = toks[i + 1..].iter().find(|x| x.kind == TokKind::Word).map(|x| sql[x.start..x.end].to_ascii_uppercase());
                        if !matches!(next.as_deref(), Some("TRAN" | "TRANSACTION" | "DISTRIBUTED")) {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    false
}

/// T-SQL scripts: `GO` lines separate batches (they are never sent to the server). A batch that
/// declares variables, uses control flow or creates a procedure/function/trigger/view runs as one
/// statement (its parts depend on each other); other batches are split on `;` like the other
/// dialects. A statement without semicolons can still return several result sets.
pub fn split_tsql(sql: &str) -> Vec<Statement> {
    let toks = lex(sql, Dialect::Sqlserver);
    let mut out = Vec::new();
    let mut u16pos = Utf16Cursor::default();
    let mut push = |start: usize, end: usize, out: &mut Vec<Statement>| {
        let from = u16pos.advance(sql, start);
        let to = u16pos.advance(sql, end);
        out.push(Statement { text: sql[start..end].to_string(), start, end, from, to });
    };
    let mut flush = |batch: &[Token], out: &mut Vec<Statement>| {
        let sig: Vec<Token> = batch.iter().copied().filter(|t| !matches!(t.kind, TokKind::Comment | TokKind::Delimiter)).collect();
        if !sig.iter().any(|t| t.kind != TokKind::Semi) {
            return;
        }
        if tsql_batch_is_whole(sql, &sig) {
            let start = sig.iter().find(|t| t.kind != TokKind::Semi).map(|t| t.start).unwrap_or(0);
            let end = sig.iter().rev().find(|t| t.kind != TokKind::Semi).map(|t| t.end).unwrap_or(start);
            push(start, end, out);
            return;
        }
        let mut first: Option<usize> = None;
        let mut last_end = 0;
        for t in &sig {
            if t.kind == TokKind::Semi {
                if let Some(s) = first.take() {
                    push(s, last_end, out);
                }
                continue;
            }
            first.get_or_insert(t.start);
            last_end = t.end;
        }
        if let Some(s) = first {
            push(s, last_end, out);
        }
    };
    let mut batch: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if is_go(sql, &t) {
            flush(&batch, &mut out);
            batch.clear();
            let line_end = sql[t.end..].find('\n').map(|p| t.end + p).unwrap_or(sql.len());
            i += 1;
            while i < toks.len() && toks[i].start < line_end {
                i += 1;
            }
            continue;
        }
        batch.push(t);
        i += 1;
    }
    flush(&batch, &mut out);
    out
}

/// Converts increasing byte offsets to UTF-16 offsets in one pass.
#[derive(Default)]
struct Utf16Cursor {
    byte: usize,
    u16: usize,
}

impl Utf16Cursor {
    fn advance(&mut self, s: &str, to: usize) -> usize {
        if to < self.byte {
            self.byte = 0;
            self.u16 = 0;
        }
        self.u16 += s[self.byte..to].encode_utf16().count();
        self.byte = to;
        self.u16
    }
}

/// The statement containing (or nearest before) the UTF-16 offset `pos` — for "run statement
/// under cursor".
pub fn statement_at(sql: &str, d: Dialect, pos: usize) -> Option<Statement> {
    let stmts = split(sql, d);
    if let Some(s) = stmts.iter().find(|s| pos >= s.from && pos <= s.to) {
        return Some(s.clone());
    }
    // Between statements: the previous one (cursor after its `;`), else the next.
    stmts.iter().rev().find(|s| s.to <= pos).or_else(|| stmts.first()).cloned()
}

// ---------- classification ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    /// Leading keyword(s), e.g. `SELECT`, `DROP TABLE`.
    pub keyword: String,
    pub read_only: bool,
    /// Why the statement counts as a write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Classification {
    fn read(keyword: &str) -> Self {
        Self { keyword: keyword.to_string(), read_only: true, reason: None }
    }
    fn write(keyword: &str, reason: impl Into<String>) -> Self {
        Self { keyword: keyword.to_string(), read_only: false, reason: Some(reason.into()) }
    }
}

const READ_ONLY_VARS: &[&str] =
    &["DEFAULT_TRANSACTION_READ_ONLY", "TRANSACTION_READ_ONLY", "TX_READ_ONLY", "READ_ONLY", "SUPER_READ_ONLY", "QUERY_ONLY"];

/// Classifies one statement (see [`split`]).
pub fn classify(stmt: &str, d: Dialect) -> Classification {
    if d == Dialect::Sqlserver {
        return classify_tsql(stmt);
    }
    let toks = lex(stmt, d);
    let words: Vec<String> = toks.iter().filter(|t| t.kind == TokKind::Word).map(|t| stmt[t.start..t.end].to_ascii_uppercase()).collect();
    let punct = |c: u8| toks.iter().any(|t| t.kind == TokKind::Punct && stmt.as_bytes()[t.start] == c);
    let upper = stmt.to_ascii_uppercase();
    classify_words(&words, d, punct(b'='), punct(b'('), &upper)
}

fn keyword_of(words: &[String]) -> String {
    match words.first().map(String::as_str) {
        Some("CREATE" | "DROP" | "ALTER" | "TRUNCATE") => {
            let skip = ["OR", "REPLACE", "TEMP", "TEMPORARY", "UNIQUE", "IF", "NOT", "EXISTS", "GLOBAL", "LOCAL", "UNLOGGED", "MATERIALIZED"];
            let obj = words.iter().skip(1).find(|w| !skip.contains(&w.as_str()));
            match obj {
                Some(o) => format!("{} {o}", words[0]),
                None => words[0].clone(),
            }
        }
        Some(w) => w.to_string(),
        None => String::new(),
    }
}

fn contains_seq(words: &[String], seq: &[&str]) -> bool {
    words.windows(seq.len()).any(|w| w.iter().zip(seq).all(|(a, b)| a == b))
}

fn classify_words(words: &[String], d: Dialect, eq: bool, paren: bool, upper: &str) -> Classification {
    let Some(first) = words.first() else { return Classification::read("") };
    let kw = keyword_of(words);
    let has = |w: &str| words.iter().any(|x| x == w);
    match first.as_str() {
        "SELECT" | "WITH" | "VALUES" | "TABLE" => {
            if first == "WITH"
                && let Some(w) = words.iter().find(|w| matches!(w.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE"))
            {
                return Classification::write(&kw, format!("this WITH query contains {w} (a data-modifying CTE)"));
            }
            if has("INTO") {
                return Classification::write(&kw, "SELECT … INTO creates a table or writes a file");
            }
            if contains_seq(words, &["FOR", "UPDATE"])
                || contains_seq(words, &["FOR", "SHARE"])
                || contains_seq(words, &["FOR", "NO", "KEY", "UPDATE"])
                || contains_seq(words, &["FOR", "KEY", "SHARE"])
                || contains_seq(words, &["LOCK", "IN", "SHARE", "MODE"])
            {
                return Classification::write(&kw, "it locks rows (FOR UPDATE / FOR SHARE)");
            }
            Classification::read(&kw)
        }
        "SHOW" | "DESCRIBE" | "DESC" | "HELP" | "LISTEN" | "UNLISTEN" | "FETCH" | "MOVE" | "CLOSE" | "COMMIT" | "ROLLBACK" | "END" | "ABORT"
        | "SAVEPOINT" | "RELEASE" | "USE" | "DEALLOCATE" | "UNLOCK" | "CHECKSUM" => Classification::read(&kw),
        "CHECK" if d == Dialect::Mysql => Classification::read(&kw),
        "EXPLAIN" => {
            if has("ANALYZE") || has("ANALYSE") {
                let inner = words.iter().skip(1).position(|w| {
                    matches!(w.as_str(), "SELECT" | "WITH" | "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "VALUES" | "TABLE" | "CREATE" | "EXECUTE" | "DECLARE" | "REPLACE")
                });
                let c = match inner {
                    Some(i) => classify_words(&words[i + 1..], d, eq, paren, upper),
                    None => Classification::write("EXPLAIN", "EXPLAIN ANALYZE executes the statement"),
                };
                if !c.read_only {
                    return Classification::write("EXPLAIN ANALYZE", format!("EXPLAIN ANALYZE executes the statement, and {}", c.reason.unwrap_or_default()));
                }
            }
            Classification::read(&kw)
        }
        "PRAGMA" => {
            const KNOWN_ARG_READS: &[&str] = &[
                "TABLE_INFO",
                "TABLE_XINFO",
                "INDEX_LIST",
                "INDEX_INFO",
                "INDEX_XINFO",
                "FOREIGN_KEY_LIST",
                "FOREIGN_KEY_CHECK",
                "INTEGRITY_CHECK",
                "QUICK_CHECK",
                "TABLE_LIST",
            ];
            const ACTIONS: &[&str] = &["OPTIMIZE", "WAL_CHECKPOINT", "INCREMENTAL_VACUUM", "SHRINK_MEMORY"];
            let name = match words.get(2) {
                Some(w) if KNOWN_ARG_READS.contains(&w.as_str()) || ACTIONS.contains(&w.as_str()) => w.as_str(),
                _ => words.get(1).map(String::as_str).unwrap_or(""),
            };
            if eq {
                Classification::write(&kw, "PRAGMA … = … changes a database setting")
            } else if ACTIONS.contains(&name) {
                Classification::write(&kw, format!("PRAGMA {} writes to the database", name.to_ascii_lowercase()))
            } else if paren && !KNOWN_ARG_READS.contains(&name) {
                Classification::write(&kw, "PRAGMA name(value) changes a database setting")
            } else {
                Classification::read(&kw)
            }
        }
        "SET" => {
            if contains_seq(words, &["READ", "WRITE"]) || has("CHARACTERISTICS") || words.iter().any(|w| READ_ONLY_VARS.contains(&w.as_str())) {
                Classification::write(&kw, "it changes the session's read-only setting")
            } else if has("GLOBAL") || has("PERSIST") || has("PERSIST_ONLY") || upper.contains("@@GLOBAL") || upper.contains("@@PERSIST") {
                Classification::write(&kw, "SET GLOBAL changes the server configuration")
            } else {
                Classification::read(&kw)
            }
        }
        "RESET" => {
            if words.get(1).is_some_and(|w| w == "ALL" || READ_ONLY_VARS.contains(&w.as_str())) {
                Classification::write(&kw, "it resets the session's read-only setting")
            } else {
                Classification::read(&kw)
            }
        }
        "DISCARD" => {
            if has("ALL") {
                Classification::write(&kw, "DISCARD ALL resets the session's read-only setting")
            } else {
                Classification::read(&kw)
            }
        }
        "BEGIN" | "START" => {
            if contains_seq(words, &["READ", "WRITE"]) {
                Classification::write(&kw, "it starts a read-write transaction")
            } else if d == Dialect::Sqlite && (has("IMMEDIATE") || has("EXCLUSIVE")) {
                Classification::write(&kw, "BEGIN IMMEDIATE/EXCLUSIVE takes a write lock")
            } else {
                Classification::read(&kw)
            }
        }
        "PREPARE" => match words.iter().position(|w| w == "AS") {
            Some(i) => {
                let c = classify_words(&words[i + 1..], d, eq, paren, upper);
                if c.read_only { Classification::read(&kw) } else { Classification::write(&kw, c.reason.unwrap_or_default()) }
            }
            None => Classification::write(&kw, "the prepared statement can't be checked"),
        },
        "DECLARE" => match words.iter().position(|w| w == "FOR") {
            Some(i) => {
                let c = classify_words(&words[i + 1..], d, eq, paren, upper);
                if c.read_only { Classification::read(&kw) } else { Classification::write(&kw, c.reason.unwrap_or_default()) }
            }
            None => Classification::write(&kw, "it can't be checked"),
        },
        "COPY" => {
            if has("STDOUT") && !has("FROM") {
                Classification::read(&kw)
            } else {
                Classification::write(&kw, "COPY … FROM loads data (and COPY … TO a file writes on the server)")
            }
        }
        "CALL" | "DO" | "EXECUTE" | "EXEC" => Classification::write(&kw, "it runs code that may modify data"),
        "ANALYZE" | "VACUUM" | "REINDEX" | "CLUSTER" | "CHECKPOINT" | "OPTIMIZE" | "REPAIR" | "FLUSH" => {
            Classification::write(&kw, "it is a maintenance command that writes to the database")
        }
        "LOCK" => Classification::write(&kw, "it locks tables"),
        _ => Classification::write(&kw, "it modifies the database"),
    }
}

/// The target after `INSERT [INTO]`, `UPDATE`, `DELETE [FROM]`, `… INTO`, `CREATE/DROP/ALTER/
/// TRUNCATE TABLE [IF EXISTS]`: whether it is a `#temp` table or a `@table` variable (writes
/// there don't change the database).
fn tsql_temp_target(sql: &str, toks: &[Token], after: usize) -> bool {
    let skip = ["INTO", "FROM", "TABLE", "IF", "EXISTS", "TOP"];
    let mut j = after + 1;
    while let Some(t) = toks.get(j) {
        match t.kind {
            TokKind::Word if skip.contains(&sql[t.start..t.end].to_ascii_uppercase().as_str()) => j += 1,
            // TOP (n)
            TokKind::Punct | TokKind::Number if matches!(&sql[t.start..t.end], "(" | ")") || t.kind == TokKind::Number => j += 1,
            TokKind::Word => return sql[t.start..].starts_with('#'),
            TokKind::Param => return sql[t.start..].starts_with('@') && (t.start == 0 || sql.as_bytes()[t.start - 1] != b'@'),
            _ => return false,
        }
    }
    false
}

/// T-SQL batches are classified as a whole: without required semicolons, any statement keyword
/// can start a new statement, so every word is checked. Writes to `#temp` tables and `@table`
/// variables are allowed; anything that can't be checked (EXEC, dynamic SQL, linked servers)
/// counts as a write.
fn classify_tsql(stmt: &str) -> Classification {
    let toks: Vec<Token> = lex(stmt, Dialect::Sqlserver).into_iter().filter(|t| t.kind != TokKind::Comment).collect();
    let up = |t: &Token| stmt[t.start..t.end].to_ascii_uppercase();
    let words: Vec<String> = toks.iter().filter(|t| t.kind == TokKind::Word).map(up).collect();
    let Some(first) = words.first().cloned() else { return Classification::read("") };
    let kw = keyword_of(&words);
    let next_word = |i: usize| toks[i + 1..].iter().find(|t| t.kind == TokKind::Word).map(up);
    let prev_word = |i: usize| toks[..i].iter().rev().find(|t| t.kind == TokKind::Word).map(up);
    for (i, t) in toks.iter().enumerate() {
        if t.kind != TokKind::Word {
            continue;
        }
        let w = up(t);
        let reason: Option<String> = match w.as_str() {
            "MERGE" if next_word(i).as_deref() == Some("JOIN") => None,
            "UPDATE" if prev_word(i).as_deref() == Some("FOR") => Some("it locks rows (FOR UPDATE)".into()),
            "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "TRUNCATE" => {
                (!tsql_temp_target(stmt, &toks, i)).then(|| format!("it contains {w}"))
            }
            "INTO" if prev_word(i).as_deref() != Some("INSERT") && prev_word(i).as_deref() != Some("MERGE") => {
                (!tsql_temp_target(stmt, &toks, i)).then(|| "SELECT … INTO creates a table".to_string())
            }
            "CREATE" | "ALTER" | "DROP" => {
                let temp_table = next_word(i).as_deref() == Some("TABLE") && tsql_temp_target(stmt, &toks, i);
                (!temp_table).then(|| format!("it contains {w}"))
            }
            "EXEC" | "EXECUTE" => Some("it runs a stored procedure or dynamic SQL, which can't be checked".into()),
            "GRANT" | "REVOKE" | "DENY" => Some("it changes permissions".into()),
            "BULK" | "BACKUP" | "RESTORE" | "DBCC" | "KILL" | "SHUTDOWN" | "RECONFIGURE" | "CHECKPOINT" | "WRITETEXT" | "UPDATETEXT" | "ENABLE" | "DISABLE" => {
                Some(format!("{w} is a maintenance command that changes the server or database"))
            }
            "SEND" | "RECEIVE" => Some("Service Broker SEND/RECEIVE changes queues".into()),
            "OPENQUERY" | "OPENROWSET" | "OPENDATASOURCE" => Some("it runs a query on a linked or external server, which can't be checked".into()),
            "UPDLOCK" | "XLOCK" | "TABLOCKX" => Some("it locks rows (UPDLOCK/XLOCK hints)".into()),
            "NEXT" if next_word(i).as_deref() == Some("VALUE") => Some("NEXT VALUE FOR advances a sequence".into()),
            _ => None,
        };
        if let Some(r) = reason {
            return Classification::write(&kw, r);
        }
    }
    const READ_STARTS: &[&str] = &[
        "SELECT", "WITH", "DECLARE", "SET", "PRINT", "IF", "WHILE", "BEGIN", "END", "ELSE", "RETURN", "USE", "WAITFOR", "COMMIT", "ROLLBACK", "SAVE",
        "DEALLOCATE", "CLOSE", "OPEN", "FETCH", "RAISERROR", "THROW", "GOTO", "BREAK", "CONTINUE", "VALUES", "INSERT", "UPDATE", "DELETE", "TRUNCATE",
        "CREATE", "DROP", "ALTER", "READTEXT",
    ];
    if !READ_STARTS.contains(&first.as_str()) {
        // A bare procedure call (`sp_who`) or something unknown.
        return Classification::write(&kw, "it modifies the database");
    }
    Classification::read(&kw)
}

/// Classifies every statement of a script; returns the first mutating one, if any.
pub fn first_write(sql: &str, d: Dialect) -> Option<(Statement, Classification)> {
    split(sql, d)
        .into_iter()
        .map(|s| {
            let c = classify(&s.text, d);
            (s, c)
        })
        .find(|(_, c)| !c.read_only)
}

/// The message shown when a statement is blocked in read-only mode.
pub fn read_only_message(c: &Classification) -> String {
    let what = if c.keyword.is_empty() { "the statement".to_string() } else { format!("`{}`", c.keyword) };
    let why = c.reason.as_deref().unwrap_or("it modifies data");
    format!("Read-only session: {what} was not run because {why}. Turn off read-only for this connection to run it.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use Dialect::*;

    fn texts(sql: &str, d: Dialect) -> Vec<String> {
        split(sql, d).into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn splits_respecting_strings_comments_and_quotes() {
        let sql = "select ';' as a; -- c;omment\nselect \"x;y\" from t /* ; */; ;; select 1";
        assert_eq!(texts(sql, Postgres), vec!["select ';' as a", "select \"x;y\" from t", "select 1"], "comments around statements are not part of them");
        assert_eq!(texts("select 'it''s;'; select 2", Sqlite), vec!["select 'it''s;'", "select 2"]);
        assert_eq!(texts("select 'a\\';b'; select 2", Mysql), vec!["select 'a\\';b'", "select 2"]);
        assert_eq!(texts("select `a;b` from t # x;y\n; select 1", Mysql), vec!["select `a;b` from t", "select 1"]);
        assert_eq!(texts("select [a;b] from t; select 1", Sqlite), vec!["select [a;b] from t", "select 1"]);
        assert_eq!(texts("-- only a comment\n/* and; another */", Postgres), Vec::<String>::new());
        assert_eq!(texts("/* outer /* nested; */ still; */ select 1; select 2", Postgres), vec!["select 1", "select 2"]);
        assert_eq!(texts("select E'a\\';b'; select 2", Postgres), vec!["select E'a\\';b'", "select 2"]);
    }

    #[test]
    fn dollar_quotes_and_params() {
        let sql = "create function f() returns int as $body$ begin; return 1; end; $body$ language plpgsql; select $1, $$a;b$$;";
        assert_eq!(texts(sql, Postgres), vec!["create function f() returns int as $body$ begin; return 1; end; $body$ language plpgsql", "select $1, $$a;b$$"]);
        assert_eq!(texts("select a$b$ from t; select 1", Postgres), vec!["select a$b$ from t", "select 1"]);
    }

    #[test]
    fn compound_bodies() {
        let trig = "CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE b SET n = n + 1; INSERT INTO c VALUES (CASE WHEN 1 THEN 2 END); END; SELECT 1";
        assert_eq!(texts(trig, Sqlite).len(), 2, "{:?}", texts(trig, Sqlite));
        let proc = "CREATE PROCEDURE p() BEGIN IF 1 THEN SELECT 1; END IF; WHILE 0 DO SELECT 2; END WHILE; END; CALL p()";
        assert_eq!(texts(proc, Mysql), vec!["CREATE PROCEDURE p() BEGIN IF 1 THEN SELECT 1; END IF; WHILE 0 DO SELECT 2; END WHILE; END", "CALL p()"]);
        let delim = "DELIMITER //\nCREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\nEND //\nDELIMITER ;\nSELECT 2;";
        assert_eq!(texts(delim, Mysql), vec!["CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\nEND", "SELECT 2"]);
        // A plain table whose columns mention EVENT is not a compound statement.
        assert_eq!(texts("CREATE TABLE log (event text); BEGIN; SELECT 1; COMMIT;", Postgres).len(), 4);
    }

    #[test]
    fn utf16_offsets_and_statement_at() {
        let sql = "select 'é😀'; select 2;";
        let s = split(sql, Postgres);
        assert_eq!(s[1].text, "select 2");
        // 'é' is 1 UTF-16 unit, '😀' is 2.
        assert_eq!((s[0].from, s[0].to), (0, 12));
        assert_eq!((s[1].from, s[1].to), (14, 22));
        assert_eq!(statement_at(sql, Postgres, 16).unwrap().text, "select 2");
        assert_eq!(statement_at(sql, Postgres, 13).unwrap().text, "select 'é😀'");
        assert_eq!(statement_at(sql, Postgres, 0).unwrap().text, "select 'é😀'");
        assert!(statement_at("", Postgres, 0).is_none());
    }

    fn ro(sql: &str, d: Dialect) -> bool {
        classify(sql, d).read_only
    }

    #[test]
    fn classification() {
        for s in [
            "SELECT * FROM t",
            "  -- hi\n select 1",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "VALUES (1)",
            "TABLE users",
            "SHOW TABLES",
            "EXPLAIN SELECT 1",
            "EXPLAIN DELETE FROM t",
            "EXPLAIN ANALYZE SELECT 1",
            "BEGIN",
            "START TRANSACTION READ ONLY",
            "COMMIT",
            "SET search_path = app",
            "DESCRIBE t",
            "PRAGMA table_info(users)",
            "PRAGMA main.index_list('t')",
            "PRAGMA foreign_keys",
            "COPY (SELECT 1) TO STDOUT",
            "",
        ] {
            assert!(ro(s, Postgres) || ro(s, Sqlite), "{s} should be read-only");
        }
        for (s, d) in [
            ("INSERT INTO t VALUES (1)", Postgres),
            ("update t set a = 1", Mysql),
            ("DELETE FROM t", Sqlite),
            ("drop table t", Postgres),
            ("TRUNCATE t", Postgres),
            ("CREATE TABLE t (a int)", Sqlite),
            ("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", Postgres),
            ("SELECT * INTO backup FROM t", Postgres),
            ("SELECT * FROM t FOR UPDATE", Postgres),
            ("EXPLAIN ANALYZE DELETE FROM t", Postgres),
            ("EXPLAIN (ANALYZE, BUFFERS) UPDATE t SET a = 1", Postgres),
            ("SET TRANSACTION READ WRITE", Postgres),
            ("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE", Postgres),
            ("set default_transaction_read_only = off", Postgres),
            ("SET SESSION transaction_read_only = 0", Mysql),
            ("SET @@session.transaction_read_only = 0", Mysql),
            ("SET GLOBAL max_connections = 10", Mysql),
            ("RESET ALL", Postgres),
            ("START TRANSACTION READ WRITE", Mysql),
            ("BEGIN IMMEDIATE", Sqlite),
            ("PRAGMA journal_mode = WAL", Sqlite),
            ("PRAGMA user_version(3)", Sqlite),
            ("PRAGMA optimize", Sqlite),
            ("CALL do_things()", Mysql),
            ("VACUUM", Sqlite),
            ("COPY t FROM STDIN", Postgres),
            ("REPLACE INTO t VALUES (1)", Mysql),
            ("GRANT ALL ON t TO bob", Postgres),
            ("mystery command", Postgres),
        ] {
            let c = classify(s, d);
            assert!(!c.read_only, "{s} should be a write");
            assert!(c.reason.is_some());
        }
        assert_eq!(classify("drop table if exists t", Postgres).keyword, "DROP TABLE");
        assert_eq!(classify("create or replace view v as select 1", Postgres).keyword, "CREATE VIEW");
        let msg = read_only_message(&classify("DELETE FROM t", Postgres));
        assert_eq!(msg, "Read-only session: `DELETE` was not run because it modifies the database. Turn off read-only for this connection to run it.");
        let (stmt, _) = first_write("select 1; update t set a = 2; select 3", Postgres).unwrap();
        assert_eq!(stmt.text, "update t set a = 2");
        assert!(first_write("select 1; select 2", Mysql).is_none());
    }

    #[test]
    fn tsql_go_batches() {
        let sql = "SELECT [a;b], N'x;GO' FROM t; SELECT 2\nGO\n/* GO\nGO */ select 3\n  go 2 -- twice\nUPDATE #t SET a = 1";
        assert_eq!(texts(sql, Sqlserver), vec!["SELECT [a;b], N'x;GO' FROM t", "SELECT 2", "select 3", "UPDATE #t SET a = 1"]);
        // Batches with variables or control flow stay whole (variables only live for the batch).
        let vars = "DECLARE @n int = 5;\nSELECT @n;\nSELECT @n + 1;\nGO\nSELECT @@ROWCOUNT; SELECT 2";
        assert_eq!(texts(vars, Sqlserver), vec!["DECLARE @n int = 5;\nSELECT @n;\nSELECT @n + 1", "SELECT @@ROWCOUNT", "SELECT 2"]);
        let proc = "CREATE OR ALTER PROCEDURE dbo.p AS\nSELECT 1;\nSELECT 2;\nGO\nEXEC dbo.p";
        assert_eq!(texts(proc, Sqlserver), vec!["CREATE OR ALTER PROCEDURE dbo.p AS\nSELECT 1;\nSELECT 2", "EXEC dbo.p"]);
        // BEGIN TRAN is a statement, BEGIN … END a block.
        assert_eq!(texts("BEGIN TRAN; DELETE FROM t; COMMIT", Sqlserver).len(), 3);
        assert_eq!(texts("BEGIN SELECT 1; SELECT 2; END", Sqlserver).len(), 1);
        // `GO` must be alone on its line, and [identifiers] escape ] by doubling it.
        assert_eq!(texts("SELECT go FROM t\nSELECT [x]]GO;] FROM u", Sqlserver).len(), 1);
        let s = split("SELECT 1\nGO\nSELECT 'é'", Sqlserver);
        assert_eq!((s[1].from, s[1].to), (12, 22));
        assert_eq!(statement_at("SELECT 1\nGO\nSELECT 2", Sqlserver, 15).unwrap().text, "SELECT 2");
    }

    #[test]
    fn tsql_classification() {
        for s in [
            "SELECT TOP 10 * FROM dbo.t WITH (NOLOCK)",
            "WITH c AS (SELECT 1 AS x) SELECT * FROM c",
            "DECLARE @t TABLE (id int); INSERT INTO @t VALUES (1); SELECT * FROM @t",
            "SELECT * INTO #tmp FROM t; UPDATE #tmp SET a = 1; DELETE FROM #tmp",
            "CREATE TABLE #x (id int); DROP TABLE IF EXISTS #x",
            "SET TRANSACTION ISOLATION LEVEL READ COMMITTED",
            "SELECT a FROM t INNER MERGE JOIN u ON t.id = u.id",
            "BEGIN TRAN",
            "PRINT 'hi'",
        ] {
            assert!(ro(s, Sqlserver), "{s} should be read-only: {:?}", classify(s, Sqlserver));
        }
        for s in [
            "UPDATE dbo.t SET a = 1",
            "SELECT 1 DELETE FROM t",
            "SELECT * INTO backup FROM t",
            "INSERT INTO t SELECT * FROM #x",
            "MERGE t USING s ON t.id = s.id WHEN MATCHED THEN DELETE;",
            "EXEC sp_executesql N'DELETE FROM t'",
            "sp_who",
            "CREATE VIEW v AS SELECT 1 AS a",
            "TRUNCATE TABLE t",
            "SELECT * FROM t WITH (UPDLOCK)",
            "SELECT NEXT VALUE FOR dbo.seq",
            "SELECT * FROM OPENQUERY(srv, 'DELETE FROM t')",
            "DECLARE c CURSOR FOR SELECT * FROM t FOR UPDATE",
            "GRANT SELECT ON t TO bob",
        ] {
            let c = classify(s, Sqlserver);
            assert!(!c.read_only, "{s} should be a write");
            assert!(c.reason.is_some());
        }
        assert_eq!(classify("CREATE TABLE dbo.x (id int)", Sqlserver).keyword, "CREATE TABLE");
        assert_eq!(quote_ident_for_test("a]b"), "[a]]b]");
    }

    fn quote_ident_for_test(s: &str) -> String {
        crate::sql::edit::quote_ident(s, Sqlserver)
    }
}
