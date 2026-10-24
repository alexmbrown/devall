//! SQL engines (Postgres, MySQL/MariaDB, SQLite, SQL Server) behind one enum, plus the
//! dialect-aware lexer, read-only classification, result types and edit-SQL generation they share.

pub mod edit;
pub mod lex;
pub mod mssql;
pub mod mysql;
pub mod pg;
pub mod sqlite;
pub mod types;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;

pub use edit::{EditPreview, EditRequest, ParamStatement, RowEdit};
pub use lex::{Classification, Dialect, Statement, classify, read_only_message, split, statement_at};
pub use types::*;

use crate::error::{DbError, Result};

// ---------- schema ----------

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaTree {
    pub schemas: Vec<SchemaNode>,
    /// The schema unqualified names resolve to (`public`, the current MySQL database, `main`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_schema: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaNode {
    pub name: String,
    pub tables: Vec<TableNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableNode {
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<ColumnInfo>,
    /// Planner estimate, when the engine keeps one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_estimate: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    pub primary_key: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_schema: Option<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<ColumnInfo>,
    pub primary_key: Vec<String>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    /// `CREATE` statement (from the engine, or reconstructed for Postgres tables).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ddl: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainOutput {
    /// `postgres` (JSON plan), `mysql` (JSON), `sqlite` (tree of `{id, detail, children}`),
    /// `sqlserver` (Postgres-shaped `[{Statement, Plan}]` parsed from SHOWPLAN XML, the XML in
    /// `text`) or `text` (only `text` is set).
    pub format: String,
    pub plan: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// EXPLAIN ANALYZE ran the statement inside a transaction that was rolled back.
    pub rolled_back: bool,
}

// ---------- engines ----------

/// An open SQL connection.
pub enum SqlConn {
    Pg(Box<pg::PgConn>),
    My(mysql::MyConn),
    Lite(sqlite::LiteConn),
    Ms(Box<mssql::MsConn>),
}

/// Statement keywords that report a meaningful affected-row count.
fn is_dml(keyword: &str) -> bool {
    matches!(keyword, "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "MERGE" | "UPSERT" | "COPY")
}

/// Tracks explicit transactions (BEGIN … COMMIT) the user opened, so that internal shortcuts
/// that would abort a transaction (cancelling an oversized result) are avoided inside one.
fn update_tx_state(flag: &AtomicBool, keyword: &str, words_after: &str) {
    match keyword {
        "BEGIN" | "START" => {
            // SQLite `BEGIN` and Postgres/MySQL `BEGIN`/`START TRANSACTION` open one; a
            // MySQL `BEGIN … END` block never reaches here as a single statement.
            let _ = words_after;
            flag.store(true, Ordering::Relaxed);
        }
        // ROLLBACK TO SAVEPOINT keeps the transaction open.
        "COMMIT" | "ROLLBACK" | "END" | "ABORT" if !words_after.to_ascii_uppercase().contains(" TO ") => {
            flag.store(false, Ordering::Relaxed);
        }
        _ => {}
    }
}

impl SqlConn {
    pub fn dialect(&self) -> Dialect {
        match self {
            SqlConn::Pg(_) => Dialect::Postgres,
            SqlConn::My(_) => Dialect::Mysql,
            SqlConn::Lite(_) => Dialect::Sqlite,
            SqlConn::Ms(_) => Dialect::Sqlserver,
        }
    }

    fn tx_flag(&self) -> &AtomicBool {
        match self {
            SqlConn::Pg(c) => &c.in_tx,
            SqlConn::My(c) => &c.in_tx,
            SqlConn::Lite(c) => &c.in_tx,
            SqlConn::Ms(c) => &c.in_tx,
        }
    }

    /// Whether a transaction the user opened is still running.
    pub fn in_transaction(&self) -> bool {
        self.tx_flag().load(Ordering::Relaxed)
    }

    /// Runs one statement. `read_only_stmt`: the statement was classified read-only, so an
    /// oversized result may be cut short server-side.
    pub async fn run_statement(&self, sql: &str, opts: ExecOptions, read_only_stmt: bool) -> Result<StatementResult> {
        let in_tx = self.tx_flag().load(Ordering::Relaxed);
        let stop_early = read_only_stmt && !in_tx;
        match self {
            SqlConn::Pg(c) => c.run_statement(sql, opts, stop_early).await,
            SqlConn::My(c) => c.run_statement(sql, opts, stop_early).await,
            SqlConn::Lite(c) => c.run_statement(sql, opts).await,
            SqlConn::Ms(c) => {
                let kw = classify(sql, Dialect::Sqlserver).keyword;
                let mut sets = c.run_statement(sql, opts, read_only_stmt, &kw).await?;
                Ok(if sets.is_empty() { StatementResult::default() } else { sets.swap_remove(0) })
            }
        }
    }

    /// Like [`run_statement`](Self::run_statement), with every result set a statement returns
    /// (a T-SQL batch can return several; the other engines return one).
    pub async fn run_statement_sets(&self, sql: &str, opts: ExecOptions, read_only_stmt: bool, keyword: &str) -> Result<Vec<StatementResult>> {
        match self {
            SqlConn::Ms(c) => c.run_statement(sql, opts, read_only_stmt, keyword).await,
            _ => Ok(vec![self.run_statement(sql, opts, read_only_stmt).await?]),
        }
    }

    /// Asks the server to cancel whatever runs now (best effort).
    pub async fn cancel(&self) -> Result<()> {
        match self {
            SqlConn::Pg(c) => c.cancel().await,
            SqlConn::My(c) => c.kill_query().await,
            SqlConn::Lite(c) => {
                c.interrupt();
                Ok(())
            }
            SqlConn::Ms(c) => c.cancel().await,
        }
    }

    pub async fn set_read_only(&self, read_only: bool) -> Result<()> {
        match self {
            SqlConn::Pg(c) => c.set_read_only(read_only).await,
            SqlConn::My(c) => c.set_read_only(read_only).await,
            SqlConn::Lite(c) => c.set_read_only(read_only).await,
            SqlConn::Ms(c) => c.set_read_only(read_only).await,
        }
    }

    pub async fn schema(&self) -> Result<SchemaTree> {
        match self {
            SqlConn::Pg(c) => c.schema().await,
            SqlConn::My(c) => c.schema().await,
            SqlConn::Lite(c) => c.schema().await,
            SqlConn::Ms(c) => c.schema().await,
        }
    }

    pub async fn table_details(&self, schema: Option<&str>, table: &str) -> Result<TableDetails> {
        match self {
            SqlConn::Pg(c) => c.table_details(schema.unwrap_or("public"), table).await,
            SqlConn::My(c) => c.table_details(schema, table).await,
            SqlConn::Lite(c) => c.table_details(schema.unwrap_or("main"), table).await,
            SqlConn::Ms(c) => c.table_details(schema, table).await,
        }
    }

    /// Enum types with their values (Postgres; empty elsewhere — MySQL enums are in the column
    /// type, SQLite has none).
    pub async fn enum_types(&self) -> Result<std::collections::HashMap<String, Vec<String>>> {
        match self {
            SqlConn::Pg(c) => c.enum_types().await,
            _ => Ok(Default::default()),
        }
    }

    pub async fn explain(&self, sql: &str, analyze: bool) -> Result<ExplainOutput> {
        let in_tx = self.tx_flag().load(Ordering::Relaxed);
        match self {
            SqlConn::Pg(c) => c.explain(sql, analyze, in_tx).await,
            SqlConn::My(c) => c.explain(sql, analyze, in_tx).await,
            SqlConn::Lite(c) => c.explain(sql).await,
            SqlConn::Ms(c) => c.explain(sql, analyze, in_tx).await,
        }
    }

    /// Runs parameterized statements in one transaction; returns rows affected per statement.
    /// A statement with `expect_one` that affects ≠ 1 row rolls everything back.
    pub async fn exec_params(&self, stmts: &[ParamStatement]) -> Result<Vec<u64>> {
        if self.tx_flag().load(Ordering::Relaxed) {
            return Err(DbError::Invalid("finish the open transaction (COMMIT or ROLLBACK) before applying edits".into()));
        }
        match self {
            SqlConn::Pg(c) => c.exec_params(stmts).await,
            SqlConn::My(c) => c.exec_params(stmts).await,
            SqlConn::Lite(c) => c.exec_params(stmts).await,
            SqlConn::Ms(c) => c.exec_params(stmts).await,
        }
    }

    pub fn server_version(&self) -> &str {
        match self {
            SqlConn::Pg(c) => &c.version,
            SqlConn::My(c) => &c.version,
            SqlConn::Lite(c) => &c.version,
            SqlConn::Ms(c) => &c.version,
        }
    }
}

pub(crate) fn expect_one_error(n: u64, preview: &str) -> DbError {
    DbError::Query(format!("{preview}\n→ affected {n} rows instead of exactly 1, so nothing was changed (the row may have been modified or deleted meanwhile)"))
}

/// Cancellation flag shared between a running script and `cancel`.
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub type ProgressFn<'a> = &'a (dyn Fn(QueryProgress) + Send + Sync);

/// Splits and runs a script statement by statement. In read-only mode the whole script is
/// checked first and rejected (nothing runs) if any statement would modify data. Paging with
/// `offset` only re-runs read-only statements.
pub async fn run_script(conn: &SqlConn, sql: &str, opts: ExecOptions, read_only: bool, progress: Option<ProgressFn<'_>>, cancel: &CancelFlag) -> Result<QueryOutput> {
    let d = conn.dialect();
    let stmts = split(sql, d);
    if stmts.is_empty() {
        return Err(DbError::Invalid("nothing to run".into()));
    }
    let classes: Vec<Classification> = stmts.iter().map(|s| classify(&s.text, d)).collect();
    if read_only && let Some(c) = classes.iter().find(|c| !c.read_only) {
        return Err(DbError::ReadOnly(lex::read_only_message(c)));
    }
    if opts.offset > 0 && classes.iter().any(|c| !c.read_only) {
        return Err(DbError::Invalid("only read-only statements can be paged (re-running would modify data again)".into()));
    }
    let started = Instant::now();
    let total = stmts.len();
    let mut out = QueryOutput { total_statements: total, ..Default::default() };
    // Result index (a T-SQL statement can return several result sets).
    let mut index = 0usize;
    for (i, (s, c)) in stmts.iter().zip(&classes).enumerate() {
        if cancel.is_cancelled() {
            out.cancelled = true;
            break;
        }
        if let Some(p) = progress {
            p(QueryProgress::Started { index: i, total, sql: s.text.clone() });
        }
        let t0 = Instant::now();
        let res = conn.run_statement_sets(&s.text, opts, c.read_only, &c.keyword).await;
        let sets = match res {
            Ok(r) if !r.is_empty() => r,
            Ok(_) => vec![StatementResult::default()],
            Err(e) => {
                let cancelled = cancel.is_cancelled() || matches!(e, DbError::Cancelled);
                vec![StatementResult { error: Some(if cancelled { "cancelled".into() } else { e.to_string() }), ..Default::default() }]
            }
        };
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let mut failed = false;
        for mut r in sets {
            r.index = index;
            index += 1;
            r.sql = s.text.clone();
            r.keyword = c.keyword.clone();
            r.offset = opts.offset;
            r.elapsed_ms = elapsed_ms;
            // Row counts are only meaningful for DML (DDL reports 0, SQLite's counter goes stale).
            if !is_dml(c.keyword.as_str()) && r.columns.is_empty() {
                r.rows_affected = None;
            }
            failed |= r.error.is_some();
            if let Some(p) = progress {
                p(QueryProgress::Finished { result: r.clone() });
            }
            out.results.push(r);
        }
        // SQL Server reports the transaction state itself (@@TRANCOUNT after every statement).
        if !failed && conn.dialect() != Dialect::Sqlserver {
            let after = s.text.get(c.keyword.len()..).unwrap_or("");
            update_tx_state(conn.tx_flag(), c.keyword.as_str(), &format!(" {after} "));
        }
        if failed {
            out.cancelled = cancel.is_cancelled();
            break;
        }
    }
    out.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(out)
}
