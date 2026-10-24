//! Microsoft SQL Server and Azure SQL via tiberius: pure-Rust TDS 7.3–8.0 over rustls (aws-lc),
//! no ODBC driver, no OpenSSL.
//!
//! * **Batches.** Statements are sent as T-SQL batches (what SSMS sends between `GO` lines; see
//!   [`crate::sql::lex::split_tsql`]); a batch can return several result sets.
//! * **Row caps and paging.** Read-only statements run under `SET ROWCOUNT offset+limit+1`, so the
//!   server stops after the rows the grid needs (and `more` is exact); writes are never capped.
//! * **Cancel.** A TDS attention signal on the same connection (tiberius `cancel_query`); a
//!   `statementTimeout` option (ms) cancels the same way.
//! * **Read-only.** SQL Server has no read-only session flag: the T-SQL classifier refuses writes
//!   before anything is sent, and read-only sessions run at `READ COMMITTED`. For availability-
//!   group secondaries, `applicationIntent: ReadOnly` routes the connection to a read-only replica.
//! * **EXPLAIN.** `SET SHOWPLAN_XML ON` (estimated plan, nothing runs) or `SET STATISTICS XML ON`
//!   inside a rolled-back transaction (actual plan); the XML is parsed into the Postgres-like plan
//!   tree the UI draws, and kept as text.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::TryStreamExt;
use serde_json::{Value, json};
use tiberius::{AuthMethod, Client, ColumnData, ColumnType, Config, EncryptionLevel, Query, QueryItem, SqlBrowser};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::edit::{ParamStatement, qualified, quote_ident};
use super::types::{ColumnMeta, ExecOptions, StatementResult, ValueKind, bytes_value, float_value, int_value};
use super::{ColumnInfo, Dialect, ExplainOutput, ForeignKeyInfo, IndexInfo, SchemaNode, SchemaTree, TableDetails, TableKind, TableNode, expect_one_error};
use crate::error::{DbError, Result};
use crate::model::TlsMode;
use crate::resolve::ResolvedConnection;

type Tds = Client<Compat<TcpStream>>;

pub struct MsConn {
    client: tokio::sync::Mutex<Tds>,
    /// Bumped by [`MsConn::cancel`]; the running statement watches it.
    cancel: watch::Sender<u64>,
    /// An interrupted request left the connection unusable.
    broken: AtomicBool,
    pub(crate) in_tx: AtomicBool,
    pub(crate) version: String,
    statement_timeout: Option<Duration>,
    default_schema: String,
}

/// `Msg 208, Level 16, State 1, Line 1: Invalid object name 'x'.`
pub fn ms_err(e: tiberius::error::Error) -> DbError {
    match e {
        tiberius::error::Error::Server(t) => {
            let mut m = format!("Msg {}, Level {}, State {}", t.code(), t.class(), t.state());
            if !t.procedure().is_empty() {
                m.push_str(&format!(", Procedure {}", t.procedure()));
            }
            if t.line() > 0 {
                m.push_str(&format!(", Line {}", t.line()));
            }
            DbError::Query(format!("{m}: {}", t.message()))
        }
        other => DbError::Query(other.to_string()),
    }
}

fn connect_err(e: tiberius::error::Error) -> DbError {
    match ms_err(e) {
        DbError::Query(m) => DbError::Connect(m),
        other => other,
    }
}

// ---------- types ----------

pub(crate) fn kind_of(t: ColumnType) -> ValueKind {
    use ColumnType::*;
    match t {
        Bit | Bitn => ValueKind::Bool,
        Int1 | Int2 | Int4 | Int8 | Intn => ValueKind::Int,
        Float4 | Float8 | Floatn => ValueKind::Float,
        Money | Money4 | Decimaln | Numericn => ValueKind::Decimal,
        Guid => ValueKind::Uuid,
        Daten => ValueKind::Date,
        Timen => ValueKind::Time,
        Datetime4 | Datetime | Datetimen | Datetime2 | DatetimeOffsetn => ValueKind::Timestamp,
        BigVarBin | BigBinary | Image | Udt => ValueKind::Bytes,
        BigVarChar | BigChar | NVarchar | NChar | Text | NText | Xml => ValueKind::Text,
        SSVariant | Null => ValueKind::Other,
    }
}

pub(crate) fn type_name(t: ColumnType) -> &'static str {
    use ColumnType::*;
    match t {
        Null => "null",
        Bit | Bitn => "bit",
        Int1 => "tinyint",
        Int2 => "smallint",
        Int4 | Intn => "int",
        Int8 => "bigint",
        Datetime4 => "smalldatetime",
        Float4 => "real",
        Float8 | Floatn => "float",
        Money => "money",
        Money4 => "smallmoney",
        Datetime | Datetimen => "datetime",
        Guid => "uniqueidentifier",
        Decimaln => "decimal",
        Numericn => "numeric",
        Daten => "date",
        Timen => "time",
        Datetime2 => "datetime2",
        DatetimeOffsetn => "datetimeoffset",
        BigVarBin => "varbinary",
        BigVarChar => "varchar",
        BigBinary => "binary",
        BigChar => "char",
        NVarchar => "nvarchar",
        NChar => "nchar",
        Xml => "xml",
        Udt => "udt",
        Text => "text",
        Image => "image",
        NText => "ntext",
        SSVariant => "sql_variant",
    }
}

/// Proleptic Gregorian date for a day count since 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Days from 0001-01-01 (the `date`/`datetime2` epoch) to 1970-01-01.
const DAYS_0001_TO_1970: i64 = 719_162;
/// Days from 1900-01-01 (the `datetime`/`smalldatetime` epoch) to 1970-01-01.
const DAYS_1900_TO_1970: i64 = 25_567;

