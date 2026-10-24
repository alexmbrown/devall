//! Log viewer + project event store commands — a thin layer over `crates/devall-logs`.
//!
//! # Integration (done by whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    ```toml
//!    devall-logs = { path = "../../../crates/devall-logs" }
//!    ```
//!    (DuckDB is bundled through the crate's default `duckdb` feature; use
//!    `default-features = false` to fall back to SQLite for SQL over logs.)
//! 2. `src/modules/mod.rs`: `pub mod logs;`
//! 3. `src/lib.rs`: on the builder `.manage(modules::logs::LogsState::default())` and in
//!    `tauri::generate_handler![...]`:
//!    ```text
//!    modules::logs::logs_open,
//!    modules::logs::logs_open_text,
//!    modules::logs::logs_append,
//!    modules::logs::logs_merge,
//!    modules::logs::logs_sources,
//!    modules::logs::logs_info,
//!    modules::logs::logs_close,
//!    modules::logs::logs_check_query,
//!    modules::logs::logs_query,
//!    modules::logs::logs_entry,
//!    modules::logs::logs_locate,
//!    modules::logs::logs_histogram,
//!    modules::logs::logs_patterns,
//!    modules::logs::logs_top_values,
//!    modules::logs::logs_fields,
//!    modules::logs::logs_sql,
//!    modules::logs::logs_sql_tables,
//!    modules::logs::logs_tail,
//!    modules::logs::logs_untail,
//!    modules::logs::events_query,
//!    modules::logs::events_list,
//!    modules::logs::events_get,
//!    modules::logs::events_correlated,
//!    modules::logs::events_histogram,
//!    modules::logs::events_patterns,
//!    modules::logs::events_top_values,
//!    modules::logs::events_locate,
//!    modules::logs::events_stats,
//!    modules::logs::events_record,
//!    modules::logs::events_tail,
//!    modules::logs::events_clear,
//!    ```
//! 4. Record events from other modules (each is one line; all are best-effort and never fail
//!    the caller; event stores are shared per project, so live tails see them immediately):
//!    * `send.rs`, in `send_request` right after `let history_id = …;`:
//!      ```ignore
//!      crate::modules::logs::record_http_send(&root, &path, &request.name, &sent, resp.as_ref(), error.as_ref().map(|e| e as &dyn std::fmt::Display), history_id.as_deref(), at);
//!      ```
//!    * `send.rs`, in `run_collection` before `Ok(summary)`:
//!      ```ignore
//!      crate::modules::logs::record_collection_run(&root, &run.folder, &summary);
//!      ```
//!    * `modules/load.rs`, in `load_run` after `let summary = result.map_err(err)?;`:
//!      ```ignore
//!      crate::modules::logs::record_load_run(&root, &summary);
//!      ```
//!    * `modules/mock.rs`, in `mock_start` once the server handle exists (`handle`):
//!      ```ignore
//!      crate::modules::logs::forward_mock_journal(&root, handle.subscribe());
//!      ```
//!    * webhooks / port-forwards / notebooks / Docker / Kubernetes modules:
//!      `crate::modules::logs::record_webhook(&root, &method, &path, &headers, body.as_deref(), status);`,
//!      `crate::modules::logs::record_forward(&root, true, "svc/api", 8080, "pod/api-7d9:80");`,
//!      `crate::modules::logs::record_notebook_run(&root, "notes/x.md", Some("cell-3"), ok, ms, json!({...}));`
//!      or generally `crate::modules::logs::record_event(&root, devall_logs::Event::new(kind, module, title)…)`.
//!    * Streaming container logs into the viewer (Docker/K8s modules):
//!      `let (handle, sink) = app.state::<crate::modules::logs::LogsState>().open_stream("api-7d9 (pod)")?;`
//!      then `sink.write(&line)?` for each chunk; the UI opens it with `openInLogViewer({ type: 'handle', handle })`.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Display;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devall_logs::analyze::{FieldInfo, Histogram, PatternOptions, PatternResult, TopValues};
use devall_logs::events::{Event, EventFilter, EventStats, EventStore, kinds};
use devall_logs::file::{FileInfo, LogFile, OpenOptions, Progress, RefreshKind, StreamSink};
use devall_logs::sql::{SqlEngine, SqlResult, TableInfo, TextKind};
use devall_logs::{Filter, Level, LogEntry, LogSet, Page, QueryError, correlation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::Channel;

type CmdResult<T> = Result<T, String>;

fn err(e: impl Display) -> String {
    e.to_string()
}

/// Runs blocking work (file scans, SQL) off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

// ---------- state ----------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SourceKind {
    File { path: String },
    Merge { paths: Vec<String> },
    Stream { name: String },
}

/// SQL engine with the `logs` table loaded: (engine, entries loaded, file generations).
type LogsSql = Option<(SqlEngine, u64, Vec<u64>)>;
/// SQL engine with `events` loaded at an event-store version.
type EventsSql = Arc<Mutex<Option<(SqlEngine, u64)>>>;

struct OpenSource {
    handle: String,
    kind: SourceKind,
    set: LogSet,
    sink: Option<StreamSink>,
    /// SQL engine with the `logs` table loaded (and how many entries / which generation).
    sql: Mutex<LogsSql>,
}

