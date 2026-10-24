//! Database clients (Postgres, MySQL/MariaDB, SQLite, Redis, MongoDB) — a thin layer over
//! `crates/devall-db`.
//!
//! Connections live in `<project>/connections/**/*.conn.yaml` (paths relative to `connections/`),
//! saved queries in `<project>/queries/**` (`*.sql`, `*.redis`, `*.mongo.json`; paths relative to
//! `queries/`). Templates (`{{host}}`, `{{secret.DB_PASSWORD}}`) are resolved exactly like HTTP
//! requests in `send.rs::prepare_for`: runtime → environment → project variables, secrets through
//! `SecretResolver::for_environment` with `AppState.providers`. Secret values are redacted from
//! every error message.
//!
//! # Integration (for whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    `devall-db = { path = "../../../crates/devall-db" }`
//!    (or add `devall-db = { path = "crates/devall-db" }` to the root `[workspace.dependencies]` and
//!    use `devall-db.workspace = true`). `tauri`, `serde`, `serde_json`, `tokio` are already deps.
//! 2. `src/modules/mod.rs`: `pub mod db;`
//! 3. `src/lib.rs`, in `.setup(…)`: `app.manage(modules::db::DbState::new());`
//!    (`state::AppState` must be managed too — it already is.)
//! 4. `src/lib.rs`, inside `tauri::generate_handler![…]`:
//!    ```text
//!    modules::db::db_connections_list,
//!    modules::db::db_connections_read,
//!    modules::db::db_connections_save,
//!    modules::db::db_connections_create,
//!    modules::db::db_connections_delete,
//!    modules::db::db_connection_schema,
//!    modules::db::db_test,
//!    modules::db::db_connect,
//!    modules::db::db_disconnect,
//!    modules::db::db_sessions,
//!    modules::db::db_set_readonly,
//!    modules::db::db_schema,
//!    modules::db::db_table,
//!    modules::db::db_split,
//!    modules::db::db_query,
//!    modules::db::db_cancel,
//!    modules::db::db_explain,
//!    modules::db::db_edit_preview,
//!    modules::db::db_edit_apply,
//!    modules::db::db_insert_rows,
//!    modules::db::db_seed,
//!    modules::db::db_seed_all,
//!    modules::db::db_fake_schema,
//!    modules::db::db_schema_summary,
//!    modules::db::db_mongo_fields,
//!    modules::db::db_query_to_json,
//!    modules::db::db_redis_scan,
//!    modules::db::db_redis_get,
//!    modules::db::db_redis_set,
//!    modules::db::db_redis_delete,
//!    modules::db::db_redis_command,
//!    modules::db::db_redis_script,
//!    modules::db::db_redis_info,
//!    modules::db::db_redis_slowlog,
//!    modules::db::db_redis_memory,
//!    modules::db::db_redis_subscribe,
//!    modules::db::db_redis_unsubscribe,
//!    modules::db::db_redis_publish,
//!    modules::db::db_mongo_databases,
//!    modules::db::db_mongo_collections,
//!    modules::db::db_mongo_find,
//!    modules::db::db_mongo_aggregate,
//!    modules::db::db_mongo_count,
//!    modules::db::db_mongo_indexes,
//!    modules::db::db_mongo_explain,
//!    modules::db::db_mongo_write,
//!    modules::db::db_mongo_command,
//!    modules::db::db_queries_list,
//!    modules::db::db_queries_read,
//!    modules::db::db_queries_save,
//!    modules::db::db_queries_create,
//!    modules::db::db_queries_delete,
//!    ```
//! 5. Frontend: in `App.tsx` add `const DbPage = lazy(() => import('@/features/db/DbPage'));` and
//!    `case 'data': return <DbPage />;` (the page handles "no project" itself); in
//!    `components/layout/SideNav.tsx` drop `soon: 'M6'` from the `data` ("Databases") item.
//!    Typed wrappers: `src/lib/api/db.ts`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use devall_core::{Project, SecretResolver};
use devall_db::mongo::{AggregateRequest, CollectionInfo, DatabaseInfo, Documents, FindRequest, MongoWrite};
use devall_db::redisdb::{CommandReply, InfoSection, KeyValue, PubSubMessage, RedisEdit, RedisKey, ScanPage, SlowlogEntry};
use devall_db::sql::edit::{EditPreview, EditRequest};
use devall_db::sql::{Dialect, ExecOptions, ExplainOutput, QueryOutput, QueryProgress, SchemaTree, Statement, TableDetails, classify, read_only_message, split};
use devall_db::store::{self, ConnectionEntry, QueryKind, SavedQuery, SavedQueryEntry};
use devall_db::{ConnectOptions, ConnectionFile, DbError, DbKind, ResolvedConnection, SeedResult, Session, SessionInfo, TunnelManager};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tokio::task::AbortHandle;

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn project_root(root: &str) -> CmdResult<PathBuf> {
    let p = PathBuf::from(root);
    if !p.is_dir() {
        return Err(format!("project folder not found: {root}"));
    }
    Ok(p)
}

