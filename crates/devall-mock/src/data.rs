//! Tabular data sources for stubs (a /dev/all take on WireMock Cloud's data sources): CSV, JSON
//! and SQLite files in `<project>/mocks/data/`, looked up from response templates.
//!
//! | File | Source name | Rows |
//! |---|---|---|
//! | `users.csv` (header row, RFC 4180) | `users` | one per line; `42`, `4.5`, `true`/`false` become numbers/booleans when they round-trip exactly |
//! | `users.json` | `users` | a JSON array of objects (or `{"rows": [...]}`) |
//! | `shop.db` / `.sqlite` / `.sqlite3` | `shop.<table>` (or `shop` when it has one table) | the table, opened read-only |
//!
//! Sub-folders are part of the name (`mocks/data/eu/users.csv` → `eu/users`).
//!
//! Two ways to use them:
//!
//! * **Template helpers** (any templated response):
//!   `{{dataSource 'users' 'id' request.path.id}}` → the first row whose `id` equals the value
//!   (compared as text), rendered as JSON; as a sub-expression it is an object:
//!   `{{lookup (dataSource 'users' 'id' request.path.id) 'name'}}`. `{{dataRows 'users' 'role'
//!   'admin' limit=10 offset=0}}` → every matching row (all rows without criteria);
//!   `{{dataCount 'users' 'role' 'admin'}}` → how many match. Several `'column' value` pairs are
//!   ANDed.
//! * **`transformerParameters.dataSource`**: the lookup runs before templating and its rows are
//!   in the model as `data` (array) and `row` (first match or null). Templating is switched on
//!   automatically.
//!   ```json
//!   "transformerParameters": {
//!     "dataSource": {
//!       "name": "users",
//!       "where": { "id": "{{request.path.id}}" },
//!       "limit": 50,
//!       "notFoundStatus": 404
//!     }
//!   }
//!   ```
//!   `where` values are templates. `notFoundStatus` replaces the stub's status when nothing
//!   matches (the body still renders, so `{{#if row}}…{{else}}…{{/if}}` can explain). SQLite
//!   sources also accept `"sql": "SELECT * FROM users WHERE email = ?1"` with
//!   `"params": ["{{request.query.email}}"]` (read-only connection).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::util::json_text;

/// Folder under `mocks/` holding data files.
pub const DATA_DIR: &str = "data";
/// Rows returned by one lookup at most.
pub const MAX_ROWS: usize = 1000;

const TABLE_EXTS: &[&str] = &["csv", "json"];
const SQLITE_EXTS: &[&str] = &["db", "sqlite", "sqlite3"];

pub type Row = Map<String, Value>;

/// A data file found under `mocks/data/` (for pickers and `devall mock check`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceInfo {
    /// Name to use in helpers (`users`, `shop.orders`).
    pub name: String,
    /// `csv`, `json` or `sqlite`.
    pub kind: String,
    /// Path relative to the project root, `/`-separated.
    pub file: String,
    pub columns: Vec<String>,
    pub rows: usize,
    /// Set when the file can't be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Equality criteria (`column == value`, compared as text) plus paging.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub criteria: Vec<(String, String)>,
    pub limit: Option<usize>,
    pub offset: usize,
}

type Cached = (SystemTime, u64, Arc<Vec<Row>>);

/// Reads data files on demand and caches parsed CSV/JSON tables until the file changes.
#[derive(Debug)]
pub struct DataSources {
    dir: Option<PathBuf>,
    root: Option<PathBuf>,
    cache: Mutex<HashMap<PathBuf, Cached>>,
}

impl DataSources {
    /// Sources of the project at `root` (`<root>/mocks/data`).
    pub fn for_project(root: &Path) -> Self {
        Self { dir: Some(root.join(crate::MOCKS_DIR).join(DATA_DIR)), root: Some(root.to_path_buf()), cache: Mutex::new(HashMap::new()) }
    }

    /// No data folder: every lookup fails with a clear message.
    pub fn none() -> Self {
        Self { dir: None, root: None, cache: Mutex::new(HashMap::new()) }
    }

    fn dir(&self) -> Result<&Path, String> {
        self.dir.as_deref().ok_or_else(|| "data sources need a project (mocks/data/)".to_string())
    }