#[derive(Default)]
pub struct LogsState {
    sources: Mutex<HashMap<String, Arc<OpenSource>>>,
    tails: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Per project: SQL engine with `events` loaded at a store version.
    event_sql: Mutex<HashMap<String, EventsSql>>,
    next: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub handle: String,
    pub kind: SourceKind,
    pub name: String,
    pub files: Vec<FileInfo>,
    pub entries: u64,
    pub indexing: bool,
    pub followable: bool,
}

/// Progress of opening (per file for merges).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenProgress {
    pub handle: String,
    pub file: usize,
    pub progress: Progress,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenOpts {
    /// Interpret zone-less timestamps as UTC instead of local time.
    pub utc: bool,
    /// Force a format (`json`, `logfmt`, `accessLog`, …).
    pub format: Option<devall_logs::Format>,
    pub name: Option<String>,
}

impl OpenOpts {
    fn to_options(&self) -> OpenOptions {
        OpenOptions {
            naive_tz: if self.utc { devall_logs::time::NaiveTz::Utc } else { devall_logs::time::NaiveTz::Local },
            year: None,
            format: self.format,
            name: self.name.clone(),
        }
    }
}

fn path_key(p: &str) -> String {
    devall_logs::events::root_key(std::path::Path::new(p))
}

impl LogsState {
    fn id(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn get(&self, handle: &str) -> CmdResult<Arc<OpenSource>> {
        self.sources.lock().unwrap().get(handle).cloned().ok_or_else(|| format!("log source '{handle}' is not open"))
    }

    fn insert(&self, src: OpenSource) -> Arc<OpenSource> {
        let src = Arc::new(src);
        self.sources.lock().unwrap().insert(src.handle.clone(), src.clone());
        src
    }

    fn find_file(&self, path: &str) -> Option<Arc<OpenSource>> {
        let key = path_key(path);
        self.sources.lock().unwrap().values().find(|s| matches!(&s.kind, SourceKind::File { path } if path_key(path) == key)).cloned()
    }

    /// A followable stream source other modules write to (container logs, process output).
    pub fn open_stream(&self, name: &str) -> CmdResult<(String, StreamWriter)> {
        let (file, sink) = LogFile::stream(name, OpenOptions::default()).map_err(err)?;
        let handle = self.id("stream");
        let src = self.insert(OpenSource {
            handle: handle.clone(),
            kind: SourceKind::Stream { name: name.to_string() },
            set: LogSet::single(file),
            sink: Some(sink),
            sql: Mutex::new(None),
        });
        Ok((handle, StreamWriter(src)))
    }

    fn event_sql(&self, root: &str) -> EventsSql {
        self.event_sql.lock().unwrap().entry(path_key(root)).or_default().clone()
    }
}

/// Write handle for a stream source (returned by [`LogsState::open_stream`]).
#[derive(Clone)]
pub struct StreamWriter(Arc<OpenSource>);

impl StreamWriter {
    pub fn write(&self, text: &str) -> CmdResult<()> {
        match &self.0.sink {
            Some(s) => s.write(text).map_err(err),
            None => Err("not a stream source".into()),
        }
    }
}

fn info_of(src: &OpenSource) -> SourceInfo {
    let files: Vec<FileInfo> = src.set.files().iter().map(|f| f.info()).collect();
    let name = match &src.kind {
        SourceKind::File { .. } | SourceKind::Stream { .. } => files.first().map(|f| f.name.clone()).unwrap_or_default(),
        SourceKind::Merge { paths } => format!("{} files merged", paths.len()),
    };
    SourceInfo {
        handle: src.handle.clone(),
        kind: src.kind.clone(),
        name,
        entries: src.set.len(),
        indexing: src.set.indexing(),
        followable: src.set.files().iter().all(|f| f.is_followable()),
        files,
    }
}

fn opening(handle: &str, file: usize, ch: Channel<OpenProgress>) -> impl FnMut(&Progress) + Send + 'static {
    let handle = handle.to_string();
    move |p: &Progress| {
        let _ = ch.send(OpenProgress { handle: handle.clone(), file, progress: p.clone() });
    }
}

// ---------- sources ----------

/// Opens a file (plain, `.gz`, UTF-16) and indexes it in the background; progress streams on
/// `on_progress`. Re-opening an already open path returns the existing handle.
#[tauri::command]
pub async fn logs_open(state: State<'_, LogsState>, path: String, options: Option<OpenOpts>, on_progress: Channel<OpenProgress>) -> CmdResult<SourceInfo> {
    if let Some(src) = state.find_file(&path) {
        let _ = on_progress.send(OpenProgress { handle: src.handle.clone(), file: 0, progress: src.set.files()[0].progress() });
        return Ok(info_of(&src));
    }
    let handle = state.id("file");
    let opts = options.unwrap_or_default().to_options();
    let file = LogFile::open_background(&path, opts, opening(&handle, 0, on_progress)).map_err(err)?;
    let src = state.insert(OpenSource { handle, kind: SourceKind::File { path }, set: LogSet::single(file), sink: None, sql: Mutex::new(None) });
    Ok(info_of(&src))
}

