//! The project event store: one timeline of what every module did (HTTP sends, mock hits,
//! webhooks, load runs, port-forwards, notebook runs…) tagged with correlation ids, kept in
//! `<project data>/events.sqlite` (on the device, outside the repository).
//!
//! Callers redact secrets from `data` before recording. Stores are shared per project through
//! [`EventStore::for_project`] (so every recorder and subscriber sees the same live stream) and
//! [`record`] is a fire-and-forget one-liner for other modules.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::broadcast;

use crate::analyze::{self, Clusterer, Histogram, PatternOptions, PatternResult, TopAcc, TopValues};
use crate::parse::{Format, Level, LogEntry};
use crate::query::Filter;
use crate::view::Page;
use crate::{Result, now_ms};

/// Well-known event kinds (`<module>.<what>`). Modules may add their own.
pub mod kinds {
    pub const HTTP_SEND: &str = "http.send";
    pub const HTTP_RUN: &str = "http.run";
    pub const MOCK_REQUEST: &str = "mock.request";
    pub const MOCK_STARTED: &str = "mock.started";
    pub const MOCK_STOPPED: &str = "mock.stopped";
    pub const WEBHOOK_RECEIVED: &str = "webhook.received";
    pub const LOAD_RUN: &str = "load.run";
    pub const FORWARD_STARTED: &str = "forward.started";
    pub const FORWARD_STOPPED: &str = "forward.stopped";
    pub const NOTEBOOK_RUN: &str = "notebook.run";
    pub const DB_QUERY: &str = "db.query";
    pub const DOCKER: &str = "docker.event";
    pub const K8S: &str = "k8s.event";
    pub const NOTE: &str = "app.note";

    pub const ALL: &[&str] = &[
        HTTP_SEND,
        HTTP_RUN,
        MOCK_REQUEST,
        MOCK_STARTED,
        MOCK_STOPPED,
        WEBHOOK_RECEIVED,
        LOAD_RUN,
        FORWARD_STARTED,
        FORWARD_STOPPED,
        NOTEBOOK_RUN,
        DB_QUERY,
        DOCKER,
        K8S,
        NOTE,
    ];
}

fn info() -> Level {
    Level::Info
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// ULID; assigned when empty.
    #[serde(default)]
    pub id: String,
    /// Unix ms; now when 0.
    #[serde(default)]
    pub at: i64,
    pub kind: String,
    /// Module that recorded it (`http`, `mock`, `load`…).
    #[serde(default)]
    pub source: String,
    #[serde(default = "info")]
    pub level: Level,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Request file (relative to `http/`) this event is about, for links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_path: Option<String>,
    /// Details, already redacted by the caller.
    #[serde(default)]
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    /// Insertion order (assigned by the store).
    #[serde(default)]
    pub seq: i64,
}

impl Event {
    pub fn new(kind: &str, source: &str, title: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            at: 0,
            kind: kind.to_string(),
            source: source.to_string(),
            level: Level::Info,
            title: title.into(),
            correlation_id: None,
            request_path: None,
            data: Value::Null,
            duration_ms: None,
            seq: 0,
        }
    }

    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    pub fn at(mut self, at: i64) -> Self {
        self.at = at;
        self
    }

    pub fn correlation(mut self, id: Option<impl Into<String>>) -> Self {
        self.correlation_id = id.map(Into::into).filter(|s: &String| !s.is_empty());
        self
    }

    pub fn request_path(mut self, path: Option<impl Into<String>>) -> Self {
        self.request_path = path.map(Into::into).filter(|s: &String| !s.is_empty());
        self
    }

    pub fn data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    pub fn duration_ms(mut self, ms: f64) -> Self {
        self.duration_ms = Some(ms);
        self
    }

    /// As a log entry, so events share the log viewer's query language, histogram and patterns.
    /// Fields: `kind`, `source`, `id`, `correlationId`, `requestPath`, `durationMs`, `data`, plus
    /// `data`'s top-level scalars (so `status>=500` works on HTTP events).
    pub fn to_log_entry(&self, index: u64) -> LogEntry {
        let mut fields = Map::new();
        fields.insert("kind".into(), Value::String(self.kind.clone()));
        fields.insert("source".into(), Value::String(self.source.clone()));
        fields.insert("id".into(), Value::String(self.id.clone()));
        if let Some(c) = &self.correlation_id {
            fields.insert("correlationId".into(), Value::String(c.clone()));
        }
        if let Some(p) = &self.request_path {
            fields.insert("requestPath".into(), Value::String(p.clone()));
        }
        if let Some(d) = self.duration_ms {
            fields.insert("durationMs".into(), Value::from(d));
        }
        if let Value::Object(o) = &self.data {
            for (k, v) in o {
                if !fields.contains_key(k) && !v.is_object() && !v.is_array() {
                    fields.insert(k.clone(), v.clone());
                }
            }
        }
        if !self.data.is_null() {
            fields.insert("data".into(), self.data.clone());
        }
        LogEntry {
            index,
            line_no: self.seq.max(0) as u64,
            line_count: 1,
            ts: Some(self.at),
            level: self.level,
            message: self.title.clone(),
            fields,
            raw: serde_json::to_string(self).unwrap_or_default(),
            format: Format::Json,
            correlation_id: self.correlation_id.clone(),
            source: Some(self.source.clone()),
        }
    }
}

