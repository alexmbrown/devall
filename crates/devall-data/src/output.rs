//! Streaming writers for every [`OutputFormat`]. Rows are written as they are generated, so
//! exports of hundreds of thousands of rows run in constant memory (apart from foreign-key pools).

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::engine::{DATE_FORMAT, DATETIME_FORMAT, GenerateOptions, GenerateStats, RowSink, generate};
use crate::error::{DataError, Result, invalid};
use crate::model::{Entity, Field, FieldKind, OutputFormat, Schema, SqlDialect};

/// Generates and writes in `format`.
pub fn write_output(schema: &Schema, opts: &GenerateOptions, format: &OutputFormat, out: &mut dyn Write) -> Result<GenerateStats> {
    let mut sink = make_sink(schema, format, out)?;
    generate(schema, opts, sink.as_mut())
}

/// A generated document as text, cut at `max_bytes` (on a char boundary) when given.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rendered {
    pub text: String,
    pub truncated: bool,
    pub stats: GenerateStats,
}

pub fn generate_string(schema: &Schema, opts: &GenerateOptions, format: &OutputFormat, max_bytes: Option<usize>) -> Result<Rendered> {
    // Resolve the seed up front so a truncated preview still reports it.
    let seed = schema.seed.or(opts.seed).unwrap_or_else(|| rand::random::<u64>() >> 11);
    let opts = GenerateOptions { seed: Some(seed), ..opts.clone() };
    let mut w = LimitWriter { buf: Vec::new(), max: max_bytes.unwrap_or(usize::MAX), hit: false };
    let result = write_output(schema, &opts, format, &mut w);
    let truncated = w.hit;
    let stats = match result {
        Ok(stats) => stats,
        Err(DataError::Io(e)) if truncated && e.kind() == io::ErrorKind::FileTooLarge => GenerateStats { seed, ..Default::default() },
        Err(e) => return Err(e),
    };
    let mut bytes = w.buf;
    if truncated {
        bytes.truncate(w.max);
    }
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            let valid = e.utf8_error().valid_up_to();
            let mut b = e.into_bytes();
            b.truncate(valid);
            String::from_utf8(b).unwrap_or_default()
        }
    };
    Ok(Rendered { text, truncated, stats })
}

struct LimitWriter {
    buf: Vec<u8>,
    max: usize,
    hit: bool,
}