/// Opens text (pasted, or fetched container logs) as a followable stream source.
#[tauri::command]
pub fn logs_open_text(state: State<'_, LogsState>, name: String, text: String) -> CmdResult<SourceInfo> {
    let (handle, sink) = state.open_stream(&name)?;
    if !text.is_empty() {
        sink.write(&text)?;
    }
    let src = state.get(&handle)?;
    for f in src.set.files() {
        f.refresh().map_err(err)?;
    }
    Ok(info_of(&src))
}

/// Appends to a stream source opened with `logs_open_text`.
#[tauri::command]
pub fn logs_append(state: State<'_, LogsState>, handle: String, text: String) -> CmdResult<u64> {
    let src = state.get(&handle)?;
    let Some(sink) = &src.sink else { return Err("only text/stream sources can be appended to".into()) };
    sink.write(&text).map_err(err)?;
    for f in src.set.files() {
        f.refresh().map_err(err)?;
    }
    Ok(src.set.len())
}

/// Opens several files merged by timestamp (`source` tells them apart).
#[tauri::command]
pub async fn logs_merge(state: State<'_, LogsState>, paths: Vec<String>, options: Option<OpenOpts>, on_progress: Channel<OpenProgress>) -> CmdResult<SourceInfo> {
    if paths.len() < 2 {
        return Err("pick at least two files to merge".into());
    }
    let handle = state.id("merge");
    let opts = options.unwrap_or_default();
    let mut files = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        match state.find_file(p) {
            Some(src) => files.push(src.set.files()[0].clone()),
            None => files.push(LogFile::open_background(p, opts.to_options(), opening(&handle, i, on_progress.clone())).map_err(err)?),
        }
    }
    let src = state.insert(OpenSource { handle, kind: SourceKind::Merge { paths }, set: LogSet::merged(files), sink: None, sql: Mutex::new(None) });
    Ok(info_of(&src))
}

#[tauri::command]
pub fn logs_sources(state: State<'_, LogsState>) -> Vec<SourceInfo> {
    let mut v: Vec<SourceInfo> = state.sources.lock().unwrap().values().map(|s| info_of(s)).collect();
    v.sort_by(|a, b| a.handle.cmp(&b.handle));
    v
}

#[tauri::command]
pub fn logs_info(state: State<'_, LogsState>, handle: String) -> CmdResult<SourceInfo> {
    let src = state.get(&handle)?;
    Ok(info_of(&src))
}

/// Closes a source (stops indexing; temp files are deleted).
#[tauri::command]
pub fn logs_close(state: State<'_, LogsState>, handle: String) -> bool {
    let removed = state.sources.lock().unwrap().remove(&handle);
    if let Some(src) = &removed {
        let shared: Vec<String> = state.sources.lock().unwrap().values().flat_map(|s| s.set.files().iter().map(|f| f.path().display().to_string()).collect::<Vec<_>>()).collect();
        for f in src.set.files() {
            if !shared.contains(&f.path().display().to_string()) {
                f.cancel();
            }
        }
        if let Some(stop) = state.tails.lock().unwrap().remove(&format!("tail-{handle}")) {
            stop.store(true, Ordering::Relaxed);
        }
    }
    removed.is_some()
}

/// Validates a query without running it (`null` when fine).
#[tauri::command]
pub fn logs_check_query(query: String) -> Option<QueryError> {
    devall_logs::Query::parse(&query).err()
}

// ---------- querying ----------

#[tauri::command]
pub async fn logs_query(state: State<'_, LogsState>, handle: String, filter: Filter, offset: u64, limit: usize) -> CmdResult<Page> {
    let src = state.get(&handle)?;
    blocking(move || src.set.query(&filter, offset, limit.min(5000)).map_err(err)).await
}

/// One entry by view position, untruncated (detail drawer).
#[tauri::command]
pub async fn logs_entry(state: State<'_, LogsState>, handle: String, index: u64) -> CmdResult<Option<LogEntry>> {
    let src = state.get(&handle)?;
    blocking(move || src.set.entry_at(index).map_err(err)).await
}

/// Row (in the filtered result) of the first entry at or after `ts`.
#[tauri::command]
pub async fn logs_locate(state: State<'_, LogsState>, handle: String, filter: Filter, ts: i64) -> CmdResult<Option<u64>> {
    let src = state.get(&handle)?;
    blocking(move || src.set.locate(&filter, ts).map_err(err)).await
}

#[tauri::command]
pub async fn logs_histogram(state: State<'_, LogsState>, handle: String, filter: Filter, buckets: Option<usize>) -> CmdResult<Histogram> {
    let src = state.get(&handle)?;
    blocking(move || src.set.histogram(&filter, buckets.unwrap_or(120).clamp(10, 1000)).map_err(err)).await
}

#[tauri::command]
pub async fn logs_patterns(state: State<'_, LogsState>, handle: String, filter: Filter, max_entries: Option<usize>) -> CmdResult<PatternResult> {
    let src = state.get(&handle)?;
    let opts = PatternOptions { max_entries: max_entries.unwrap_or(500_000), ..Default::default() };
    blocking(move || src.set.patterns(&filter, &opts).map_err(err)).await
}

