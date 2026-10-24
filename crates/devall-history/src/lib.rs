//! Local history, kept on the user's device (never in the repository):
//!
//! * [`History`] — every response, in `<project data>/history.sqlite`.
//! * [`files::FileHistory`] — versions of the project's own files (local file history).
//! * [`utilities::UtilityHistory`] — utility runs, device-wide.
//!
//! `<project data>` is [`devall_core::local::project_dir`] (in the app data directory), shared by
//! the desktop app and the CLI.
//!
//! Response history is unlimited apart from a configurable per-request retention (default 100
//! entries) to keep the database small. Request details are stored with secrets already masked by
//! the caller; response bodies are stored as received (they stay on the user's machine).

pub mod files;
pub mod utilities;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const DEFAULT_KEEP_PER_REQUEST: usize = 100;
/// Bodies above this are stored truncated.
pub const MAX_STORED_BODY: usize = 5 * 1024 * 1024;
pub const DB_FILE: &str = "history.sqlite";

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("history database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("history: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Core(#[from] devall_core::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, HistoryError>;

/// One send, as stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    /// Request path relative to `http/` (empty for unsaved drafts).
    pub request_path: String,
    pub request_name: String,
    pub environment: Option<String>,
    pub method: String,
    /// Masked.
    pub url: String,
    /// Unix milliseconds.
    pub at: i64,
    /// `None` when the request failed before a response.
    pub status: Option<u16>,
    pub error: Option<String>,
    pub duration_ms: f64,
    pub size: usize,
    pub content_type: Option<String>,
    /// Masked request headers + body summary, as JSON (`SentSummary` in the app).
    pub sent: serde_json::Value,
    pub response_headers: Vec<(String, String)>,
    /// Assertion results as JSON (empty array when none).
    pub tests: serde_json::Value,
    pub body_truncated: bool,
}

/// List item without the heavy fields.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistorySummary {
    pub id: String,
    pub request_path: String,
    pub request_name: String,
    pub environment: Option<String>,
    pub method: String,
    pub url: String,
    pub at: i64,
    pub status: Option<u16>,
    pub error: Option<String>,
    pub duration_ms: f64,
    pub size: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
}

pub struct History {
    conn: Mutex<Connection>,
    keep_per_request: AtomicUsize,
    path: PathBuf,
}

const SUMMARY_COLUMNS: &str = "id, request_path, request_name, environment, method, url, at, status, error, duration_ms, size, tests";

