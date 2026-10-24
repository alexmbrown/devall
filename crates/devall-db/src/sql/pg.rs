//! PostgreSQL via tokio-postgres + rustls.
//!
//! Statements run through the simple-query protocol (exactly what psql shows, any statement
//! type), with column types from a `Parse/Describe` of the same statement so values can be typed
//! in JSON. Oversized read-only results are cut short with a cancel request instead of streaming
//! millions of rows. Read-only sessions use `SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY`.

use std::pin::pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::AbortHandle;
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::types::private::BytesMut;
use tokio_postgres::types::{Format, IsNull, ToSql, Type, to_sql_checked};
use tokio_postgres::{AsyncMessage, CancelToken, Client, Config, NoTls, SimpleQueryMessage};

use super::edit::{ParamStatement, qualified, quote_ident};
use super::types::{ColumnMeta, ExecOptions, StatementResult, ValueKind, text_to_value};
use super::{ColumnInfo, Dialect, ExplainOutput, ForeignKeyInfo, IndexInfo, SchemaNode, SchemaTree, TableDetails, TableKind, TableNode, expect_one_error};
use crate::error::{DbError, Result};
use crate::model::TlsMode;
use crate::resolve::ResolvedConnection;
use crate::tls::{PgTls, client_config};

pub struct PgConn {
    client: Client,
    cancel_token: CancelToken,
    tls: Option<PgTls>,
    notices: Arc<Mutex<Vec<String>>>,
    task: AbortHandle,
    lock: tokio::sync::Mutex<()>,
    pub(crate) in_tx: AtomicBool,
    pub(crate) version: String,
}

impl Drop for PgConn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// `ERROR 42P01: relation "x" does not exist` + detail/hint/position.
pub fn pg_err(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        let mut m = format!("{} {}: {}", db.severity(), db.code().code(), db.message());
        if let Some(d) = db.detail() {
            m.push_str(&format!("\nDETAIL: {d}"));
        }
        if let Some(h) = db.hint() {
            m.push_str(&format!("\nHINT: {h}"));
        }
        if let Some(ErrorPosition::Original(p)) = db.position() {
            m.push_str(&format!("\nPOSITION: {p}"));
        }
        if let Some(w) = db.where_() {
            m.push_str(&format!("\nWHERE: {w}"));
        }
        return m;
    }
    let mut m = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        let t = s.to_string();
        if !m.contains(&t) {
            m.push_str(": ");
            m.push_str(&t);
        }
        src = s.source();
    }
    m
}

fn is_cancel(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&SqlState::QUERY_CANCELED)
}

fn query_err(e: tokio_postgres::Error) -> DbError {
    if is_cancel(&e) { DbError::Cancelled } else { DbError::Query(pg_err(&e)) }
}

fn kind_of(t: &Type) -> ValueKind {
    match t.oid() {
        16 => ValueKind::Bool,
        20 | 21 | 23 | 26 => ValueKind::Int,
        700 | 701 => ValueKind::Float,
        1700 | 790 => ValueKind::Decimal,
        114 | 3802 => ValueKind::Json,
        17 => ValueKind::Bytes,
        1082 => ValueKind::Date,
        1083 | 1266 | 1186 => ValueKind::Time,
        1114 | 1184 => ValueKind::Timestamp,
        2950 => ValueKind::Uuid,
        25 | 1043 | 1042 | 19 | 18 => ValueKind::Text,
        _ => ValueKind::Other,
    }
}

/// A parameter sent in text format, so the server parses it for whatever type it infers
/// (int, uuid, timestamptz, jsonb, enums, …).
#[derive(Debug)]
struct PgText(Option<String>);

impl PgText {
    fn from_json(v: &Value) -> Self {
        PgText(match v {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            Value::Bool(b) => Some(b.to_string()),
            other => Some(other.to_string()),
        })
    }
}