#[tauri::command]
pub async fn logs_top_values(state: State<'_, LogsState>, handle: String, filter: Filter, field: String, limit: Option<usize>) -> CmdResult<TopValues> {
    let src = state.get(&handle)?;
    blocking(move || src.set.top_values(&filter, &field, limit.unwrap_or(10)).map_err(err)).await
}

#[tauri::command]
pub async fn logs_fields(state: State<'_, LogsState>, handle: String, filter: Filter) -> CmdResult<Vec<FieldInfo>> {
    let src = state.get(&handle)?;
    blocking(move || src.set.fields(&filter, 1000).map_err(err)).await
}

// ---------- SQL ----------

/// Which tables a SQL query sees: `logs` (an open source) and/or `events` (a project).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SqlTarget {
    pub handle: Option<String>,
    pub root: Option<String>,
}

/// Extra tables for a query.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SqlSource {
    /// CSV/TSV/JSON/JSONL/Parquet file.
    File { name: String, path: String },
    /// Inline text (e.g. a response body from the HTTP module).
    Text { name: String, text: String, format: TextKind },
    /// A saved response body from the project's history (needs `target.root`).
    History { name: String, id: String },
}

fn ensure_logs_table(src: &OpenSource) -> CmdResult<std::sync::MutexGuard<'_, LogsSql>> {
    let mut guard = src.sql.lock().unwrap();
    let gens: Vec<u64> = src.set.files().iter().map(|f| f.generation()).collect();
    let total = src.set.len();
    let stale = match guard.as_ref() {
        Some((_, loaded, g)) => *g != gens || *loaded > total,
        None => true,
    };
    if stale {
        let mut engine = SqlEngine::new().map_err(err)?;
        engine.create_logs_table("logs").map_err(err)?;
        *guard = Some((engine, 0, gens));
    }
    let (engine, loaded, _) = guard.as_mut().unwrap();
    if *loaded < total {
        let added = src.set.batches(&Filter::default(), *loaded, 50_000, |batch| engine.insert_entries("logs", &batch)).map_err(err)?;
        *loaded += added;
    }
    Ok(guard)
}

fn load_events(engine: &mut SqlEngine, store: &EventStore) -> CmdResult<()> {
    engine.create_events_table("events").map_err(err)?;
    let events = store.query(&EventFilter { limit: Some(1_000_000), ..Default::default() }).map_err(err)?;
    engine.insert_events("events", &events).map_err(err)
}

fn register(engine: &mut SqlEngine, sources: &[SqlSource], root: Option<&str>) -> CmdResult<()> {
    for s in sources {
        match s {
            SqlSource::File { name, path } => engine.register_file(name, std::path::Path::new(path)).map_err(err)?,
            SqlSource::Text { name, text, format } => engine.register_text(name, text, *format).map_err(err)?,
            SqlSource::History { name, id } => {
                let root = root.ok_or("history tables need a project")?;
                let h = devall_history::History::open_project(std::path::Path::new(root)).map_err(err)?;
                let (entry, body) = h.get(id).map_err(err)?.ok_or_else(|| format!("history entry {id} not found"))?;
                let text = String::from_utf8(body).map_err(|_| "that response body isn't text")?;
                let ct = entry.content_type.unwrap_or_default();
                let kind = if ct.contains("csv") { TextKind::Csv } else { TextKind::Json };
                engine.register_text(name, &text, kind).map_err(err)?;
            }
        }
    }
    Ok(())
}

/// Runs SQL over `logs` (an open source), `events` (a project) and any `sources`.
/// DuckDB also reads files directly: `SELECT * FROM read_csv_auto('C:/data/x.csv')`.
#[tauri::command]
pub async fn logs_sql(state: State<'_, LogsState>, target: SqlTarget, sql: String, sources: Option<Vec<SqlSource>>, max_rows: Option<usize>) -> CmdResult<SqlResult> {
    let max_rows = max_rows.unwrap_or(2000).min(100_000);
    let sources = sources.unwrap_or_default();
    match (&target.handle, &target.root) {
        (Some(handle), root) => {
            let src = state.get(handle)?;
            let root = root.clone();
            blocking(move || {
                let mut guard = ensure_logs_table(&src)?;
                let (engine, _, _) = guard.as_mut().unwrap();
                if let Some(r) = &root {
                    let store = EventStore::for_project(std::path::Path::new(r)).map_err(err)?;
                    load_events(engine, &store)?;
                }
                register(engine, &sources, root.as_deref())?;
                engine.query(&sql, max_rows).map_err(err)
            })
            .await
        }
        (None, Some(root)) => {
            let cell = state.event_sql(root);
            let root = root.clone();
            blocking(move || {
                let store = EventStore::for_project(std::path::Path::new(&root)).map_err(err)?;
                let mut guard = cell.lock().unwrap();
                let fresh = !matches!(guard.as_ref(), Some((_, v)) if *v == store.version());
                if fresh {
                    let mut engine = SqlEngine::new().map_err(err)?;
                    load_events(&mut engine, &store)?;
                    *guard = Some((engine, store.version()));
                }
                let (engine, _) = guard.as_mut().unwrap();
                register(engine, &sources, Some(&root))?;
                engine.query(&sql, max_rows).map_err(err)
            })
            .await
        }
        (None, None) => blocking(move || {
            let mut engine = SqlEngine::new().map_err(err)?;
            register(&mut engine, &sources, None)?;
            engine.query(&sql, max_rows).map_err(err)
        })
        .await,
    }
}

