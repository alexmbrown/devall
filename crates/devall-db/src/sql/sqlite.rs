//! SQLite via rusqlite (bundled, same version as devall-history). Read-only sessions open the file
//! with `SQLITE_OPEN_READ_ONLY` (in-memory databases use `PRAGMA query_only`); switching modes
//! reopens the connection. Work runs on the blocking pool; cancel uses the interrupt handle.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, InterruptHandle, OpenFlags, ToSql};
use serde_json::Value;

use super::edit::{ParamStatement, quote_ident};
use super::types::{ColumnMeta, ExecOptions, StatementResult, ValueKind, bytes_value, float_value, int_value};
use super::{ColumnInfo, ExplainOutput, ForeignKeyInfo, IndexInfo, SchemaNode, SchemaTree, TableDetails, TableKind, TableNode, expect_one_error};
use crate::error::{DbError, Result};

pub struct LiteConn {
    conn: Arc<Mutex<Connection>>,
    interrupt: Arc<Mutex<Arc<InterruptHandle>>>,
    path: String,
    create: bool,
    pub(crate) in_tx: AtomicBool,
    pub(crate) version: String,
}

fn lite_err(e: rusqlite::Error) -> DbError {
    match e {
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::OperationInterrupted => DbError::Cancelled,
        rusqlite::Error::SqliteFailure(f, Some(msg)) => DbError::Query(format!("{msg} ({:?})", f.code)),
        other => DbError::Query(other.to_string()),
    }
}

fn open(path: &str, read_only: bool, create: bool) -> Result<Connection> {
    let conn = if path == ":memory:" {
        Connection::open_in_memory().map_err(lite_err)?
    } else {
        let exists = std::path::Path::new(path).exists();
        if !exists && (read_only || !create) {
            return Err(DbError::Connect(format!(
                "SQLite file not found: {path}{}",
                if read_only { "" } else { " (set the option create: \"true\" to create it)" }
            )));
        }
        let mut flags = OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        flags |= if read_only { OpenFlags::SQLITE_OPEN_READ_ONLY } else { OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE };
        Connection::open_with_flags(path, flags).map_err(|e| DbError::Connect(format!("{path}: {e}")))?
    };
    conn.busy_timeout(Duration::from_secs(5)).map_err(lite_err)?;
    // Belt and braces: in-memory databases can't be opened read-only.
    conn.pragma_update(None, "query_only", read_only).map_err(lite_err)?;
    Ok(conn)
}

struct Json<'a>(&'a Value);

impl ToSql for Json<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        use rusqlite::types::Value as V;
        Ok(ToSqlOutput::Owned(match self.0 {
            Value::Null => V::Null,
            Value::Bool(b) => V::Integer(i64::from(*b)),
            Value::Number(n) => match (n.as_i64(), n.as_f64()) {
                (Some(i), _) => V::Integer(i),
                (None, Some(f)) if n.is_f64() => V::Real(f),
                _ => V::Text(n.to_string()),
            },
            Value::String(s) => V::Text(s.clone()),
            other => V::Text(other.to_string()),
        }))
    }
}

fn kind_of(decl: Option<&str>) -> ValueKind {
    let d = decl.unwrap_or("").to_ascii_uppercase();
    if d.is_empty() {
        ValueKind::Other
    } else if d.contains("INT") {
        ValueKind::Int
    } else if d.contains("CHAR") || d.contains("CLOB") || d.contains("TEXT") {
        ValueKind::Text
    } else if d.contains("BLOB") {
        ValueKind::Bytes
    } else if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") || d.contains("NUMERIC") || d.contains("DECIMAL") {
        ValueKind::Float
    } else if d.contains("BOOL") {
        ValueKind::Bool
    } else if d.contains("DATETIME") || d.contains("TIMESTAMP") {
        ValueKind::Timestamp
    } else if d.contains("DATE") {
        ValueKind::Date
    } else if d.contains("JSON") {
        ValueKind::Json
    } else {
        ValueKind::Other
    }
}