/// An open session and where it came from.
struct Entry {
    session: Arc<Session>,
    root: String,
    path: String,
    env: Option<String>,
}

#[derive(Default)]
pub struct DbState {
    sessions: Mutex<HashMap<String, Entry>>,
    tunnels: TunnelManager,
    /// Query id → session id, for cancel.
    queries: Mutex<HashMap<String, String>>,
    /// Pub/sub subscriptions by client-chosen id.
    subscriptions: Mutex<HashMap<String, (String, AbortHandle)>>,
}

impl DbState {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn session(&self, id: &str) -> CmdResult<Arc<Session>> {
        self.sessions.lock().unwrap().get(id).map(|e| e.session.clone()).ok_or_else(|| "this connection was closed — connect again".to_string())
    }

    fn remove(&self, id: &str) -> bool {
        let removed = self.sessions.lock().unwrap().remove(id).is_some();
        self.subscriptions.lock().unwrap().retain(|_, (sid, h)| {
            if sid == id {
                h.abort();
                false
            } else {
                true
            }
        });
        removed
    }
}

/// Records an event in the project's event log (Logs module). `data` holds counts, keywords and
/// at most a truncated SQL text — never edit parameter values — and is redacted with the session's
/// secret values.
#[allow(clippy::too_many_arguments)]
fn log_event(state: &DbState, session_id: &str, s: &Session, kind: &str, title: String, level: devall_logs::Level, ms: f64, mut data: Value) {
    let Some((root, path)) = state.sessions.lock().unwrap().get(session_id).map(|e| (e.root.clone(), e.path.clone())) else { return };
    crate::modules::logs::redact_json(&mut data, s.secrets());
    let title = devall_core::vars::redact_values(&title, s.secrets());
    if let Value::Object(o) = &mut data {
        o.insert("connection".into(), Value::String(path));
        o.insert("engine".into(), json!(s.kind()));
    }
    let _ = crate::modules::logs::record_event(&root, devall_logs::Event::new(kind, "db", title).level(level).duration_ms(ms).data(data));
}

/// First `max` characters of `text` (on a char boundary).
fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
}

/// An open session as listed for the UI (survives page reloads).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSession {
    pub root: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    pub info: SessionInfo,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ConnectOutcome {
    Connected { session: SessionInfo },
    /// The SSH server's key isn't known: ask the user, then connect again with `trustHostKey`.
    HostKey { host: String, port: u16, key_type: String, fingerprint: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestOutcome {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    pub elapsed_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_key: Option<HostKeyPrompt>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
}

/// Resolves a connection's templates for `env` (runs secret-manager CLIs: blocking pool).
async fn resolve(app: &AppHandle, root: &str, conn: ConnectionFile, env: Option<String>) -> CmdResult<ResolvedConnection> {
    let app = app.clone();
    let root = root.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let p = Project::open(&root).map_err(err)?;
        let env = env.as_deref().filter(|e| !e.is_empty());
        let env_file = env.map(|id| p.read_environment(id)).transpose().map_err(err)?;
        let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env, p.root(), env_file.as_ref(), &state.providers);
        let runtime = state.runtime_vars(&root);
        let mut r = crate::commands::resolver_for(&p, &runtime, env, env_file.as_ref(), &[], &secrets);
        let out = devall_db::resolve_connection(&conn, &mut r, p.root());
        let secret_errors = secrets.errors.lock().unwrap().clone();
        out.map_err(|e| {
            let mut msg = e.redacted(&r.secret_values);
            if !secret_errors.is_empty() {
                msg.push_str(&format!(" (secret store: {})", secret_errors.join("; ")));
            }
            msg
        })
    })
    .await
    .map_err(err)?
}