/// Tables and columns visible to `logs_sql` for a target (loads them if needed).
#[tauri::command]
pub async fn logs_sql_tables(state: State<'_, LogsState>, target: SqlTarget) -> CmdResult<(String, Vec<TableInfo>)> {
    match (&target.handle, &target.root) {
        (Some(handle), _) => {
            let src = state.get(handle)?;
            blocking(move || {
                let guard = ensure_logs_table(&src)?;
                let (engine, _, _) = guard.as_ref().unwrap();
                Ok((engine.backend().to_string(), engine.tables().map_err(err)?))
            })
            .await
        }
        (None, Some(root)) => {
            let cell = state.event_sql(root);
            let root = root.clone();
            blocking(move || {
                let store = EventStore::for_project(std::path::Path::new(&root)).map_err(err)?;
                let mut guard = cell.lock().unwrap();
                if !matches!(guard.as_ref(), Some((_, v)) if *v == store.version()) {
                    let mut engine = SqlEngine::new().map_err(err)?;
                    load_events(&mut engine, &store)?;
                    *guard = Some((engine, store.version()));
                }
                let (engine, _) = guard.as_ref().unwrap();
                Ok((engine.backend().to_string(), engine.tables().map_err(err)?))
            })
            .await
        }
        (None, None) => {
            let e = SqlEngine::new().map_err(err)?;
            Ok((e.backend().to_string(), Vec::new()))
        }
    }
}

// ---------- tail ----------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TailMsg {
    /// New entries (up to 200 of the newest are included).
    Appended { total: u64, lines: u64, entries: Vec<LogEntry> },
    /// The file was truncated or replaced: everything was re-indexed.
    Reset { reason: RefreshKind, total: u64 },
    /// The file is gone (rotation in progress?); following continues.
    Missing,
    Error { message: String },
}