impl Write for LimitWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len() >= self.max {
            self.hit = true;
            return Err(io::Error::new(io::ErrorKind::FileTooLarge, "preview limit reached"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Result of [`export_to_path`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// Files written (one per entity for CSV/NDJSON when several entities are exported).
    pub files: Vec<String>,
    pub rows: u64,
    pub bytes: u64,
    pub seed: u64,
}

/// Writes the generated data to `dest` (atomically: temp file + rename). CSV and NDJSON hold one
/// table per file, so exporting several entities writes `<stem>.<entity>.<ext>` next to `dest`,
/// all from the same seed (foreign keys agree across files).
pub fn export_to_path(schema: &Schema, opts: &GenerateOptions, format: &OutputFormat, dest: &Path) -> Result<ExportResult> {
    let seed = schema.seed.or(opts.seed).unwrap_or_else(|| rand::random::<u64>() >> 11);
    let emitted: Vec<&Entity> = match &opts.entity {
        Some(name) => vec![schema.entity(name).ok_or_else(|| DataError::UnknownEntity(name.clone()))?],
        None => schema.entities.iter().collect(),
    };
    let mut result = ExportResult { files: Vec::new(), rows: 0, bytes: 0, seed };
    let jobs: Vec<(PathBuf, GenerateOptions)> = if format.single_table() && emitted.len() > 1 {
        let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("data").to_string();
        let ext = dest.extension().and_then(|s| s.to_str()).unwrap_or(format.extension()).to_string();
        emitted
            .iter()
            .map(|e| {
                let file = format!("{stem}.{}.{ext}", devall_core::fsutil::slugify(&e.name));
                (dest.with_file_name(file), GenerateOptions { entity: Some(e.name.clone()), seed: Some(seed), ..opts.clone() })
            })
            .collect()
    } else {
        vec![(dest.to_path_buf(), GenerateOptions { seed: Some(seed), ..opts.clone() })]
    };
    for (path, o) in jobs {
        let (stats, bytes) = write_atomically(&path, |w| write_output(schema, &o, format, w))?;
        result.rows += stats.rows.values().sum::<u64>();
        result.bytes += bytes;
        result.files.push(path.display().to_string());
    }
    Ok(result)
}

fn write_atomically<T>(path: &Path, f: impl FnOnce(&mut dyn Write) -> Result<T>) -> Result<(T, u64)> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("export");
    let tmp = dir.join(format!(".{name}.{:08x}.tmp", rand::random::<u32>()));
    let run = || -> Result<(T, u64)> {
        let file = fs::File::create(&tmp)?;
        let mut w = CountingWriter { inner: BufWriter::with_capacity(1 << 16, file), count: 0 };
        let value = f(&mut w)?;
        w.flush()?;
        let count = w.count;
        w.inner.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok((value, count))
    };
    match run() {
        Ok(v) => {
            if let Err(e) = fs::rename(&tmp, path) {
                let _ = fs::remove_file(&tmp);
                return Err(e.into());
            }
            Ok(v)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

struct CountingWriter<W: Write> {
    inner: W,
    count: u64,
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(data)?;
        self.count += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn make_sink<'a>(schema: &'a Schema, format: &OutputFormat, w: &'a mut dyn Write) -> Result<Box<dyn RowSink + 'a>> {
    Ok(match format {
        OutputFormat::Json { pretty } => Box::new(JsonSink { w, pretty: *pretty, multi: false, entities: 0, rows: 0, buf: Vec::new() }),
        OutputFormat::Ndjson => Box::new(NdjsonSink { w }),
        OutputFormat::Csv { delimiter, header } => {
            let delim = parse_delimiter(delimiter.as_deref())?;
            Box::new(CsvSink { w, delim, header: *header, multi: false, entities: 0, columns: Vec::new(), line: String::new() })
        }
        OutputFormat::Sql { dialect, create_table, batch_size } => Box::new(SqlSink {
            w,
            schema,
            dialect: *dialect,
            create_table: *create_table,
            // SQL Server accepts at most 1000 rows per VALUES list.
            batch: batch_size.unwrap_or(100).max(1).min(if *dialect == SqlDialect::Sqlserver { 1000 } else { usize::MAX }),
            table: String::new(),
            columns: Vec::new(),
            in_batch: 0,
            line: String::new(),
        }),
        OutputFormat::Yaml => Box::new(YamlSink { w, multi: false, rows: 0 }),
    })
}

fn parse_delimiter(d: Option<&str>) -> Result<char> {
    let d = match d {
        None | Some("") => return Ok(','),
        Some("\\t") | Some("tab") => return Ok('\t'),
        Some(d) => d,
    };
    let mut chars = d.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !matches!(c, '"' | '\n' | '\r') => Ok(c),
        _ => Err(invalid(format!("CSV delimiter must be a single character other than a quote or newline, got {d:?}"))),
    }
}

// ---------- JSON ----------

struct JsonSink<'w> {
    w: &'w mut dyn Write,
    pretty: bool,
    multi: bool,
    entities: usize,
    rows: u64,
    buf: Vec<u8>,
}

impl JsonSink<'_> {
    fn indent(&self) -> &'static [u8] {
        if self.multi { b"    " } else { b"  " }
    }
}

impl RowSink for JsonSink<'_> {
    fn begin(&mut self, entities: &[&Entity]) -> Result<()> {
        self.multi = entities.len() != 1;
        if self.multi {
            self.w.write_all(b"{")?;
        }
        Ok(())
    }
    fn begin_entity(&mut self, entity: &Entity, _: u64) -> Result<()> {
        if self.multi {
            if self.entities > 0 {
                self.w.write_all(b",")?;
            }
            if self.pretty {
                self.w.write_all(b"\n  ")?;
            }
            serde_json::to_writer(&mut *self.w, &entity.name).map_err(io::Error::from)?;
            self.w.write_all(if self.pretty { b": " } else { b":" })?;
        }
        self.entities += 1;
        self.rows = 0;
        self.w.write_all(b"[")?;
        Ok(())
    }
    fn row(&mut self, row: &Map<String, Value>) -> Result<()> {
        if self.rows > 0 {
            self.w.write_all(b",")?;
        }
        self.rows += 1;
        if !self.pretty {
            serde_json::to_writer(&mut *self.w, row).map_err(io::Error::from)?;
            return Ok(());
        }
        self.buf.clear();
        serde_json::to_writer_pretty(&mut self.buf, row).map_err(io::Error::from)?;
        let indent = self.indent();
        self.w.write_all(b"\n")?;
        self.w.write_all(indent)?;
        // Pretty JSON never has raw newlines inside strings, so indenting every line is safe.
        let mut start = 0;
        for (i, b) in self.buf.iter().enumerate() {
            if *b == b'\n' {
                self.w.write_all(&self.buf[start..=i])?;
                self.w.write_all(indent)?;
                start = i + 1;
            }
        }
        self.w.write_all(&self.buf[start..])?;
        Ok(())
    }
    fn end_entity(&mut self) -> Result<()> {
        if self.pretty && self.rows > 0 {
            self.w.write_all(if self.multi { b"\n  " } else { b"\n" })?;
        }
        self.w.write_all(b"]")?;
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        if self.multi {
            if self.pretty && self.entities > 0 {
                self.w.write_all(b"\n")?;
            }
            self.w.write_all(b"}")?;
        }
        if self.pretty {
            self.w.write_all(b"\n")?;
        }
        self.w.flush()?;
        Ok(())
    }
}

// ---------- NDJSON ----------

struct NdjsonSink<'w> {
    w: &'w mut dyn Write,
}

