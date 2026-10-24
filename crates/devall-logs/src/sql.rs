//! SQL over logs, events and data files.
//!
//! The default backend is an in-memory DuckDB (feature `duckdb`), which also reads CSV, JSON and
//! Parquet files directly (`SELECT * FROM read_parquet('x.parquet')`) and can be given extra
//! tables with [`SqlEngine::register_file`] / [`SqlEngine::register_text`] (e.g. saved HTTP
//! response bodies). Without the feature (or if DuckDB fails to start) SQLite is used: the same
//! `logs`/`events` tables, `fields`/`data` as JSON text (`json_extract(fields, '$.status')` or
//! `fields ->> '$.status'`), and CSV/JSON registered by loading them into tables.
//!
//! Tables: `logs(idx, line, ts, ts_ms, level, message, fields, raw, source, format, correlation_id)`
//! and `events(id, seq, ts, ts_ms, kind, source, level, title, correlation_id, request_path,
//! duration_ms, data)`. DuckDB disables extension auto-install so queries never hit the network.

use std::path::Path;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::events::Event;
use crate::parse::LogEntry;
use crate::time::format_iso;
use crate::{LogError, Result};

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// More rows existed than `max_rows`.
    pub truncated: bool,
    pub elapsed_ms: f64,
    pub backend: &'static str,
    /// Statements executed (the last one's rows are returned).
    pub statements: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<(String, String)>,
}

/// What a registered text holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TextKind {
    Json,
    Csv,
}

enum Backend {
    #[cfg(feature = "duckdb")]
    Duck(duckdb::Connection),
    Lite(rusqlite::Connection),
}

pub struct SqlEngine {
    backend: Backend,
    temps: Vec<tempfile::TempPath>,
}

fn sql_err(e: impl std::fmt::Display) -> LogError {
    LogError::Sql(e.to_string())
}

/// Table/view names are restricted to identifiers.
fn check_ident(name: &str) -> Result<()> {
    let ok = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && name.len() <= 64;
    if ok { Ok(()) } else { Err(LogError::Invalid(format!("'{name}' is not a valid table name (letters, digits, _)"))) }
}

fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

impl SqlEngine {
    /// DuckDB when compiled in (falling back to SQLite if it can't start), else SQLite.
    pub fn new() -> Result<Self> {
        #[cfg(feature = "duckdb")]
        if let Ok(e) = Self::duckdb() {
            return Ok(e);
        }
        Self::sqlite()
    }

    #[cfg(feature = "duckdb")]
    pub fn duckdb() -> Result<Self> {
        let conn = duckdb::Connection::open_in_memory().map_err(sql_err)?;
        conn.execute_batch(
            "SET autoinstall_known_extensions = false;
             SET autoload_known_extensions = false;
             SET threads = 4;
             SET memory_limit = '2GB';",
        )
        .map_err(sql_err)?;
        Ok(Self { backend: Backend::Duck(conn), temps: Vec::new() })
    }

    pub fn sqlite() -> Result<Self> {
        let conn = rusqlite::Connection::open_in_memory()?;
        Ok(Self { backend: Backend::Lite(conn), temps: Vec::new() })
    }