/// Follows a source (polling every `interval_ms`, default 500). Returns a tail id for
/// `logs_untail`; one tail per source (a new call replaces the old one).
#[tauri::command]
pub fn logs_tail(state: State<'_, LogsState>, handle: String, on_lines: Channel<TailMsg>, interval_ms: Option<u64>) -> CmdResult<String> {
    let src = state.get(&handle)?;
    if !src.set.files().iter().all(|f| f.is_followable()) {
        return Err("this source can't be followed (compressed or transcoded files are static)".into());
    }
    let id = format!("tail-{handle}");
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(old) = state.tails.lock().unwrap().insert(id.clone(), stop.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    let interval = Duration::from_millis(interval_ms.unwrap_or(500).clamp(100, 10_000));
    tauri::async_runtime::spawn(async move {
        let mut last_total = src.set.len();
        let mut missing = false;
        while !stop.load(Ordering::Relaxed) {
            tokio::time::sleep(interval).await;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let s = src.clone();
            let res = tauri::async_runtime::spawn_blocking(move || {
                let mut kinds = Vec::new();
                for f in s.set.files() {
                    kinds.push(f.refresh().map_err(err)?.kind);
                }
                Ok::<_, String>(kinds)
            })
            .await;
            let msg = match res {
                Ok(Ok(kinds)) => {
                    let total = src.set.len();
                    if let Some(k) = kinds.iter().find(|k| matches!(k, RefreshKind::Truncated | RefreshKind::Rotated)) {
                        last_total = total;
                        missing = false;
                        Some(TailMsg::Reset { reason: *k, total })
                    } else if kinds.contains(&RefreshKind::Missing) {
                        (!std::mem::replace(&mut missing, true)).then_some(TailMsg::Missing)
                    } else if total != last_total || kinds.contains(&RefreshKind::Appended) {
                        missing = false;
                        let from = last_total.min(total).max(total.saturating_sub(200));
                        let s = src.clone();
                        let entries = tauri::async_runtime::spawn_blocking(move || {
                            let positions: Vec<u64> = (from..total).collect();
                            let mut out = Vec::new();
                            for p in positions {
                                if let Ok(Some(e)) = s.set.entry_at(p) {
                                    out.push(e);
                                }
                            }
                            out
                        })
                        .await
                        .unwrap_or_default();
                        let lines = src.set.files().iter().map(|f| f.line_count()).sum();
                        last_total = total;
                        Some(TailMsg::Appended { total, lines, entries })
                    } else {
                        None
                    }
                }
                Ok(Err(e)) => Some(TailMsg::Error { message: e }),
                Err(e) => Some(TailMsg::Error { message: e.to_string() }),
            };
            if let Some(m) = msg
                && on_lines.send(m).is_err()
            {
                break;
            }
        }
    });
    Ok(id)
}

/// Stops a `logs_tail` or `events_tail`.
#[tauri::command]
pub fn logs_untail(state: State<'_, LogsState>, tail_id: String) -> bool {
    match state.tails.lock().unwrap().remove(&tail_id) {
        Some(stop) => {
            stop.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

// ---------- events ----------

fn store(root: &str) -> CmdResult<Arc<EventStore>> {
    EventStore::for_project(std::path::Path::new(root)).map_err(err)
}

/// Project events matching a log-viewer filter (query language), oldest first, paged.
#[tauri::command]
pub async fn events_query(root: String, filter: Filter, offset: Option<u64>, limit: Option<usize>) -> CmdResult<Page> {
    blocking(move || store(&root)?.page(&filter, offset.unwrap_or(0), limit.unwrap_or(500).min(5000)).map_err(err)).await
}

/// Raw events by kind/time/correlation (for other modules and the AI).
#[tauri::command]
pub async fn events_list(root: String, filter: EventFilter) -> CmdResult<Vec<Event>> {
    blocking(move || store(&root)?.query(&filter).map_err(err)).await
}

#[tauri::command]
pub fn events_get(root: String, id: String) -> CmdResult<Option<Event>> {
    store(&root)?.get(&id).map_err(err)
}

/// Everything that shares a correlation id (one request across HTTP, mocks, webhooks…).
#[tauri::command]
pub async fn events_correlated(root: String, correlation_id: String) -> CmdResult<Vec<Event>> {
    blocking(move || store(&root)?.correlated(&correlation_id).map_err(err)).await
}

#[tauri::command]
pub async fn events_histogram(root: String, filter: Filter, buckets: Option<usize>) -> CmdResult<Histogram> {
    blocking(move || store(&root)?.histogram(&filter, buckets.unwrap_or(120).clamp(10, 1000)).map_err(err)).await
}

#[tauri::command]
pub async fn events_patterns(root: String, filter: Filter) -> CmdResult<PatternResult> {
    blocking(move || store(&root)?.patterns(&filter, &PatternOptions::default()).map_err(err)).await
}

#[tauri::command]
pub async fn events_top_values(root: String, filter: Filter, field: String, limit: Option<usize>) -> CmdResult<TopValues> {
    blocking(move || store(&root)?.top_values(&filter, &field, limit.unwrap_or(10)).map_err(err)).await
}

#[tauri::command]
pub async fn events_locate(root: String, filter: Filter, ts: i64) -> CmdResult<Option<u64>> {
    blocking(move || store(&root)?.locate(&filter, ts).map_err(err)).await
}

#[tauri::command]
pub fn events_stats(root: String) -> CmdResult<EventStats> {
    store(&root)?.stats().map_err(err)
}

/// Records an event from the UI (e.g. a note, or a frontend-only action).
#[tauri::command]
pub fn events_record(root: String, event: Event) -> CmdResult<Event> {
    store(&root)?.record(event).map_err(err)
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EventTailMsg {
    Events { events: Vec<Event> },
    /// Too many events arrived to stream; refetch.
    Lagged { skipped: u64 },
}

/// Streams newly recorded events (batched every ~100 ms). Stop with `logs_untail`.
#[tauri::command]
pub fn events_tail(state: State<'_, LogsState>, root: String, on_event: Channel<EventTailMsg>) -> CmdResult<String> {
    let mut rx = store(&root)?.subscribe();
    let id = state.id("events-tail");
    let stop = Arc::new(AtomicBool::new(false));
    state.tails.lock().unwrap().insert(id.clone(), stop.clone());
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        while !stop.load(Ordering::Relaxed) {
            let first = match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                Err(_) => continue,
                Ok(Ok(e)) => e,
                Ok(Err(RecvError::Lagged(n))) => {
                    if on_event.send(EventTailMsg::Lagged { skipped: n }).is_err() {
                        break;
                    }
                    continue;
                }
                Ok(Err(RecvError::Closed)) => break,
            };
            let mut events = vec![first];
            let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
            while events.len() < 500 {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Ok(e)) => events.push(e),
                    _ => break,
                }
            }
            if on_event.send(EventTailMsg::Events { events }).is_err() {
                break;
            }
        }
    });
    Ok(id)
}

#[tauri::command]
pub async fn events_clear(state: State<'_, LogsState>, root: String) -> CmdResult<usize> {
    let cell = state.event_sql(&root);
    *cell.lock().unwrap() = None;
    blocking(move || store(&root)?.clear().map_err(err)).await
}

// ---------- recording helpers for other modules ----------

/// Records any event into a project's store (best effort).
pub fn record_event(root: &str, event: Event) -> Option<Event> {
    devall_logs::events::record(root, event)
}

const SENSITIVE: &[&str] = &["authorization", "proxy-authorization", "cookie", "set-cookie", "x-api-key", "api-key", "x-auth-token"];

fn safe_headers(headers: &[(String, String)]) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(k, v)| {
                let v = if SENSITIVE.contains(&k.to_ascii_lowercase().as_str()) { "••••••".to_string() } else { v.clone() };
                json!([k, v])
            })
            .collect(),
    )
}

