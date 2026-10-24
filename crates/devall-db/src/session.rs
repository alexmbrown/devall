//! An open connection ("session"): engine + optional SSH tunnel + read-only state + the running
//! query's cancel flag. Hosts (the Tauri app, the CLI) keep sessions by id.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{DbError, Result, invalid};
use crate::model::DbKind;
use crate::mongo::MongoConn;
use crate::redisdb::RedisConn;
use crate::resolve::ResolvedConnection;
use crate::sql::edit::{EditPreview, EditRequest, build_edits, insert_batches};
use crate::sql::lex::{Dialect, classify, read_only_message, split};
use crate::sql::mssql::MsConn;
use crate::sql::mysql::MyConn;
use crate::sql::pg::PgConn;
use crate::sql::sqlite::LiteConn;
use crate::sql::types::{ExecOptions, QueryOutput, rows_to_objects};
use crate::sql::{CancelFlag, ProgressFn, SqlConn, run_script};
use crate::ssh::{Tunnel, TunnelManager};

/// What the UI knows about an open session.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub kind: DbKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialect: Option<Dialect>,
    pub read_only: bool,
    pub protected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub server_version: String,
    /// `host:port/db` or the SQLite file (never credentials).
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// `user@bastion:22` when connected through an SSH tunnel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<String>,
    pub connected_at: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    /// SHA-256 fingerprint of an SSH host key the user confirmed (trust on first use).
    pub trust_host_key: Option<String>,
}

pub enum Engine {
    Sql(SqlConn),
    Redis(Box<RedisConn>),
    Mongo(MongoConn),
}