fn to_json(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => int_value(i),
        ValueRef::Real(f) => float_value(f),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => bytes_value(b),
    }
}

impl LiteConn {
    pub async fn open(path: &str, read_only: bool, create: bool) -> Result<Self> {
        let p = path.to_string();
        let conn = tokio::task::spawn_blocking(move || open(&p, read_only, create)).await.map_err(|e| DbError::Connect(e.to_string()))??;
        let version: String = conn.query_row("select sqlite_version()", [], |r| r.get(0)).map_err(lite_err)?;
        let interrupt = Arc::new(conn.get_interrupt_handle());
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            interrupt: Arc::new(Mutex::new(interrupt)),
            path: path.to_string(),
            create,
            in_tx: AtomicBool::new(false),
            version: format!("SQLite {version}"),
        })
    }

    pub fn interrupt(&self) {
        self.interrupt.lock().unwrap().interrupt();
    }

    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static) -> Result<T> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut c = conn.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut c)
        })
        .await
        .map_err(|e| DbError::Query(format!("worker failed: {e}")))?
    }

    pub async fn set_read_only(&self, read_only: bool) -> Result<()> {
        if self.path == ":memory:" {
            return self.blocking(move |c| c.pragma_update(None, "query_only", read_only).map_err(lite_err)).await;
        }
        let (path, create) = (self.path.clone(), self.create);
        let fresh = tokio::task::spawn_blocking(move || open(&path, read_only, create)).await.map_err(|e| DbError::Connect(e.to_string()))??;
        let handle = Arc::new(fresh.get_interrupt_handle());
        *self.conn.lock().unwrap_or_else(|p| p.into_inner()) = fresh;
        *self.interrupt.lock().unwrap() = handle;
        self.in_tx.store(false, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub async fn run_statement(&self, sql: &str, opts: ExecOptions) -> Result<StatementResult> {
        let sql = sql.to_string();
        self.blocking(move |c| {
            let mut stmt = c.prepare(&sql).map_err(lite_err)?;
            let columns: Vec<ColumnMeta> = stmt
                .columns()
                .iter()
                .map(|col| ColumnMeta { name: col.name().to_string(), type_name: col.decl_type().unwrap_or("").to_string(), kind: kind_of(col.decl_type()) })
                .collect();
            let mut out = StatementResult::default();
            if columns.is_empty() {
                let n = stmt.execute([]).map_err(lite_err)?;
                out.rows_affected = Some(n as u64);
                return Ok(out);
            }
            let n = columns.len();
            let mut rows = stmt.query([]).map_err(lite_err)?;
            let mut skipped = 0;
            while let Some(row) = rows.next().map_err(lite_err)? {
                if skipped < opts.offset {
                    skipped += 1;
                    continue;
                }
                if out.rows.len() >= opts.limit {
                    out.more = true;
                    break;
                }
                out.rows.push((0..n).map(|i| row.get_ref(i).map(to_json).unwrap_or(Value::Null)).collect());
            }
            out.columns = columns;
            Ok(out)
        })
        .await
    }

    pub async fn schema(&self) -> Result<SchemaTree> {
        self.blocking(|c| {
            let mut dbs: Vec<String> = Vec::new();
            {
                let mut st = c.prepare("SELECT name FROM pragma_database_list ORDER BY seq").map_err(lite_err)?;
                let mut rows = st.query([]).map_err(lite_err)?;
                while let Some(r) = rows.next().map_err(lite_err)? {
                    dbs.push(r.get(0).map_err(lite_err)?);
                }
            }
            let mut schemas = Vec::new();
            for db in dbs {
                let master = if db == "temp" { "sqlite_temp_master".to_string() } else { format!("{}.sqlite_master", quote_ident(&db, super::Dialect::Sqlite)) };
                let mut tables = Vec::new();
                let names: Vec<(String, String)> = {
                    let mut st = c
                        .prepare(&format!("SELECT name, type FROM {master} WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name"))
                        .map_err(lite_err)?;
                    let mut rows = st.query([]).map_err(lite_err)?;
                    let mut v = Vec::new();
                    while let Some(r) = rows.next().map_err(lite_err)? {
                        v.push((r.get(0).map_err(lite_err)?, r.get(1).map_err(lite_err)?));
                    }
                    v
                };
                for (name, ty) in names {
                    let columns = table_columns(c, &db, &name)?;
                    tables.push(TableNode { name, kind: if ty == "view" { TableKind::View } else { TableKind::Table }, columns, rows_estimate: None });
                }
                if db == "temp" && tables.is_empty() {
                    continue;
                }
                schemas.push(SchemaNode { name: db, tables });
            }
            Ok(SchemaTree { schemas, default_schema: Some("main".into()) })
        })
        .await
    }

    pub async fn table_details(&self, schema: &str, table: &str) -> Result<TableDetails> {
        let (schema, table) = (schema.to_string(), table.to_string());
        self.blocking(move |c| {
            let columns = table_columns(c, &schema, &table)?;
            if columns.is_empty() {
                return Err(DbError::NotFound(format!("table {schema}.{table}")));
            }
            let master = if schema == "temp" { "sqlite_temp_master".to_string() } else { format!("{}.sqlite_master", quote_ident(&schema, super::Dialect::Sqlite)) };
            let (kind, ddl): (String, Option<String>) = c
                .query_row(&format!("SELECT type, sql FROM {master} WHERE name = ?1"), [&table], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(lite_err)?;
            let mut pk: Vec<(i64, String)> = Vec::new();
            {
                let mut st = c.prepare("SELECT name, pk FROM pragma_table_info(?1, ?2) WHERE pk > 0").map_err(lite_err)?;
                let mut rows = st.query([&table, &schema]).map_err(lite_err)?;
                while let Some(r) = rows.next().map_err(lite_err)? {
                    pk.push((r.get(1).map_err(lite_err)?, r.get(0).map_err(lite_err)?));
                }
            }
            pk.sort();
            let mut indexes = Vec::new();
            {
                let list: Vec<(String, bool, String)> = {
                    let mut st = c.prepare("SELECT name, \"unique\", origin FROM pragma_index_list(?1, ?2) ORDER BY name").map_err(lite_err)?;
                    let mut rows = st.query([&table, &schema]).map_err(lite_err)?;
                    let mut v = Vec::new();
                    while let Some(r) = rows.next().map_err(lite_err)? {
                        v.push((r.get(0).map_err(lite_err)?, r.get::<_, i64>(1).map_err(lite_err)? != 0, r.get(2).map_err(lite_err)?));
                    }
                    v
                };
                for (name, unique, origin) in list {
                    let mut st = c.prepare("SELECT name FROM pragma_index_info(?1, ?2) ORDER BY seqno").map_err(lite_err)?;
                    let mut rows = st.query([&name, &schema]).map_err(lite_err)?;
                    let mut cols = Vec::new();
                    while let Some(r) = rows.next().map_err(lite_err)? {
                        cols.push(r.get::<_, Option<String>>(0).map_err(lite_err)?.unwrap_or_else(|| "<expr>".into()));
                    }
                    let definition: Option<String> = c.query_row(&format!("SELECT sql FROM {master} WHERE type = 'index' AND name = ?1"), [&name], |r| r.get(0)).ok().flatten();
                    indexes.push(IndexInfo { name, columns: cols, unique, primary: origin == "pk", method: None, definition });
                }
            }
            let mut fks: Vec<ForeignKeyInfo> = Vec::new();
            {
                let mut st = c.prepare("SELECT id, \"table\", \"from\", \"to\", on_update, on_delete FROM pragma_foreign_key_list(?1, ?2) ORDER BY id, seq").map_err(lite_err)?;
                let mut rows = st.query([&table, &schema]).map_err(lite_err)?;
                let mut last_id = -1i64;
                while let Some(r) = rows.next().map_err(lite_err)? {
                    let id: i64 = r.get(0).map_err(lite_err)?;
                    let from: String = r.get(2).map_err(lite_err)?;
                    let to: Option<String> = r.get(3).map_err(lite_err)?;
                    if id != last_id {
                        last_id = id;
                        fks.push(ForeignKeyInfo {
                            name: format!("fk_{table}_{id}"),
                            columns: Vec::new(),
                            ref_schema: None,
                            ref_table: r.get(1).map_err(lite_err)?,
                            ref_columns: Vec::new(),
                            on_update: r.get(4).map_err(lite_err)?,
                            on_delete: r.get(5).map_err(lite_err)?,
                        });
                    }
                    if let Some(fk) = fks.last_mut() {
                        fk.columns.push(from);
                        fk.ref_columns.push(to.unwrap_or_default());
                    }
                }
            }
            Ok(TableDetails {
                schema: Some(schema),
                name: table,
                kind: if kind == "view" { TableKind::View } else { TableKind::Table },
                columns,
                primary_key: pk.into_iter().map(|(_, n)| n).collect(),
                indexes,
                foreign_keys: fks,
                ddl,
            })
        })
        .await
    }

    pub async fn explain(&self, sql: &str) -> Result<ExplainOutput> {
        let sql = format!("EXPLAIN QUERY PLAN {sql}");
        self.blocking(move |c| {
            let mut st = c.prepare(&sql).map_err(lite_err)?;
            let mut rows = st.query([]).map_err(lite_err)?;
            let mut nodes: Vec<(i64, i64, String)> = Vec::new();
            while let Some(r) = rows.next().map_err(lite_err)? {
                nodes.push((r.get(0).map_err(lite_err)?, r.get(1).map_err(lite_err)?, r.get(3).map_err(lite_err)?));
            }
            fn children(nodes: &[(i64, i64, String)], parent: i64) -> Vec<Value> {
                nodes
                    .iter()
                    .filter(|(_, p, _)| *p == parent)
                    .map(|(id, _, detail)| serde_json::json!({ "id": id, "detail": detail, "children": children(nodes, *id) }))
                    .collect()
            }
            let text = nodes.iter().map(|(_, _, d)| d.as_str()).collect::<Vec<_>>().join("\n");
            Ok(ExplainOutput { format: "sqlite".into(), plan: Value::Array(children(&nodes, 0)), text: Some(text), rolled_back: false })
        })
        .await
    }

    pub async fn exec_params(&self, stmts: &[ParamStatement]) -> Result<Vec<u64>> {
        let stmts = stmts.to_vec();
        self.blocking(move |c| {
            let tx = c.transaction().map_err(lite_err)?;
            let mut out = Vec::with_capacity(stmts.len());
            for s in &stmts {
                let params: Vec<Json<'_>> = s.params.iter().map(Json).collect();
                let n = tx.execute(&s.sql, rusqlite::params_from_iter(params.iter())).map_err(|e| DbError::Query(format!("{}\n→ {}", s.preview, lite_err(e))))? as u64;
                if s.expect_one && n != 1 {
                    return Err(expect_one_error(n, &s.preview));
                }
                out.push(n);
            }
            tx.commit().map_err(lite_err)?;
            Ok(out)
        })
        .await
    }
}

fn table_columns(c: &Connection, schema: &str, table: &str) -> Result<Vec<ColumnInfo>> {
    let mut st = c.prepare("SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1, ?2) ORDER BY cid").map_err(lite_err)?;
    let mut rows = st.query([table, schema]).map_err(lite_err)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(lite_err)? {
        out.push(ColumnInfo {
            name: r.get(0).map_err(lite_err)?,
            data_type: r.get::<_, Option<String>>(1).map_err(lite_err)?.unwrap_or_default(),
            nullable: r.get::<_, i64>(2).map_err(lite_err)? == 0,
            default: r.get(3).map_err(lite_err)?,
            primary_key: r.get::<_, i64>(4).map_err(lite_err)? > 0,
        });
    }
    Ok(out)
}