fn url_path(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match after_scheme.find('/') {
        Some(i) => after_scheme[i..].split(['?', '#']).next().unwrap_or("/").to_string(),
        None => "/".to_string(),
    }
}

fn status_level(status: Option<u16>, failed: bool) -> Level {
    match status {
        _ if failed => Level::Error,
        Some(s) if s >= 500 => Level::Error,
        Some(s) if s >= 400 => Level::Warn,
        _ => Level::Info,
    }
}

/// `http.send`: one line in `send_request` (see the module docs). `sent` is the masked
/// `SentSummary` (`{method, url, headers, body}`); bodies are not stored (history has them).
#[allow(clippy::too_many_arguments)]
pub fn record_http_send(
    root: &str,
    path: &str,
    name: &str,
    sent: &impl Serialize,
    resp: Option<&devall_http::HttpResponseData>,
    error: Option<&dyn Display>,
    history_id: Option<&str>,
    at: i64,
) {
    let sent = serde_json::to_value(sent).unwrap_or(Value::Null);
    let method = sent.get("method").and_then(Value::as_str).unwrap_or("GET").to_string();
    let url = sent.get("url").and_then(Value::as_str).unwrap_or("").to_string();
    let req_headers: Vec<(String, String)> = sent
        .get("headers")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|h| Some((h.get(0)?.as_str()?.to_string(), h.get(1)?.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let status = resp.map(|r| r.status);
    let body_text = resp.filter(|r| r.body.len() <= 1 << 20).and_then(|r| std::str::from_utf8(&r.body).ok());
    let corr = correlation::from_headers(req_headers.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .or_else(|| resp.and_then(|r| correlation::from_headers(r.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())))))
        .or_else(|| body_text.and_then(correlation::from_body));
    let title = match (status, error) {
        (Some(s), _) => format!("{method} {} → {s}", url_path(&url)),
        (None, Some(e)) => format!("{method} {} failed: {e}", url_path(&url)),
        (None, None) => format!("{method} {}", url_path(&url)),
    };
    let data = json!({
        "name": name,
        "method": method,
        "url": url,
        "status": status,
        "statusText": resp.map(|r| r.status_text.clone()),
        "size": resp.map(|r| r.body_size),
        "contentType": resp.and_then(|r| r.header("content-type").map(str::to_string)),
        "ttfbMs": resp.map(|r| r.timings.ttfb_ms),
        "error": error.map(|e| e.to_string()),
        "historyId": history_id,
        "requestHeaders": safe_headers(&req_headers),
        "responseHeaders": resp.map(|r| safe_headers(&r.headers)),
    });
    let mut e = Event::new(kinds::HTTP_SEND, "http", title)
        .at(at)
        .level(status_level(status, error.is_some() && status.is_none()))
        .correlation(corr)
        .request_path(Some(path))
        .data(data);
    if let Some(r) = resp {
        e = e.duration_ms(r.timings.total_ms);
    }
    record_event(root, e);
}

/// `http.run`: a collection run finished. `summary` is the runner's `RunSummary`.
pub fn record_collection_run(root: &str, folder: &str, summary: &impl Serialize) {
    let s = serde_json::to_value(summary).unwrap_or(Value::Null);
    let n = |k: &str| s.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (total, failed) = (n("total"), n("failed"));
    let label = if folder.is_empty() { "collection".to_string() } else { folder.to_string() };
    let title = format!("Ran {label}: {}/{total} passed", total.saturating_sub(failed));
    let data = json!({
        "folder": folder,
        "total": total,
        "passed": n("passed"),
        "failed": failed,
        "testsPassed": n("testsPassed"),
        "testsFailed": n("testsFailed"),
        "cancelled": s.get("cancelled"),
    });
    let mut e = Event::new(kinds::HTTP_RUN, "http", title).level(if failed > 0 { Level::Warn } else { Level::Info }).data(data);
    if let Some(ms) = s.get("durationMs").and_then(Value::as_f64) {
        e = e.duration_ms(ms);
    }
    record_event(root, e);
}

/// `load.run`: a load test finished. `summary` is `devall_load::LoadSummary`.
pub fn record_load_run(root: &str, summary: &impl Serialize) {
    let s = serde_json::to_value(summary).unwrap_or(Value::Null);
    let get = |p: &str| s.pointer(p).cloned().unwrap_or(Value::Null);
    let test = get("/test").as_str().unwrap_or("load test").to_string();
    let requests = get("/totals/requests").as_u64().unwrap_or(0);
    let err_rate = get("/totals/errorRate").as_f64().unwrap_or(0.0);
    let passed = get("/thresholdsPassed").as_bool().unwrap_or(true);
    let aborted = get("/aborted").as_bool().unwrap_or(false);
    let title = format!(
        "{test}: {requests} requests, p95 {:.0} ms, {:.1}% errors{}",
        get("/latency/p95").as_f64().unwrap_or(0.0),
        err_rate * 100.0,
        if aborted { " (aborted)" } else if passed { "" } else { " — thresholds failed" }
    );
    let level = if !passed || err_rate > 0.05 { Level::Error } else if aborted || err_rate > 0.0 { Level::Warn } else { Level::Info };
    let data = json!({
        "runId": get("/runId"),
        "testPath": get("/testPath"),
        "environment": get("/environment"),
        "requests": requests,
        "failed": get("/totals/failed"),
        "errorRate": err_rate,
        "rps": get("/totals/rps"),
        "p50": get("/latency/p50"),
        "p95": get("/latency/p95"),
        "p99": get("/latency/p99"),
        "thresholdsPassed": passed,
        "aborted": aborted,
        "statusCodes": get("/statusCodes"),
    });
    let mut e = Event::new(kinds::LOAD_RUN, "load", title).level(level).correlation(get("/runId").as_str().map(str::to_string)).data(data);
    if let Some(ms) = get("/durationMs").as_f64() {
        e = e.duration_ms(ms);
    }
    record_event(root, e);
}

/// `mock.request` (+ `mock.stopped`): forwards a mock server's journal broadcast
/// (`MockHandle::subscribe()`) into the event store until the server stops.
pub fn forward_mock_journal<T: Serialize + Clone + Send + 'static>(root: &str, mut rx: tokio::sync::broadcast::Receiver<T>) {
    let root = root.to_string();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            let msg = match rx.recv().await {
                Ok(m) => m,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            };
            let v = serde_json::to_value(&msg).unwrap_or(Value::Null);
            match v.get("type").and_then(Value::as_str) {
                Some("request") => {
                    let method = v.get("method").and_then(Value::as_str).unwrap_or("GET");
                    let url = v.get("url").and_then(Value::as_str).unwrap_or("/");
                    let status = v.get("status").and_then(Value::as_u64).map(|s| s as u16);
                    let matched = v.get("wasMatched").and_then(Value::as_bool).unwrap_or(true);
                    let title = format!("{method} {} → {}{}", url_path(url), status.unwrap_or(0), if matched { "" } else { " (no stub matched)" });
                    let level = if !matched { Level::Warn } else { status_level(status, false) };
                    let mut e = Event::new(kinds::MOCK_REQUEST, "mock", title).level(level).data(v.clone());
                    if let Some(at) = v.get("loggedDate").and_then(Value::as_i64) {
                        e = e.at(at);
                    }
                    if let Some(ms) = v.get("totalTimeMs").and_then(Value::as_f64) {
                        e = e.duration_ms(ms);
                    }
                    record_event(&root, e);
                }
                Some("stopped") => {
                    record_event(&root, Event::new(kinds::MOCK_STOPPED, "mock", "Mock server stopped"));
                    break;
                }
                _ => {}
            }
        }
    });
}