impl ToSql for PgText {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> std::result::Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match &self.0 {
            None => Ok(IsNull::Yes),
            Some(s) => {
                out.extend_from_slice(s.as_bytes());
                Ok(IsNull::No)
            }
        }
    }
    fn accepts(_: &Type) -> bool {
        true
    }
    fn encode_format(&self, _: &Type) -> Format {
        Format::Text
    }
    to_sql_checked!();
}

fn drive<S, T>(mut conn: tokio_postgres::Connection<S, T>, notices: Arc<Mutex<Vec<String>>>) -> AbortHandle
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let stream = futures_util::stream::poll_fn(move |cx| conn.poll_message(cx));
        let mut stream = pin!(stream);
        while let Some(m) = stream.next().await {
            match m {
                Ok(AsyncMessage::Notice(n)) => {
                    let mut v = notices.lock().unwrap();
                    if v.len() < 500 {
                        v.push(format!("{}: {}", n.severity(), n.message()));
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    })
    .abort_handle()
}

impl PgConn {
    /// Connects to `host:port` (the tunnel's local end when tunnelling; TLS still verifies
    /// `rc.host`).
    pub async fn connect(rc: &ResolvedConnection, host: &str, port: u16, read_only: bool) -> Result<Self> {
        let mut cfg = Config::new();
        cfg.host(host).port(port).user(rc.user.as_deref().unwrap_or("postgres"));
        if let Some(pw) = &rc.password {
            cfg.password(pw);
        }
        if let Some(db) = &rc.database {
            cfg.dbname(db);
        }
        cfg.application_name(rc.option("applicationName").unwrap_or("devall"));
        let timeout = rc.connect_timeout();
        cfg.connect_timeout(timeout);
        cfg.keepalives(true);
        let notices = Arc::new(Mutex::new(Vec::new()));
        let tls = match rc.tls {
            TlsMode::Disable => {
                cfg.ssl_mode(tokio_postgres::config::SslMode::Disable);
                None
            }
            mode => {
                cfg.ssl_mode(if mode == TlsMode::Prefer { tokio_postgres::config::SslMode::Prefer } else { tokio_postgres::config::SslMode::Require });
                Some(PgTls::new(client_config(mode == TlsMode::VerifyFull, rc.ca_cert.as_deref())?, Some(rc.host.clone())))
            }
        };
        let connect = async {
            match &tls {
                None => {
                    let (client, conn) = cfg.connect(NoTls).await?;
                    Ok::<_, tokio_postgres::Error>((client, drive(conn, notices.clone()), None))
                }
                Some(t) => {
                    let (client, conn) = cfg.connect(t.clone()).await?;
                    Ok((client, drive(conn, notices.clone()), Some(t.clone())))
                }
            }
        };
        let (client, task, tls) = tokio::time::timeout(timeout * 2, connect)
            .await
            .map_err(|_| DbError::Timeout(format!("{timeout:?} connecting to {}:{}", rc.host, rc.port)))?
            .map_err(|e| DbError::Connect(pg_err(&e)))?;
        let mut conn = PgConn {
            cancel_token: client.cancel_token(),
            client,
            tls,
            notices,
            task,
            lock: tokio::sync::Mutex::new(()),
            in_tx: AtomicBool::new(false),
            version: String::new(),
        };
        let v = conn.simple_value("SHOW server_version").await?.unwrap_or_default();
        conn.version = format!("PostgreSQL {}", v.split_whitespace().next().unwrap_or(&v));
        if let Some(ms) = rc.option("statementTimeout") {
            let ms: u64 = ms.trim().parse().map_err(|_| DbError::Invalid(format!("statementTimeout must be milliseconds (got '{ms}')")))?;
            conn.client.batch_execute(&format!("SET statement_timeout = {ms}")).await.map_err(|e| DbError::Connect(pg_err(&e)))?;
        }
        conn.set_read_only(read_only).await?;
        Ok(conn)
    }

    async fn simple_value(&self, sql: &str) -> Result<Option<String>> {
        let msgs = self.client.simple_query(sql).await.map_err(query_err)?;
        Ok(msgs.iter().find_map(|m| match m {
            SimpleQueryMessage::Row(r) => r.get(0).map(str::to_string),
            _ => None,
        }))
    }

    pub async fn set_read_only(&self, read_only: bool) -> Result<()> {
        let _g = self.lock.lock().await;
        let mode = if read_only { "READ ONLY" } else { "READ WRITE" };
        self.client.batch_execute(&format!("SET SESSION CHARACTERISTICS AS TRANSACTION {mode}")).await.map_err(query_err)
    }

    pub async fn cancel(&self) -> Result<()> {
        let r = match &self.tls {
            Some(t) => self.cancel_token.cancel_query(t.clone()).await,
            None => self.cancel_token.cancel_query(NoTls).await,
        };
        r.map_err(|e| DbError::Query(format!("cancel failed: {}", pg_err(&e))))
    }

    pub async fn run_statement(&self, sql: &str, opts: ExecOptions, stop_early: bool) -> Result<StatementResult> {
        if self.client.is_closed() {
            return Err(DbError::Closed);
        }
        let _g = self.lock.lock().await;
        self.notices.lock().unwrap().clear();
        let types: Option<Vec<Type>> = self.client.prepare(sql).await.ok().map(|st| st.columns().iter().map(|c| c.type_().clone()).collect());
        let stream = self.client.simple_query_raw(sql).await.map_err(query_err)?;
        let mut stream = pin!(stream);
        let mut out = StatementResult::default();
        let mut kinds: Vec<ValueKind> = Vec::new();
        let mut skipped = 0usize;
        let mut stopping = false;
        let mut first_done = false;
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(SimpleQueryMessage::RowDescription(cols)) if !first_done && out.columns.is_empty() => {
                    let types = types.as_ref().filter(|t| t.len() == cols.len());
                    out.columns = cols
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let t = types.and_then(|t| t.get(i));
                            ColumnMeta { name: c.name().to_string(), type_name: t.map(|t| t.name().to_string()).unwrap_or_default(), kind: t.map(kind_of).unwrap_or(ValueKind::Other) }
                        })
                        .collect();
                    kinds = out.columns.iter().map(|c| c.kind).collect();
                }
                Ok(SimpleQueryMessage::Row(r)) => {
                    if stopping || first_done {
                        continue;
                    }
                    if skipped < opts.offset {
                        skipped += 1;
                        continue;
                    }
                    if out.rows.len() >= opts.limit {
                        out.more = true;
                        if stop_early {
                            stopping = true;
                            let _ = self.cancel().await;
                        }
                        continue;
                    }
                    out.rows.push((0..r.len()).map(|i| r.get(i).map(|s| text_to_value(s, kinds.get(i).copied().unwrap_or(ValueKind::Other))).unwrap_or(Value::Null)).collect());
                }
                Ok(SimpleQueryMessage::CommandComplete(n)) => {
                    if !first_done {
                        out.rows_affected = Some(n);
                    }
                    first_done = true;
                }
                Ok(_) => {}
                Err(e) if stopping && is_cancel(&e) => {}
                Err(e) => return Err(query_err(e)),
            }
        }
        if stopping {
            // Cut short: the server's count covers only what was sent.
            out.rows_affected = None;
        }
        out.notices = std::mem::take(&mut *self.notices.lock().unwrap());
        Ok(out)
    }

    async fn text_rows(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Vec<tokio_postgres::Row>> {
        self.client.query(sql, params).await.map_err(query_err)
    }

    pub async fn schema(&self) -> Result<SchemaTree> {
        let _g = self.lock.lock().await;
        const FILTER: &str = "n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%'";
        let schemas = self.text_rows(&format!("SELECT n.nspname::text FROM pg_catalog.pg_namespace n WHERE {FILTER} ORDER BY 1"), &[]).await?;
        let tables = self
            .text_rows(
                &format!(
                    "SELECT n.nspname::text, c.relname::text, c.relkind::text, c.reltuples::bigint::text \
                     FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE c.relkind IN ('r','p','v','m','f') AND {FILTER} ORDER BY 1, 2"
                ),
                &[],
            )
            .await?;
        let columns = self.text_rows(&columns_sql(FILTER), &[]).await?;
        let default_schema: Option<String> = self.text_rows("SELECT current_schema()::text", &[]).await?.first().and_then(|r| r.get(0));

        // Group columns by table (the server's collation order may differ from Rust's).
        let mut by_table: std::collections::HashMap<(String, String), Vec<ColumnInfo>> = std::collections::HashMap::new();
        for c in &columns {
            by_table.entry((c.get(0), c.get(1))).or_default().push(column_info(c, 2));
        }
        let mut tree = SchemaTree { schemas: Vec::new(), default_schema };
        for r in &schemas {
            tree.schemas.push(SchemaNode { name: r.get(0), tables: Vec::new() });
        }
        for r in &tables {
            let (schema, name, kind): (String, String, String) = (r.get(0), r.get(1), r.get(2));
            let est: Option<String> = r.get(3);
            let cols = by_table.remove(&(schema.clone(), name.clone())).unwrap_or_default();
            let node = TableNode { name, kind: table_kind(&kind), columns: cols, rows_estimate: est.and_then(|e| e.parse().ok()).filter(|e: &i64| *e >= 0) };
            if let Some(s) = tree.schemas.iter_mut().find(|s| s.name == schema) {
                s.tables.push(node);
            }
        }
        Ok(tree)
    }

    /// Enum types and their labels, keyed by `type` and `schema.type`.
    pub async fn enum_types(&self) -> Result<std::collections::HashMap<String, Vec<String>>> {
        let _g = self.lock.lock().await;
        let rows = self
            .text_rows(
                "SELECT n.nspname::text, t.typname::text, e.enumlabel::text FROM pg_catalog.pg_enum e                  JOIN pg_catalog.pg_type t ON t.oid = e.enumtypid JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace                  ORDER BY n.nspname, t.typname, e.enumsortorder",
                &[],
            )
            .await?;
        let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for r in &rows {
            let (schema, name, label): (String, String, String) = (r.get(0), r.get(1), r.get(2));
            out.entry(format!("{schema}.{name}")).or_default().push(label.clone());
            out.entry(name).or_default().push(label);
        }
        Ok(out)
    }

    pub async fn table_details(&self, schema: &str, table: &str) -> Result<TableDetails> {
        let _g = self.lock.lock().await;
        let p: [&(dyn ToSql + Sync); 2] = [&schema, &table];
        let cols = self.text_rows(&columns_sql("n.nspname::text = $1 AND c.relname::text = $2"), &p).await?;
        let kind_rows = self
            .text_rows(
                "SELECT c.relkind::text, CASE WHEN c.relkind IN ('v','m') THEN pg_catalog.pg_get_viewdef(c.oid, true) END \
                 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname::text = $1 AND c.relname::text = $2",
                &p,
            )
            .await?;
        let Some(kr) = kind_rows.first() else { return Err(DbError::NotFound(format!("table {schema}.{table}"))) };
        let kind = table_kind(&kr.get::<_, String>(0));
        let viewdef: Option<String> = kr.get(1);
        let idx = self
            .text_rows(
                "SELECT i.relname::text, ix.indisunique, ix.indisprimary, am.amname::text, pg_catalog.pg_get_indexdef(ix.indexrelid), \
                 ARRAY(SELECT pg_catalog.pg_get_indexdef(ix.indexrelid, k + 1, true) FROM generate_subscripts(ix.indkey, 1) AS k ORDER BY k)::text[] \
                 FROM pg_catalog.pg_index ix JOIN pg_catalog.pg_class t ON t.oid = ix.indrelid JOIN pg_catalog.pg_class i ON i.oid = ix.indexrelid \
                 JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace JOIN pg_catalog.pg_am am ON am.oid = i.relam \
                 WHERE n.nspname::text = $1 AND t.relname::text = $2 ORDER BY ix.indisprimary DESC, i.relname",
                &p,
            )
            .await?;
        let fks = self
            .text_rows(
                "SELECT con.conname::text, \
                 ARRAY(SELECT a.attname::text FROM unnest(con.conkey) WITH ORDINALITY AS k(n, o) JOIN pg_catalog.pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.n ORDER BY k.o), \
                 fn.nspname::text, ft.relname::text, \
                 ARRAY(SELECT a.attname::text FROM unnest(con.confkey) WITH ORDINALITY AS k(n, o) JOIN pg_catalog.pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.n ORDER BY k.o), \
                 con.confupdtype::text, con.confdeltype::text \
                 FROM pg_catalog.pg_constraint con \
                 JOIN pg_catalog.pg_class t ON t.oid = con.conrelid JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace \
                 JOIN pg_catalog.pg_class ft ON ft.oid = con.confrelid JOIN pg_catalog.pg_namespace fn ON fn.oid = ft.relnamespace \
                 WHERE con.contype = 'f' AND n.nspname::text = $1 AND t.relname::text = $2 ORDER BY con.conname",
                &p,
            )
            .await?;
        let constraints = self
            .text_rows(
                "SELECT con.conname::text, pg_catalog.pg_get_constraintdef(con.oid, true) FROM pg_catalog.pg_constraint con \
                 JOIN pg_catalog.pg_class t ON t.oid = con.conrelid JOIN pg_catalog.pg_namespace n ON n.oid = t.relnamespace \
                 WHERE n.nspname::text = $1 AND t.relname::text = $2 ORDER BY CASE con.contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'f' THEN 2 ELSE 3 END, con.conname",
                &p,
            )
            .await?;

        let columns: Vec<ColumnInfo> = cols.iter().map(|c| column_info(c, 2)).collect();
        let indexes: Vec<IndexInfo> = idx
            .iter()
            .map(|r| IndexInfo { name: r.get(0), unique: r.get(1), primary: r.get(2), method: r.get(3), definition: r.get(4), columns: r.get(5) })
            .collect();
        let primary_key = indexes.iter().find(|i| i.primary).map(|i| i.columns.clone()).unwrap_or_default();
        let action = |c: Option<String>| -> Option<String> {
            Some(
                match c?.as_str() {
                    "r" => "RESTRICT",
                    "c" => "CASCADE",
                    "n" => "SET NULL",
                    "d" => "SET DEFAULT",
                    _ => return None,
                }
                .to_string(),
            )
        };
        let foreign_keys: Vec<ForeignKeyInfo> = fks
            .iter()
            .map(|r| ForeignKeyInfo {
                name: r.get(0),
                columns: r.get(1),
                ref_schema: r.get(2),
                ref_table: r.get(3),
                ref_columns: r.get(4),
                on_update: action(r.get(5)),
                on_delete: action(r.get(6)),
            })
            .collect();
        let qt = qualified(Some(schema), table, Dialect::Postgres);
        let ddl = match kind {
            TableKind::View => viewdef.map(|v| format!("CREATE VIEW {qt} AS\n{v}")),
            TableKind::MaterializedView => viewdef.map(|v| format!("CREATE MATERIALIZED VIEW {qt} AS\n{v}")),
            _ => {
                let mut lines: Vec<String> = columns
                    .iter()
                    .map(|c| {
                        let mut l = format!("  {} {}", quote_ident(&c.name, Dialect::Postgres), c.data_type);
                        if !c.nullable {
                            l.push_str(" NOT NULL");
                        }
                        if let Some(d) = &c.default {
                            l.push_str(&format!(" DEFAULT {d}"));
                        }
                        l
                    })
                    .collect();
                for r in &constraints {
                    let (name, def): (String, String) = (r.get(0), r.get(1));
                    lines.push(format!("  CONSTRAINT {} {def}", quote_ident(&name, Dialect::Postgres)));
                }
                let mut ddl = format!("CREATE TABLE {qt} (\n{}\n);\n", lines.join(",\n"));
                for i in indexes.iter().filter(|i| !i.primary) {
                    // Unique constraints already create their index.
                    let from_constraint = constraints.iter().any(|r| r.get::<_, String>(0) == i.name);
                    if let (false, Some(def)) = (from_constraint, &i.definition) {
                        ddl.push_str(def);
                        ddl.push_str(";\n");
                    }
                }
                Some(ddl)
            }
        };
        Ok(TableDetails { schema: Some(schema.to_string()), name: table.to_string(), kind, columns, primary_key, indexes, foreign_keys, ddl })
    }

    pub async fn explain(&self, sql: &str, analyze: bool, in_tx: bool) -> Result<ExplainOutput> {
        let _g = self.lock.lock().await;
        let wrap = analyze && !in_tx;
        if wrap {
            self.client.batch_execute("BEGIN").await.map_err(query_err)?;
        }
        let q = format!("EXPLAIN ({}FORMAT JSON) {sql}", if analyze { "ANALYZE, BUFFERS, " } else { "" });
        let res = self.client.simple_query(&q).await;
        if wrap {
            let _ = self.client.batch_execute("ROLLBACK").await;
        }
        let msgs = res.map_err(query_err)?;
        let text: String = msgs
            .iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(r) => r.get(0).map(str::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let plan: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
        Ok(ExplainOutput { format: "postgres".into(), plan, text: None, rolled_back: wrap })
    }

    pub async fn exec_params(&self, stmts: &[ParamStatement]) -> Result<Vec<u64>> {
        let _g = self.lock.lock().await;
        self.client.batch_execute("BEGIN").await.map_err(query_err)?;
        let mut out = Vec::with_capacity(stmts.len());
        for s in stmts {
            let params: Vec<PgText> = s.params.iter().map(PgText::from_json).collect();
            let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as &(dyn ToSql + Sync)).collect();
            match self.client.execute(s.sql.as_str(), &refs).await {
                Ok(n) if s.expect_one && n != 1 => {
                    let _ = self.client.batch_execute("ROLLBACK").await;
                    return Err(expect_one_error(n, &s.preview));
                }
                Ok(n) => out.push(n),
                Err(e) => {
                    let _ = self.client.batch_execute("ROLLBACK").await;
                    return Err(DbError::Query(format!("{}\n→ {}", s.preview, pg_err(&e))));
                }
            }
        }
        self.client.batch_execute("COMMIT").await.map_err(query_err)?;
        Ok(out)
    }
}

fn table_kind(k: &str) -> TableKind {
    match k {
        "v" => TableKind::View,
        "m" => TableKind::MaterializedView,
        "f" => TableKind::ForeignTable,
        _ => TableKind::Table,
    }
}

fn columns_sql(filter: &str) -> String {
    format!(
        "SELECT n.nspname::text, c.relname::text, a.attname::text, pg_catalog.format_type(a.atttypid, a.atttypmod), NOT a.attnotnull, \
         pg_catalog.pg_get_expr(d.adbin, d.adrelid), \
         EXISTS (SELECT 1 FROM pg_catalog.pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY (i.indkey)) \
         FROM pg_catalog.pg_attribute a \
         JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attnum > 0 AND NOT a.attisdropped AND c.relkind IN ('r','p','v','m','f') AND {filter} \
         ORDER BY 1, 2, a.attnum"
    )
}

fn column_info(r: &tokio_postgres::Row, at: usize) -> ColumnInfo {
    ColumnInfo { name: r.get(at), data_type: r.get(at + 1), nullable: r.get(at + 2), default: r.get(at + 3), primary_key: r.get(at + 4) }
}