impl RowSink for NdjsonSink<'_> {
    fn begin(&mut self, _: &[&Entity]) -> Result<()> {
        Ok(())
    }
    fn begin_entity(&mut self, _: &Entity, _: u64) -> Result<()> {
        Ok(())
    }
    fn row(&mut self, row: &Map<String, Value>) -> Result<()> {
        serde_json::to_writer(&mut *self.w, row).map_err(io::Error::from)?;
        self.w.write_all(b"\n")?;
        Ok(())
    }
    fn end_entity(&mut self) -> Result<()> {
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        self.w.flush()?;
        Ok(())
    }
}

// ---------- YAML ----------

struct YamlSink<'w> {
    w: &'w mut dyn Write,
    multi: bool,
    rows: u64,
}

impl RowSink for YamlSink<'_> {
    fn begin(&mut self, entities: &[&Entity]) -> Result<()> {
        self.multi = entities.len() != 1;
        if entities.is_empty() {
            self.w.write_all(b"{}\n")?;
        }
        Ok(())
    }
    fn begin_entity(&mut self, entity: &Entity, _: u64) -> Result<()> {
        self.rows = 0;
        if self.multi {
            let key = serde_yaml_ng::to_string(&entity.name).map_err(|e| invalid(e.to_string()))?;
            self.w.write_all(key.trim_end().as_bytes())?;
            self.w.write_all(b":")?;
        }
        Ok(())
    }
    fn row(&mut self, row: &Map<String, Value>) -> Result<()> {
        if self.multi && self.rows == 0 {
            self.w.write_all(b"\n")?;
        }
        self.rows += 1;
        let text = serde_yaml_ng::to_string(row).map_err(|e| invalid(e.to_string()))?;
        let indent: &[u8] = if self.multi { b"  " } else { b"" };
        for (i, line) in text.lines().enumerate() {
            self.w.write_all(indent)?;
            self.w.write_all(if i == 0 { b"- " } else { b"  " })?;
            self.w.write_all(line.as_bytes())?;
            self.w.write_all(b"\n")?;
        }
        Ok(())
    }
    fn end_entity(&mut self) -> Result<()> {
        if self.rows == 0 {
            self.w.write_all(if self.multi { b" []\n" } else { b"[]\n" })?;
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        self.w.flush()?;
        Ok(())
    }
}

// ---------- CSV ----------

struct CsvSink<'w> {
    w: &'w mut dyn Write,
    delim: char,
    header: bool,
    multi: bool,
    entities: usize,
    columns: Vec<Vec<String>>,
    line: String,
}

/// Column paths of an entity: nested objects flatten to `a.b`, everything else is one column.
pub fn csv_columns(fields: &[Field]) -> Vec<Vec<String>> {
    fn walk(fields: &[Field], prefix: &[String], out: &mut Vec<Vec<String>>) {
        for f in fields {
            let mut path = prefix.to_vec();
            path.push(f.name.clone());
            match &f.kind {
                FieldKind::Object { fields } if !fields.is_empty() => walk(fields, &path, out),
                _ => out.push(path),
            }
        }
    }
    let mut out = Vec::new();
    walk(fields, &[], &mut out);
    out
}

