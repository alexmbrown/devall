//! MySQL / MariaDB via mysql_async (rustls, aws-lc). Statements use the text protocol (works for
//! every statement type); values are typed from column metadata. Read-only sessions use
//! `SET SESSION TRANSACTION READ ONLY` (every following transaction, implicit or explicit, is
//! read-only). Cancel = `KILL QUERY <id>` from a second connection.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, Params, Row, SslOpts, Value as MyValue};
use serde_json::Value;

use super::edit::{ParamStatement, qualified};
use super::types::{ColumnMeta, ExecOptions, StatementResult, ValueKind, bytes_value, float_value, int_value, text_to_value, uint_value};
use super::{ColumnInfo, Dialect, ExplainOutput, ForeignKeyInfo, IndexInfo, SchemaNode, SchemaTree, TableDetails, TableKind, TableNode, expect_one_error};
use crate::error::{DbError, Result};
use crate::model::TlsMode;
use crate::resolve::ResolvedConnection;

pub struct MyConn {
    conn: tokio::sync::Mutex<Conn>,
    opts: Opts,
    conn_id: AtomicU32,
    pub(crate) in_tx: AtomicBool,
    pub(crate) version: String,
    mariadb: bool,
}

pub fn my_err(e: mysql_async::Error) -> DbError {
    match e {
        mysql_async::Error::Server(s) if s.code == 1317 => DbError::Cancelled,
        mysql_async::Error::Server(s) => DbError::Query(format!("ERROR {} ({}): {}", s.code, s.state, s.message)),
        other => DbError::Query(other.to_string()),
    }
}

const SYSTEM_SCHEMAS: &str = "'mysql','information_schema','performance_schema','sys'";

fn kind_of(c: &mysql_async::Column) -> ValueKind {
    let binary = c.character_set() == 63;
    match c.column_type() {
        ColumnType::MYSQL_TYPE_TINY
        | ColumnType::MYSQL_TYPE_SHORT
        | ColumnType::MYSQL_TYPE_LONG
        | ColumnType::MYSQL_TYPE_INT24
        | ColumnType::MYSQL_TYPE_LONGLONG
        | ColumnType::MYSQL_TYPE_YEAR
        | ColumnType::MYSQL_TYPE_BIT => ValueKind::Int,
        ColumnType::MYSQL_TYPE_FLOAT | ColumnType::MYSQL_TYPE_DOUBLE => ValueKind::Float,
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => ValueKind::Decimal,
        ColumnType::MYSQL_TYPE_JSON => ValueKind::Json,
        ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => ValueKind::Date,
        ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => ValueKind::Time,
        ColumnType::MYSQL_TYPE_DATETIME | ColumnType::MYSQL_TYPE_DATETIME2 | ColumnType::MYSQL_TYPE_TIMESTAMP | ColumnType::MYSQL_TYPE_TIMESTAMP2 => {
            ValueKind::Timestamp
        }
        ColumnType::MYSQL_TYPE_GEOMETRY | ColumnType::MYSQL_TYPE_VECTOR => ValueKind::Bytes,
        ColumnType::MYSQL_TYPE_TINY_BLOB
        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
        | ColumnType::MYSQL_TYPE_LONG_BLOB
        | ColumnType::MYSQL_TYPE_BLOB
        | ColumnType::MYSQL_TYPE_VAR_STRING
        | ColumnType::MYSQL_TYPE_STRING
        | ColumnType::MYSQL_TYPE_VARCHAR => {
            if binary {
                ValueKind::Bytes
            } else {
                ValueKind::Text
            }
        }
        ColumnType::MYSQL_TYPE_ENUM | ColumnType::MYSQL_TYPE_SET => ValueKind::Text,
        _ => ValueKind::Other,
    }
}