    pub fn backend(&self) -> &'static str {
        match &self.backend {
            #[cfg(feature = "duckdb")]
            Backend::Duck(_) => "duckdb",
            Backend::Lite(_) => "sqlite",
        }
    }

    fn exec(&self, sql: &str) -> Result<()> {
        match &self.backend {
            #[cfg(feature = "duckdb")]
            Backend::Duck(c) => c.execute_batch(sql).map_err(sql_err),
            Backend::Lite(c) => c.execute_batch(sql).map_err(sql_err),
        }
    }

    /// (Re)creates an empty log table.
    pub fn create_logs_table(&mut self, name: &str) -> Result<()> {
        check_ident(name)?;
        let ts = if self.backend() == "duckdb" { "TIMESTAMP" } else { "TEXT" };
        let json = if self.backend() == "duckdb" { "JSON" } else { "TEXT" };
        self.exec(&format!(
            "DROP TABLE IF EXISTS {name};
             CREATE TABLE {name} (idx BIGINT, line BIGINT, ts {ts}, ts_ms BIGINT, level VARCHAR, message VARCHAR, fields {json},
                raw VARCHAR, source VARCHAR, format VARCHAR, correlation_id VARCHAR);"
        ))
    }

    /// Appends entries to a table made by [`Self::create_logs_table`].
    pub fn insert_entries(&mut self, name: &str, entries: &[LogEntry]) -> Result<()> {
        check_ident(name)?;
        match &mut self.backend {
            #[cfg(feature = "duckdb")]
            Backend::Duck(c) => {
                use duckdb::types::{TimeUnit, Value as D};
                let mut app = c.appender(name).map_err(sql_err)?;
                for e in entries {
                    let row = [
                        D::BigInt(e.index as i64),
                        D::BigInt(e.line_no as i64),
                        e.ts.map(|t| D::Timestamp(TimeUnit::Millisecond, t)).unwrap_or(D::Null),
                        e.ts.map(D::BigInt).unwrap_or(D::Null),
                        D::Text(e.level.as_str().into()),
                        D::Text(e.message.clone()),
                        D::Text(Value::Object(e.fields.clone()).to_string()),
                        D::Text(e.raw.clone()),
                        e.source.clone().map(D::Text).unwrap_or(D::Null),
                        D::Text(e.format.as_str().into()),
                        e.correlation_id.clone().map(D::Text).unwrap_or(D::Null),
                    ];
                    app.append_row(duckdb::appender_params_from_iter(row)).map_err(sql_err)?;
                }
                app.flush().map_err(sql_err)?;
            }
            Backend::Lite(c) => {
                let tx = c.transaction()?;
                {
                    let mut st = tx.prepare(&format!("INSERT INTO {name} VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"))?;
                    for e in entries {
                        st.execute(rusqlite::params![
                            e.index as i64,
                            e.line_no as i64,
                            e.ts.map(format_iso),
                            e.ts,
                            e.level.as_str(),
                            e.message,
                            Value::Object(e.fields.clone()).to_string(),
                            e.raw,
                            e.source,
                            e.format.as_str(),
                            e.correlation_id,
                        ])?;
                    }
                }
                tx.commit()?;
            }
        }
        Ok(())
    }

    pub fn create_events_table(&mut self, name: &str) -> Result<()> {
        check_ident(name)?;
        let ts = if self.backend() == "duckdb" { "TIMESTAMP" } else { "TEXT" };
        let json = if self.backend() == "duckdb" { "JSON" } else { "TEXT" };
        self.exec(&format!(
            "DROP TABLE IF EXISTS {name};
             CREATE TABLE {name} (id VARCHAR, seq BIGINT, ts {ts}, ts_ms BIGINT, kind VARCHAR, source VARCHAR, level VARCHAR, title VARCHAR,
                correlation_id VARCHAR, request_path VARCHAR, duration_ms DOUBLE, data {json});"
        ))
    }

    pub fn insert_events(&mut self, name: &str, events: &[Event]) -> Result<()> {
        check_ident(name)?;
        match &mut self.backend {
            #[cfg(feature = "duckdb")]
            Backend::Duck(c) => {
                use duckdb::types::{TimeUnit, Value as D};
                let mut app = c.appender(name).map_err(sql_err)?;
                for e in events {
                    let row = [
                        D::Text(e.id.clone()),
                        D::BigInt(e.seq),
                        D::Timestamp(TimeUnit::Millisecond, e.at),
                        D::BigInt(e.at),
                        D::Text(e.kind.clone()),
                        D::Text(e.source.clone()),
                        D::Text(e.level.as_str().into()),
                        D::Text(e.title.clone()),
                        e.correlation_id.clone().map(D::Text).unwrap_or(D::Null),
                        e.request_path.clone().map(D::Text).unwrap_or(D::Null),
                        e.duration_ms.map(D::Double).unwrap_or(D::Null),
                        D::Text(e.data.to_string()),
                    ];
                    app.append_row(duckdb::appender_params_from_iter(row)).map_err(sql_err)?;
                }
                app.flush().map_err(sql_err)?;
            }
            Backend::Lite(c) => {
                let tx = c.transaction()?;
                {
                    let mut st = tx.prepare(&format!("INSERT INTO {name} VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"))?;
                    for e in events {
                        st.execute(rusqlite::params![
                            e.id,
                            e.seq,
                            format_iso(e.at),
                            e.at,
                            e.kind,
                            e.source,
                            e.level.as_str(),
                            e.title,
                            e.correlation_id,
                            e.request_path,
                            e.duration_ms,
                            e.data.to_string(),
                        ])?;
                    }
                }
                tx.commit()?;
            }
        }
        Ok(())
    }

    /// Exposes a CSV, JSON (array or lines) or Parquet file as table `name`.
    pub fn register_file(&mut self, name: &str, path: &Path) -> Result<()> {
        check_ident(name)?;
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let kind = match ext.as_str() {
            "csv" | "tsv" => "csv",
            "parquet" | "pq" => "parquet",
            "json" | "jsonl" | "ndjson" => "json",
            other => return Err(LogError::Invalid(format!("unsupported file type '.{other}' (csv, tsv, json, jsonl, ndjson, parquet)"))),
        };
        match &self.backend {
            #[cfg(feature = "duckdb")]
            Backend::Duck(_) => {
                let p = sql_string(&path.to_string_lossy());
                let reader = match kind {
                    "csv" => format!("read_csv_auto({p})"),
                    "parquet" => format!("read_parquet({p})"),
                    _ => format!("read_json_auto({p})"),
                };
                self.exec(&format!("CREATE OR REPLACE VIEW {name} AS SELECT * FROM {reader};"))
            }
            Backend::Lite(_) => {
                if kind == "parquet" {
                    return Err(LogError::Invalid("Parquet needs the DuckDB backend".into()));
                }
                let text = std::fs::read_to_string(path)?;
                let tk = if kind == "csv" { TextKind::Csv } else { TextKind::Json };
                let delim = if ext == "tsv" { '\t' } else { ',' };
                self.load_text_sqlite(name, &text, tk, delim)
            }
        }
    }

    /// Exposes JSON or CSV text (e.g. a saved response body) as table `name`.
    pub fn register_text(&mut self, name: &str, text: &str, kind: TextKind) -> Result<()> {
        check_ident(name)?;
        match &self.backend {
            #[cfg(feature = "duckdb")]
            Backend::Duck(_) => {
                let suffix = if kind == TextKind::Csv { ".csv" } else { ".json" };
                let f = tempfile::Builder::new().prefix("devall-sql-").suffix(suffix).tempfile()?;
                std::fs::write(f.path(), text)?;
                let path = f.into_temp_path();
                let r = self.register_file(name, &path);
                self.temps.push(path);
                r
            }
            Backend::Lite(_) => self.load_text_sqlite(name, text, kind, ','),
        }
    }

    #[allow(irrefutable_let_patterns)]
    fn load_text_sqlite(&mut self, name: &str, text: &str, kind: TextKind, delim: char) -> Result<()> {
        let Backend::Lite(c) = &mut self.backend else { return Ok(()) };
        let (columns, rows): (Vec<String>, Vec<Vec<Value>>) = match kind {
            TextKind::Csv => {
                let mut lines = parse_csv(text, delim).into_iter();
                let header = lines.next().unwrap_or_default();
                let rows = lines.map(|r| r.into_iter().map(|v| scalar(&v)).collect()).collect();
                (header, rows)
            }
            TextKind::Json => {
                let items: Vec<Value> = match serde_json::from_str::<Value>(text.trim()) {
                    Ok(Value::Array(a)) => a,
                    Ok(other) => vec![other],
                    Err(_) => text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).map_err(sql_err)).collect::<Result<_>>()?,
                };
                let mut cols: Vec<String> = Vec::new();
                for it in items.iter().take(1000) {
                    if let Value::Object(o) = it {
                        for k in o.keys() {
                            if !cols.contains(k) {
                                cols.push(k.clone());
                            }
                        }
                    }
                }
                if cols.is_empty() {
                    cols.push("value".into());
                }
                let rows = items
                    .into_iter()
                    .map(|it| match it {
                        Value::Object(o) => cols.iter().map(|k| o.get(k).cloned().unwrap_or(Value::Null)).collect(),
                        other => vec![other],
                    })
                    .collect();
                (cols, rows)
            }
        };
        let cols: Vec<String> = columns.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect();
        c.execute_batch(&format!("DROP TABLE IF EXISTS {name}; CREATE TABLE {name} ({});", cols.join(", ")))?;
        let tx = c.transaction()?;
        {
            let ph: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
            let mut st = tx.prepare(&format!("INSERT INTO {name} VALUES ({})", ph.join(", ")))?;
            for r in rows {
                let vals: Vec<rusqlite::types::Value> = (0..cols.len()).map(|i| to_lite(r.get(i).unwrap_or(&Value::Null))).collect();
                st.execute(rusqlite::params_from_iter(vals))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Runs one or more `;`-separated statements and returns the last one's rows.
    pub fn query(&self, sql: &str, max_rows: usize) -> Result<SqlResult> {
        let t0 = Instant::now();
        let stmts = split_statements(sql);
        let Some((last, before)) = stmts.split_last() else { return Err(LogError::Invalid("empty query".into())) };
        for s in before {
            self.exec(s)?;
        }
        let (columns, rows, truncated) = match &self.backend {
            #[cfg(feature = "duckdb")]
            Backend::Duck(c) => {
                let mut st = c.prepare(last).map_err(sql_err)?;
                let mut rows = st.query([]).map_err(sql_err)?;
                let columns = rows.as_ref().map(|s| s.column_names()).unwrap_or_default();
                let mut out = Vec::new();
                let mut truncated = false;
                while let Some(row) = rows.next().map_err(sql_err)? {
                    if out.len() >= max_rows {
                        truncated = true;
                        break;
                    }
                    let mut r = Vec::with_capacity(columns.len());
                    for i in 0..columns.len() {
                        let v: duckdb::types::Value = row.get(i).map_err(sql_err)?;
                        r.push(duck_json(v));
                    }
                    out.push(r);
                }
                (columns, out, truncated)
            }
            Backend::Lite(c) => {
                let mut st = c.prepare(last)?;
                let columns: Vec<String> = st.column_names().into_iter().map(str::to_string).collect();
                let n = columns.len();
                let mut rows = st.query([])?;
                let mut out = Vec::new();
                let mut truncated = false;
                while let Some(row) = rows.next()? {
                    if out.len() >= max_rows {
                        truncated = true;
                        break;
                    }
                    let mut r = Vec::with_capacity(n);
                    for i in 0..n {
                        r.push(lite_json(row.get_ref(i)?));
                    }
                    out.push(r);
                }
                (columns, out, truncated)
            }
        };
        Ok(SqlResult { columns, rows, truncated, elapsed_ms: t0.elapsed().as_secs_f64() * 1000.0, backend: self.backend(), statements: stmts.len() })
    }

    /// Tables and views with their columns (for the editor's help panel).
    pub fn tables(&self) -> Result<Vec<TableInfo>> {
        let sql = match &self.backend {
            #[cfg(feature = "duckdb")]
            Backend::Duck(_) => "SELECT table_name, column_name, data_type FROM information_schema.columns WHERE table_schema = 'main' ORDER BY table_name, ordinal_position",
            Backend::Lite(_) => "SELECT m.name, p.name, p.type FROM sqlite_master m JOIN pragma_table_info(m.name) p WHERE m.type IN ('table','view') ORDER BY m.name, p.cid",
        };
        let r = self.query(sql, 100_000)?;
        let mut out: Vec<TableInfo> = Vec::new();
        for row in r.rows {
            let s = |i: usize| row.get(i).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let (t, c, ty) = (s(0), s(1), s(2));
            match out.last_mut() {
                Some(last) if last.name == t => last.columns.push((c, ty)),
                _ => out.push(TableInfo { name: t, columns: vec![(c, ty)] }),
            }
        }
        Ok(out)
    }
}

fn to_lite(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as L;
    match v {
        Value::Null => L::Null,
        Value::Bool(b) => L::Integer(*b as i64),
        Value::Number(n) => n.as_i64().map(L::Integer).unwrap_or_else(|| L::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => L::Text(s.clone()),
        other => L::Text(other.to_string()),
    }
}

fn scalar(v: &str) -> Value {
    if let Ok(i) = v.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = v.parse::<f64>()
        && f.is_finite()
    {
        return Value::from(f);
    }
    if v.is_empty() { Value::Null } else { Value::String(v.to_string()) }
}

fn lite_json(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef as R;
    match v {
        R::Null => Value::Null,
        R::Integer(i) => Value::from(i),
        R::Real(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        R::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        R::Blob(b) => Value::String(format!("<{} bytes>", b.len())),
    }
}

#[cfg(feature = "duckdb")]
fn duck_json(v: duckdb::types::Value) -> Value {
    use duckdb::types::{TimeUnit, Value as D};
    let micros = |u: TimeUnit, x: i64| u.to_micros(x);
    match v {
        D::Null => Value::Null,
        D::Boolean(b) => Value::Bool(b),
        D::TinyInt(i) => Value::from(i),
        D::SmallInt(i) => Value::from(i),
        D::Int(i) => Value::from(i),
        D::BigInt(i) => Value::from(i),
        D::HugeInt(i) => i64::try_from(i).map(Value::from).unwrap_or_else(|_| Value::String(i.to_string())),
        D::UHugeInt(i) => u64::try_from(i).map(Value::from).unwrap_or_else(|_| Value::String(i.to_string())),
        D::UTinyInt(i) => Value::from(i),
        D::USmallInt(i) => Value::from(i),
        D::UInt(i) => Value::from(i),
        D::UBigInt(i) => Value::from(i),
        D::Float(f) => serde_json::Number::from_f64(f as f64).map(Value::Number).unwrap_or(Value::Null),
        D::Double(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        D::Decimal(d) => {
            let s = d.to_string();
            s.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::String(s))
        }
        D::Timestamp(u, x) => {
            let us = micros(u, x);
            Value::String(format_iso(us.div_euclid(1000)))
        }
        D::Text(s) | D::Enum(s) => Value::String(s),
        D::Blob(b) | D::Geometry(b) => Value::String(format!("<{} bytes>", b.len())),
        D::Date32(d) => Value::String(chrono::DateTime::from_timestamp(d as i64 * 86_400, 0).map(|t| t.format("%Y-%m-%d").to_string()).unwrap_or_default()),
        D::Time64(u, x) => {
            let us = micros(u, x);
            let s = us / 1_000_000;
            Value::String(format!("{:02}:{:02}:{:02}.{:06}", s / 3600, (s / 60) % 60, s % 60, us % 1_000_000))
        }
        D::Interval { months, days, nanos } => Value::String(format!("{months} months {days} days {}ms", nanos / 1_000_000)),
        D::List(items) | D::Array(items) => Value::Array(items.into_iter().map(duck_json).collect()),
        D::Struct(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), duck_json(v.clone()))).collect::<Map<String, Value>>()),
        D::Map(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let key = match duck_json(k.clone()) {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (key, duck_json(v.clone()))
                })
                .collect(),
        ),
        D::Union(v) => duck_json(*v),
        #[allow(unreachable_patterns)]
        other => Value::String(format!("{other:?}")),
    }
}