    /// Every data file, sorted by name. SQLite files list one entry per table.
    pub fn list(&self) -> Vec<DataSourceInfo> {
        let Ok(dir) = self.dir() else { return Vec::new() };
        let mut files = Vec::new();
        collect(dir, &mut files);
        files.sort();
        let mut out = Vec::new();
        for path in files {
            let Some(ext) = ext_of(&path) else { continue };
            let base = name_of(dir, &path);
            let file = self.project_rel(&path);
            if SQLITE_EXTS.contains(&ext.as_str()) {
                match sqlite_tables(&path) {
                    Ok(tables) => {
                        for t in tables {
                            let (columns, rows) = sqlite_shape(&path, &t).unwrap_or_default();
                            out.push(DataSourceInfo { name: format!("{base}.{t}"), kind: "sqlite".into(), file: file.clone(), columns, rows, error: None });
                        }
                    }
                    Err(e) => out.push(DataSourceInfo { name: base, kind: "sqlite".into(), file, columns: Vec::new(), rows: 0, error: Some(e) }),
                }
                continue;
            }
            match self.table(&path) {
                Ok(rows) => out.push(DataSourceInfo { name: base, kind: ext, file, columns: columns_of(&rows), rows: rows.len(), error: None }),
                Err(e) => out.push(DataSourceInfo { name: base, kind: ext, file, columns: Vec::new(), rows: 0, error: Some(e) }),
            }
        }
        out
    }

    fn project_rel(&self, path: &Path) -> String {
        let rel = self.root.as_deref().and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path);
        rel.to_string_lossy().replace('\\', "/")
    }

    /// Rows of `name` matching `q`.
    pub fn query(&self, name: &str, q: &Query) -> Result<Vec<Row>, String> {
        let limit = q.limit.unwrap_or(MAX_ROWS).min(MAX_ROWS);
        match self.resolve(name)? {
            Resolved::Table(path) => {
                let rows = self.table(&path)?;
                Ok(rows.iter().filter(|r| matches(r, &q.criteria)).skip(q.offset).take(limit).cloned().collect())
            }
            Resolved::Sqlite(path, table) => sqlite_select(&path, &table, q, limit),
        }
    }

    /// Runs a read-only SQL statement against the SQLite source `name` (`shop` or `shop.table`).
    pub fn sql(&self, name: &str, sql: &str, params: &[String], limit: Option<usize>) -> Result<Vec<Row>, String> {
        let dir = self.dir()?;
        let file = name.split_once('.').map_or(name, |(f, _)| f);
        let path = SQLITE_EXTS.iter().map(|e| dir.join(format!("{file}.{e}"))).find(|p| p.is_file()).ok_or_else(|| format!("no SQLite data source '{file}' in mocks/data/"))?;
        safe_inside(dir, &path)?;
        let conn = open_ro(&path)?;
        let mut stmt = conn.prepare(sql).map_err(|e| format!("{name}: {e}"))?;
        if !stmt.readonly() {
            return Err("only read-only SQL is allowed in data sources".into());
        }
        let bound: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
        collect_rows(&mut stmt, bound.as_slice(), limit.unwrap_or(MAX_ROWS).min(MAX_ROWS))
    }

    fn resolve(&self, name: &str) -> Result<Resolved, String> {
        let dir = self.dir()?;
        let name = name.trim().trim_matches('/');
        if name.is_empty() {
            return Err("data source name is empty".into());
        }
        if name.split(['/', '\\']).any(|p| p == ".." || p.is_empty()) {
            return Err(format!("invalid data source name '{name}'"));
        }
        for ext in TABLE_EXTS {
            let p = dir.join(format!("{name}.{ext}"));
            if p.is_file() {
                safe_inside(dir, &p)?;
                return Ok(Resolved::Table(p));
            }
        }
        // `shop.orders` → shop.db table orders; `shop` → its only table.
        let (file, table) = match name.rsplit_once('.') {
            Some((f, t)) => (f, Some(t.to_string())),
            None => (name, None),
        };
        for candidate in [(file, table.clone()), (name, None)] {
            for ext in SQLITE_EXTS {
                let p = dir.join(format!("{}.{ext}", candidate.0));
                if !p.is_file() {
                    continue;
                }
                safe_inside(dir, &p)?;
                let table = match &candidate.1 {
                    Some(t) => t.clone(),
                    None => {
                        let tables = sqlite_tables(&p)?;
                        match tables.as_slice() {
                            [only] => only.clone(),
                            [] => return Err(format!("{} has no tables", self.project_rel(&p))),
                            many => return Err(format!("{} has several tables ({}); use '{}.<table>'", self.project_rel(&p), many.join(", "), candidate.0)),
                        }
                    }
                };
                return Ok(Resolved::Sqlite(p, table));
            }
        }
        Err(format!("no data source '{name}' in mocks/data/ (.csv, .json, .db, .sqlite)"))
    }

    fn table(&self, path: &Path) -> Result<Arc<Vec<Row>>, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", self.project_rel(path)))?;
        let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((t, len, rows)) = cache.get(path)
                && (*t, *len) == stamp
            {
                return Ok(rows.clone());
            }
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", self.project_rel(path)))?;
        let rows = match ext_of(path).as_deref() {
            Some("csv") => parse_csv(&text),
            Some("json") => parse_json_rows(&text).map_err(|e| format!("{}: {e}", self.project_rel(path)))?,
            _ => return Err(format!("{} is not a CSV or JSON file", self.project_rel(path))),
        };
        let rows = Arc::new(rows);
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf(), (stamp.0, stamp.1, rows.clone()));
        Ok(rows)
    }

    /// Evaluates a stub's `transformerParameters.dataSource` (template values already rendered
    /// by `render`). Returns the rows and the `notFoundStatus`.
    pub(crate) fn lookup_parameters(&self, spec: &Value, render: &dyn Fn(&str) -> Result<String, String>) -> Result<(Vec<Row>, Option<u16>), String> {
        let obj = spec.as_object().ok_or("transformerParameters.dataSource must be an object")?;
        let name = obj.get("name").map(json_text).filter(|n| !n.trim().is_empty()).ok_or("dataSource.name is required")?;
        let name = render(&name)?;
        let limit = obj.get("limit").and_then(Value::as_u64).map(|n| n as usize);
        let not_found = obj.get("notFoundStatus").and_then(|v| v.as_u64().or_else(|| json_text(v).trim().parse().ok())).map(|s| s as u16);
        if let Some(sql) = obj.get("sql").map(json_text).filter(|s| !s.trim().is_empty()) {
            let params = match obj.get("params") {
                Some(Value::Array(a)) => a.iter().map(|p| render(&json_text(p))).collect::<Result<Vec<_>, _>>()?,
                _ => Vec::new(),
            };
            return Ok((self.sql(&name, &sql, &params, limit)?, not_found));
        }
        let mut criteria = Vec::new();
        if let Some(Value::Object(w)) = obj.get("where") {
            for (k, v) in w {
                criteria.push((k.clone(), render(&json_text(v))?));
            }
        }
        let offset = obj.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        Ok((self.query(&name, &Query { criteria, limit, offset })?, not_found))
    }
}