fn type_name(c: &mysql_async::Column) -> String {
    let binary = c.character_set() == 63;
    let unsigned = c.flags().contains(ColumnFlags::UNSIGNED_FLAG);
    let base = match c.column_type() {
        ColumnType::MYSQL_TYPE_TINY => "TINYINT",
        ColumnType::MYSQL_TYPE_SHORT => "SMALLINT",
        ColumnType::MYSQL_TYPE_LONG => "INT",
        ColumnType::MYSQL_TYPE_INT24 => "MEDIUMINT",
        ColumnType::MYSQL_TYPE_LONGLONG => "BIGINT",
        ColumnType::MYSQL_TYPE_YEAR => "YEAR",
        ColumnType::MYSQL_TYPE_BIT => "BIT",
        ColumnType::MYSQL_TYPE_FLOAT => "FLOAT",
        ColumnType::MYSQL_TYPE_DOUBLE => "DOUBLE",
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => "DECIMAL",
        ColumnType::MYSQL_TYPE_JSON => "JSON",
        ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => "DATE",
        ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => "TIME",
        ColumnType::MYSQL_TYPE_DATETIME | ColumnType::MYSQL_TYPE_DATETIME2 => "DATETIME",
        ColumnType::MYSQL_TYPE_TIMESTAMP | ColumnType::MYSQL_TYPE_TIMESTAMP2 => "TIMESTAMP",
        ColumnType::MYSQL_TYPE_GEOMETRY => "GEOMETRY",
        ColumnType::MYSQL_TYPE_VECTOR => "VECTOR",
        ColumnType::MYSQL_TYPE_ENUM => "ENUM",
        ColumnType::MYSQL_TYPE_SET => "SET",
        ColumnType::MYSQL_TYPE_TINY_BLOB | ColumnType::MYSQL_TYPE_MEDIUM_BLOB | ColumnType::MYSQL_TYPE_LONG_BLOB | ColumnType::MYSQL_TYPE_BLOB => {
            if binary {
                "BLOB"
            } else {
                "TEXT"
            }
        }
        ColumnType::MYSQL_TYPE_VAR_STRING | ColumnType::MYSQL_TYPE_VARCHAR => {
            if binary {
                "VARBINARY"
            } else {
                "VARCHAR"
            }
        }
        ColumnType::MYSQL_TYPE_STRING => {
            if binary {
                "BINARY"
            } else {
                "CHAR"
            }
        }
        ColumnType::MYSQL_TYPE_NULL => "NULL",
        _ => "UNKNOWN",
    };
    if unsigned { format!("{base} UNSIGNED") } else { base.to_string() }
}

fn to_json(v: &MyValue, kind: ValueKind, bit: bool) -> Value {
    match v {
        MyValue::NULL => Value::Null,
        MyValue::Bytes(b) => {
            if bit {
                // BIT(n) arrives as big-endian bytes.
                return if b.len() <= 8 { uint_value(b.iter().fold(0u64, |acc, x| (acc << 8) | u64::from(*x))) } else { bytes_value(b) };
            }
            if kind == ValueKind::Bytes {
                return bytes_value(b);
            }
            match std::str::from_utf8(b) {
                Ok(s) => text_to_value(s, kind),
                Err(_) => bytes_value(b),
            }
        }
        MyValue::Int(i) => int_value(*i),
        MyValue::UInt(u) => uint_value(*u),
        MyValue::Float(f) => float_value(f64::from(*f)),
        MyValue::Double(f) => float_value(*f),
        MyValue::Date(y, mo, d, h, mi, s, us) => {
            Value::String(if *h == 0 && *mi == 0 && *s == 0 && *us == 0 && kind == ValueKind::Date {
                format!("{y:04}-{mo:02}-{d:02}")
            } else if *us > 0 {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}")
            } else {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
            })
        }
        MyValue::Time(neg, days, h, mi, s, us) => {
            let hours = u32::from(*h) + days * 24;
            let sign = if *neg { "-" } else { "" };
            Value::String(if *us > 0 { format!("{sign}{hours:02}:{mi:02}:{s:02}.{us:06}") } else { format!("{sign}{hours:02}:{mi:02}:{s:02}") })
        }
    }
}

fn my_str(v: &MyValue) -> Option<String> {
    match to_json(v, ValueKind::Text, false) {
        Value::Null => None,
        Value::String(s) => Some(s),
        other => Some(other.to_string()),
    }
}