impl History {
    /// Opens (creating if needed) `<project data>/history.sqlite`.
    pub fn open_project(project_root: &Path) -> Result<Self> {
        let dir = devall_core::local::project_dir(project_root);
        std::fs::create_dir_all(&dir)?;
        Self::open(&dir.join(DB_FILE))
    }

    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id TEXT PRIMARY KEY,
                request_path TEXT NOT NULL,
                request_name TEXT NOT NULL,
                environment TEXT,
                method TEXT NOT NULL,
                url TEXT NOT NULL,
                at INTEGER NOT NULL,
                status INTEGER,
                error TEXT,
                duration_ms REAL NOT NULL,
                size INTEGER NOT NULL,
                content_type TEXT,
                sent TEXT NOT NULL,
                response_headers TEXT NOT NULL,
                tests TEXT NOT NULL,
                body BLOB,
                body_truncated INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS history_by_request ON history(request_path, at DESC);
            CREATE INDEX IF NOT EXISTS history_by_time ON history(at DESC);",
        )?;
        Ok(Self { conn: Mutex::new(conn), keep_per_request: AtomicUsize::new(DEFAULT_KEEP_PER_REQUEST), path: path.to_path_buf() })
    }

    pub fn with_retention(self, keep_per_request: usize) -> Self {
        self.set_retention(keep_per_request);
        self
    }

    /// Responses kept per request from now on (applied at each request's next insert).
    pub fn set_retention(&self, keep_per_request: usize) {
        self.keep_per_request.store(keep_per_request.max(1), Ordering::Relaxed);
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stores an entry with its body; returns the id. Prunes old entries for the same request.
    pub fn insert(&self, mut e: HistoryEntry, body: &[u8]) -> Result<String> {
        if e.id.is_empty() {
            e.id = ulid::Ulid::generate().to_string();
        }
        let truncated = e.body_truncated || body.len() > MAX_STORED_BODY;
        let stored = &body[..body.len().min(MAX_STORED_BODY)];
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO history (id, request_path, request_name, environment, method, url, at, status, error, duration_ms, size,
                content_type, sent, response_headers, tests, body, body_truncated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                e.id,
                e.request_path,
                e.request_name,
                e.environment,
                e.method,
                e.url,
                e.at,
                e.status,
                e.error,
                e.duration_ms,
                e.size as i64,
                e.content_type,
                e.sent.to_string(),
                serde_json::to_string(&e.response_headers).unwrap_or_else(|_| "[]".into()),
                e.tests.to_string(),
                stored,
                truncated as i64,
            ],
        )?;
        conn.execute(
            "DELETE FROM history WHERE request_path = ?1 AND id NOT IN (
                SELECT id FROM history WHERE request_path = ?1 ORDER BY at DESC, id DESC LIMIT ?2)",
            params![e.request_path, self.keep_per_request.load(Ordering::Relaxed) as i64],
        )?;
        Ok(e.id)
    }

    /// Newest first. `request_path = None` lists across all requests.
    pub fn list(&self, request_path: Option<&str>, limit: usize) -> Result<Vec<HistorySummary>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {SUMMARY_COLUMNS} FROM history WHERE (?1 IS NULL OR request_path = ?1) ORDER BY at DESC, id DESC LIMIT ?2");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![request_path, limit as i64], summary_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every request's responses, newest first, optionally filtered by `query` (matched against
    /// the URL, request name and path, method and status). For the History page.
    pub fn search(&self, query: Option<&str>, offset: usize, limit: usize) -> Result<Vec<HistorySummary>> {
        let conn = self.conn.lock().unwrap();
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(like_pattern);
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM history
             WHERE (?1 IS NULL OR url LIKE ?1 ESCAPE '!' OR request_name LIKE ?1 ESCAPE '!' OR request_path LIKE ?1 ESCAPE '!'
                    OR method LIKE ?1 ESCAPE '!' OR CAST(status AS TEXT) LIKE ?1 ESCAPE '!')
             ORDER BY at DESC, id DESC LIMIT ?2 OFFSET ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, limit as i64, offset as i64], summary_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Number of stored responses and the database size on disk (with its WAL).
    pub fn stats(&self) -> Result<(usize, u64)> {
        let n: i64 = self.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM history", [], |r| r.get(0))?;
        Ok((n as usize, db_size(&self.path)))
    }

    /// Full entry plus body.
    pub fn get(&self, id: &str) -> Result<Option<(HistoryEntry, Vec<u8>)>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, request_path, request_name, environment, method, url, at, status, error, duration_ms, size, content_type,
                    sent, response_headers, tests, body, body_truncated FROM history WHERE id = ?1",
            params![id],
            |row| {
                let sent: String = row.get(12)?;
                let headers: String = row.get(13)?;
                let tests: String = row.get(14)?;
                let body: Option<Vec<u8>> = row.get(15)?;
                Ok((
                    HistoryEntry {
                        id: row.get(0)?,
                        request_path: row.get(1)?,
                        request_name: row.get(2)?,
                        environment: row.get(3)?,
                        method: row.get(4)?,
                        url: row.get(5)?,
                        at: row.get(6)?,
                        status: row.get(7)?,
                        error: row.get(8)?,
                        duration_ms: row.get(9)?,
                        size: row.get::<_, i64>(10)? as usize,
                        content_type: row.get(11)?,
                        sent: serde_json::from_str(&sent).unwrap_or(serde_json::Value::Null),
                        response_headers: serde_json::from_str(&headers).unwrap_or_default(),
                        tests: serde_json::from_str(&tests).unwrap_or(serde_json::Value::Array(vec![])),
                        body_truncated: row.get::<_, i64>(16)? != 0,
                    },
                    body.unwrap_or_default(),
                ))
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// The entry sent just before `id` for the same request (for "compare with previous").
    pub fn previous(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT h2.id FROM history h1 JOIN history h2 ON h2.request_path = h1.request_path
             WHERE h1.id = ?1 AND (h2.at < h1.at OR (h2.at = h1.at AND h2.id < h1.id))
             ORDER BY h2.at DESC, h2.id DESC LIMIT 1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        Ok(self.conn.lock().unwrap().execute("DELETE FROM history WHERE id = ?1", params![id])? > 0)
    }

    /// Clears one request's history, or everything when `request_path` is `None`.
    pub fn clear(&self, request_path: Option<&str>) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM history WHERE (?1 IS NULL OR request_path = ?1)", params![request_path])?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        if request_path.is_none() {
            // Give the space back when everything goes.
            conn.execute_batch("VACUUM;")?;
        }
        Ok(n)
    }

    /// Re-points history after a request file was renamed or moved.
    pub fn rename_request(&self, from: &str, to: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("UPDATE history SET request_path = ?2 WHERE request_path = ?1", params![from, to])?;
        let prefix = format!("{from}/");
        let n2 = conn.execute(
            "UPDATE history SET request_path = ?2 || substr(request_path, ?3) WHERE substr(request_path, 1, length(?1)) = ?1",
            params![prefix, format!("{to}/"), prefix.chars().count() as i64 + 1],
        )?;
        Ok(n + n2)
    }
}