fn fmt_date(days_since_1970: i64) -> String {
    let (y, m, d) = civil_from_days(days_since_1970);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `hh:mm:ss[.fffffff]` from 10^-scale second increments since midnight.
fn fmt_time(increments: u64, scale: u8) -> String {
    let scale = scale.min(7);
    let unit = 10u64.pow(u32::from(scale));
    let secs = increments / unit;
    let frac = increments % unit;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if scale == 0 { format!("{h:02}:{m:02}:{s:02}") } else { format!("{h:02}:{m:02}:{s:02}.{frac:0width$}", width = usize::from(scale)) }
}

/// ISO 8601 text for date/time values (`T` separator: unambiguous for `datetime` whatever the
/// session's DATEFORMAT, so edited values round-trip).
pub(crate) fn time_value(d: &ColumnData<'_>) -> Option<String> {
    Some(match d {
        ColumnData::Date(Some(v)) => fmt_date(i64::from(v.days()) - DAYS_0001_TO_1970),
        ColumnData::Time(Some(t)) => fmt_time(t.increments(), t.scale()),
        ColumnData::DateTime2(Some(dt)) => format!("{}T{}", fmt_date(i64::from(dt.date().days()) - DAYS_0001_TO_1970), fmt_time(dt.time().increments(), dt.time().scale())),
        ColumnData::DateTimeOffset(Some(o)) => {
            // The value is UTC; shift to the stored offset for display.
            let dt = o.datetime2();
            let scale = dt.time().scale().min(7);
            let unit = 10u64.pow(u32::from(scale));
            let total = i128::from(dt.date().days()) * 86_400 * i128::from(unit) + i128::from(dt.time().increments()) + i128::from(o.offset()) * 60 * i128::from(unit);
            let per_day = 86_400 * i128::from(unit);
            let days = total.div_euclid(per_day) as i64;
            let inc = total.rem_euclid(per_day) as u64;
            let off = o.offset();
            let sign = if off < 0 { '-' } else { '+' };
            let off = off.unsigned_abs();
            format!("{}T{}{sign}{:02}:{:02}", fmt_date(days - DAYS_0001_TO_1970), fmt_time(inc, scale), off / 60, off % 60)
        }
        ColumnData::DateTime(Some(dt)) => {
            // 1/300 s ticks.
            let ms = (u64::from(dt.seconds_fragments()) * 10 + 1) / 3;
            format!("{}T{}", fmt_date(i64::from(dt.days()) - DAYS_1900_TO_1970), fmt_time(ms, 3))
        }
        ColumnData::SmallDateTime(Some(dt)) => format!("{}T{}", fmt_date(i64::from(dt.days()) - DAYS_1900_TO_1970), fmt_time(u64::from(dt.seconds_fragments()) * 60, 0)),
        _ => return None,
    })
}

/// A cell as JSON: big integers and exact numerics as strings, binary as base64, GUIDs in SQL
/// Server's upper-case form, date/times as ISO 8601.
pub(crate) fn to_json(d: &ColumnData<'_>, col: ColumnType) -> Value {
    let money = matches!(col, ColumnType::Money | ColumnType::Money4);
    match d {
        ColumnData::U8(v) => v.map(Value::from).unwrap_or(Value::Null),
        ColumnData::I16(v) => v.map(Value::from).unwrap_or(Value::Null),
        ColumnData::I32(v) => v.map(Value::from).unwrap_or(Value::Null),
        ColumnData::I64(v) => v.map(int_value).unwrap_or(Value::Null),
        ColumnData::F32(v) => v.map(|f| if money { Value::String(format!("{f:.4}")) } else { float_value(f64::from(f)) }).unwrap_or(Value::Null),
        ColumnData::F64(v) => v.map(|f| if money { Value::String(format!("{f:.4}")) } else { float_value(f) }).unwrap_or(Value::Null),
        ColumnData::Bit(v) => v.map(Value::Bool).unwrap_or(Value::Null),
        ColumnData::String(v) => v.as_ref().map(|s| Value::String(s.to_string())).unwrap_or(Value::Null),
        ColumnData::Guid(v) => v.map(|g| Value::String(g.to_string().to_ascii_uppercase())).unwrap_or(Value::Null),
        ColumnData::Binary(v) => v.as_ref().map(|b| bytes_value(b)).unwrap_or(Value::Null),
        ColumnData::Numeric(v) => v.map(|n| Value::String(n.to_string())).unwrap_or(Value::Null),
        ColumnData::Xml(v) => v.as_ref().map(|x| Value::String(x.to_string())).unwrap_or(Value::Null),
        other => time_value(other).map(Value::String).unwrap_or(Value::Null),
    }
}

fn cell_text(d: &ColumnData<'_>, col: ColumnType) -> Option<String> {
    match to_json(d, col) {
        Value::Null => None,
        Value::String(s) => Some(s),
        other => Some(other.to_string()),
    }
}

/// Binds a JSON value: null, bit, bigint, float or nvarchar (the server converts nvarchar to the
/// column's type — uniqueidentifier, datetime2, decimal, …).
fn bind(q: &mut Query<'_>, v: &Value) {
    match v {
        Value::Null => q.bind(Option::<String>::None),
        Value::Bool(b) => q.bind(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i);
            } else if let Some(f) = n.as_f64() {
                q.bind(f);
            } else {
                q.bind(n.to_string());
            }
        }
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}

/// Runs a batch and drains it (no rows wanted).
async fn simple(c: &mut Tds, sql: &str) -> Result<()> {
    c.simple_query(sql).await.map_err(ms_err)?.into_results().await.map_err(ms_err)?;
    Ok(())
}

fn has_word(sql: &str, word: &str) -> bool {
    super::lex::lex(sql, Dialect::Sqlserver).iter().any(|t| t.kind == super::lex::TokKind::Word && sql[t.start..t.end].eq_ignore_ascii_case(word))
}

async fn open_stream(cfg: &Config, named: bool) -> std::result::Result<Tds, tiberius::error::Error> {
    let tcp = if named { TcpStream::connect_named(cfg).await? } else { TcpStream::connect(cfg.get_addr()).await? };
    tcp.set_nodelay(true)?;
    Client::connect(cfg.clone(), tcp.compat_write()).await
}