pub struct Session {
    pub id: String,
    // Field order matters: the engine (its sockets) drops before the tunnel.
    engine: Engine,
    info: Mutex<SessionInfo>,
    read_only: AtomicBool,
    secrets: BTreeSet<String>,
    running: Mutex<Option<CancelFlag>>,
    _tunnel: Option<Tunnel>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedResult {
    pub rows: u64,
    pub target: String,
    pub elapsed_ms: f64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

const MAX_SEED_ROWS: usize = 1_000_000;

impl Session {
    /// Opens a session (and its SSH tunnel, shared through `tunnels`).
    pub async fn connect(rc: ResolvedConnection, tunnels: &TunnelManager, opts: ConnectOptions) -> Result<Session> {
        let timeout = rc.connect_timeout();
        let mut warnings = Vec::new();
        let (host, port, tunnel) = match (&rc.ssh, rc.kind) {
            (Some(_), DbKind::Sqlite) => {
                warnings.push("SSH settings are ignored for SQLite (the file must be local).".into());
                (rc.host.clone(), rc.port, None)
            }
            (Some(ssh), _) => {
                let t = tunnels.acquire(ssh, &rc.host, rc.port, opts.trust_host_key.as_deref(), timeout).await?;
                ("127.0.0.1".to_string(), t.local_port(), Some(t))
            }
            (None, _) => (rc.host.clone(), rc.port, None),
        };
        let read_only = rc.read_only;
        let engine = match rc.kind {
            DbKind::Postgres => Engine::Sql(SqlConn::Pg(Box::new(PgConn::connect(&rc, &host, port, read_only).await?))),
            DbKind::Mysql => Engine::Sql(SqlConn::My(MyConn::connect(&rc, &host, port, read_only).await?)),
            DbKind::Sqlserver => Engine::Sql(SqlConn::Ms(Box::new(MsConn::connect(&rc, &host, port, read_only).await?))),
            DbKind::Sqlite => {
                let db = rc.database.clone().ok_or_else(|| invalid("set the SQLite database file"))?;
                let create = rc.option("create").is_some_and(|v| matches!(v.trim(), "true" | "1" | "yes"));
                Engine::Sql(SqlConn::Lite(LiteConn::open(&db, read_only, create).await?))
            }
            DbKind::Redis => Engine::Redis(Box::new(RedisConn::connect(&rc, &host, port).await?)),
            DbKind::Mongodb => Engine::Mongo(MongoConn::connect(&rc, &host, port).await?),
        };
        let (server_version, dialect, database) = match &engine {
            Engine::Sql(c) => (c.server_version().to_string(), Some(c.dialect()), rc.database.clone()),
            Engine::Redis(c) => {
                warnings.extend(c.warnings.iter().cloned());
                (c.version.clone(), None, rc.database.clone())
            }
            Engine::Mongo(c) => (c.version.clone(), None, c.default_db.clone()),
        };
        let id = ulid::Ulid::generate().to_string();
        let info = SessionInfo {
            id: id.clone(),
            name: rc.name.clone(),
            kind: rc.kind,
            dialect,
            read_only,
            protected: rc.protected,
            color: rc.color.clone(),
            server_version,
            target: rc.display_target(),
            database,
            tunnel: rc.ssh.as_ref().filter(|_| tunnel.is_some()).map(|s| format!("{}@{}:{}", s.user, s.host, s.port)),
            connected_at: now_ms(),
            warnings,
        };
        Ok(Session {
            id,
            engine,
            info: Mutex::new(info),
            read_only: AtomicBool::new(read_only),
            secrets: rc.secret_values,
            running: Mutex::new(None),
            _tunnel: tunnel,
        })
    }

    pub fn info(&self) -> SessionInfo {
        self.info.lock().unwrap().clone()
    }

    pub fn kind(&self) -> DbKind {
        self.info.lock().unwrap().kind
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::SeqCst)
    }

    /// The error text with this session's secret values masked.
    pub fn redact(&self, e: &DbError) -> String {
        e.redacted(&self.secrets)
    }

    pub fn secrets(&self) -> &BTreeSet<String> {
        &self.secrets
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn sql(&self) -> Result<&SqlConn> {
        match &self.engine {
            Engine::Sql(c) => Ok(c),
            _ => Err(invalid("not a SQL connection")),
        }
    }

    pub fn redis(&self) -> Result<&RedisConn> {
        match &self.engine {
            Engine::Redis(c) => Ok(c),
            _ => Err(invalid("not a Redis connection")),
        }
    }

    pub fn mongo(&self) -> Result<&MongoConn> {
        match &self.engine {
            Engine::Mongo(c) => Ok(c),
            _ => Err(invalid("not a MongoDB connection")),
        }
    }

    /// Switches read-only mode. Turning it off on a protected connection requires `confirm` to
    /// equal the connection name (the UI asks the user to type it).
    pub async fn set_read_only(&self, read_only: bool, confirm: Option<&str>) -> Result<SessionInfo> {
        let (protected, name) = {
            let i = self.info.lock().unwrap();
            (i.protected, i.name.clone())
        };
        if !read_only && protected && confirm.map(str::trim) != Some(name.trim()) {
            return Err(invalid(format!("'{name}' is protected: type its name to allow writes")));
        }
        if let Engine::Sql(c) = &self.engine {
            c.set_read_only(read_only).await?;
        }
        self.read_only.store(read_only, Ordering::SeqCst);
        let mut i = self.info.lock().unwrap();
        i.read_only = read_only;
        Ok(i.clone())
    }

    fn require_writable(&self, what: &str) -> Result<()> {
        if self.is_read_only() {
            return Err(DbError::ReadOnly(format!("Read-only session: {what} needs write access. Turn off read-only for this connection first.")));
        }
        Ok(())
    }

    /// Runs a SQL script (see [`run_script`]); `cancel` stops it.
    pub async fn query(&self, sql: &str, opts: ExecOptions, progress: Option<ProgressFn<'_>>) -> Result<QueryOutput> {
        let conn = self.sql()?;
        let flag = CancelFlag::default();
        *self.running.lock().unwrap() = Some(flag.clone());
        let out = run_script(conn, sql, opts, self.is_read_only(), progress, &flag).await;
        *self.running.lock().unwrap() = None;
        out
    }

    /// Cancels the running SQL script, if any. Returns whether something was running.
    pub async fn cancel(&self) -> Result<bool> {
        let flag = self.running.lock().unwrap().clone();
        let Some(flag) = flag else { return Ok(false) };
        flag.cancel();
        if let Engine::Sql(c) = &self.engine {
            c.cancel().await?;
        }
        Ok(true)
    }

    pub fn is_running(&self) -> bool {
        self.running.lock().unwrap().is_some()
    }

    pub fn edit_preview(&self, req: &EditRequest) -> Result<EditPreview> {
        build_edits(self.sql()?.dialect(), req)
    }

    /// Applies grid edits in one transaction; returns rows affected per statement.
    pub async fn edit_apply(&self, req: &EditRequest) -> Result<Vec<u64>> {
        self.require_writable("editing rows")?;
        let preview = self.edit_preview(req)?;
        self.sql()?.exec_params(&preview.statements).await
    }

    /// Inserts rows (batched, parameterized, one transaction). SQL tables, MongoDB collections
    /// (`schema` = database) or Redis hashes (`table` = key prefix, keyed by `id`/`_id` or index).
    pub async fn insert_rows(&self, schema: Option<&str>, table: &str, rows: Vec<Map<String, Value>>) -> Result<u64> {
        self.require_writable("inserting rows")?;
        if rows.len() > MAX_SEED_ROWS {
            return Err(invalid(format!("at most {MAX_SEED_ROWS} rows per insert")));
        }
        match &self.engine {
            Engine::Sql(c) => {
                let stmts = insert_batches(c.dialect(), schema, table, &rows)?;
                Ok(c.exec_params(&stmts).await?.iter().sum())
            }
            Engine::Mongo(m) => {
                let db = schema.map(str::to_string).or_else(|| m.default_db.clone()).ok_or_else(|| invalid("choose a database"))?;
                let docs = serde_json::to_string(&rows).map_err(|e| invalid(e.to_string()))?;
                let r = m.write(&crate::mongo::MongoWrite::Insert { database: db, collection: table.to_string(), documents: docs }).await?;
                Ok(r["insertedCount"].as_u64().unwrap_or(0))
            }
            Engine::Redis(r) => r.hset_rows(table, &rows).await,
        }
    }

    /// Generates `entity` from `data/<data_path>` with the fake-data engine and inserts it.
    pub async fn seed_from_schema(&self, project_root: &Path, data_path: &str, entity: &str, schema: Option<&str>, table: &str, count: Option<u64>) -> Result<SeedResult> {
        self.require_writable("seeding")?;
        let started = std::time::Instant::now();
        let file = devall_data::store::read(project_root, data_path)?;
        if !file.entities.iter().any(|e| e.name == entity) {
            return Err(DbError::NotFound(format!("entity '{entity}' in data/{data_path}")));
        }
        let opts = devall_data::GenerateOptions { entity: Some(entity.to_string()), count_override: count, limit: None, seed: None };
        let entity_name = entity.to_string();
        let mut data = tokio::task::spawn_blocking(move || devall_data::generate_values(&file, &opts)).await.map_err(|e| invalid(e.to_string()))??;
        let rows: Vec<Map<String, Value>> = data
            .swap_remove(&entity_name)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .collect();
        let n = self.insert_rows(schema, table, rows).await?;
        let target = match schema {
            Some(s) => format!("{s}.{table}"),
            None => table.to_string(),
        };
        Ok(SeedResult { rows: n, target, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 })
    }

    /// Generates every entity of `data/<data_path>` and inserts them parents first, so foreign
    /// keys point at inserted rows. Each entity goes into its `table` (set by "fake-data schema
    /// from database"; `schema.table` or `table`) or the table named like the entity. `schema`
    /// overrides the schema (SQL) / database (MongoDB) for every table. Stops at the first error;
    /// what was inserted before stays.
    pub async fn seed_all_from_schema(&self, project_root: &Path, data_path: &str, schema: Option<&str>, count: Option<u64>) -> Result<Vec<SeedResult>> {
        self.require_writable("seeding")?;
        let file = devall_data::store::read(project_root, data_path)?;
        let tables: Vec<(String, Option<String>, String)> = file
            .entities
            .iter()
            .map(|e| {
                let target = e.extra.get("table").and_then(Value::as_str).unwrap_or(&e.name).to_string();
                let (sch, table) = match (schema, target.split_once('.')) {
                    (Some(s), Some((_, t))) => (Some(s.to_string()), t.to_string()),
                    (Some(s), None) => (Some(s.to_string()), target.clone()),
                    (None, Some((s, t))) if self.kind() != DbKind::Redis => (Some(s.to_string()), t.to_string()),
                    _ => (None, target.clone()),
                };
                (e.name.clone(), sch, table)
            })
            .collect();
        let opts = devall_data::GenerateOptions { entity: None, count_override: count, limit: None, seed: None };
        let data = tokio::task::spawn_blocking(move || devall_data::generate_values(&file, &opts)).await.map_err(|e| invalid(e.to_string()))??;
        let mut out = Vec::new();
        // `data` is in generation order: parents before the entities referencing them.
        for (entity, values) in data {
            let started = std::time::Instant::now();
            let Some((_, sch, table)) = tables.iter().find(|(n, _, _)| *n == entity) else { continue };
            let rows: Vec<Map<String, Value>> = values
                .into_iter()
                .filter_map(|v| match v {
                    Value::Object(m) => Some(m),
                    _ => None,
                })
                .collect();
            let n = self.insert_rows(sch.as_deref(), table, rows).await.map_err(|e| DbError::Query(format!("{entity} → {table}: {}", self.redact(&e))))?;
            let target = match sch {
                Some(s) => format!("{s}.{table}"),
                None => table.clone(),
            };
            out.push(SeedResult { rows: n, target, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 });
        }
        Ok(out)
    }

    /// Runs one read-only statement and returns its rows as JSON objects (for mock responses).
    pub async fn query_to_json(&self, sql: &str, limit: Option<usize>) -> Result<Value> {
        let conn = self.sql()?;
        let stmts = split(sql, conn.dialect());
        if stmts.len() != 1 {
            return Err(invalid("query → JSON needs exactly one statement"));
        }
        let c = classify(&stmts[0].text, conn.dialect());
        if !c.read_only {
            return Err(DbError::ReadOnly(read_only_message(&c)));
        }
        let out = self.query(&stmts[0].text, ExecOptions::new(limit.or(Some(1000)), None), None).await?;
        let r = out.results.into_iter().next().ok_or_else(|| invalid("no result"))?;
        if let Some(e) = r.error {
            return Err(DbError::Query(e));
        }
        Ok(rows_to_objects(&r.columns, &r.rows))
    }
}