/// Plain SQL-side filter for listing events.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EventFilter {
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Exact kinds, or prefixes ending in `.` or `*` (`http.*`).
    pub kinds: Vec<String>,
    pub sources: Vec<String>,
    pub levels: Vec<Level>,
    pub correlation_id: Option<String>,
    pub request_path: Option<String>,
    /// Substring of title, kind or data.
    pub text: Option<String>,
    /// Only events recorded after this sequence number (live catch-up).
    pub after_seq: Option<i64>,
    /// Default 1000.
    pub limit: Option<usize>,
    pub newest_first: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Retention {
    pub max_events: u64,
    pub max_age_ms: i64,
}

impl Default for Retention {
    fn default() -> Self {
        Self { max_events: 50_000, max_age_ms: 30 * 86_400_000 }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventStats {
    pub count: u64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    pub kinds: Vec<(String, u64)>,
}

struct Cached {
    filter: Filter,
    version: u64,
    entries: Arc<Vec<LogEntry>>,
}

pub struct EventStore {
    conn: Mutex<Connection>,
    tx: broadcast::Sender<Event>,
    retention: Retention,
    since_prune: AtomicU32,
    version: AtomicU64,
    path: PathBuf,
    cache: Mutex<Option<Cached>>,
}

const PRUNE_EVERY: u32 = 256;
/// Most entries the query-language view loads (retention keeps the table near this anyway).
const MAX_SCAN: usize = 200_000;

fn registry() -> &'static Mutex<HashMap<String, Arc<EventStore>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<EventStore>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Registry key: separators unified, trailing separators dropped, case-folded on Windows.
pub fn root_key(root: &Path) -> String {
    let s = root.to_string_lossy().replace('\\', "/");
    let t = s.trim_end_matches('/');
    let t = if t.is_empty() { s.as_str() } else { t };
    if cfg!(windows) { t.to_lowercase() } else { t.to_string() }
}

/// Records into the project's store, ignoring failures (for one-line calls from other modules).
pub fn record(root: impl AsRef<Path>, event: Event) -> Option<Event> {
    EventStore::for_project(root.as_ref()).ok()?.record(event).ok()
}

impl EventStore {
    /// The shared store of a project (opened on first use).
    pub fn for_project(root: &Path) -> Result<Arc<EventStore>> {
        let key = root_key(root);
        let mut reg = registry().lock().unwrap();
        if let Some(s) = reg.get(&key) {
            return Ok(s.clone());
        }
        let s = Arc::new(Self::open_project(root)?);
        reg.insert(key, s.clone());
        Ok(s)
    }

    /// Drops the shared store of a project (e.g. after its folder was deleted).
    pub fn forget(root: &Path) {
        registry().lock().unwrap().remove(&root_key(root));
    }

    /// Opens (creating if needed) `<project data>/events.sqlite` (in the app data directory, see
    /// `devall_core::local::project_dir`).
    pub fn open_project(root: &Path) -> Result<Self> {
        let dir = devall_core::local::project_dir(root);
        std::fs::create_dir_all(&dir)?;
        Self::open(&dir.join("events.sqlite"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                at INTEGER NOT NULL,
                kind TEXT NOT NULL,
                source TEXT NOT NULL,
                level TEXT NOT NULL,
                title TEXT NOT NULL,
                correlation_id TEXT,
                request_path TEXT,
                data TEXT NOT NULL,
                duration_ms REAL
            );
            CREATE INDEX IF NOT EXISTS events_at ON events(at);
            CREATE INDEX IF NOT EXISTS events_kind ON events(kind, at);
            CREATE INDEX IF NOT EXISTS events_corr ON events(correlation_id) WHERE correlation_id IS NOT NULL;",
        )?;
        let (tx, _) = broadcast::channel(4096);
        let s = Self {
            conn: Mutex::new(conn),
            tx,
            retention: Retention::default(),
            since_prune: AtomicU32::new(0),
            version: AtomicU64::new(0),
            path: path.to_path_buf(),
            cache: Mutex::new(None),
        };
        s.prune()?;
        Ok(s)
    }

    pub fn with_retention(mut self, r: Retention) -> Self {
        self.retention = r;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn retention(&self) -> Retention {
        self.retention
    }

    /// Live stream of recorded events (lagging receivers skip ahead).
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Bumped on every change (record, clear, prune).
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Stores an event (filling `id`, `at`, `seq`) and broadcasts it.
    pub fn record(&self, mut e: Event) -> Result<Event> {
        if e.id.is_empty() {
            e.id = ulid::Ulid::generate().to_string();
        }
        if e.at == 0 {
            e.at = now_ms();
        }
        if e.source.is_empty() {
            e.source = e.kind.split('.').next().unwrap_or("app").to_string();
        }
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO events (id, at, kind, source, level, title, correlation_id, request_path, data, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![e.id, e.at, e.kind, e.source, e.level.as_str(), e.title, e.correlation_id, e.request_path, e.data.to_string(), e.duration_ms],
            )?;
            e.seq = conn.last_insert_rowid();
        }
        self.version.fetch_add(1, Ordering::AcqRel);
        let _ = self.tx.send(e.clone());
        if self.since_prune.fetch_add(1, Ordering::Relaxed) + 1 >= PRUNE_EVERY {
            self.since_prune.store(0, Ordering::Relaxed);
            self.prune()?;
        }
        Ok(e)
    }

    /// Applies retention (max events, max age); returns rows removed.
    pub fn prune(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let mut n = conn.execute("DELETE FROM events WHERE at < ?1", params![now_ms() - self.retention.max_age_ms])?;
        n += conn.execute(
            "DELETE FROM events WHERE seq <= (SELECT seq FROM events ORDER BY seq DESC LIMIT 1 OFFSET ?1)",
            params![self.retention.max_events as i64],
        )?;
        if n > 0 {
            self.version.fetch_add(1, Ordering::AcqRel);
        }
        Ok(n)
    }

    pub fn clear(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM events", [])?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        self.version.fetch_add(1, Ordering::AcqRel);
        Ok(n)
    }

    pub fn get(&self, id: &str) -> Result<Option<Event>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(&format!("SELECT {COLS} FROM events WHERE id = ?1"), params![id], row_event).optional()?)
    }