fn csv_cell(v: Option<&Value>, delim: char, out: &mut String) {
    let text: std::borrow::Cow<str> = match v {
        None | Some(Value::Null) => return,
        Some(Value::String(s)) => s.as_str().into(),
        Some(Value::Bool(b)) => b.to_string().into(),
        Some(Value::Number(n)) => n.to_string().into(),
        Some(other) => other.to_string().into(),
    };
    let needs_quotes = text.contains(delim)
        || text.contains(['"', '\n', '\r'])
        || text.starts_with(char::is_whitespace)
        || text.ends_with(char::is_whitespace);
    if needs_quotes {
        out.push('"');
        out.push_str(&text.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(&text);
    }
}

impl RowSink for CsvSink<'_> {
    fn begin(&mut self, entities: &[&Entity]) -> Result<()> {
        self.multi = entities.len() > 1;
        Ok(())
    }
    fn begin_entity(&mut self, entity: &Entity, _: u64) -> Result<()> {
        if self.multi {
            if self.entities > 0 {
                self.w.write_all(b"\n")?;
            }
            writeln!(self.w, "# {}", entity.name)?;
        }
        self.entities += 1;
        self.columns = csv_columns(&entity.fields);
        if self.header {
            self.line.clear();
            for (i, c) in self.columns.iter().enumerate() {
                if i > 0 {
                    self.line.push(self.delim);
                }
                csv_cell(Some(&Value::String(c.join("."))), self.delim, &mut self.line);
            }
            self.line.push('\n');
            self.w.write_all(self.line.as_bytes())?;
        }
        Ok(())
    }
    fn row(&mut self, row: &Map<String, Value>) -> Result<()> {
        self.line.clear();
        for (i, path) in self.columns.iter().enumerate() {
            if i > 0 {
                self.line.push(self.delim);
            }
            let mut v = row.get(&path[0]);
            for seg in &path[1..] {
                v = v.and_then(|x| x.get(seg));
            }
            csv_cell(v, self.delim, &mut self.line);
        }
        self.line.push('\n');
        self.w.write_all(self.line.as_bytes())?;
        Ok(())
    }
    fn end_entity(&mut self) -> Result<()> {
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        self.w.flush()?;
        Ok(())
    }
}

// ---------- SQL ----------

struct SqlSink<'a> {
    w: &'a mut dyn Write,
    schema: &'a Schema,
    dialect: SqlDialect,
    create_table: bool,
    batch: usize,
    table: String,
    columns: Vec<String>,
    in_batch: usize,
    line: String,
}

/// Quotes an identifier (`"name"`, `` `name` `` for MySQL, `[name]` for SQL Server), doubling
/// embedded closing quotes.
pub fn sql_ident(name: &str, dialect: SqlDialect) -> String {
    let (open, close) = match dialect {
        SqlDialect::Mysql => ('`', '`'),
        SqlDialect::Sqlserver => ('[', ']'),
        _ => ('"', '"'),
    };
    let mut out = String::with_capacity(name.len() + 2);
    out.push(open);
    for c in name.chars().filter(|c| *c != '\0') {
        if c == close {
            out.push(close);
        }
        out.push(c);
    }
    out.push(close);
    out
}

/// A string literal. Quotes are doubled; MySQL also escapes backslashes and control characters
/// (it treats `\` as an escape by default). PostgreSQL and SQLite text cannot hold NUL, so NUL
/// characters are dropped there. SQL Server strings are Unicode (`N'…'`).
pub fn sql_string(s: &str, dialect: SqlDialect, out: &mut String) {
    if dialect == SqlDialect::Sqlserver {
        out.push('N');
    }
    out.push('\'');
    for c in s.chars() {
        match (dialect, c) {
            (_, '\'') => out.push_str("''"),
            (SqlDialect::Mysql, '\\') => out.push_str("\\\\"),
            (SqlDialect::Mysql, '\0') => out.push_str("\\0"),
            (SqlDialect::Mysql, '\n') => out.push_str("\\n"),
            (SqlDialect::Mysql, '\r') => out.push_str("\\r"),
            (SqlDialect::Mysql, '\u{1a}') => out.push_str("\\Z"),
            (_, '\0') => {}
            _ => out.push(c),
        }
    }
    out.push('\'');
}