/// `webhook.received`: a webhook hit the local receiver.
pub fn record_webhook(root: &str, method: &str, path: &str, headers: &[(String, String)], body: Option<&str>, status: u16) {
    let corr = correlation::extract(headers.iter().map(|(k, v)| (k.as_str(), v.as_str())), body);
    let preview: Option<String> = body.map(|b| b.chars().take(2000).collect());
    let e = Event::new(kinds::WEBHOOK_RECEIVED, "webhooks", format!("{method} {path} → {status}"))
        .level(status_level(Some(status), false))
        .correlation(corr)
        .data(json!({ "method": method, "path": path, "status": status, "headers": safe_headers(headers), "bodyPreview": preview }));
    record_event(root, e);
}

/// `forward.started` / `forward.stopped`: a port-forward (Kubernetes, SSH tunnel) changed.
pub fn record_forward(root: &str, started: bool, target: &str, local_port: u16, remote: &str) {
    let (kind, verb) = if started { (kinds::FORWARD_STARTED, "Forwarding") } else { (kinds::FORWARD_STOPPED, "Stopped forwarding") };
    let e = Event::new(kind, "infra", format!("{verb} localhost:{local_port} → {target}"))
        .data(json!({ "target": target, "localPort": local_port, "remote": remote }));
    record_event(root, e);
}

/// `notebook.run`: a notebook (or one cell) ran.
pub fn record_notebook_run(root: &str, notebook: &str, cell: Option<&str>, ok: bool, duration_ms: f64, data: Value) {
    let title = match cell {
        Some(c) => format!("{notebook} › {c} {}", if ok { "ran" } else { "failed" }),
        None => format!("{notebook} {}", if ok { "ran" } else { "failed" }),
    };
    let e = Event::new(kinds::NOTEBOOK_RUN, "notebook", title)
        .level(if ok { Level::Info } else { Level::Error })
        .duration_ms(duration_ms)
        .data(json!({ "notebook": notebook, "cell": cell, "ok": ok, "details": data }));
    record_event(root, e);
}

/// Masks the given secret values in any string inside `v` (for callers building `data`).
pub fn redact_json(v: &mut Value, secrets: &BTreeSet<String>) {
    match v {
        Value::String(s) => *s = devall_core::vars::redact_values(s, secrets),
        Value::Array(a) => a.iter_mut().for_each(|x| redact_json(x, secrets)),
        Value::Object(o) => o.values_mut().for_each(|x| redact_json(x, secrets)),
        _ => {}
    }
}