/// Splits on `;` outside quotes and comments; drops empty statements.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = sql.chars().peekable();
    let mut has_code = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                cur.push(c);
                has_code = true;
                for d in chars.by_ref() {
                    cur.push(d);
                    if d == c {
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        cur.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for d in chars.by_ref() {
                    if prev == '*' && d == '/' {
                        break;
                    }
                    prev = d;
                }
                cur.push(' ');
            }
            ';' => {
                if has_code {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                has_code = false;
            }
            c => {
                if !c.is_whitespace() {
                    has_code = true;
                }
                cur.push(c);
            }
        }
    }
    if has_code {
        out.push(cur.trim().to_string());
    }
    out
}

/// Minimal RFC 4180 CSV parsing (quoted fields, doubled quotes, CRLF).
fn parse_csv(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_q = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_q = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            in_q = true;
        } else if c == delim {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            if !(row.len() == 1 && row[0].is_empty()) {
                rows.push(std::mem::take(&mut row));
            } else {
                row.clear();
            }
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_text;
    use crate::time::TimeCtx;

    fn sample() -> Vec<LogEntry> {
        let text = (0..50)
            .map(|i| {
                format!(
                    "{{\"ts\":\"2024-01-02T03:04:{:02}Z\",\"level\":\"{}\",\"msg\":\"req {i}\",\"status\":{},\"user\":{{\"id\":{}}}}}",
                    i,
                    if i % 10 == 0 { "error" } else { "info" },
                    if i % 10 == 0 { 500 } else { 200 },
                    i % 3
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        parse_text(&text, &TimeCtx::utc()).1
    }

    fn exercise(mut e: SqlEngine) {
        e.create_logs_table("logs").unwrap();
        e.insert_entries("logs", &sample()).unwrap();
        let r = e.query("SELECT level, count(*) AS n FROM logs GROUP BY level ORDER BY n DESC", 100).unwrap();
        assert_eq!(r.columns, vec!["level", "n"]);
        assert_eq!(r.rows[0], vec![Value::from("info"), Value::from(45)]);
        assert_eq!(r.rows[1], vec![Value::from("error"), Value::from(5)]);

        let status = if e.backend() == "duckdb" { "CAST(fields->>'$.status' AS INTEGER)" } else { "CAST(json_extract(fields, '$.status') AS INTEGER)" };
        let r = e.query(&format!("SELECT count(*) FROM logs WHERE {status} >= 500"), 10).unwrap();
        assert_eq!(r.rows[0][0], Value::from(5));
        let r = e.query("SELECT json_extract(fields, '$.user.id') AS uid, count(*) c FROM logs GROUP BY 1 ORDER BY 1", 10).unwrap();
        assert_eq!(r.rows.len(), 3);
        let r = e.query("SELECT ts, ts_ms FROM logs ORDER BY ts_ms LIMIT 1", 10).unwrap();
        assert_eq!(r.rows[0][0].as_str().unwrap()[..19], *"2024-01-02T03:04:00");
        // Several statements; truncation.
        let r = e.query("CREATE TABLE t AS SELECT * FROM logs WHERE level = 'error'; SELECT * FROM t", 2).unwrap();
        assert_eq!((r.statements, r.rows.len(), r.truncated), (2, 2, true));
        assert!(e.query("SELEC nope", 1).is_err());
        assert!(e.tables().unwrap().iter().any(|t| t.name == "logs" && t.columns.iter().any(|c| c.0 == "fields")));

        // Registered data.
        e.register_text("resp", r#"[{"id":1,"name":"a"},{"id":2,"name":"b"}]"#, TextKind::Json).unwrap();
        let r = e.query("SELECT sum(id) AS s FROM resp", 10).unwrap();
        assert_eq!(r.rows[0][0].as_f64(), Some(3.0));
        e.register_text("csv1", "a,b\n1,\"x, y\"\n2,z\n", TextKind::Csv).unwrap();
        let r = e.query("SELECT b FROM csv1 WHERE a = 1", 10).unwrap();
        assert_eq!(r.rows[0][0], Value::from("x, y"));
        assert!(e.register_text("bad name", "[]", TextKind::Json).is_err());

        let ev = vec![
            Event::new("http.send", "http", "GET /x").correlation(Some("c-1")).duration_ms(12.0),
            Event::new("mock.request", "mock", "GET /x → 200").correlation(Some("c-1")),
        ];
        let ev: Vec<Event> = ev.into_iter().enumerate().map(|(i, mut e)| {
            e.seq = i as i64 + 1;
            e.at = 1_704_164_645_000 + i as i64;
            e
        }).collect();
        e.create_events_table("events").unwrap();
        e.insert_events("events", &ev).unwrap();
        let r = e.query("SELECT kind FROM events WHERE correlation_id = 'c-1' ORDER BY seq", 10).unwrap();
        assert_eq!(r.rows.len(), 2);
    }

    #[test]
    fn sqlite_backend() {
        exercise(SqlEngine::sqlite().unwrap());
    }

    #[cfg(feature = "duckdb")]
    #[test]
    fn duckdb_backend() {
        let e = SqlEngine::duckdb().unwrap();
        assert_eq!(e.backend(), "duckdb");
        exercise(e);
        // DuckDB reads files directly.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("data.csv");
        std::fs::write(&p, "k,v\na,1\nb,2\n").unwrap();
        let e = SqlEngine::duckdb().unwrap();
        let r = e.query(&format!("SELECT sum(v) FROM read_csv_auto({})", sql_string(&p.to_string_lossy())), 10).unwrap();
        assert_eq!(r.rows[0][0].as_f64(), Some(3.0));
    }

    #[test]
    fn statements_and_csv() {
        assert_eq!(split_statements("select ';' as x; -- c;\n select 2; /* ; */ ;"), vec!["select ';' as x", "select 2"]);
        assert_eq!(parse_csv("a,b\r\n\"q\"\"x\",2\n", ','), vec![vec!["a", "b"], vec!["q\"x", "2"]]);
    }
}