/// A SQL literal for any JSON value; objects and arrays become JSON text.
pub fn sql_literal(v: &Value, dialect: SqlDialect, out: &mut String) {
    match v {
        Value::Null => out.push_str("NULL"),
        Value::Bool(b) => out.push_str(match (dialect, b) {
            (SqlDialect::Sqlite | SqlDialect::Sqlserver, true) => "1",
            (SqlDialect::Sqlite | SqlDialect::Sqlserver, false) => "0",
            (_, true) => "TRUE",
            (_, false) => "FALSE",
        }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => sql_string(s, dialect, out),
        other => sql_string(&other.to_string(), dialect, out),
    }
}

impl RowSink for SqlSink<'_> {
    fn begin(&mut self, _: &[&Entity]) -> Result<()> {
        Ok(())
    }
    fn begin_entity(&mut self, entity: &Entity, rows: u64) -> Result<()> {
        let label = entity.name.replace(['\n', '\r'], " ");
        writeln!(self.w, "-- {label}: {rows} row{}", if rows == 1 { "" } else { "s" })?;
        if self.create_table {
            self.w.write_all(create_table_sql(self.schema, entity, self.dialect).as_bytes())?;
        }
        self.table = sql_ident(&entity.name, self.dialect);
        self.columns = entity.fields.iter().map(|f| f.name.clone()).collect();
        self.in_batch = 0;
        Ok(())
    }
    fn row(&mut self, row: &Map<String, Value>) -> Result<()> {
        self.line.clear();
        if self.columns.is_empty() {
            self.line.push_str(&format!(
                "INSERT INTO {} {};\n",
                self.table,
                if self.dialect == SqlDialect::Mysql { "() VALUES ()" } else { "DEFAULT VALUES" }
            ));
            self.w.write_all(self.line.as_bytes())?;
            return Ok(());
        }
        if self.in_batch == 0 {
            let cols: Vec<String> = self.columns.iter().map(|c| sql_ident(c, self.dialect)).collect();
            self.line.push_str(&format!("INSERT INTO {} ({}) VALUES\n  (", self.table, cols.join(", ")));
        } else {
            self.line.push_str(",\n  (");
        }
        for (i, c) in self.columns.iter().enumerate() {
            if i > 0 {
                self.line.push_str(", ");
            }
            sql_literal(row.get(c).unwrap_or(&Value::Null), self.dialect, &mut self.line);
        }
        self.line.push(')');
        self.in_batch += 1;
        if self.in_batch >= self.batch {
            self.line.push_str(";\n");
            self.in_batch = 0;
        }
        self.w.write_all(self.line.as_bytes())?;
        Ok(())
    }
    fn end_entity(&mut self) -> Result<()> {
        if self.in_batch > 0 {
            self.w.write_all(b";\n")?;
            self.in_batch = 0;
        }
        self.w.write_all(b"\n")?;
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        self.w.flush()?;
        Ok(())
    }
}

fn is_primary_key(f: &Field) -> bool {
    f.name.eq_ignore_ascii_case("id")
        && f.null_percent == 0.0
        && (f.unique || matches!(f.kind, FieldKind::Sequence { .. } | FieldKind::Uuid { .. } | FieldKind::Ulid))
}

fn primary_key(e: &Entity) -> Option<&Field> {
    e.fields.iter().find(|f| is_primary_key(f))
}

/// `CREATE TABLE IF NOT EXISTS` with column types inferred from the generators, `NOT NULL` for
/// fields without nulls, a primary key on a unique/id-typed `id`, `UNIQUE`, and foreign keys
/// for references to a primary-key or unique column.
pub fn create_table_sql(schema: &Schema, entity: &Entity, d: SqlDialect) -> String {
    let pk = primary_key(entity).map(|f| f.name.as_str());
    let mut lines = Vec::new();
    for f in &entity.fields {
        let ty = sql_type(schema, &f.kind, d, 0);
        let mut line = format!("  {} {ty}", sql_ident(&f.name, d));
        let self_ref = matches!(&f.kind, FieldKind::Ref { entity: e, .. } if *e == entity.name);
        let nullable_const = matches!(&f.kind, FieldKind::Json { value: Value::Null });
        if f.null_percent == 0.0 && !self_ref && !nullable_const {
            line.push_str(" NOT NULL");
        }
        if Some(f.name.as_str()) == pk {
            line.push_str(" PRIMARY KEY");
        } else if f.unique && !(d == SqlDialect::Mysql && ty == "TEXT") && !(d == SqlDialect::Sqlserver && ty.ends_with("(MAX)")) {
            line.push_str(" UNIQUE");
        }
        lines.push(line);
    }
    for f in &entity.fields {
        if let FieldKind::Ref { entity: target, field } = &f.kind
            && let Some(t) = schema.entity(target)
            && let Some(tf) = t.fields.iter().find(|x| &x.name == field)
            && (is_primary_key(tf) || tf.unique)
        {
            lines.push(format!(
                "  FOREIGN KEY ({}) REFERENCES {} ({})",
                sql_ident(&f.name, d),
                sql_ident(&t.name, d),
                sql_ident(&tf.name, d)
            ));
        }
    }
    if d == SqlDialect::Sqlserver {
        // T-SQL's CREATE TABLE has no IF NOT EXISTS.
        let mut name = String::new();
        sql_string(&entity.name, d, &mut name);
        return format!("IF OBJECT_ID({name}, N'U') IS NULL\nCREATE TABLE {} (\n{}\n);\n", sql_ident(&entity.name, d), lines.join(",\n"));
    }
    format!("CREATE TABLE IF NOT EXISTS {} (\n{}\n);\n", sql_ident(&entity.name, d), lines.join(",\n"))
}