fn summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistorySummary> {
    let tests: String = row.get(11)?;
    let (passed, failed) = count_tests(&tests);
    Ok(HistorySummary {
        id: row.get(0)?,
        request_path: row.get(1)?,
        request_name: row.get(2)?,
        environment: row.get(3)?,
        method: row.get(4)?,
        url: row.get(5)?,
        at: row.get(6)?,
        status: row.get(7)?,
        error: row.get(8)?,
        duration_ms: row.get(9)?,
        size: row.get::<_, i64>(10)? as usize,
        tests_passed: passed,
        tests_failed: failed,
    })
}

/// `%text%` for `LIKE … ESCAPE '!'`, matching `%`, `_` and `!` in `text` literally.
pub(crate) fn like_pattern(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('%');
    for c in text.chars() {
        if matches!(c, '%' | '_' | '!') {
            out.push('!');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// Size of a SQLite database file plus its `-wal` (0 when missing).
pub fn db_size(path: &Path) -> u64 {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    [path, Path::new(&wal)].iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum()
}

fn count_tests(json: &str) -> (usize, usize) {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json) else { return (0, 0) };
    let passed = items.iter().filter(|t| t.get("passed").and_then(|p| p.as_bool()) == Some(true)).count();
    (passed, items.len() - passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, at: i64, status: u16) -> HistoryEntry {
        HistoryEntry {
            id: String::new(),
            request_path: path.into(),
            request_name: "R".into(),
            environment: Some("local".into()),
            method: "GET".into(),
            url: "http://x/••••••".into(),
            at,
            status: Some(status),
            error: None,
            duration_ms: 12.5,
            size: 3,
            content_type: Some("application/json".into()),
            sent: serde_json::json!({"headers": [["Authorization", "Bearer ••••••"]]}),
            response_headers: vec![("content-type".into(), "application/json".into())],
            tests: serde_json::json!([{"label": "status eq 200", "passed": true}, {"label": "x", "passed": false}]),
            body_truncated: false,
        }
    }

    #[test]
    fn insert_list_get_previous_prune_rename() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::open(&dir.path().join(DB_FILE)).unwrap().with_retention(3);
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(h.insert(entry("a.req.yaml", 1000 + i, 200 + i as u16), format!("{{\"n\":{i}}}").as_bytes()).unwrap());
        }
        h.insert(entry("dir/b.req.yaml", 2000, 500), b"oops").unwrap();

        let a = h.list(Some("a.req.yaml"), 50).unwrap();
        assert_eq!(a.len(), 3, "pruned to retention");
        assert_eq!(a[0].status, Some(204));
        assert_eq!((a[0].tests_passed, a[0].tests_failed), (1, 1));
        assert_eq!(h.list(None, 50).unwrap().len(), 4);

        let (e, body) = h.get(&ids[4]).unwrap().unwrap();
        assert_eq!(body, b"{\"n\":4}");
        assert_eq!(e.sent["headers"][0][1], "Bearer ••••••");
        assert_eq!(h.previous(&ids[4]).unwrap().as_deref(), Some(ids[3].as_str()));
        assert_eq!(h.previous(&ids[2]).unwrap(), None);

        assert_eq!(h.search(Some("dir/b"), 0, 10).unwrap().len(), 1);
        assert_eq!(h.search(Some("500"), 0, 10).unwrap()[0].status, Some(500));
        assert_eq!(h.search(None, 1, 2).unwrap().len(), 2);
        assert!(h.search(Some("100%"), 0, 10).unwrap().is_empty(), "LIKE wildcards are literal");
        assert_eq!(h.stats().unwrap().0, 4);

        assert_eq!(h.rename_request("dir", "folder").unwrap(), 1);
        assert_eq!(h.list(Some("folder/b.req.yaml"), 10).unwrap().len(), 1);
        assert!(h.delete(&ids[4]).unwrap());
        assert_eq!(h.clear(Some("a.req.yaml")).unwrap(), 2);
        assert_eq!(h.clear(None).unwrap(), 1);
    }

    #[test]
    fn open_project_lives_outside_the_repo() {
        let data = devall_core::local::isolate_for_tests();
        let dir = tempfile::tempdir().unwrap();
        let h = History::open_project(dir.path()).unwrap();
        assert!(h.path().starts_with(&data), "{}", h.path().display());
        assert!(!dir.path().join(".devall").exists());
    }

    #[test]
    fn big_bodies_are_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::open(&dir.path().join(DB_FILE)).unwrap();
        let big = vec![b'x'; MAX_STORED_BODY + 10];
        let id = h.insert(entry("a.req.yaml", 1, 200), &big).unwrap();
        let (e, body) = h.get(&id).unwrap().unwrap();
        assert!(e.body_truncated);
        assert_eq!(body.len(), MAX_STORED_BODY);
    }

    #[test]
    fn migration_checkpoints_wal_databases() {
        // A WAL database with uncheckpointed writes, as an older version left it in `.devall/`.
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("proj").join(".devall");
        std::fs::create_dir_all(&legacy).unwrap();
        let db = legacy.join(DB_FILE);
        let writer = History::open(&db).unwrap();
        writer.insert(entry("a.req.yaml", 1, 200), b"kept").unwrap();
        // SQLite folds the WAL in when the last connection closes, so snapshot the files while
        // the connection is still open — what a crash or a still-running old version leaves.
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        assert!(wal.is_file() && std::fs::metadata(&wal).unwrap().len() > 0, "writes are in the WAL");
        let snapshot = tmp.path().join("snapshot");
        std::fs::create_dir_all(&snapshot).unwrap();
        for f in [DB_FILE.to_string(), format!("{DB_FILE}-wal")] {
            std::fs::copy(legacy.join(&f), snapshot.join(&f)).unwrap();
        }
        drop(writer);
        std::fs::remove_dir_all(&legacy).unwrap();
        std::fs::rename(&snapshot, &legacy).unwrap();
        assert!(legacy.join(format!("{DB_FILE}-wal")).is_file());

        let dest = tmp.path().join("data");
        let r = devall_core::local::migrate_legacy(&legacy, &dest);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.moved, vec![DB_FILE]);
        assert!(r.removed_legacy_dir, "the WAL was checkpointed and removed");
        assert!(!dest.join(format!("{DB_FILE}-wal")).exists() || std::fs::metadata(dest.join(format!("{DB_FILE}-wal"))).unwrap().len() == 0);
        let moved = History::open(&dest.join(DB_FILE)).unwrap();
        let list = moved.list(None, 10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(moved.get(&list[0].id).unwrap().unwrap().1, b"kept");
    }
}