enum Resolved {
    Table(PathBuf),
    Sqlite(PathBuf, String),
}

fn safe_inside(dir: &Path, path: &Path) -> Result<(), String> {
    // Names are checked for `..`; this also catches symlinks pointing elsewhere.
    match (dir.canonicalize(), path.canonicalize()) {
        (Ok(d), Ok(p)) if p.starts_with(&d) => Ok(()),
        _ => Err(format!("{} is outside mocks/data/", path.display())),
    }
}

fn ext_of(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).filter(|e| TABLE_EXTS.contains(&e.as_str()) || SQLITE_EXTS.contains(&e.as_str()))
}

fn name_of(dir: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(dir).unwrap_or(path).with_extension("");
    rel.to_string_lossy().replace('\\', "/")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if ext_of(&p).is_some() {
            out.push(p);
        }
    }
}

fn columns_of(rows: &[Row]) -> Vec<String> {
    let mut cols: Vec<String> = Vec::new();
    for r in rows.iter().take(50) {
        for k in r.keys() {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    cols
}

fn matches(row: &Row, criteria: &[(String, String)]) -> bool {
    criteria.iter().all(|(k, v)| row.get(k).is_some_and(|cell| json_text(cell) == *v))
}

/// CSV text cell → JSON: numbers and booleans when they round-trip exactly (so `007` and
/// `1e3` stay strings).
fn cell(s: &str) -> Value {
    if s == "true" || s == "false" {
        return Value::Bool(s == "true");
    }
    if let Ok(i) = s.parse::<i64>()
        && i.to_string() == s
    {
        return json!(i);
    }
    if let Ok(f) = s.parse::<f64>()
        && f.is_finite()
        && f.to_string() == s
    {
        return json!(f);
    }
    Value::String(s.to_string())
}

/// RFC 4180: header row, quoted fields, escaped quotes, CRLF; blank lines skipped.
pub(crate) fn parse_csv(text: &str) -> Vec<Row> {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        records.push(row);
    }
    let mut it = records.into_iter().filter(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    let Some(header) = it.next() else { return Vec::new() };
    let header: Vec<String> = header.into_iter().map(|h| h.trim().to_string()).collect();
    it.map(|rec| header.iter().enumerate().map(|(i, h)| (h.clone(), rec.get(i).map(|s| cell(s)).unwrap_or(Value::String(String::new())))).collect()).collect()
}

fn parse_json_rows(text: &str) -> Result<Vec<Row>, String> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("invalid JSON: {e}"))?;
    let arr = match v {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("rows") {
            Some(Value::Array(a)) => a,
            _ => return Err("expected an array of objects (or {\"rows\": [...]})".into()),
        },
        _ => return Err("expected an array of objects".into()),
    };
    arr.into_iter()
        .enumerate()
        .map(|(i, r)| match r {
            Value::Object(o) => Ok(o),
            _ => Err(format!("row {} is not an object", i + 1)),
        })
        .collect()
}