impl MsConn {
    /// Connects to `host:port` (the tunnel's local end when tunnelling; TLS still checks the
    /// certificate against `rc.host`).
    pub async fn connect(rc: &ResolvedConnection, host: &str, port: u16, read_only: bool) -> Result<Self> {
        let tunneled = host != rc.host;
        let mut cfg = Config::new();
        cfg.host(host);
        cfg.port(port);
        let instance = rc.option("instance").or_else(|| rc.option("instanceName")).map(str::to_string);
        let named = instance.is_some() && !rc.port_explicit && !tunneled;
        if let Some(i) = &instance {
            cfg.instance_name(i);
        }
        if let Some(db) = &rc.database {
            cfg.database(db);
        }
        cfg.application_name(rc.option("applicationName").unwrap_or("devall"));
        let user = rc.user.clone().unwrap_or_else(|| "sa".into());
        cfg.authentication(AuthMethod::sql_server(user, rc.password.clone().unwrap_or_default()));
        if rc.option("applicationIntent").is_some_and(|v| v.trim().eq_ignore_ascii_case("readonly")) {
            cfg.readonly(true);
        }
        let strict = rc.option("encrypt").is_some_and(|v| v.trim().eq_ignore_ascii_case("strict"));
        match rc.tls {
            TlsMode::Disable => cfg.encryption(EncryptionLevel::NotSupported),
            TlsMode::Prefer => {
                cfg.encryption(EncryptionLevel::On);
                cfg.trust_cert();
            }
            TlsMode::Require => {
                cfg.encryption(if strict { EncryptionLevel::Strict } else { EncryptionLevel::Required });
                cfg.trust_cert();
            }
            TlsMode::VerifyFull => {
                cfg.encryption(if strict { EncryptionLevel::Strict } else { EncryptionLevel::Required });
                if let Some(ca) = &rc.ca_cert {
                    cfg.trust_cert_ca(ca.to_string_lossy());
                }
            }
        }
        if tunneled {
            cfg.hostname_in_certificate(&rc.host);
        }
        if instance.is_some() && tunneled && !rc.port_explicit {
            return Err(DbError::Invalid("named instances behind an SSH tunnel need an explicit port (the SQL Browser lookup can't go through the tunnel)".into()));
        }
        let timeout = rc.connect_timeout();
        let connect = async {
            match open_stream(&cfg, named).await {
                // Azure SQL's gateway redirects to the database's node.
                Err(tiberius::error::Error::Routing { host: h, port: p }) if !tunneled => {
                    let mut c2 = cfg.clone();
                    c2.host(&h);
                    c2.port(p);
                    open_stream(&c2, false).await
                }
                other => other,
            }
        };
        let mut client = tokio::time::timeout(timeout * 2, connect)
            .await
            .map_err(|_| DbError::Timeout(format!("{timeout:?} connecting to {}:{}", rc.host, rc.port)))?
            .map_err(connect_err)?;
        // The session options SSMS and the ODBC/OLE DB drivers set (QUOTED_IDENTIFIER for indexed
        // views and filtered indexes, TEXTSIZE so (n)varchar(max) isn't cut at 4 KB).
        simple(
            &mut client,
            "SET ANSI_NULLS ON; SET ANSI_PADDING ON; SET ANSI_WARNINGS ON; SET ARITHABORT ON; SET CONCAT_NULL_YIELDS_NULL ON; SET QUOTED_IDENTIFIER ON; SET NUMERIC_ROUNDABORT OFF; SET TEXTSIZE 2147483647",
        )
        .await?;
        let info = client
            .simple_query(
                "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)), CAST(SERVERPROPERTY('Edition') AS nvarchar(128)), CAST(SERVERPROPERTY('EngineEdition') AS int), SCHEMA_NAME()",
            )
            .await
            .map_err(ms_err)?
            .into_row()
            .await
            .map_err(ms_err)?;
        let get = |i: usize| info.as_ref().and_then(|r| r.try_get::<&str, _>(i).ok().flatten()).map(str::to_string);
        let engine = info.as_ref().and_then(|r| r.try_get::<i32, _>(2).ok().flatten()).unwrap_or(0);
        let product = if matches!(engine, 5 | 8) { "Azure SQL" } else { "SQL Server" };
        let version = format!("{product} {}{}", get(0).unwrap_or_default(), get(1).map(|e| format!(" ({e})")).unwrap_or_default());
        let statement_timeout = match rc.option("statementTimeout") {
            Some(ms) => Some(Duration::from_millis(ms.trim().parse().map_err(|_| DbError::Invalid(format!("statementTimeout must be milliseconds (got '{ms}')")))?)),
            None => None,
        };
        let me = MsConn {
            client: tokio::sync::Mutex::new(client),
            cancel: watch::channel(0).0,
            broken: AtomicBool::new(false),
            in_tx: AtomicBool::new(false),
            version,
            statement_timeout,
            default_schema: get(3).unwrap_or_else(|| "dbo".into()),
        };
        me.set_read_only(read_only).await?;
        Ok(me)
    }

    fn check(&self) -> Result<()> {
        if self.broken.load(Ordering::SeqCst) { Err(DbError::Closed) } else { Ok(()) }
    }

    pub fn default_schema(&self) -> &str {
        &self.default_schema
    }

    /// SQL Server has no read-only session flag: writes are refused by the classifier before
    /// they're sent; read-only sessions also pin the isolation level to READ COMMITTED.
    pub async fn set_read_only(&self, read_only: bool) -> Result<()> {
        if read_only {
            let mut c = self.client.lock().await;
            simple(&mut c, "SET TRANSACTION ISOLATION LEVEL READ COMMITTED").await?;
        }
        Ok(())
    }

    /// Cancels the running statement (TDS attention, sent by the statement's own task).
    pub async fn cancel(&self) -> Result<()> {
        self.cancel.send_modify(|v| *v += 1);
        Ok(())
    }

    /// Sends attention after an interrupted request and drains the acknowledgement; a connection
    /// that can't be recovered is marked broken.
    async fn interrupt(&self, c: &mut Tds) {
        match tokio::time::timeout(Duration::from_secs(10), c.cancel_query()).await {
            Ok(Ok(())) => {}
            _ => self.broken.store(true, Ordering::SeqCst),
        }
    }

    /// Runs one statement (a T-SQL batch) and returns one result per result set — at least one.
    pub async fn run_statement(&self, sql: &str, opts: ExecOptions, read_only_stmt: bool, keyword: &str) -> Result<Vec<StatementResult>> {
        self.check()?;
        let mut c = self.client.lock().await;
        let mut rx = self.cancel.subscribe();
        rx.borrow_and_update();
        // SET ROWCOUNT also caps INSERT/UPDATE/DELETE and SELECT … INTO, which read-only batches
        // may still run against #temp tables and @table variables: only pure reads are capped.
        if read_only_stmt && !["INSERT", "UPDATE", "DELETE", "MERGE", "INTO", "TRUNCATE"].iter().any(|w| has_word(sql, w)) {
            let cap = opts.offset.saturating_add(opts.limit).saturating_add(1).min(i32::MAX as usize);
            simple(&mut c, &format!("SET ROWCOUNT {cap}")).await?;
        }
        let dml = matches!(keyword, "INSERT" | "UPDATE" | "DELETE" | "MERGE") && !has_word(sql, "OUTPUT");
        let outcome = if dml { self.exec_counted(&mut c, sql, &mut rx).await } else { self.stream_sets(&mut c, sql, opts, &mut rx).await };
        if !self.broken.load(Ordering::SeqCst) {
            // Always undo the cap (it would also cap later UPDATE/DELETEs) and refresh the
            // transaction state.
            let after = async {
                let row = c.simple_query("SET ROWCOUNT 0; SELECT @@TRANCOUNT").await.map_err(ms_err)?.into_row().await.map_err(ms_err)?;
                Ok::<_, DbError>(row.and_then(|r| r.try_get::<i32, _>(0).ok().flatten()).unwrap_or(0))
            };
            match after.await {
                Ok(n) => self.in_tx.store(n > 0, Ordering::SeqCst),
                Err(e) if outcome.is_ok() => return Err(e),
                Err(_) => {}
            }
        }
        outcome
    }

    async fn exec_counted(&self, c: &mut Tds, sql: &str, rx: &mut watch::Receiver<u64>) -> Result<Vec<StatementResult>> {
        let deadline = self.statement_timeout.map(|d| tokio::time::Instant::now() + d);
        let res = {
            let run = c.execute(sql, &[]);
            tokio::select! {
                r = run => Some(r),
                _ = rx.changed() => None,
                _ = sleep_until(deadline) => None,
            }
        };
        match res {
            Some(Ok(r)) => Ok(vec![StatementResult { rows_affected: Some(r.total()), ..Default::default() }]),
            Some(Err(e)) => Err(ms_err(e)),
            None => {
                self.interrupt(c).await;
                Err(self.interruption(deadline))
            }
        }
    }