// ---------- connection files ----------

#[tauri::command]
pub fn db_connections_list(root: String) -> CmdResult<Vec<ConnectionEntry>> {
    store::list_connections(&project_root(&root)?).map_err(err)
}

#[tauri::command]
pub fn db_connections_read(root: String, path: String) -> CmdResult<ConnectionFile> {
    store::read_connection(&project_root(&root)?, &path).map_err(err)
}

#[tauri::command]
pub fn db_connections_save(root: String, path: String, connection: ConnectionFile) -> CmdResult<()> {
    store::save_connection(&project_root(&root)?, &path, &connection).map_err(err)
}

/// Creates `connections/<slug>.conn.yaml` (from `connection`, or a starter for `kind`); returns its path.
#[tauri::command]
pub fn db_connections_create(root: String, name: String, kind: DbKind, connection: Option<ConnectionFile>) -> CmdResult<String> {
    store::create_connection(&project_root(&root)?, &name, kind, connection).map_err(err)
}

/// Moves the file to the OS trash and closes its sessions.
#[tauri::command]
pub fn db_connections_delete(state: State<'_, DbState>, root: String, path: String) -> CmdResult<()> {
    store::delete_connection(&project_root(&root)?, &path).map_err(err)?;
    let ids: Vec<String> = state.sessions.lock().unwrap().iter().filter(|(_, e)| e.root == root && e.path == path).map(|(id, _)| id.clone()).collect();
    for id in ids {
        state.remove(&id);
    }
    Ok(())
}

/// JSON Schema of `.conn.yaml`.
#[tauri::command]
pub fn db_connection_schema() -> Value {
    devall_db::json_schema()
}

// ---------- sessions ----------

fn host_key_of(e: &DbError) -> Option<HostKeyPrompt> {
    match e {
        DbError::HostKeyUnknown { host, port, key_type, fingerprint } => Some(HostKeyPrompt { host: host.clone(), port: *port, key_type: key_type.clone(), fingerprint: fingerprint.clone() }),
        _ => None,
    }
}

/// Tests a (possibly unsaved) connection: connects, reads the server version, disconnects.
#[tauri::command]
pub async fn db_test(app: AppHandle, state: State<'_, DbState>, root: String, connection: ConnectionFile, env: Option<String>, trust_host_key: Option<String>) -> CmdResult<TestOutcome> {
    let started = std::time::Instant::now();
    let rc = match resolve(&app, &root, connection, env).await {
        Ok(rc) => rc,
        Err(message) => return Ok(TestOutcome { ok: false, message, server_version: None, elapsed_ms: 0.0, host_key: None, warnings: vec![] }),
    };
    let secrets = rc.secret_values.clone();
    let res = Session::connect(rc, &state.tunnels, ConnectOptions { trust_host_key }).await;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(match res {
        Ok(s) => {
            let info = s.info();
            TestOutcome { ok: true, message: format!("Connected to {}", info.server_version), server_version: Some(info.server_version), elapsed_ms, host_key: None, warnings: info.warnings }
        }
        Err(e) => TestOutcome { ok: false, message: e.redacted(&secrets), server_version: None, elapsed_ms, host_key: host_key_of(&e), warnings: vec![] },
    })
}

/// Opens a session for `connections/<path>` with environment `env`. An existing session for the
/// same connection and environment is reused.
#[tauri::command]
pub async fn db_connect(app: AppHandle, state: State<'_, DbState>, root: String, path: String, env: Option<String>, trust_host_key: Option<String>) -> CmdResult<ConnectOutcome> {
    let env = env.filter(|e| !e.is_empty());
    if let Some(e) = state.sessions.lock().unwrap().values().find(|e| e.root == root && e.path == path && e.env == env) {
        return Ok(ConnectOutcome::Connected { session: e.session.info() });
    }
    let conn = store::read_connection(&project_root(&root)?, &path).map_err(err)?;
    let rc = resolve(&app, &root, conn, env.clone()).await?;
    let secrets = rc.secret_values.clone();
    let session = match Session::connect(rc, &state.tunnels, ConnectOptions { trust_host_key }).await {
        Ok(s) => s,
        Err(e) => {
            if let Some(h) = host_key_of(&e) {
                return Ok(ConnectOutcome::HostKey { host: h.host, port: h.port, key_type: h.key_type, fingerprint: h.fingerprint });
            }
            return Err(e.redacted(&secrets));
        }
    };
    let info = session.info();
    state.sessions.lock().unwrap().insert(info.id.clone(), Entry { session: Arc::new(session), root, path, env });
    Ok(ConnectOutcome::Connected { session: info })
}