// ---------- SQLite ----------

fn open_ro(path: &Path) -> Result<rusqlite::Connection, String> {
    use rusqlite::OpenFlags;
    rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| format!("{}: {e}", path.display()))
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn sqlite_tables(path: &Path) -> Result<Vec<String>, String> {
    let conn = open_ro(path)?;
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(|e| e.to_string())?;
    let names = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
    names.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

fn sqlite_shape(path: &Path, table: &str) -> Result<(Vec<String>, usize), String> {
    let conn = open_ro(path)?;
    let stmt = conn.prepare(&format!("SELECT * FROM {} LIMIT 0", quote_ident(table))).map_err(|e| e.to_string())?;
    let cols = stmt.column_names().into_iter().map(str::to_string).collect();
    let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {}", quote_ident(table)), [], |r| r.get(0)).map_err(|e| e.to_string())?;
    Ok((cols, count.max(0) as usize))
}

fn sqlite_select(path: &Path, table: &str, q: &Query, limit: usize) -> Result<Vec<Row>, String> {
    let conn = open_ro(path)?;
    let mut sql = format!("SELECT * FROM {}", quote_ident(table));
    if !q.criteria.is_empty() {
        let conds: Vec<String> = q.criteria.iter().enumerate().map(|(i, (c, _))| format!("CAST({} AS TEXT) = ?{}", quote_ident(c), i + 1)).collect();
        sql.push_str(&format!(" WHERE {}", conds.join(" AND ")));
    }
    sql.push_str(&format!(" LIMIT {limit} OFFSET {}", q.offset));
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("{table}: {e}"))?;
    let values: Vec<&dyn rusqlite::ToSql> = q.criteria.iter().map(|(_, v)| v as &dyn rusqlite::ToSql).collect();
    collect_rows(&mut stmt, values.as_slice(), limit)
}