fn sql_type(schema: &Schema, kind: &FieldKind, d: SqlDialect, depth: usize) -> String {
    use SqlDialect::*;
    // Postgres, MySQL, SQLite, SQL Server.
    let pick4 = |pg: &str, my: &str, lite: &str, ms: &str| match d {
        Postgres => pg.to_string(),
        Mysql => my.to_string(),
        Sqlite => lite.to_string(),
        Sqlserver => ms.to_string(),
    };
    let short_text = || pick4("TEXT", "VARCHAR(255)", "TEXT", "NVARCHAR(255)");
    let long_text = || pick4("TEXT", "TEXT", "TEXT", "NVARCHAR(MAX)");
    let int = || pick4("BIGINT", "BIGINT", "INTEGER", "BIGINT");
    let float = || pick4("DOUBLE PRECISION", "DOUBLE", "REAL", "FLOAT");
    let boolean = || pick4("BOOLEAN", "BOOLEAN", "INTEGER", "BIT");
    match kind {
        FieldKind::Uuid { .. } => pick4("UUID", "CHAR(36)", "TEXT", "UNIQUEIDENTIFIER"),
        FieldKind::Ulid => pick4("CHAR(26)", "CHAR(26)", "TEXT", "CHAR(26)"),
        FieldKind::Sequence { .. } | FieldKind::Int { .. } | FieldKind::Timestamp { .. } => int(),
        FieldKind::Float { .. } | FieldKind::Latitude | FieldKind::Longitude => float(),
        FieldKind::Price { decimals, .. } => {
            let dd = decimals.unwrap_or(2).min(12);
            pick4(&format!("NUMERIC(16, {dd})"), &format!("DECIMAL(16, {dd})"), "NUMERIC", &format!("DECIMAL(16, {dd})"))
        }
        FieldKind::Bool { .. } => boolean(),
        FieldKind::Date { format, .. } if format.as_deref().unwrap_or(DATE_FORMAT) == DATE_FORMAT => pick4("DATE", "DATE", "TEXT", "DATE"),
        FieldKind::Datetime { format, .. } => match format.as_deref().unwrap_or(DATETIME_FORMAT) {
            DATETIME_FORMAT => pick4("TIMESTAMPTZ", "VARCHAR(32)", "TEXT", "DATETIMEOFFSET"),
            "%Y-%m-%d %H:%M:%S" => pick4("TIMESTAMP", "DATETIME", "TEXT", "DATETIME2"),
            _ => short_text(),
        },
        FieldKind::Object { .. } | FieldKind::Array { .. } | FieldKind::Json { .. } => pick4("JSONB", "JSON", "TEXT", "NVARCHAR(MAX)"),
        FieldKind::Paragraph { .. } | FieldKind::Lorem { .. } | FieldKind::Template { .. } | FieldKind::Regex { .. } | FieldKind::UserAgent | FieldKind::Sentence { .. } => long_text(),
        FieldKind::Enum { values, .. } => {
            if values.iter().all(|v| v.is_i64() || v.is_u64()) {
                int()
            } else if values.iter().all(Value::is_number) {
                float()
            } else if values.iter().all(Value::is_boolean) {
                boolean()
            } else if values.iter().all(|v| v.as_str().is_some_and(|s| s.chars().count() <= 255)) {
                short_text()
            } else {
                long_text()
            }
        }
        FieldKind::Ref { entity, field } => {
            let target = schema.entity(entity).and_then(|e| e.fields.iter().find(|f| &f.name == field));
            match target {
                Some(t) if depth < 8 => sql_type(schema, &t.kind, d, depth + 1),
                _ => short_text(),
            }
        }
        _ => short_text(),
    }
}