#[tauri::command]
pub fn db_disconnect(state: State<'_, DbState>, session: String) -> bool {
    state.remove(&session)
}

#[tauri::command]
pub fn db_sessions(state: State<'_, DbState>) -> Vec<OpenSession> {
    let mut out: Vec<OpenSession> = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .map(|e| OpenSession { root: e.root.clone(), path: e.path.clone(), env: e.env.clone(), info: e.session.info() })
        .collect();
    out.sort_by_key(|s| s.info.connected_at);
    out
}

/// Switches read-only mode; for protected connections `confirm` must be the connection name.
#[tauri::command]
pub async fn db_set_readonly(state: State<'_, DbState>, session: String, read_only: bool, confirm: Option<String>) -> CmdResult<SessionInfo> {
    let s = state.session(&session)?;
    s.set_read_only(read_only, confirm.as_deref()).await.map_err(|e| s.redact(&e))
}

// ---------- SQL ----------

#[tauri::command]
pub async fn db_schema(state: State<'_, DbState>, session: String) -> CmdResult<SchemaTree> {
    let s = state.session(&session)?;
    s.sql().map_err(err)?.schema().await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_table(state: State<'_, DbState>, session: String, schema: Option<String>, table: String) -> CmdResult<TableDetails> {
    let s = state.session(&session)?;
    s.sql().map_err(err)?.table_details(schema.as_deref(), &table).await.map_err(|e| s.redact(&e))
}

/// Statement ranges of a script (UTF-16 offsets) — for "run statement under cursor".
#[tauri::command]
pub fn db_split(sql: String, dialect: Dialect) -> Vec<Statement> {
    split(&sql, dialect)
}

/// Runs a script. `limit` caps rows per result (default 1000); `offset` pages a read-only
/// statement. Progress (statement started/finished) streams over `on_progress`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn db_query(
    state: State<'_, DbState>,
    session: String,
    query_id: String,
    sql: String,
    limit: Option<usize>,
    offset: Option<usize>,
    on_progress: Channel<QueryProgress>,
) -> CmdResult<QueryOutput> {
    let s = state.session(&session)?;
    state.queries.lock().unwrap().insert(query_id.clone(), session.clone());
    let progress = move |p: QueryProgress| {
        let _ = on_progress.send(p);
    };
    let started = std::time::Instant::now();
    let res = s.query(&sql, ExecOptions::new(limit, offset), Some(&progress)).await;
    state.queries.lock().unwrap().remove(&query_id);
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let name = s.info().name;
    match &res {
        Ok(out) => {
            let failed = out.results.iter().find(|r| r.error.is_some());
            let rows: usize = out.results.iter().map(|r| r.rows.len()).sum();
            let keywords: Vec<&str> = out.results.iter().map(|r| r.keyword.as_str()).collect();
            let what = match out.results.as_slice() {
                [one] => one.keyword.clone(),
                _ => format!("{} statements", out.total_statements),
            };
            let level = if failed.is_some() {
                devall_logs::Level::Error
            } else if out.cancelled {
                devall_logs::Level::Warn
            } else {
                devall_logs::Level::Info
            };
            let title = match failed {
                Some(r) => format!("{name} · {what} failed: {}", clip(r.error.as_deref().unwrap_or(""), 120)),
                None if out.cancelled => format!("{name} · {what} cancelled"),
                None => format!("{name} · {what} · {rows} row{}", if rows == 1 { "" } else { "s" }),
            };
            let data = json!({ "sql": clip(&sql, 1000), "keywords": keywords, "statements": out.total_statements, "rows": rows, "offset": offset.unwrap_or(0), "readOnly": s.is_read_only(), "cancelled": out.cancelled });
            log_event(&state, &session, &s, "db.query", title, level, ms, data);
        }
        Err(e) => {
            let level = if matches!(e, DbError::ReadOnly(_)) { devall_logs::Level::Warn } else { devall_logs::Level::Error };
            let msg = s.redact(e);
            let data = json!({ "sql": clip(&sql, 1000), "error": clip(&msg, 500), "readOnly": s.is_read_only() });
            log_event(&state, &session, &s, "db.query", format!("{name} · query rejected: {}", clip(&msg, 120)), level, ms, data);
        }
    }
    res.map_err(|e| s.redact(&e))
}