    fn interruption(&self, deadline: Option<tokio::time::Instant>) -> DbError {
        match deadline {
            Some(d) if tokio::time::Instant::now() >= d => DbError::Timeout(format!("{:?} (statementTimeout; the statement was cancelled on the server)", self.statement_timeout.unwrap_or_default())),
            _ => DbError::Cancelled,
        }
    }

    async fn stream_sets(&self, c: &mut Tds, sql: &str, opts: ExecOptions, rx: &mut watch::Receiver<u64>) -> Result<Vec<StatementResult>> {
        let deadline = self.statement_timeout.map(|d| tokio::time::Instant::now() + d);
        let mut sets: Vec<StatementResult> = Vec::new();
        let mut col_types: Vec<ColumnType> = Vec::new();
        let mut skipped = 0usize;
        let mut interrupted = false;
        let mut failure: Option<DbError> = None;
        {
            let started = tokio::select! {
                r = c.simple_query(sql) => Some(r),
                _ = rx.changed() => None,
                _ = sleep_until(deadline) => None,
            };
            match started {
                None => interrupted = true,
                Some(Err(e)) => failure = Some(ms_err(e)),
                Some(Ok(mut stream)) => loop {
                    let item = tokio::select! {
                        it = stream.try_next() => it,
                        _ = rx.changed() => { interrupted = true; break }
                        _ = sleep_until(deadline) => { interrupted = true; break }
                    };
                    match item {
                        Ok(None) => break,
                        Err(e) => {
                            failure = Some(ms_err(e));
                            break;
                        }
                        Ok(Some(QueryItem::Metadata(meta))) => {
                            col_types = meta.columns().iter().map(|c| c.column_type()).collect();
                            let columns = meta.columns().iter().map(|c| ColumnMeta { name: c.name().to_string(), type_name: type_name(c.column_type()).to_string(), kind: kind_of(c.column_type()) }).collect();
                            sets.push(StatementResult { columns, ..Default::default() });
                            skipped = 0;
                        }
                        Ok(Some(QueryItem::Row(row))) => {
                            let Some(cur) = sets.last_mut() else { continue };
                            if skipped < opts.offset {
                                skipped += 1;
                                continue;
                            }
                            if cur.rows.len() >= opts.limit {
                                cur.more = true;
                                continue;
                            }
                            cur.rows.push(row.cells().enumerate().map(|(i, (_, d))| to_json(d, col_types.get(i).copied().unwrap_or(ColumnType::Null))).collect());
                        }
                    }
                },
            }
        }
        if interrupted {
            self.interrupt(c).await;
            return Err(self.interruption(deadline));
        }
        // Paging re-runs the statement and keeps the first result set, so only that one can page.
        if sets.len() > 1 {
            for s in sets.iter_mut().skip(1).filter(|s| s.more) {
                s.more = false;
                s.notices.push(format!("Only the first {} rows are shown: paging works for the first result of a batch — run this query on its own to page it.", opts.limit));
            }
        }
        match failure {
            Some(e) if sets.is_empty() => Err(e),
            Some(e) => {
                // Results that arrived before the error are kept; the error ends the script.
                sets.push(StatementResult { error: Some(e.to_string()), ..Default::default() });
                Ok(sets)
            }
            None => {
                if sets.is_empty() {
                    sets.push(StatementResult::default());
                }
                Ok(sets)
            }
        }
    }