    /// Events matching `f`, oldest first unless `newest_first`.
    pub fn query(&self, f: &EventFilter) -> Result<Vec<Event>> {
        let mut sql = format!("SELECT {COLS} FROM events WHERE 1=1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut arg = |sql: &mut String, clause: &str, v: Box<dyn rusqlite::ToSql>| {
            args.push(v);
            sql.push_str(&clause.replace('?', &format!("?{}", args.len())));
        };
        if let Some(s) = f.since {
            arg(&mut sql, " AND at >= ?", Box::new(s));
        }
        if let Some(u) = f.until {
            arg(&mut sql, " AND at < ?", Box::new(u));
        }
        if let Some(a) = f.after_seq {
            arg(&mut sql, " AND seq > ?", Box::new(a));
        }
        if let Some(c) = &f.correlation_id {
            arg(&mut sql, " AND correlation_id = ?", Box::new(c.clone()));
        }
        if let Some(p) = &f.request_path {
            arg(&mut sql, " AND request_path = ?", Box::new(p.clone()));
        }
        if let Some(t) = f.text.as_ref().filter(|t| !t.is_empty()) {
            let like = format!("%{}%", t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            args.push(Box::new(like));
            let i = args.len();
            sql.push_str(&format!(" AND (title LIKE ?{i} ESCAPE '\\' OR kind LIKE ?{i} ESCAPE '\\' OR data LIKE ?{i} ESCAPE '\\')"));
        }
        if !f.kinds.is_empty() {
            let mut parts = Vec::new();
            for k in &f.kinds {
                let prefix = k.strip_suffix('*').or_else(|| k.ends_with('.').then_some(k.as_str()));
                match prefix {
                    Some(p) => {
                        args.push(Box::new(format!("{}%", p.replace('%', "\\%").replace('_', "\\_"))));
                        parts.push(format!("kind LIKE ?{} ESCAPE '\\'", args.len()));
                    }
                    None => {
                        args.push(Box::new(k.clone()));
                        parts.push(format!("kind = ?{}", args.len()));
                    }
                }
            }
            sql.push_str(&format!(" AND ({})", parts.join(" OR ")));
        }
        if !f.sources.is_empty() {
            let mut parts = Vec::new();
            for s in &f.sources {
                args.push(Box::new(s.clone()));
                parts.push(format!("?{}", args.len()));
            }
            sql.push_str(&format!(" AND source IN ({})", parts.join(", ")));
        }
        if !f.levels.is_empty() && f.levels.len() < Level::ALL.len() {
            let list: Vec<String> = f.levels.iter().map(|l| format!("'{}'", l.as_str())).collect();
            sql.push_str(&format!(" AND level IN ({})", list.join(", ")));
        }
        let limit = f.limit.unwrap_or(1000).min(MAX_SCAN);
        if f.newest_first {
            sql.push_str(&format!(" ORDER BY at DESC, seq DESC LIMIT {limit}"));
        } else {
            // Oldest-first within the newest `limit`.
            sql = format!("SELECT * FROM ({sql} ORDER BY at DESC, seq DESC LIMIT {limit}) ORDER BY at, seq");
        }
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = st.query_map(refs.as_slice(), row_event)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every event sharing a correlation id, oldest first.
    pub fn correlated(&self, correlation_id: &str) -> Result<Vec<Event>> {
        self.query(&EventFilter { correlation_id: Some(correlation_id.to_string()), limit: Some(10_000), ..Default::default() })
    }

    pub fn stats(&self) -> Result<EventStats> {
        let conn = self.conn.lock().unwrap();
        let (count, first_at, last_at): (i64, Option<i64>, Option<i64>) =
            conn.query_row("SELECT count(*), min(at), max(at) FROM events", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut st = conn.prepare("SELECT kind, count(*) FROM events GROUP BY kind ORDER BY 2 DESC")?;
        let kinds = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?.collect::<std::result::Result<_, _>>()?;
        Ok(EventStats { count: count as u64, first_at, last_at, kinds })
    }

    // ---------- log-viewer view (query language) ----------

    /// Events (oldest first) matching a log-viewer [`Filter`], as entries; cached per version.
    pub fn entries(&self, filter: &Filter) -> Result<Arc<Vec<LogEntry>>> {
        let version = self.version();
        if let Some(c) = self.cache.lock().unwrap().as_ref()
            && c.version == version
            && c.filter == *filter
        {
            return Ok(c.entries.clone());
        }
        let compiled = filter.compile()?;
        let events = self.query(&EventFilter {
            since: filter.from,
            until: filter.to,
            levels: filter.levels.clone(),
            limit: Some(MAX_SCAN),
            ..Default::default()
        })?;
        let mut out = Vec::new();
        for e in &events {
            let entry = e.to_log_entry(out.len() as u64);
            if compiled.matches(&entry) {
                out.push(entry);
            }
        }
        let out = Arc::new(out);
        *self.cache.lock().unwrap() = Some(Cached { filter: filter.clone(), version, entries: out.clone() });
        Ok(out)
    }

    /// A page of matching events as entries (`index` = row in the result).
    pub fn page(&self, filter: &Filter, offset: u64, limit: usize) -> Result<Page> {
        let t0 = std::time::Instant::now();
        let all = self.entries(filter)?;
        let total = all.len() as u64;
        let start = offset.min(total) as usize;
        let end = (start + limit).min(all.len());
        Ok(Page {
            total,
            view_total: self.stats()?.count,
            offset,
            entries: all[start..end].to_vec(),
            indexing: false,
            elapsed_ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    pub fn histogram(&self, filter: &Filter, target: usize) -> Result<Histogram> {
        let all = self.entries(filter)?;
        let range = match (filter.from, filter.to) {
            (Some(a), Some(b)) if b > a => Some((a, b)),
            _ => None,
        };
        Ok(analyze::histogram(range, target, &mut |f| {
            for e in all.iter() {
                f(e.ts, e.level);
            }
        }))
    }

    pub fn patterns(&self, filter: &Filter, opts: &PatternOptions) -> Result<PatternResult> {
        let all = self.entries(filter)?;
        let range = match (filter.from, filter.to) {
            (Some(a), Some(b)) if b > a => Some((a, b)),
            _ => match (all.first().and_then(|e| e.ts), all.last().and_then(|e| e.ts)) {
                (Some(a), Some(b)) => Some((a, b + 1)),
                _ => None,
            },
        };
        let mut c = Clusterer::new(range, opts.spark_buckets);
        for e in all.iter() {
            c.add(&e.message, e.ts, e.level, e.index);
        }
        Ok(c.finish(opts))
    }

    pub fn top_values(&self, filter: &Filter, field: &str, limit: usize) -> Result<TopValues> {
        let all = self.entries(filter)?;
        let mut acc = TopAcc::default();
        for e in all.iter() {
            acc.add(analyze::field_values(e, field));
        }
        Ok(acc.finish(field, limit))
    }

    /// Row of the first matching event at or after `ts`.
    pub fn locate(&self, filter: &Filter, ts: i64) -> Result<Option<u64>> {
        Ok(self.entries(filter)?.iter().position(|e| e.ts.is_some_and(|t| t >= ts)).map(|p| p as u64))
    }
}

const COLS: &str = "seq, id, at, kind, source, level, title, correlation_id, request_path, data, duration_ms";

fn row_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let level: String = r.get(5)?;
    let data: String = r.get(9)?;
    Ok(Event {
        seq: r.get(0)?,
        id: r.get(1)?,
        at: r.get(2)?,
        kind: r.get(3)?,
        source: r.get(4)?,
        level: Level::parse(&level).unwrap_or_default(),
        title: r.get(6)?,
        correlation_id: r.get(7)?,
        request_path: r.get(8)?,
        data: serde_json::from_str(&data).unwrap_or(Value::Null),
        duration_ms: r.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_query_correlate() {
        devall_core::local::isolate_for_tests();
        let dir = tempfile::tempdir().unwrap();
        let s = EventStore::open_project(dir.path()).unwrap();
        let mut rx = s.subscribe();
        let base = now_ms() - 10_000;
        let a = s
            .record(
                Event::new(kinds::HTTP_SEND, "http", "GET /users → 200")
                    .at(base)
                    .correlation(Some("c-1"))
                    .request_path(Some("users/list.req.yaml"))
                    .data(json!({"method": "GET", "status": 200, "url": "http://x/users"}))
                    .duration_ms(12.5),
            )
            .unwrap();
        assert!(!a.id.is_empty() && a.seq == 1);
        assert_eq!(rx.try_recv().unwrap().id, a.id);
        s.record(Event::new(kinds::MOCK_REQUEST, "mock", "GET /users").at(base + 1).correlation(Some("c-1")).data(json!({"status": 200}))).unwrap();
        s.record(Event::new(kinds::HTTP_SEND, "http", "POST /orders → 503").at(base + 2).level(Level::Error).data(json!({"status": 503}))).unwrap();
        s.record(Event::new(kinds::LOAD_RUN, "", "smoke: 1000 requests").at(base + 3)).unwrap();

        assert_eq!(s.query(&EventFilter::default()).unwrap().len(), 4);
        assert_eq!(s.query(&EventFilter { kinds: vec!["http.*".into()], ..Default::default() }).unwrap().len(), 2);
        assert_eq!(s.query(&EventFilter { kinds: vec!["mock.request".into(), "load.run".into()], ..Default::default() }).unwrap().len(), 2);
        assert_eq!(s.correlated("c-1").unwrap().iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["http.send", "mock.request"]);
        assert_eq!(s.query(&EventFilter { text: Some("orders".into()), ..Default::default() }).unwrap().len(), 1);
        assert_eq!(s.query(&EventFilter { text: Some("50%".into()), ..Default::default() }).unwrap().len(), 0);
        assert_eq!(s.query(&EventFilter { levels: vec![Level::Error], ..Default::default() }).unwrap().len(), 1);
        assert_eq!(s.query(&EventFilter { since: Some(base + 2), ..Default::default() }).unwrap().len(), 2);
        assert_eq!(s.query(&EventFilter { after_seq: Some(3), ..Default::default() }).unwrap()[0].kind, "load.run");
        let newest = s.query(&EventFilter { newest_first: true, limit: Some(1), ..Default::default() }).unwrap();
        assert_eq!(newest[0].kind, "load.run");
        let oldest_of_last2 = s.query(&EventFilter { limit: Some(2), ..Default::default() }).unwrap();
        assert_eq!(oldest_of_last2[0].title, "POST /orders → 503");
        assert_eq!(s.get(&a.id).unwrap().unwrap().data["url"], "http://x/users");
        assert_eq!(s.query(&EventFilter::default()).unwrap()[3].source, "load");

        // The log-viewer view: query language over events.
        let p = s.page(&Filter::query("status>=500"), 0, 10).unwrap();
        assert_eq!((p.total, p.entries[0].message.as_str()), (1, "POST /orders → 503"));
        assert_eq!(s.page(&Filter::query("kind=http.send trace=c-1"), 0, 10).unwrap().total, 1);
        assert_eq!(s.page(&Filter::query("source=mock"), 0, 10).unwrap().total, 1);
        assert_eq!(s.histogram(&Filter::default(), 10).unwrap().total, 4);
        assert_eq!(s.top_values(&Filter::default(), "kind", 5).unwrap().values[0].value, "http.send");
        assert_eq!(s.patterns(&Filter::default(), &PatternOptions::default()).unwrap().total, 4);
        let st = s.stats().unwrap();
        assert_eq!((st.count, st.kinds[0].0.as_str()), (4, "http.send"));

        assert_eq!(s.clear().unwrap(), 4);
        assert_eq!(s.page(&Filter::default(), 0, 10).unwrap().total, 0);
    }

    #[test]
    fn retention_and_registry() {
        let data = devall_core::local::isolate_for_tests();
        let dir = tempfile::tempdir().unwrap();
        let s = EventStore::open_project(dir.path()).unwrap().with_retention(Retention { max_events: 100, max_age_ms: 86_400_000 });
        s.record(Event::new("app.note", "app", "ancient").at(now_ms() - 2 * 86_400_000)).unwrap();
        for i in 0..300 {
            s.record(Event::new("app.note", "app", format!("n{i}"))).unwrap();
        }
        s.prune().unwrap();
        let all = s.query(&EventFilter { limit: Some(1000), ..Default::default() }).unwrap();
        assert_eq!(all.len(), 100);
        assert_eq!(all.last().unwrap().title, "n299");
        assert!(all.iter().all(|e| e.title != "ancient"));

        let other = tempfile::tempdir().unwrap();
        let a = EventStore::for_project(other.path()).unwrap();
        let b = EventStore::for_project(&other.path().join("")).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let mut rx = b.subscribe();
        assert!(record(other.path(), Event::new(kinds::NOTE, "app", "hello")).is_some());
        assert_eq!(rx.try_recv().unwrap().title, "hello");
        assert!(devall_core::local::project_dir(other.path()).join("events.sqlite").is_file());
        assert!(devall_core::local::project_dir(other.path()).starts_with(&data) && !other.path().join(".devall").exists());
        EventStore::forget(other.path());
    }
}