/// Cancels a running query (server-side cancel / KILL QUERY / SQLite interrupt).
#[tauri::command]
pub async fn db_cancel(state: State<'_, DbState>, query_id: String) -> CmdResult<bool> {
    let sid = state.queries.lock().unwrap().get(&query_id).cloned();
    let Some(sid) = sid else { return Ok(false) };
    let s = state.session(&sid)?;
    s.cancel().await.map_err(|e| s.redact(&e))
}

/// EXPLAIN (plan only), or EXPLAIN ANALYZE (runs the statement inside a rolled-back
/// transaction; refused for writes in read-only sessions).
#[tauri::command]
pub async fn db_explain(state: State<'_, DbState>, session: String, sql: String, analyze: bool) -> CmdResult<ExplainOutput> {
    let s = state.session(&session)?;
    let conn = s.sql().map_err(err)?;
    let stmts = split(&sql, conn.dialect());
    let [stmt] = stmts.as_slice() else { return Err("EXPLAIN needs exactly one statement".into()) };
    if analyze && s.is_read_only() {
        let c = classify(&stmt.text, conn.dialect());
        if !c.read_only {
            return Err(read_only_message(&c));
        }
    }
    conn.explain(&stmt.text, analyze).await.map_err(|e| s.redact(&e))
}

/// The parameterized statements (and a literal preview) for grid edits.
#[tauri::command]
pub fn db_edit_preview(state: State<'_, DbState>, session: String, edit: EditRequest) -> CmdResult<EditPreview> {
    let s = state.session(&session)?;
    s.edit_preview(&edit).map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_edit_apply(state: State<'_, DbState>, session: String, edit: EditRequest) -> CmdResult<Vec<u64>> {
    let s = state.session(&session)?;
    let started = std::time::Instant::now();
    let res = s.edit_apply(&edit).await;
    let table = match &edit.schema {
        Some(sc) => format!("{sc}.{}", edit.table),
        None => edit.table.clone(),
    };
    let count = |t: &str| edit.edits.iter().filter(|e| matches!((e, t), (devall_db::sql::RowEdit::Update { .. }, "update") | (devall_db::sql::RowEdit::Insert { .. }, "insert") | (devall_db::sql::RowEdit::Delete { .. }, "delete"))).count();
    // Counts only: edited values are user data and never go to the log.
    let data = json!({ "table": table, "updates": count("update"), "inserts": count("insert"), "deletes": count("delete"), "ok": res.is_ok() });
    let (level, title) = match &res {
        Ok(n) => (devall_logs::Level::Info, format!("{} · edited {table} ({} statement{})", s.info().name, n.len(), if n.len() == 1 { "" } else { "s" })),
        Err(_) => (devall_logs::Level::Error, format!("{} · edits to {table} rolled back", s.info().name)),
    };
    log_event(&state, &session, &s, "db.edit", title, level, started.elapsed().as_secs_f64() * 1000.0, data);
    res.map_err(|e| s.redact(&e))
}

/// Inserts JSON rows (SQL table, Mongo collection with `schema` = database, or Redis hashes with
/// `table` = key prefix).
#[tauri::command]
pub async fn db_insert_rows(state: State<'_, DbState>, session: String, schema: Option<String>, table: String, rows: Vec<Map<String, Value>>) -> CmdResult<u64> {
    let s = state.session(&session)?;
    s.insert_rows(schema.as_deref(), &table, rows).await.map_err(|e| s.redact(&e))
}

/// Generates `entity` from `data/<dataPath>` (fake-data module) and inserts it into `table`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn db_seed(
    state: State<'_, DbState>,
    session: String,
    root: String,
    data_path: String,
    entity: String,
    schema: Option<String>,
    table: String,
    count: Option<u64>,
) -> CmdResult<SeedResult> {
    let s = state.session(&session)?;
    let started = std::time::Instant::now();
    let res = s.seed_from_schema(&project_root(&root)?, &data_path, &entity, schema.as_deref(), &table, count).await;
    let data = json!({ "dataFile": data_path, "entity": entity, "table": table, "rows": res.as_ref().map(|r| r.rows).unwrap_or(0), "ok": res.is_ok() });
    let (level, title) = match &res {
        Ok(r) => (devall_logs::Level::Info, format!("{} · seeded {} rows into {}", s.info().name, r.rows, r.target)),
        Err(e) => (devall_logs::Level::Error, format!("{} · seeding {table} failed: {}", s.info().name, clip(&s.redact(e), 120))),
    };
    log_event(&state, &session, &s, "db.seed", title, level, started.elapsed().as_secs_f64() * 1000.0, data);
    res.map_err(|e| s.redact(&e))
}