    /// First result set as text cells (metadata queries).
    async fn rows(&self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Option<String>>>> {
        self.check()?;
        let mut c = self.client.lock().await;
        let mut q = Query::new(sql.to_string());
        for p in params {
            bind(&mut q, p);
        }
        let rows = q.query(&mut c).await.map_err(ms_err)?.into_first_result().await.map_err(ms_err)?;
        Ok(rows.iter().map(|r| r.cells().map(|(col, d)| cell_text(d, col.column_type())).collect()).collect())
    }

    pub async fn schema(&self) -> Result<SchemaTree> {
        let schemas = self.rows("SELECT s.name FROM sys.schemas s WHERE s.schema_id < 16384 AND s.name NOT IN ('sys', 'INFORMATION_SCHEMA', 'guest') ORDER BY s.name", &[]).await?;
        let tables = self
            .rows(
                "SELECT s.name, o.name, RTRIM(o.type), (SELECT SUM(p.rows) FROM sys.partitions p WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)) \
                 FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name",
                &[],
            )
            .await?;
        let columns = self.rows(&format!("{COLUMNS_SQL} WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name, c.column_id"), &[]).await?;
        let mut by_table: std::collections::HashMap<(String, String), Vec<ColumnInfo>> = std::collections::HashMap::new();
        for r in columns {
            by_table.entry((r[0].clone().unwrap_or_default(), r[1].clone().unwrap_or_default())).or_default().push(column_info(&r[2..]));
        }
        let mut tree = SchemaTree {
            schemas: schemas.into_iter().map(|r| SchemaNode { name: r[0].clone().unwrap_or_default(), tables: Vec::new() }).collect(),
            default_schema: Some(self.default_schema.clone()),
        };
        for r in tables {
            let (schema, name) = (r[0].clone().unwrap_or_default(), r[1].clone().unwrap_or_default());
            let kind = if r[2].as_deref() == Some("V") { TableKind::View } else { TableKind::Table };
            let node = TableNode { columns: by_table.remove(&(schema.clone(), name.clone())).unwrap_or_default(), name, kind, rows_estimate: r[3].as_deref().and_then(|v| v.parse().ok()) };
            match tree.schemas.iter_mut().find(|s| s.name == schema) {
                Some(s) => s.tables.push(node),
                None => tree.schemas.push(SchemaNode { name: schema, tables: vec![node] }),
            }
        }
        // Empty role schemas (db_owner …) are noise; keep the default schema even when empty.
        let default = self.default_schema.clone();
        tree.schemas.retain(|s| !s.tables.is_empty() || s.name == default);
        Ok(tree)
    }

    pub async fn table_details(&self, schema: Option<&str>, table: &str) -> Result<TableDetails> {
        let schema = schema.unwrap_or(&self.default_schema).to_string();
        let p = [Value::String(schema.clone()), Value::String(table.to_string())];
        let obj = self
            .rows("SELECT CAST(o.object_id AS bigint), RTRIM(o.type) FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id WHERE s.name = @P1 AND o.name = @P2 AND o.type IN ('U', 'V')", &p)
            .await?;
        let Some(o) = obj.first() else { return Err(DbError::NotFound(format!("table {schema}.{table}"))) };
        let id: i64 = o[0].as_deref().and_then(|v| v.parse().ok()).unwrap_or(0);
        let kind = if o[1].as_deref() == Some("V") { TableKind::View } else { TableKind::Table };
        let idp = [Value::from(id)];
        let columns: Vec<ColumnInfo> = self.rows(&format!("{COLUMNS_SQL} WHERE c.object_id = @P1 ORDER BY c.column_id"), &idp).await?.iter().map(|r| column_info(&r[2..])).collect();
        let idx_rows = self
            .rows(
                "SELECT i.name, CAST(i.is_unique AS int), CAST(i.is_primary_key AS int), i.type_desc, c.name, CAST(ic.is_descending_key AS int), CAST(ic.is_included_column AS int), i.filter_definition \
                 FROM sys.indexes i JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                 JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
                 WHERE i.object_id = @P1 AND i.type > 0 AND i.is_hypothetical = 0 ORDER BY i.index_id, ic.is_included_column, ic.key_ordinal, ic.index_column_id",
                &idp,
            )
            .await?;
        let mut indexes: Vec<IndexInfo> = Vec::new();
        let mut included: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        let mut filters: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for r in idx_rows {
            let name = r[0].clone().unwrap_or_default();
            let col = r[4].clone().unwrap_or_default();
            if r[6].as_deref() == Some("1") {
                included.entry(name).or_default().push(col);
                continue;
            }
            let col = if r[5].as_deref() == Some("1") { format!("{col} DESC") } else { col };
            if let Some(f) = r[7].clone() {
                filters.insert(name.clone(), f);
            }
            match indexes.last_mut() {
                Some(i) if i.name == name => i.columns.push(col),
                _ => indexes.push(IndexInfo {
                    unique: r[1].as_deref() == Some("1"),
                    primary: r[2].as_deref() == Some("1"),
                    method: r[3].as_deref().map(|t| t.to_ascii_lowercase().replace('_', " ")),
                    definition: None,
                    columns: vec![col],
                    name,
                }),
            }
        }
        let qt = qualified(Some(&schema), table, Dialect::Sqlserver);
        for i in indexes.iter_mut().filter(|i| !i.primary) {
            let mut def = format!(
                "CREATE {}{}INDEX {} ON {qt} ({})",
                if i.unique { "UNIQUE " } else { "" },
                if i.method.as_deref().is_some_and(|m| m.starts_with("clustered")) { "CLUSTERED " } else { "" },
                quote_ident(&i.name, Dialect::Sqlserver),
                i.columns.iter().map(|c| ident_with_order(c)).collect::<Vec<_>>().join(", ")
            );
            if let Some(inc) = included.get(&i.name) {
                def.push_str(&format!(" INCLUDE ({})", inc.iter().map(|c| quote_ident(c, Dialect::Sqlserver)).collect::<Vec<_>>().join(", ")));
            }
            if let Some(f) = filters.get(&i.name) {
                def.push_str(&format!(" WHERE {f}"));
            }
            i.definition = Some(def);
        }
        let fk_rows = self
            .rows(
                "SELECT fk.name, pc.name, rs.name, rt.name, rc.name, fk.update_referential_action_desc, fk.delete_referential_action_desc \
                 FROM sys.foreign_keys fk JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
                 JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
                 JOIN sys.objects rt ON rt.object_id = fkc.referenced_object_id JOIN sys.schemas rs ON rs.schema_id = rt.schema_id \
                 JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
                 WHERE fk.parent_object_id = @P1 ORDER BY fk.name, fkc.constraint_column_id",
                &idp,
            )
            .await?;
        let mut foreign_keys: Vec<ForeignKeyInfo> = Vec::new();
        for r in fk_rows {
            let name = r[0].clone().unwrap_or_default();
            let (col, rcol) = (r[1].clone().unwrap_or_default(), r[4].clone().unwrap_or_default());
            let action = |a: &Option<String>| a.clone().map(|x| x.replace('_', " ")).filter(|x| x != "NO ACTION");
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
                    on_update: action(&r[5]),
                    on_delete: action(&r[6]),
                }),
            }
        }
        let primary_key: Vec<String> = indexes.iter().find(|i| i.primary).map(|i| i.columns.iter().map(|c| c.trim_end_matches(" DESC").to_string()).collect()).unwrap_or_default();
        let ddl = if kind == TableKind::View {
            self.rows("SELECT OBJECT_DEFINITION(@P1)", &idp).await?.first().and_then(|r| r[0].clone())
        } else {
            let checks = self.rows("SELECT name, definition FROM sys.check_constraints WHERE parent_object_id = @P1 ORDER BY name", &idp).await.unwrap_or_default();
            let identity = self
                .rows("SELECT c.name, CAST(ic.seed_value AS nvarchar(40)), CAST(ic.increment_value AS nvarchar(40)) FROM sys.identity_columns ic JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id WHERE ic.object_id = @P1", &idp)
                .await
                .unwrap_or_default();
            let pk_name = indexes.iter().find(|i| i.primary).map(|i| i.name.clone());
            Some(table_ddl(&qt, &columns, pk_name.as_deref(), &primary_key, &foreign_keys, &checks, &identity, &indexes))
        };
        Ok(TableDetails { schema: Some(schema), name: table.to_string(), kind, columns, primary_key, indexes, foreign_keys, ddl })
    }

    pub async fn explain(&self, sql: &str, analyze: bool, in_tx: bool) -> Result<ExplainOutput> {
        self.check()?;
        let mut c = self.client.lock().await;
        simple(&mut c, "SET ROWCOUNT 0").await?;
        let wrap = analyze && !in_tx;
        let (on, off) = if analyze { ("SET STATISTICS XML ON", "SET STATISTICS XML OFF") } else { ("SET SHOWPLAN_XML ON", "SET SHOWPLAN_XML OFF") };
        if wrap {
            simple(&mut c, "BEGIN TRANSACTION").await?;
        }
        simple(&mut c, on).await?;
        let res = async {
            // Streamed: with STATISTICS XML the statement's own rows come first and are dropped.
            let mut stream = c.simple_query(sql).await.map_err(ms_err)?;
            let mut plans = Vec::new();
            while let Some(item) = stream.try_next().await.map_err(ms_err)? {
                if let QueryItem::Row(r) = item
                    && let Some((_, d)) = r.cells().next()
                {
                    let text = match d {
                        ColumnData::String(Some(x)) => Some(x.to_string()),
                        ColumnData::Xml(Some(x)) => Some(x.to_string()),
                        _ => None,
                    };
                    if let Some(x) = text.filter(|x| x.trim_start().starts_with("<ShowPlanXML")) {
                        plans.push(x);
                    }
                }
            }
            Ok::<_, DbError>(plans)
        }
        .await;
        let _ = simple(&mut c, off).await;
        if wrap {
            let _ = simple(&mut c, "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION").await;
        }
        let plans = res?;
        if plans.is_empty() {
            return Err(DbError::Query("SQL Server returned no plan for this statement".into()));
        }
        let mut statements = Vec::new();
        for p in &plans {
            statements.extend(parse_showplan(p)?);
        }
        Ok(ExplainOutput { format: "sqlserver".into(), plan: Value::Array(statements), text: Some(plans.join("\n")), rolled_back: wrap })
    }

    /// Parameterized statements in one transaction (grid edits, inserts, seeding).
    pub async fn exec_params(&self, stmts: &[ParamStatement]) -> Result<Vec<u64>> {
        self.check()?;
        let mut c = self.client.lock().await;
        simple(&mut c, "SET ROWCOUNT 0; BEGIN TRANSACTION").await?;
        let mut out = Vec::with_capacity(stmts.len());
        for s in stmts {
            let mut q = Query::new(s.sql.clone());
            for p in &s.params {
                bind(&mut q, p);
            }
            match q.execute(&mut c).await {
                Ok(r) => {
                    // A trigger's own counts come first; the statement's count is the last one.
                    let n = if s.expect_one { r.rows_affected().last().copied().unwrap_or(0) } else { r.total() };
                    if s.expect_one && n != 1 {
                        let _ = simple(&mut c, "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION").await;
                        return Err(expect_one_error(n, &s.preview));
                    }
                    out.push(n);
                }
                Err(e) => {
                    let _ = simple(&mut c, "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION").await;
                    return Err(DbError::Query(format!("{}\n→ {}", s.preview, ms_err(e))));
                }
            }
        }
        simple(&mut c, "COMMIT TRANSACTION").await?;
        Ok(out)
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

/// Columns with display types (`nvarchar(50)`, `decimal(10,2)`, `datetime2(3)`), nullability,
/// defaults and primary-key membership. Row: schema, table, name, type, nullable, default, pk.
const COLUMNS_SQL: &str = "SELECT s.name, o.name, c.name, \
    CASE WHEN t.name IN ('varchar', 'char', 'varbinary', 'binary') THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length AS varchar(10)) END + ')' \
         WHEN t.name IN ('nvarchar', 'nchar') THEN t.name + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length / 2 AS varchar(10)) END + ')' \
         WHEN t.name IN ('decimal', 'numeric') THEN t.name + '(' + CAST(c.precision AS varchar(10)) + ',' + CAST(c.scale AS varchar(10)) + ')' \
         WHEN t.name IN ('datetime2', 'time', 'datetimeoffset') THEN t.name + '(' + CAST(c.scale AS varchar(10)) + ')' \
         ELSE t.name END, \
    CAST(c.is_nullable AS int), OBJECT_DEFINITION(c.default_object_id), \
    CASE WHEN EXISTS (SELECT 1 FROM sys.index_columns ic JOIN sys.indexes i ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
         WHERE i.is_primary_key = 1 AND ic.object_id = c.object_id AND ic.column_id = c.column_id) THEN 1 ELSE 0 END \
    FROM sys.columns c JOIN sys.objects o ON o.object_id = c.object_id JOIN sys.schemas s ON s.schema_id = o.schema_id \
    JOIN sys.types t ON t.user_type_id = c.user_type_id";