fn param(v: &Value) -> MyValue {
    match v {
        Value::Null => MyValue::NULL,
        Value::Bool(b) => MyValue::Int(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                MyValue::Int(i)
            } else if let Some(u) = n.as_u64() {
                MyValue::UInt(u)
            } else {
                MyValue::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => MyValue::Bytes(s.clone().into_bytes()),
        other => MyValue::Bytes(other.to_string().into_bytes()),
    }
}

fn base_opts(rc: &ResolvedConnection, host: &str, port: u16) -> OptsBuilder {
    OptsBuilder::default()
        .ip_or_hostname(host)
        .tcp_port(port)
        .user(rc.user.clone().or_else(|| Some("root".into())))
        .pass(rc.password.clone())
        .db_name(rc.database.clone())
        .prefer_socket(false)
        .stmt_cache_size(Some(16))
        .tcp_nodelay(true)
}

impl MyConn {
    pub async fn connect(rc: &ResolvedConnection, host: &str, port: u16, read_only: bool) -> Result<Self> {
        let timeout = rc.connect_timeout();
        let tunneled = host != rc.host;
        let tls_opts = |verify: bool| -> Result<SslOpts> {
            let mut s = SslOpts::default();
            if verify {
                if let Some(ca) = &rc.ca_cert {
                    s = s.with_root_certs(vec![ca.clone().into()]);
                }
                if tunneled {
                    s = s.with_danger_tls_hostname_override(Some(rc.host.clone()));
                }
            } else {
                s = s.with_danger_accept_invalid_certs(true).with_danger_skip_domain_validation(true);
            }
            Ok(s)
        };
        let attempts: Vec<Option<SslOpts>> = match rc.tls {
            TlsMode::Disable => vec![None],
            TlsMode::Prefer => vec![Some(tls_opts(false)?), None],
            TlsMode::Require => vec![Some(tls_opts(false)?)],
            TlsMode::VerifyFull => vec![Some(tls_opts(true)?)],
        };
        let mut last = None;
        let mut conn = None;
        let mut used_opts = None;
        for ssl in attempts {
            let opts: Opts = base_opts(rc, host, port).ssl_opts(ssl).into();
            match tokio::time::timeout(timeout, Conn::new(opts.clone())).await {
                Ok(Ok(c)) => {
                    conn = Some(c);
                    used_opts = Some(opts);
                    break;
                }
                Ok(Err(e)) => last = Some(DbError::Connect(my_err(e).to_string())),
                Err(_) => last = Some(DbError::Timeout(format!("{timeout:?} connecting to {}:{}", rc.host, rc.port))),
            }
        }
        let (Some(mut c), Some(opts)) = (conn, used_opts) else { return Err(last.unwrap_or_else(|| DbError::Connect("no connection attempt".into()))) };
        let version: Option<String> = c.query_first("SELECT VERSION()").await.map_err(my_err)?;
        let version = version.unwrap_or_default();
        let mariadb = version.to_ascii_lowercase().contains("mariadb");
        if let Some(ms) = rc.option("statementTimeout") {
            let ms: u64 = ms.trim().parse().map_err(|_| DbError::Invalid(format!("statementTimeout must be milliseconds (got '{ms}')")))?;
            let q = if mariadb { format!("SET SESSION max_statement_time = {}", ms as f64 / 1000.0) } else { format!("SET SESSION max_execution_time = {ms}") };
            c.query_drop(q).await.map_err(my_err)?;
        }
        let id = c.id();
        let me = MyConn {
            conn: tokio::sync::Mutex::new(c),
            opts,
            conn_id: AtomicU32::new(id),
            in_tx: AtomicBool::new(false),
            version: format!("{} {}", if mariadb { "MariaDB" } else { "MySQL" }, version.split('-').next().unwrap_or(&version)),
            mariadb,
        };
        me.set_read_only(read_only).await?;
        Ok(me)
    }

    pub async fn set_read_only(&self, read_only: bool) -> Result<()> {
        let mut c = self.conn.lock().await;
        let q = if read_only { "SET SESSION TRANSACTION READ ONLY" } else { "SET SESSION TRANSACTION READ WRITE" };
        c.query_drop(q).await.map_err(my_err)
    }

    pub async fn kill_query(&self) -> Result<()> {
        let id = self.conn_id.load(Ordering::Relaxed);
        let mut c = tokio::time::timeout(std::time::Duration::from_secs(10), Conn::new(self.opts.clone()))
            .await
            .map_err(|_| DbError::Timeout("10s opening a connection to cancel the query".into()))?
            .map_err(my_err)?;
        let r = c.query_drop(format!("KILL QUERY {id}")).await.map_err(my_err);
        let _ = c.disconnect().await;
        r
    }

    pub async fn run_statement(&self, sql: &str, opts: ExecOptions, stop_early: bool) -> Result<StatementResult> {
        let mut c = self.conn.lock().await;
        let mut out = StatementResult::default();
        let mut result = c.query_iter(sql).await.map_err(my_err)?;
        let cols = result.columns().unwrap_or_else(|| Vec::new().into());
        let kinds: Vec<(ValueKind, bool)> = cols.iter().map(|col| (kind_of(col), col.column_type() == ColumnType::MYSQL_TYPE_BIT)).collect();
        out.columns = cols.iter().map(|col| ColumnMeta { name: col.name_str().into_owned(), type_name: type_name(col), kind: kind_of(col) }).collect();
        let mut skipped = 0usize;
        let mut killed = false;
        if !cols.is_empty() {
            loop {
                let row: Option<Row> = match result.next().await {
                    Ok(r) => r,
                    Err(e) => {
                        if killed {
                            break;
                        }
                        return Err(my_err(e));
                    }
                };
                let Some(row) = row else { break };
                if killed {
                    continue;
                }
                if skipped < opts.offset {
                    skipped += 1;
                    continue;
                }
                if out.rows.len() >= opts.limit {
                    out.more = true;
                    if stop_early {
                        killed = true;
                        let _ = self.kill_query().await;
                    }
                    continue;
                }
                let vals = row.unwrap();
                out.rows.push(vals.iter().zip(&kinds).map(|(v, (k, bit))| to_json(v, *k, *bit)).collect());
            }
        }
        if !killed && cols.is_empty() {
            out.rows_affected = Some(result.affected_rows());
        }
        // Further result sets (e.g. from CALL) are drained.
        let drained = result.drop_result().await;
        if let Err(e) = drained
            && !killed
        {
            return Err(my_err(e));
        }
        let warnings = c.get_warnings();
        if warnings > 0 {
            let rows: Vec<(String, u32, String)> = c.query("SHOW WARNINGS LIMIT 50").await.unwrap_or_default();
            out.notices = rows.into_iter().map(|(level, code, msg)| format!("{level} {code}: {msg}")).collect();
        }
        Ok(out)
    }

    async fn rows(&self, sql: &str, params: Vec<MyValue>) -> Result<Vec<Vec<Option<String>>>> {
        let mut c = self.conn.lock().await;
        let rows: Vec<Row> = if params.is_empty() { c.query(sql).await.map_err(my_err)? } else { c.exec(sql, Params::Positional(params)).await.map_err(my_err)? };
        Ok(rows.into_iter().map(|r| r.unwrap().iter().map(my_str).collect()).collect())
    }

    pub async fn schema(&self) -> Result<SchemaTree> {
        let schemas = self.rows(&format!("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN ({SYSTEM_SCHEMAS}) ORDER BY 1"), vec![]).await?;
        let tables = self
            .rows(
                &format!(
                    "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE, TABLE_ROWS FROM information_schema.TABLES WHERE TABLE_SCHEMA NOT IN ({SYSTEM_SCHEMAS}) ORDER BY 1, 2"
                ),
                vec![],
            )
            .await?;
        let columns = self
            .rows(
                &format!(
                    "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY FROM information_schema.COLUMNS \
                     WHERE TABLE_SCHEMA NOT IN ({SYSTEM_SCHEMAS}) ORDER BY TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION"
                ),
                vec![],
            )
            .await?;
        let current = self.rows("SELECT DATABASE()", vec![]).await?.into_iter().next().and_then(|r| r.into_iter().next().flatten());
        let mut by_table: std::collections::HashMap<(String, String), Vec<ColumnInfo>> = std::collections::HashMap::new();
        for r in columns {
            by_table.entry((r[0].clone().unwrap_or_default(), r[1].clone().unwrap_or_default())).or_default().push(column_info(&r[2..]));
        }
        let mut tree = SchemaTree { schemas: schemas.into_iter().map(|r| SchemaNode { name: r[0].clone().unwrap_or_default(), tables: Vec::new() }).collect(), default_schema: current };
        for r in tables {
            let (schema, name) = (r[0].clone().unwrap_or_default(), r[1].clone().unwrap_or_default());
            let kind = if r[2].as_deref().unwrap_or("").contains("VIEW") { TableKind::View } else { TableKind::Table };
            let node = TableNode {
                columns: by_table.remove(&(schema.clone(), name.clone())).unwrap_or_default(),
                name,
                kind,
                rows_estimate: r[3].as_deref().and_then(|v| v.parse().ok()),
            };
            if let Some(s) = tree.schemas.iter_mut().find(|s| s.name == schema) {
                s.tables.push(node);
            }
        }
        Ok(tree)
    }

    pub async fn table_details(&self, schema: Option<&str>, table: &str) -> Result<TableDetails> {
        let schema = match schema {
            Some(s) => s.to_string(),
            None => self.rows("SELECT DATABASE()", vec![]).await?.into_iter().next().and_then(|r| r.into_iter().next().flatten()).ok_or_else(|| DbError::Invalid("no database selected".into()))?,
        };
        let p = || vec![MyValue::from(schema.as_str()), MyValue::from(table)];
        let kind_rows = self.rows("SELECT TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?", p()).await?;
        let Some(kr) = kind_rows.first() else { return Err(DbError::NotFound(format!("table {schema}.{table}"))) };
        let kind = if kr[0].as_deref().unwrap_or("").contains("VIEW") { TableKind::View } else { TableKind::Table };
        let columns: Vec<ColumnInfo> = self
            .rows(
                "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
                p(),
            )
            .await?
            .iter()
            .map(|r| column_info(r))
            .collect();
        const STATS: &str = "SELECT INDEX_NAME, NON_UNIQUE, COLUMN_NAME, INDEX_TYPE, {expr} FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX";
        let stats = match self.rows(&STATS.replace("{expr}", "EXPRESSION"), p()).await {
            Ok(s) => s,
            // MariaDB and MySQL 5.7 have no EXPRESSION column.
            Err(_) => self.rows(&STATS.replace("{expr}", "NULL"), p()).await?,
        };
        let mut indexes: Vec<IndexInfo> = Vec::new();
        for r in stats {
            let name = r[0].clone().unwrap_or_default();
            let col = r[2].clone().or_else(|| r[4].clone().map(|e| format!("({e})"))).unwrap_or_default();
            match indexes.last_mut() {
                Some(i) if i.name == name => i.columns.push(col),
                _ => indexes.push(IndexInfo {
                    primary: name == "PRIMARY",
                    unique: r[1].as_deref() == Some("0"),
                    method: r[3].clone(),
                    definition: None,
                    columns: vec![col],
                    name,
                }),
            }
        }
        let fk_rows = self
            .rows(
                "SELECT k.CONSTRAINT_NAME, k.COLUMN_NAME, k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME, r.UPDATE_RULE, r.DELETE_RULE \
                 FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
                 ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME AND r.TABLE_NAME = k.TABLE_NAME \
                 WHERE k.TABLE_SCHEMA = ? AND k.TABLE_NAME = ? AND k.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
                p(),
            )
            .await?;
        let mut foreign_keys: Vec<ForeignKeyInfo> = Vec::new();
        for r in fk_rows {
            let name = r[0].clone().unwrap_or_default();
            let (col, rcol) = (r[1].clone().unwrap_or_default(), r[4].clone().unwrap_or_default());
            match foreign_keys.last_mut() {
                Some(f) if f.name == name => {
                    f.columns.push(col);
                    f.ref_columns.push(rcol);
                }
                _ => foreign_keys.push(ForeignKeyInfo {
                    name,
                    columns: vec![col],
                    ref_schema: r[2].clone(),
                    ref_table: r[3].clone().unwrap_or_default(),
                    ref_columns: vec![rcol],
                    on_update: r[5].clone().filter(|a| a != "NO ACTION" && a != "RESTRICT"),
                    on_delete: r[6].clone().filter(|a| a != "NO ACTION" && a != "RESTRICT"),
                }),
            }
        }
        let qt = qualified(Some(&schema), table, Dialect::Mysql);
        let ddl_rows = self.rows(&format!("SHOW CREATE {} {qt}", if kind == TableKind::View { "VIEW" } else { "TABLE" }), vec![]).await.unwrap_or_default();
        let ddl = ddl_rows.first().and_then(|r| r.get(1).cloned().flatten());
        let primary_key = indexes.iter().find(|i| i.primary).map(|i| i.columns.clone()).unwrap_or_default();
        Ok(TableDetails { schema: Some(schema.clone()), name: table.to_string(), kind, columns, primary_key, indexes, foreign_keys, ddl })
    }

    pub async fn explain(&self, sql: &str, analyze: bool, in_tx: bool) -> Result<ExplainOutput> {
        let mut c = self.conn.lock().await;
        let wrap = analyze && !in_tx;
        if wrap {
            c.query_drop("START TRANSACTION").await.map_err(my_err)?;
        }
        let q = match (analyze, self.mariadb) {
            (false, _) => format!("EXPLAIN FORMAT=JSON {sql}"),
            (true, true) => format!("ANALYZE FORMAT=JSON {sql}"),
            (true, false) => format!("EXPLAIN ANALYZE {sql}"),
        };
        let res: std::result::Result<Vec<Row>, _> = c.query(q).await;
        if wrap {
            let _ = c.query_drop("ROLLBACK").await;
        }
        let rows = res.map_err(my_err)?;
        let text = rows.first().and_then(|r| r.as_ref(0)).and_then(my_str).unwrap_or_default();
        Ok(match serde_json::from_str::<Value>(&text) {
            Ok(plan) => ExplainOutput { format: "mysql".into(), plan, text: None, rolled_back: wrap },
            Err(_) => ExplainOutput { format: "text".into(), plan: Value::Null, text: Some(text), rolled_back: wrap },
        })
    }

    pub async fn exec_params(&self, stmts: &[ParamStatement]) -> Result<Vec<u64>> {
        let mut c = self.conn.lock().await;
        c.query_drop("START TRANSACTION").await.map_err(my_err)?;
        let mut out = Vec::with_capacity(stmts.len());
        for s in stmts {
            let params = Params::Positional(s.params.iter().map(param).collect());
            match c.exec_drop(s.sql.as_str(), params).await {
                Ok(()) => {
                    let n = c.affected_rows();
                    if s.expect_one && n != 1 {
                        let _ = c.query_drop("ROLLBACK").await;
                        return Err(expect_one_error(n, &s.preview));
                    }
                    out.push(n);
                }
                Err(e) => {
                    let _ = c.query_drop("ROLLBACK").await;
                    return Err(DbError::Query(format!("{}\n→ {}", s.preview, my_err(e))));
                }
            }
        }
        c.query_drop("COMMIT").await.map_err(my_err)?;
        Ok(out)
    }
}

fn column_info(r: &[Option<String>]) -> ColumnInfo {
    ColumnInfo {
        name: r[0].clone().unwrap_or_default(),
        data_type: r[1].clone().unwrap_or_default(),
        nullable: r[2].as_deref() == Some("YES"),
        default: r[3].clone(),
        primary_key: r[4].as_deref() == Some("PRI"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::edit::quote_ident;

    #[test]
    fn values() {
        assert_eq!(to_json(&MyValue::Bytes(b"42".to_vec()), ValueKind::Int, false), serde_json::json!(42));
        assert_eq!(to_json(&MyValue::Bytes(b"18446744073709551615".to_vec()), ValueKind::Int, false), serde_json::json!("18446744073709551615"));
        assert_eq!(to_json(&MyValue::Bytes(vec![0x01, 0x00]), ValueKind::Int, true), serde_json::json!(256));
        assert_eq!(to_json(&MyValue::Bytes(vec![0xff, 0xfe]), ValueKind::Text, false), serde_json::json!("//4="));
        assert_eq!(to_json(&MyValue::Bytes(b"12.50".to_vec()), ValueKind::Decimal, false), serde_json::json!("12.50"));
        assert_eq!(to_json(&MyValue::Date(2024, 5, 1, 10, 20, 30, 0), ValueKind::Timestamp, false), serde_json::json!("2024-05-01 10:20:30"));
        assert_eq!(to_json(&MyValue::Time(true, 1, 2, 3, 4, 0), ValueKind::Time, false), serde_json::json!("-26:03:04"));
        assert_eq!(param(&serde_json::json!(true)), MyValue::Int(1));
        assert_eq!(param(&serde_json::json!({"a": 1})), MyValue::Bytes(b"{\"a\":1}".to_vec()));
        assert_eq!(quote_ident("a`b", Dialect::Mysql), "`a``b`");
    }
}