/// Generates every entity of `data/<dataPath>` and inserts them parents first (each into its
/// `table`, set when the schema was built from a database, or the table named like the entity).
#[tauri::command]
pub async fn db_seed_all(state: State<'_, DbState>, session: String, root: String, data_path: String, schema: Option<String>, count: Option<u64>) -> CmdResult<Vec<SeedResult>> {
    let s = state.session(&session)?;
    let started = std::time::Instant::now();
    let res = s.seed_all_from_schema(&project_root(&root)?, &data_path, schema.as_deref(), count).await;
    let rows: u64 = res.as_ref().map(|r| r.iter().map(|x| x.rows).sum()).unwrap_or(0);
    let data = json!({ "dataFile": data_path, "tables": res.as_ref().map(|r| r.len()).unwrap_or(0), "rows": rows, "ok": res.is_ok() });
    let (level, title) = match &res {
        Ok(r) => (devall_logs::Level::Info, format!("{} · seeded {rows} rows into {} tables", s.info().name, r.len())),
        Err(e) => (devall_logs::Level::Error, format!("{} · seeding from data/{data_path} failed: {}", s.info().name, clip(&s.redact(e), 120))),
    };
    log_event(&state, &session, &s, "db.seed", title, level, started.elapsed().as_secs_f64() * 1000.0, data);
    res.map_err(|e| s.redact(&e))
}

/// A fake-data generator schema built from the connection's tables (SQL: columns, keys,
/// enums; never row data) or sampled collections (MongoDB). Not saved — the UI reviews it and
/// creates the file with `data_create`.
#[tauri::command]
pub async fn db_fake_schema(state: State<'_, DbState>, session: String, request: devall_db::fakeschema::FakeSchemaRequest) -> CmdResult<devall_data::FromDbResult> {
    let s = state.session(&session)?;
    s.fake_schema(&request).await.map_err(|e| s.redact(&e))
}

/// Tables, columns, keys and enum values as text, for NL → SQL (never rows).
#[tauri::command]
pub async fn db_schema_summary(state: State<'_, DbState>, session: String, tables: Option<Vec<devall_db::fakeschema::TableRef>>, max_tables: Option<usize>) -> CmdResult<String> {
    let s = state.session(&session)?;
    s.schema_summary(&tables.unwrap_or_default(), max_tables.unwrap_or(60)).await.map_err(|e| s.redact(&e))
}

/// Field names and types of sampled documents, for NL → MongoDB (values are dropped).
#[tauri::command]
pub async fn db_mongo_fields(state: State<'_, DbState>, session: String, database: String, collection: String, sample: Option<i64>) -> CmdResult<String> {
    let s = state.session(&session)?;
    s.mongo_field_summary(&database, &collection, sample.unwrap_or(50)).await.map_err(|e| s.redact(&e))
}

/// Runs one read-only statement and returns its rows as pretty JSON (for mock responses).
#[tauri::command]
pub async fn db_query_to_json(state: State<'_, DbState>, session: String, sql: String, limit: Option<usize>) -> CmdResult<String> {
    let s = state.session(&session)?;
    let v = s.query_to_json(&sql, limit).await.map_err(|e| s.redact(&e))?;
    serde_json::to_string_pretty(&v).map_err(err)
}

// ---------- Redis ----------

fn read_only_redis(s: &Session, what: &str) -> CmdResult<()> {
    if s.is_read_only() {
        return Err(format!("Read-only session: {what} was not run because it modifies data. Turn off read-only for this connection to run it."));
    }
    Ok(())
}