fn column_info(r: &[Option<String>]) -> ColumnInfo {
    ColumnInfo {
        name: r[0].clone().unwrap_or_default(),
        data_type: r[1].clone().unwrap_or_default(),
        nullable: r[2].as_deref() == Some("1"),
        default: r[3].clone().map(|d| strip_parens(&d)),
        primary_key: r[4].as_deref() == Some("1"),
    }
}

/// `((0))` → `0`, `(getdate())` → `getdate()`.
fn strip_parens(s: &str) -> String {
    let mut t = s.trim();
    while t.starts_with('(') && t.ends_with(')') && balanced_inner(&t[1..t.len() - 1]) {
        t = t[1..t.len() - 1].trim();
    }
    t.to_string()
}

fn balanced_inner(s: &str) -> bool {
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn ident_with_order(col: &str) -> String {
    match col.strip_suffix(" DESC") {
        Some(c) => format!("{} DESC", quote_ident(c, Dialect::Sqlserver)),
        None => quote_ident(col, Dialect::Sqlserver),
    }
}

/// A best-effort `CREATE TABLE` (SQL Server has no built-in DDL export): columns with identity,
/// nullability and defaults, primary key, foreign keys, CHECK constraints, then the other indexes.
#[allow(clippy::too_many_arguments)]
fn table_ddl(
    qt: &str,
    columns: &[ColumnInfo],
    pk_name: Option<&str>,
    pk: &[String],
    fks: &[ForeignKeyInfo],
    checks: &[Vec<Option<String>>],
    identity: &[Vec<Option<String>>],
    indexes: &[IndexInfo],
) -> String {
    let q = |s: &str| quote_ident(s, Dialect::Sqlserver);
    let mut lines: Vec<String> = Vec::new();
    for c in columns {
        let mut l = format!("  {} {}", q(&c.name), c.data_type);
        if let Some(id) = identity.iter().find(|r| r[0].as_deref() == Some(c.name.as_str())) {
            l.push_str(&format!(" IDENTITY({},{})", id[1].as_deref().unwrap_or("1"), id[2].as_deref().unwrap_or("1")));
        }
        l.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
        if let Some(d) = &c.default {
            l.push_str(&format!(" DEFAULT {d}"));
        }
        lines.push(l);
    }
    if !pk.is_empty() {
        let name = pk_name.map(|n| format!("CONSTRAINT {} ", q(n))).unwrap_or_default();
        lines.push(format!("  {name}PRIMARY KEY ({})", pk.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ")));
    }
    for f in fks {
        let target = match &f.ref_schema {
            Some(s) => format!("{}.{}", q(s), q(&f.ref_table)),
            None => q(&f.ref_table),
        };
        let mut l = format!(
            "  CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {target} ({})",
            q(&f.name),
            f.columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", "),
            f.ref_columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ")
        );
        if let Some(a) = &f.on_delete {
            l.push_str(&format!(" ON DELETE {a}"));
        }
        if let Some(a) = &f.on_update {
            l.push_str(&format!(" ON UPDATE {a}"));
        }
        lines.push(l);
    }
    for ch in checks {
        if let (Some(n), Some(def)) = (&ch[0], &ch[1]) {
            lines.push(format!("  CONSTRAINT {} CHECK {def}", q(n)));
        }
    }
    let mut out = format!("CREATE TABLE {qt} (\n{}\n);", lines.join(",\n"));
    for i in indexes.iter().filter(|i| !i.primary) {
        if let Some(d) = &i.definition {
            out.push_str(&format!("\n{d};"));
        }
    }
    out
}

// ---------- SHOWPLAN XML ----------

fn attr_f64(v: Option<&String>) -> Option<f64> {
    v.and_then(|s| s.parse::<f64>().ok())
}

fn unbracket(s: &str) -> String {
    s.trim_start_matches('[').trim_end_matches(']').replace("]]", "]")
}

#[derive(Default)]
struct PlanNode {
    attrs: std::collections::HashMap<String, String>,
    relation: Option<String>,
    index: Option<String>,
    filter: Option<String>,
    actual_rows: Option<f64>,
    actual_ms: Option<f64>,
    executions: Option<f64>,
    children: Vec<Value>,
}

impl PlanNode {
    fn into_json(self) -> Value {
        let mut m = serde_json::Map::new();
        let physical = self.attrs.get("PhysicalOp").cloned().unwrap_or_else(|| "?".into());
        let logical = self.attrs.get("LogicalOp").cloned().unwrap_or_default();
        let node_type = if !logical.is_empty() && logical != physical { format!("{physical} ({logical})") } else { physical };
        m.insert("Node Type".into(), Value::String(node_type));
        if let Some(c) = attr_f64(self.attrs.get("EstimatedTotalSubtreeCost")) {
            m.insert("Total Cost".into(), json!((c * 1e6).round() / 1e6));
        }
        if let Some(r) = attr_f64(self.attrs.get("EstimateRows")) {
            m.insert("Plan Rows".into(), json!((r * 100.0).round() / 100.0));
        }
        if let Some(r) = &self.relation {
            m.insert("Relation Name".into(), Value::String(r.clone()));
        }
        if let Some(i) = &self.index {
            m.insert("Index Name".into(), Value::String(i.clone()));
        }
        if let Some(f) = &self.filter {
            m.insert("Filter".into(), Value::String(f.clone()));
        }
        if let Some(r) = self.actual_rows {
            m.insert("Actual Rows".into(), json!(r));
        }
        if let Some(t) = self.actual_ms {
            m.insert("Actual Total Time".into(), json!(t));
        }
        if let Some(e) = self.executions {
            m.insert("Actual Loops".into(), json!(e));
        }
        for k in ["EstimateIO", "EstimateCPU", "Parallel", "NodeId"] {
            if let Some(v) = self.attrs.get(k) {
                m.insert(k.into(), Value::String(v.clone()));
            }
        }
        if !self.children.is_empty() {
            m.insert("Plans".into(), Value::Array(self.children));
        }
        Value::Object(m)
    }
}

/// Parses `<ShowPlanXML>` into one `{ "Statement", "Plan": {…} }` per statement, with nodes
/// shaped like Postgres' JSON plans (`Node Type`, `Total Cost`, `Plan Rows`, `Relation Name`,
/// `Index Name`, `Filter`, `Actual Rows`, `Actual Total Time`, `Actual Loops`, `Plans`) so the
/// plan tree view draws both.
pub fn parse_showplan(xml: &str) -> Result<Vec<Value>> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = Vec::new();
    let mut stmt: Option<(String, Option<f64>)> = None;
    let mut stack: Vec<PlanNode> = Vec::new();
    let mut root: Option<Value> = None;
    // Inside a <Predicate>/<SeekPredicates> of the current operator (and how deep).
    let mut predicate_depth: Option<usize> = None;
    let mut depth = 0usize;
    let attrs_of = |e: &quick_xml::events::BytesStart<'_>| -> std::collections::HashMap<String, String> {
        e.attributes()
            .flatten()
            .map(|a| {
                let k = a.key.local_name().as_ref().to_string();
                let v = quick_xml::escape::unescape(&a.value).map(|c| c.into_owned()).unwrap_or_else(|_| a.value.to_string());
                (k, v)
            })
            .collect()
    };
    loop {
        let ev = reader.read_event().map_err(|e| DbError::Query(format!("unreadable SHOWPLAN XML: {e}")))?;
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e.clone()), false),
            Event::Empty(e) => (Some(e.clone()), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            let name = e.local_name().as_ref().to_string();
            if !empty {
                depth += 1;
            }
            match name.as_str() {
                "StmtSimple" | "StmtCond" | "StmtCursor" | "StmtUseDb" => {
                    let a = attrs_of(&e);
                    stmt = Some((a.get("StatementText").cloned().unwrap_or_default(), attr_f64(a.get("StatementSubTreeCost"))));
                    root = None;
                }
                "RelOp" => {
                    stack.push(PlanNode { attrs: attrs_of(&e), ..Default::default() });
                    predicate_depth = None;
                }
                "Object" => {
                    if let Some(n) = stack.last_mut()
                        && n.relation.is_none()
                    {
                        let a = attrs_of(&e);
                        if let Some(t) = a.get("Table") {
                            n.relation = Some(match a.get("Schema") {
                                Some(s) => format!("{}.{}", unbracket(s), unbracket(t)),
                                None => unbracket(t),
                            });
                        }
                        n.index = a.get("Index").map(|i| unbracket(i));
                    }
                }
                "Predicate" | "SeekPredicates" | "SeekPredicateNew" if !empty => {
                    if predicate_depth.is_none() {
                        predicate_depth = Some(depth);
                    }
                }
                "ScalarOperator" => {
                    if predicate_depth.is_some()
                        && let Some(n) = stack.last_mut()
                        && n.filter.is_none()
                    {
                        n.filter = attrs_of(&e).get("ScalarString").cloned();
                    }
                }
                "RunTimeCountersPerThread" => {
                    if let Some(n) = stack.last_mut() {
                        let a = attrs_of(&e);
                        if let Some(r) = attr_f64(a.get("ActualRows")) {
                            n.actual_rows = Some(n.actual_rows.unwrap_or(0.0) + r);
                        }
                        if let Some(x) = attr_f64(a.get("ActualExecutions")) {
                            n.executions = Some(n.executions.unwrap_or(0.0) + x);
                        }
                        if let Some(t) = attr_f64(a.get("ActualElapsedms")) {
                            n.actual_ms = Some(n.actual_ms.unwrap_or(0.0).max(t));
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        match ev {
            Event::End(e) => {
                let name = e.local_name().as_ref().to_string();
                if predicate_depth == Some(depth) {
                    predicate_depth = None;
                }
                depth = depth.saturating_sub(1);
                match name.as_str() {
                    "RelOp" => {
                        if let Some(n) = stack.pop() {
                            let v = n.into_json();
                            match stack.last_mut() {
                                Some(parent) => parent.children.push(v),
                                None => root = Some(v),
                            }
                        }
                    }
                    "StmtSimple" | "StmtCond" | "StmtCursor" | "StmtUseDb" => {
                        if let Some((text, cost)) = stmt.take() {
                            let mut m = serde_json::Map::new();
                            m.insert("Statement".into(), Value::String(text));
                            if let Some(c) = cost {
                                m.insert("Total Cost".into(), json!(c));
                            }
                            if let Some(r) = root.take() {
                                m.insert("Plan".into(), r);
                                out.push(Value::Object(m));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiberius::numeric::Numeric;
    use tiberius::time::{Date, DateTime, DateTime2, DateTimeOffset, SmallDateTime, Time};

    #[test]
    fn values_are_typed() {
        assert_eq!(to_json(&ColumnData::I64(Some(9_007_199_254_740_993)), ColumnType::Int8), json!("9007199254740993"));
        assert_eq!(to_json(&ColumnData::I32(Some(-4)), ColumnType::Int4), json!(-4));
        assert_eq!(to_json(&ColumnData::Numeric(Some(Numeric::new_with_scale(123_456, 2))), ColumnType::Decimaln), json!("1234.56"));
        assert_eq!(to_json(&ColumnData::F64(Some(12.5)), ColumnType::Money), json!("12.5000"));
        assert_eq!(to_json(&ColumnData::F64(Some(0.25)), ColumnType::Float8), json!(0.25));
        assert_eq!(to_json(&ColumnData::Bit(Some(true)), ColumnType::Bitn), json!(true));
        assert_eq!(to_json(&ColumnData::Binary(Some(vec![0u8, 255].into())), ColumnType::BigVarBin), json!("AP8="));
        assert_eq!(to_json(&ColumnData::String(None), ColumnType::NVarchar), Value::Null);
        let g = tiberius::Uuid::parse_str("6f9619ff-8b86-d011-b42d-00c04fc964ff").unwrap();
        assert_eq!(to_json(&ColumnData::Guid(Some(g)), ColumnType::Guid), json!("6F9619FF-8B86-D011-B42D-00C04FC964FF"));
        // date: days since 0001-01-01.
        assert_eq!(to_json(&ColumnData::Date(Some(Date::new(739_005))), ColumnType::Daten), json!("2024-04-30"));
        assert_eq!(to_json(&ColumnData::Time(Some(Time::new(372_301_234_567, 7))), ColumnType::Timen), json!("10:20:30.1234567"));
        let dt2 = DateTime2::new(Date::new(739_005), Time::new(37_230_123, 3));
        assert_eq!(to_json(&ColumnData::DateTime2(Some(dt2)), ColumnType::Datetime2), json!("2024-04-30T10:20:30.123"));
        // Stored as UTC 2024-04-30T23:30:00 with +02:00 → local 2024-05-01T01:30:00+02:00.
        let utc = DateTime2::new(Date::new(739_005), Time::new(84_600, 0));
        assert_eq!(to_json(&ColumnData::DateTimeOffset(Some(DateTimeOffset::new(utc, 120))), ColumnType::DatetimeOffsetn), json!("2024-05-01T01:30:00+02:00"));
        assert_eq!(to_json(&ColumnData::DateTimeOffset(Some(DateTimeOffset::new(utc, -330))), ColumnType::DatetimeOffsetn), json!("2024-04-30T18:00:00-05:30"));
        // datetime: days since 1900-01-01 + 1/300 s ticks; 10:20:30.123 ≈ 11_169_037 ticks.
        assert_eq!(to_json(&ColumnData::DateTime(Some(DateTime::new(45_410, 11_169_037))), ColumnType::Datetimen), json!("2024-04-30T10:20:30.123"));
        assert_eq!(to_json(&ColumnData::SmallDateTime(Some(SmallDateTime::new(45_410, 620))), ColumnType::Datetime4), json!("2024-04-30T10:20:00"));
        assert_eq!(fmt_date(-DAYS_0001_TO_1970), "0001-01-01");
        assert_eq!(kind_of(ColumnType::Money), ValueKind::Decimal);
        assert_eq!(kind_of(ColumnType::DatetimeOffsetn), ValueKind::Timestamp);
        assert_eq!(type_name(ColumnType::NVarchar), "nvarchar");
    }

    #[test]
    fn showplan_to_tree() {
        let xml = r#"<?xml version="1.0" encoding="utf-16"?>
<ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan" Version="1.564" Build="16.0.1000.6">
 <BatchSequence><Batch><Statements>
  <StmtSimple StatementText="SELECT * FROM dbo.orders o JOIN dbo.users u ON u.id = o.user_id WHERE o.total &gt; 10" StatementSubTreeCost="0.0067" StatementEstRows="3">
   <QueryPlan>
    <RelOp NodeId="0" PhysicalOp="Nested Loops" LogicalOp="Inner Join" EstimateRows="3" EstimatedTotalSubtreeCost="0.0067">
     <RunTimeInformation><RunTimeCountersPerThread Thread="0" ActualRows="2" ActualExecutions="1" ActualElapsedms="1" /></RunTimeInformation>
     <NestedLoops Optimized="false">
      <RelOp NodeId="1" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="3" EstimatedTotalSubtreeCost="0.0032">
       <IndexScan Ordered="false">
        <Object Database="[shop]" Schema="[dbo]" Table="[orders]" Index="[PK_orders]" Alias="[o]" />
        <Predicate><ScalarOperator ScalarString="[shop].[dbo].[orders].[total] as [o].[total]&gt;(10)" /></Predicate>
       </IndexScan>
      </RelOp>
      <RelOp NodeId="2" PhysicalOp="Clustered Index Seek" LogicalOp="Clustered Index Seek" EstimateRows="1" EstimatedTotalSubtreeCost="0.0031">
       <IndexScan><Object Database="[shop]" Schema="[dbo]" Table="[users]" Index="[PK_users]" /></IndexScan>
      </RelOp>
     </NestedLoops>
    </RelOp>
   </QueryPlan>
  </StmtSimple>
 </Statements></Batch></BatchSequence>
</ShowPlanXML>"#;
        let plans = parse_showplan(xml).unwrap();
        assert_eq!(plans.len(), 1);
        let p = &plans[0]["Plan"];
        assert_eq!(p["Node Type"], "Nested Loops (Inner Join)");
        assert_eq!(p["Actual Rows"], 2.0);
        assert_eq!(p["Plans"][0]["Node Type"], "Clustered Index Scan");
        assert_eq!(p["Plans"][0]["Relation Name"], "dbo.orders");
        assert_eq!(p["Plans"][0]["Index Name"], "PK_orders");
        assert_eq!(p["Plans"][0]["Filter"], "[shop].[dbo].[orders].[total] as [o].[total]>(10)");
        assert_eq!(p["Plans"][1]["Relation Name"], "dbo.users");
        assert!(p["Plans"][1].get("Filter").is_none(), "predicates don't leak into siblings");
        assert!(plans[0]["Statement"].as_str().unwrap().contains("total > 10"));
        assert!(parse_showplan("<ShowPlanXML><unclosed").is_err() || parse_showplan("<ShowPlanXML><unclosed").unwrap().is_empty());
    }

    #[test]
    fn ddl_and_defaults() {
        assert_eq!(strip_parens("((0))"), "0");
        assert_eq!(strip_parens("(getdate())"), "getdate()");
        assert_eq!(strip_parens("(1)+(2)"), "(1)+(2)");
        let cols = vec![
            ColumnInfo { name: "id".into(), data_type: "int".into(), nullable: false, default: None, primary_key: true },
            ColumnInfo { name: "status".into(), data_type: "nvarchar(20)".into(), nullable: false, default: Some("N'open'".into()), primary_key: false },
        ];
        let identity = vec![vec![Some("id".to_string()), Some("1".to_string()), Some("1".to_string())]];
        let checks = vec![vec![Some("CK_status".to_string()), Some("([status]=N'closed' OR [status]=N'open')".to_string())]];
        let ddl = table_ddl("[dbo].[t]", &cols, Some("PK_t"), &["id".into()], &[], &checks, &identity, &[]);
        assert!(ddl.contains("[id] int IDENTITY(1,1) NOT NULL"), "{ddl}");
        assert!(ddl.contains("CONSTRAINT [PK_t] PRIMARY KEY ([id])"));
        let enums = devall_data::fromdb::check_enum_values(&ddl);
        assert_eq!(enums.get("status").unwrap(), &vec!["closed".to_string(), "open".into()]);
    }
}