fn collect_rows(stmt: &mut rusqlite::Statement<'_>, params: &[&dyn rusqlite::ToSql], limit: usize) -> Result<Vec<Row>, String> {
    use rusqlite::types::ValueRef;
    let cols: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
    let mut rows = stmt.query(params).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(|e| e.to_string())? {
        if out.len() >= limit {
            break;
        }
        let mut m = Map::new();
        for (i, c) in cols.iter().enumerate() {
            let v = match r.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!(n),
                // Whole reals (a REAL column holding 20) read as 20, not 20.0.
                ValueRef::Real(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => json!(f as i64),
                ValueRef::Real(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
                ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
                ValueRef::Blob(b) => Value::String(crate::util::b64_encode(b)),
            };
            m.insert(c.clone(), v);
        }
        out.push(m);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, DataSources) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("mocks").join("data");
        std::fs::create_dir_all(data.join("eu")).unwrap();
        std::fs::write(data.join("users.csv"), "id,name,role,score\r\n1,Ann,admin,4.5\n2,\"Bob, Jr.\",user,007\n3,\"Cy \"\"C\"\"\",user,true\n\n").unwrap();
        std::fs::write(data.join("eu/products.json"), r#"[{"sku":"A1","price":3},{"sku":"B2","price":5}]"#).unwrap();
        std::fs::write(data.join("broken.json"), "{nope").unwrap();
        let conn = rusqlite::Connection::open(data.join("shop.db")).unwrap();
        conn.execute_batch("CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, total REAL, note TEXT); INSERT INTO orders VALUES (10, 1, 9.5, 'first'), (11, 1, 20, NULL), (12, 2, 1.25, 'x'); CREATE TABLE customers (id INTEGER, name TEXT);").unwrap();
        drop(conn);
        let ds = DataSources::for_project(dir.path());
        (dir, ds)
    }

    #[test]
    fn csv_parsing_and_typing() {
        let rows = parse_csv("a,b,c\n1,\"x,y\",\n\"q\"\"t\",2.50,false\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["a"], json!(1));
        assert_eq!(rows[0]["b"], json!("x,y"));
        assert_eq!(rows[0]["c"], json!(""));
        assert_eq!(rows[1]["a"], json!("q\"t"));
        assert_eq!(rows[1]["b"], json!("2.50"), "does not round-trip, stays text");
        assert_eq!(rows[1]["c"], json!(false));
    }

    #[test]
    fn lookups_across_formats() {
        let (_d, ds) = project();
        let q = |c: &[(&str, &str)]| Query { criteria: c.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), ..Default::default() };
        let ann = ds.query("users", &q(&[("id", "1")])).unwrap();
        assert_eq!(ann.len(), 1);
        assert_eq!(ann[0]["name"], json!("Ann"));
        assert_eq!(ann[0]["score"], json!(4.5));
        assert_eq!(ds.query("users", &q(&[("role", "user")])).unwrap().len(), 2);
        assert_eq!(ds.query("users", &q(&[("role", "user"), ("name", "Bob, Jr.")])).unwrap()[0]["score"], json!("007"));
        assert!(ds.query("users", &q(&[("id", "9")])).unwrap().is_empty());
        assert_eq!(ds.query("users", &Query { limit: Some(1), offset: 1, ..Default::default() }).unwrap()[0]["id"], json!(2));
        assert_eq!(ds.query("eu/products", &q(&[("sku", "B2")])).unwrap()[0]["price"], json!(5));
        let orders = ds.query("shop.orders", &q(&[("user_id", "1")])).unwrap();
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0]["total"], json!(9.5));
        assert_eq!(orders[1]["note"], Value::Null);
        assert!(ds.query("shop", &Query::default()).unwrap_err().contains("several tables"));
        assert!(ds.query("missing", &Query::default()).unwrap_err().contains("no data source"));
        assert!(ds.query("../secrets", &Query::default()).is_err());
        assert!(ds.query("broken", &Query::default()).unwrap_err().contains("invalid JSON"));
        let sql = ds.sql("shop", "SELECT id, total FROM orders WHERE total > ?1 ORDER BY id", &["5".into()], None).unwrap();
        assert_eq!(sql.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(), vec![json!(10), json!(11)]);
        assert!(ds.sql("shop", "DELETE FROM orders", &[], None).unwrap_err().contains("read-only"));
    }

    #[test]
    fn listing() {
        let (_d, ds) = project();
        let list = ds.list();
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["broken", "eu/products", "shop.customers", "shop.orders", "users"]);
        let users = list.iter().find(|s| s.name == "users").unwrap();
        assert_eq!((users.rows, users.columns.clone()), (3, vec!["id".to_string(), "name".into(), "role".into(), "score".into()]));
        assert_eq!(users.file, "mocks/data/users.csv");
        assert!(list.iter().find(|s| s.name == "broken").unwrap().error.is_some());
        assert_eq!(list.iter().find(|s| s.name == "shop.orders").unwrap().rows, 3);
    }

    #[test]
    fn cache_notices_changes() {
        let (d, ds) = project();
        assert_eq!(ds.query("users", &Query::default()).unwrap().len(), 3);
        std::fs::write(d.path().join("mocks/data/users.csv"), "id,name\n1,Zed,extra\n").unwrap();
        let rows = ds.query("users", &Query::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], json!("Zed"));
    }
}