#[tauri::command]
pub async fn db_redis_scan(state: State<'_, DbState>, session: String, cursor: Option<String>, pattern: Option<String>, type_filter: Option<String>, count: Option<usize>) -> CmdResult<ScanPage> {
    let s = state.session(&session)?;
    s.redis().map_err(err)?.scan(cursor.as_deref().unwrap_or("0"), pattern.as_deref(), type_filter.as_deref(), count.unwrap_or(500)).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_redis_get(state: State<'_, DbState>, session: String, key: RedisKey, cursor: Option<String>, offset: Option<u64>, count: Option<u64>) -> CmdResult<KeyValue> {
    let s = state.session(&session)?;
    s.redis().map_err(err)?.get(&key, cursor.as_deref(), offset.unwrap_or(0), count.unwrap_or(200)).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_redis_set(state: State<'_, DbState>, session: String, edit: RedisEdit) -> CmdResult<CommandReply> {
    let s = state.session(&session)?;
    read_only_redis(&s, "the change")?;
    s.redis().map_err(err)?.edit(&edit).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_redis_delete(state: State<'_, DbState>, session: String, keys: Vec<RedisKey>) -> CmdResult<u64> {
    let s = state.session(&session)?;
    read_only_redis(&s, "the delete")?;
    s.redis().map_err(err)?.delete(&keys).await.map_err(|e| s.redact(&e))
}

/// Console: one `redis-cli` command line.
#[tauri::command]
pub async fn db_redis_command(state: State<'_, DbState>, session: String, line: String) -> CmdResult<CommandReply> {
    let s = state.session(&session)?;
    let read_only = s.is_read_only();
    s.redis().map_err(err)?.command(&line, read_only).await.map_err(|e| s.redact(&e))
}

/// A saved `.redis` script: one command per line.
#[tauri::command]
pub async fn db_redis_script(state: State<'_, DbState>, session: String, script: String) -> CmdResult<Vec<CommandReply>> {
    let s = state.session(&session)?;
    let read_only = s.is_read_only();
    s.redis().map_err(err)?.run_script(&script, read_only).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_redis_info(state: State<'_, DbState>, session: String, section: Option<String>) -> CmdResult<Vec<InfoSection>> {
    let s = state.session(&session)?;
    s.redis().map_err(err)?.info(section.as_deref()).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_redis_slowlog(state: State<'_, DbState>, session: String, count: Option<usize>) -> CmdResult<Vec<SlowlogEntry>> {
    let s = state.session(&session)?;
    s.redis().map_err(err)?.slowlog(count.unwrap_or(128)).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_redis_memory(state: State<'_, DbState>, session: String, key: RedisKey) -> CmdResult<Option<u64>> {
    let s = state.session(&session)?;
    s.redis().map_err(err)?.memory_usage(&key).await.map_err(|e| s.redact(&e))
}

/// Subscribes (dedicated connection); messages stream over `on_message` until
/// `db_redis_unsubscribe(subId)` or disconnect.
#[tauri::command]
pub async fn db_redis_subscribe(
    state: State<'_, DbState>,
    session: String,
    sub_id: String,
    channels: Vec<String>,
    patterns: Vec<String>,
    on_message: Channel<PubSubMessage>,
) -> CmdResult<()> {
    let s = state.session(&session)?;
    let cb = Arc::new(move |m: PubSubMessage| {
        let _ = on_message.send(m);
    });
    let handle = s.redis().map_err(err)?.subscribe(&channels, &patterns, cb).await.map_err(|e| s.redact(&e))?;
    if let Some((_, old)) = state.subscriptions.lock().unwrap().insert(sub_id, (session, handle)) {
        old.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn db_redis_unsubscribe(state: State<'_, DbState>, sub_id: String) -> bool {
    match state.subscriptions.lock().unwrap().remove(&sub_id) {
        Some((_, h)) => {
            h.abort();
            true
        }
        None => false,
    }
}

#[tauri::command]
pub async fn db_redis_publish(state: State<'_, DbState>, session: String, channel: String, message: String) -> CmdResult<i64> {
    let s = state.session(&session)?;
    read_only_redis(&s, "PUBLISH")?;
    s.redis().map_err(err)?.publish(&channel, &message).await.map_err(|e| s.redact(&e))
}

// ---------- MongoDB ----------

#[tauri::command]
pub async fn db_mongo_databases(state: State<'_, DbState>, session: String) -> CmdResult<Vec<DatabaseInfo>> {
    let s = state.session(&session)?;
    s.mongo().map_err(err)?.databases().await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_mongo_collections(state: State<'_, DbState>, session: String, database: String) -> CmdResult<Vec<CollectionInfo>> {
    let s = state.session(&session)?;
    s.mongo().map_err(err)?.collections(&database).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_mongo_find(state: State<'_, DbState>, session: String, request: FindRequest) -> CmdResult<Documents> {
    let s = state.session(&session)?;
    s.mongo().map_err(err)?.find(&request).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_mongo_aggregate(state: State<'_, DbState>, session: String, request: AggregateRequest) -> CmdResult<Documents> {
    let s = state.session(&session)?;
    let read_only = s.is_read_only();
    s.mongo().map_err(err)?.aggregate(&request, read_only).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_mongo_count(state: State<'_, DbState>, session: String, database: String, collection: String, filter: Option<String>) -> CmdResult<u64> {
    let s = state.session(&session)?;
    s.mongo().map_err(err)?.count(&database, &collection, filter.as_deref()).await.map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_mongo_indexes(state: State<'_, DbState>, session: String, database: String, collection: String) -> CmdResult<Vec<Value>> {
    let s = state.session(&session)?;
    s.mongo().map_err(err)?.indexes(&database, &collection).await.map_err(|e| s.redact(&e))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MongoExplain {
    Find { request: FindRequest },
    Aggregate { request: AggregateRequest },
}

#[tauri::command]
pub async fn db_mongo_explain(state: State<'_, DbState>, session: String, explain: MongoExplain, analyze: bool) -> CmdResult<Value> {
    let s = state.session(&session)?;
    let m = s.mongo().map_err(err)?;
    match explain {
        MongoExplain::Find { request } => m.explain_find(&request, analyze).await,
        MongoExplain::Aggregate { request } => m.explain_aggregate(&request, analyze).await,
    }
    .map_err(|e| s.redact(&e))
}

#[tauri::command]
pub async fn db_mongo_write(state: State<'_, DbState>, session: String, op: MongoWrite) -> CmdResult<Value> {
    let s = state.session(&session)?;
    if s.is_read_only() {
        return Err("Read-only session: the write was not run. Turn off read-only for this connection to run it.".into());
    }
    s.mongo().map_err(err)?.write(&op).await.map_err(|e| s.redact(&e))
}

/// `db.runCommand(…)` (read-only sessions: known read commands only).
#[tauri::command]
pub async fn db_mongo_command(state: State<'_, DbState>, session: String, database: String, command: String) -> CmdResult<Value> {
    let s = state.session(&session)?;
    let read_only = s.is_read_only();
    s.mongo().map_err(err)?.command(&database, &command, read_only).await.map_err(|e| s.redact(&e))
}

// ---------- saved queries ----------

#[tauri::command]
pub fn db_queries_list(root: String) -> CmdResult<Vec<SavedQueryEntry>> {
    store::list_queries(&project_root(&root)?).map_err(err)
}

#[tauri::command]
pub fn db_queries_read(root: String, path: String) -> CmdResult<SavedQuery> {
    store::read_query(&project_root(&root)?, &path).map_err(err)
}

#[tauri::command]
pub fn db_queries_save(root: String, path: String, text: String) -> CmdResult<()> {
    store::save_query(&project_root(&root)?, &path, &text).map_err(err)
}

/// Creates `queries/<folder>/<slug(name)>.<ext>`; `connection` is the connection slug written to
/// the header. Returns the new path.
#[tauri::command]
pub fn db_queries_create(root: String, folder: Option<String>, name: String, kind: QueryKind, connection: Option<String>, text: Option<String>) -> CmdResult<String> {
    store::create_query(&project_root(&root)?, folder.as_deref().unwrap_or(""), &name, kind, connection.as_deref(), text.as_deref()).map_err(err)
}

#[tauri::command]
pub fn db_queries_delete(root: String, path: String) -> CmdResult<()> {
    store::delete_query(&project_root(&root)?, &path).map_err(err)
}
