//! Local file history, like an editor's "Local History": versions of the project's own files
//! (requests, environments, notes, mocks, …) in `<project data>/file-history.sqlite`, outside the
//! repository.
//!
//! Contents are stored once per SHA-256 (deflate-compressed when that helps). A version is
//! recorded when the app opens the project (a scan: files never seen get an "initial" version,
//! files that differ from their latest version a "changed outside" one, vanished files a deletion
//! marker) and on every file-watcher change while the app runs. Writes this process made through
//! `devall_core::fsutil::atomic_write` are labelled "saved in /dev/all" (see
//! [`devall_core::local::recent_write`]); agent edits and restores carry their own label.
//!
//! Retention: at most `file_max_versions` per file, `file_max_days` old (the latest version of
//! each file is always kept) and a total size cap — see [`Limits`].

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use devall_core::fsutil::{normalize_rel, safe_join};
use devall_core::local::{HistorySettings, WriteSource};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{HistoryError, Result, like_pattern};

pub const DB_FILE: &str = "file-history.sqlite";
/// Larger files are not tracked.
pub const MAX_FILE_SIZE: u64 = 1024 * 1024;
/// Mock response bodies (`mocks/__files/`) are tracked only up to this size.
pub const MAX_MOCK_BODY: u64 = 64 * 1024;
/// Saves by the app within this window replace the previous app-saved version (autosave while
/// typing would otherwise fill the history with keystrokes).
pub const MERGE_WINDOW_MS: i64 = 10_000;
/// Top-level folders whose files are tracked, besides `devall.yaml`.
pub const TRACKED_DIRS: &[&str] = &["http", "environments", "notes", "mocks", "load", "data", "webhooks", "queries", "connections", "utilities"];

const MANIFEST: &str = devall_core::project::MANIFEST;
const COMPRESS_FROM: usize = 256;
const DAY_MS: i64 = 86_400_000;

/// Whether `rel` (project-relative, `/`-separated) is a file local history keeps versions of.
/// Never: hidden files and folders (`.env*`, `.git/`, `.devall/`), temp files, load-run summaries,
/// notes other than Markdown/notebooks.
pub fn is_tracked(rel: &str) -> bool {
    let rel = normalize_rel(rel);
    if rel == MANIFEST {
        return true;
    }
    let Some((top, rest)) = rel.split_once('/') else { return false };
    let name = devall_core::fsutil::rel_name(&rel);
    if rest.is_empty() || rel.split('/').any(|p| p.starts_with('.')) || name.ends_with(".tmp") || name.ends_with('~') {
        return false;
    }
    match top {
        "notes" => name.ends_with(".md") || name.ends_with(".ipynb"),
        "load" => name.ends_with(".load.yaml") && !rest.starts_with("runs/"),
        _ => TRACKED_DIRS.contains(&top),
    }
}

fn size_limit(rel: &str) -> u64 {
    if rel.starts_with("mocks/__files/") { MAX_MOCK_BODY } else { MAX_FILE_SIZE }
}

/// Reads a tracked file when it is small enough and text (no NUL bytes, valid UTF-8).
fn read_text(abs: &Path, rel: &str) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(abs).ok()?;
    if !meta.is_file() || meta.len() > size_limit(rel) {
        return None;
    }
    let data = std::fs::read(abs).ok()?;
    if data.contains(&0) || std::str::from_utf8(&data).is_err() {
        return None;
    }
    Some(data)
}

/// Who made a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VersionSource {
    /// First seen when the project was opened.
    Initial,
    /// Changed outside the app (editor, git, CLI…), or deleted.
    External,
    /// Saved in /dev/all.
    App,
    /// Edited by a coding agent (MCP).
    Agent,
    /// A version restored from local history.
    Restore,
}

impl VersionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::External => "external",
            Self::App => "app",
            Self::Agent => "agent",
            Self::Restore => "restore",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "initial" => Self::Initial,
            "app" => Self::App,
            "agent" => Self::Agent,
            "restore" => Self::Restore,
            _ => Self::External,
        }
    }
}

impl From<WriteSource> for VersionSource {
    fn from(w: WriteSource) -> Self {
        match w {
            WriteSource::App => Self::App,
            WriteSource::Agent => Self::Agent,
            WriteSource::Restore => Self::Restore,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    pub id: i64,
    /// Project-relative, `/`-separated.
    pub path: String,
    /// Unix milliseconds.
    pub at: i64,
    pub size: u64,
    pub source: VersionSource,
    /// The file was deleted (no content).
    pub deleted: bool,
}

/// One tracked file with its history, for the "all files" list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSummary {
    pub path: String,
    pub versions: usize,
    pub last_at: i64,
    pub last_source: VersionSource,
    /// The latest version is a deletion.
    pub deleted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub recorded: usize,
    pub deleted: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryStats {
    pub files: usize,
    pub versions: usize,
    /// Stored (compressed) content bytes.
    pub content_bytes: u64,
    /// Database size on disk.
    pub disk_bytes: u64,
}

/// Retention, from [`HistorySettings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub max_versions: usize,
    pub max_age_ms: i64,
    pub max_bytes: u64,
}

impl From<&HistorySettings> for Limits {
    fn from(s: &HistorySettings) -> Self {
        Self { max_versions: s.file_max_versions.max(1), max_age_ms: i64::from(s.file_max_days) * DAY_MS, max_bytes: s.file_max_mb * 1024 * 1024 }
    }
}

pub struct FileHistory {
    conn: Mutex<Connection>,
    root: PathBuf,
    path: PathBuf,
    max_versions: AtomicUsize,
}

fn registry() -> &'static Mutex<HashMap<String, Arc<FileHistory>>> {
    static REG: OnceLock<Mutex<HashMap<String, Arc<FileHistory>>>> = OnceLock::new();
    REG.get_or_init(Default::default)
}

fn hash_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn compress(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < COMPRESS_FROM {
        return None;
    }
    let mut enc = flate2::write::DeflateEncoder::new(Vec::with_capacity(data.len() / 3), flate2::Compression::default());
    enc.write_all(data).ok()?;
    let out = enc.finish().ok()?;
    (out.len() < data.len()).then_some(out)
}

fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 3);
    flate2::read::DeflateDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

struct Latest {
    id: i64,
    at: i64,
    hash: Option<String>,
    source: VersionSource,
}

fn latest(conn: &Connection, rel: &str) -> Result<Option<Latest>> {
    conn.query_row("SELECT id, at, hash, source FROM versions WHERE path = ?1 ORDER BY id DESC LIMIT 1", params![rel], |r| {
        Ok(Latest { id: r.get(0)?, at: r.get(1)?, hash: r.get(2)?, source: VersionSource::parse(&r.get::<_, String>(3)?) })
    })
    .optional()
    .map_err(Into::into)
}

fn put_blob(conn: &Connection, hash: &str, data: &[u8]) -> Result<()> {
    let exists: Option<i64> = conn.query_row("SELECT 1 FROM blobs WHERE hash = ?1", params![hash], |r| r.get(0)).optional()?;
    if exists.is_none() {
        let (stored, compressed) = match compress(data) {
            Some(c) => (c, true),
            None => (data.to_vec(), false),
        };
        conn.execute("INSERT INTO blobs (hash, size, compressed, data) VALUES (?1, ?2, ?3, ?4)", params![hash, data.len() as i64, compressed, stored])?;
    }
    Ok(())
}

/// Records `content` (or a deletion when `None`) for `rel` unless it equals the latest version.
fn record_in(conn: &Connection, rel: &str, content: Option<&[u8]>, source: VersionSource, at: i64, max_versions: usize) -> Result<Option<i64>> {
    let prev = latest(conn, rel)?;
    let hash = content.map(hash_hex);
    match &prev {
        None if hash.is_none() => return Ok(None),
        Some(p) if p.hash == hash => return Ok(None),
        _ => {}
    }
    if let (Some(data), Some(h)) = (content, &hash) {
        put_blob(conn, h, data)?;
    }
    let size = content.map(|c| c.len() as i64).unwrap_or(0);
    // Consecutive app saves (autosave) collapse into one version.
    if let Some(p) = &prev
        && source == VersionSource::App
        && p.source == VersionSource::App
        && p.hash.is_some()
        && hash.is_some()
        && at - p.at < MERGE_WINDOW_MS
        && at >= p.at
    {
        conn.execute("UPDATE versions SET at = ?2, hash = ?3, size = ?4 WHERE id = ?1", params![p.id, at, hash, size])?;
        return Ok(Some(p.id));
    }
    conn.execute("INSERT INTO versions (path, at, hash, size, source) VALUES (?1, ?2, ?3, ?4, ?5)", params![rel, at, hash, size, source.as_str()])?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "DELETE FROM versions WHERE path = ?1 AND id NOT IN (SELECT id FROM versions WHERE path = ?1 ORDER BY id DESC LIMIT ?2)",
        params![rel, max_versions.max(1) as i64],
    )?;
    Ok(Some(id))
}

fn version_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FileVersion> {
    let hash: Option<String> = r.get(4)?;
    Ok(FileVersion {
        id: r.get(0)?,
        path: r.get(1)?,
        at: r.get(2)?,
        size: r.get::<_, i64>(3)? as u64,
        deleted: hash.is_none(),
        source: VersionSource::parse(&r.get::<_, String>(5)?),
    })
}

const VERSION_COLUMNS: &str = "id, path, at, size, hash, source";

impl FileHistory {
    /// Opens (creating if needed) `<project data>/file-history.sqlite` for the project at `root`.
    pub fn open_project(root: &Path) -> Result<Self> {
        let dir = devall_core::local::project_dir(root);
        std::fs::create_dir_all(&dir)?;
        Self::open(&dir.join(DB_FILE), root)
    }

    /// The project's shared instance (the watcher, the scan and the UI commands must use one
    /// connection so deduplication sees every write). Limits come from the history settings.
    pub fn shared(root: &Path) -> Result<Arc<Self>> {
        let key = devall_core::local::root_key(root);
        let mut reg = registry().lock().unwrap();
        if let Some(h) = reg.get(&key) {
            return Ok(h.clone());
        }
        let h = Arc::new(Self::open_project(root)?);
        h.set_max_versions(devall_core::local::load_settings().file_max_versions);
        reg.insert(key, h.clone());
        Ok(h)
    }

    /// Applies new limits to every open project (after the settings changed).
    pub fn set_max_versions_all(max_versions: usize) {
        for h in registry().lock().unwrap().values() {
            h.set_max_versions(max_versions);
        }
    }

    pub fn open(db: &Path, root: &Path) -> Result<Self> {
        let conn = Connection::open(db)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // Only takes effect on a new database (before the first table): lets prune give space back.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blobs (
                hash TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                compressed INTEGER NOT NULL,
                data BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL,
                at INTEGER NOT NULL,
                hash TEXT,
                size INTEGER NOT NULL,
                source TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS versions_by_path ON versions(path, id DESC);
            CREATE INDEX IF NOT EXISTS versions_by_time ON versions(at DESC, id DESC);",
        )?;
        Ok(Self { conn: Mutex::new(conn), root: root.to_path_buf(), path: db.to_path_buf(), max_versions: AtomicUsize::new(HistorySettings::default().file_max_versions) })
    }

    pub fn set_max_versions(&self, n: usize) {
        self.max_versions.store(n.max(1), Ordering::Relaxed);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn max_versions(&self) -> usize {
        self.max_versions.load(Ordering::Relaxed)
    }

    /// Records `content` for `rel` (a deletion when `None`) unless it equals the latest version.
    /// Returns the version id when something was recorded.
    pub fn record(&self, rel: &str, content: Option<&[u8]>, source: VersionSource) -> Result<Option<i64>> {
        self.record_at(rel, content, source, devall_core::local::now_ms())
    }

    pub fn record_at(&self, rel: &str, content: Option<&[u8]>, source: VersionSource, at: i64) -> Result<Option<i64>> {
        let rel = normalize_rel(rel);
        let conn = self.conn.lock().unwrap();
        record_in(&conn, &rel, content, source, at, self.max_versions())
    }

    /// Records what is on disk now at `rel` after a file-watcher event: the file's content
    /// (attributed to this process when it wrote exactly that), every tracked file of a folder
    /// that appeared, or deletion markers for a file or folder that vanished. Returns how many
    /// versions were recorded.
    pub fn record_change(&self, rel: &str) -> Result<usize> {
        let rel = normalize_rel(rel);
        if rel.is_empty() || rel.split('/').any(|p| p.starts_with('.')) {
            return Ok(0);
        }
        let abs = safe_join(&self.root, &rel)?;
        let now = devall_core::local::now_ms();
        let conn = self.conn.lock().unwrap();
        let max = self.max_versions();
        if abs.is_dir() {
            // Only folders that can hold tracked files are walked (not `src/`, `node_modules/`…).
            if !TRACKED_DIRS.contains(&rel.split('/').next().unwrap_or_default()) {
                return Ok(0);
            }
            let mut files = Vec::new();
            collect(&abs, &rel, &mut files);
            let tx = conn.unchecked_transaction()?;
            let mut n = 0;
            for f in files {
                if let Some(data) = read_text(&safe_join(&self.root, &f)?, &f) {
                    let source = devall_core::local::recent_write(&safe_join(&self.root, &f)?, &data).map(VersionSource::from).unwrap_or(VersionSource::External);
                    n += usize::from(record_in(&tx, &f, Some(&data), source, now, max)?.is_some());
                }
            }
            tx.commit()?;
            return Ok(n);
        }
        if abs.is_file() {
            if !is_tracked(&rel) {
                return Ok(0);
            }
            let Some(data) = read_text(&abs, &rel) else { return Ok(0) };
            let source = devall_core::local::recent_write(&abs, &data).map(VersionSource::from).unwrap_or(VersionSource::External);
            return Ok(usize::from(record_in(&conn, &rel, Some(&data), source, now, max)?.is_some()));
        }
        // Gone: the file itself, or a folder (deleted or renamed away) with tracked files in it.
        let prefix = format!("{rel}/");
        let gone: Vec<String> = live_paths(&conn)?.into_iter().filter(|p| *p == rel || p.starts_with(&prefix)).collect();
        let tx = conn.unchecked_transaction()?;
        let mut n = 0;
        for p in gone {
            if !self.root.join(&p).exists() {
                n += usize::from(record_in(&tx, &p, None, VersionSource::External, now, max)?.is_some());
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// [`Self::record_change`] for each path of a watcher batch; errors are skipped.
    pub fn record_changes(&self, rels: &[String]) -> usize {
        rels.iter().map(|r| self.record_change(r).unwrap_or(0)).sum()
    }

    /// Brings the history up to date with the files on disk (when the project is opened): new
    /// files get an "initial" version, changed ones a "changed outside" version, vanished ones a
    /// deletion marker.
    pub fn scan(&self) -> Result<ScanReport> {
        let mut files = Vec::new();
        if self.root.join(MANIFEST).is_file() {
            files.push(MANIFEST.to_string());
        }
        for d in TRACKED_DIRS {
            collect(&self.root.join(d), d, &mut files);
        }
        let now = devall_core::local::now_ms();
        let conn = self.conn.lock().unwrap();
        let max = self.max_versions();
        let known: HashSet<String> = {
            let mut stmt = conn.prepare("SELECT DISTINCT path FROM versions")?;
            stmt.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?
        };
        let live = live_paths(&conn)?;
        let tx = conn.unchecked_transaction()?;
        let mut report = ScanReport::default();
        let mut seen = HashSet::new();
        for rel in files {
            let Some(data) = read_text(&self.root.join(&rel), &rel) else { continue };
            let source = if known.contains(&rel) { VersionSource::External } else { VersionSource::Initial };
            if record_in(&tx, &rel, Some(&data), source, now, max)?.is_some() {
                report.recorded += 1;
            }
            seen.insert(rel);
        }
        for rel in live {
            if !seen.contains(&rel) && !self.root.join(&rel).exists() && record_in(&tx, &rel, None, VersionSource::External, now, max)?.is_some() {
                report.deleted += 1;
            }
        }
        tx.commit()?;
        Ok(report)
    }

    /// Versions of one file, newest first.
    pub fn versions(&self, rel: &str, limit: usize) -> Result<Vec<FileVersion>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT {VERSION_COLUMNS} FROM versions WHERE path = ?1 ORDER BY id DESC LIMIT ?2"))?;
        let rows = stmt.query_map(params![normalize_rel(rel), limit as i64], version_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Versions of every file, newest first, optionally only paths containing `query`.
    pub fn recent(&self, query: Option<&str>, offset: usize, limit: usize) -> Result<Vec<FileVersion>> {
        let conn = self.conn.lock().unwrap();
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(like_pattern);
        let mut stmt =
            conn.prepare(&format!("SELECT {VERSION_COLUMNS} FROM versions WHERE (?1 IS NULL OR path LIKE ?1 ESCAPE '!') ORDER BY at DESC, id DESC LIMIT ?2 OFFSET ?3"))?;
        let rows = stmt.query_map(params![pattern, limit as i64, offset as i64], version_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every file with history, most recently changed first.
    pub fn files(&self, query: Option<&str>) -> Result<Vec<FileSummary>> {
        let conn = self.conn.lock().unwrap();
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(like_pattern);
        let mut stmt = conn.prepare(
            "SELECT v.path, (SELECT COUNT(*) FROM versions c WHERE c.path = v.path), v.at, v.source, v.hash IS NULL
             FROM versions v WHERE v.id = (SELECT MAX(id) FROM versions m WHERE m.path = v.path)
               AND (?1 IS NULL OR v.path LIKE ?1 ESCAPE '!')
             ORDER BY v.at DESC, v.id DESC",
        )?;
        let rows = stmt.query_map(params![pattern], |r| {
            Ok(FileSummary {
                path: r.get(0)?,
                versions: r.get::<_, i64>(1)? as usize,
                last_at: r.get(2)?,
                last_source: VersionSource::parse(&r.get::<_, String>(3)?),
                deleted: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn version(&self, id: i64) -> Result<Option<FileVersion>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(&format!("SELECT {VERSION_COLUMNS} FROM versions WHERE id = ?1"), params![id], version_row).optional().map_err(Into::into)
    }

    /// The content of a version (`None` for a deletion marker or an unknown id).
    pub fn content(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(Vec<u8>, bool)> = conn
            .query_row("SELECT b.data, b.compressed FROM versions v JOIN blobs b ON b.hash = v.hash WHERE v.id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match row {
            Some((data, true)) => Ok(Some(decompress(&data)?)),
            Some((data, false)) => Ok(Some(data)),
            None => Ok(None),
        }
    }

    /// Writes a version back to its file (through `atomic_write`, labelled as a restore) and
    /// records it as the newest version. Returns the file's project-relative path.
    pub fn restore(&self, id: i64) -> Result<String> {
        let v = self.version(id)?.ok_or_else(|| HistoryError::Invalid(format!("version {id} not found")))?;
        let data = self.content(id)?.ok_or_else(|| HistoryError::Invalid("that version is a deletion — there is nothing to restore".into()))?;
        let abs = safe_join(&self.root, &v.path)?;
        devall_core::local::mark_write(&abs, WriteSource::Restore);
        devall_core::fsutil::atomic_write(&abs, &data)?;
        devall_core::local::mark_write(&abs, WriteSource::Restore);
        self.record(&v.path, Some(&data), VersionSource::Restore)?;
        Ok(v.path)
    }

    pub fn delete_version(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM versions WHERE id = ?1", params![id])?;
        gc_blobs(&conn)?;
        Ok(n > 0)
    }

    /// Forgets one file's history, or every file's when `rel` is `None`.
    pub fn clear(&self, rel: Option<&str>) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM versions WHERE (?1 IS NULL OR path = ?1)", params![rel.map(normalize_rel)])?;
        gc_blobs(&conn)?;
        conn.execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(n)
    }

    /// Applies retention (at startup): versions older than `max_age_ms` (keeping each file's
    /// latest), more than `max_versions` per file, deleted files whose deletion is older than
    /// `max_age_ms`, then the oldest versions until the stored content fits `max_bytes`.
    /// Returns how many versions were removed.
    pub fn prune(&self, limits: &Limits) -> Result<usize> {
        self.prune_at(limits, devall_core::local::now_ms())
    }

    pub fn prune_at(&self, limits: &Limits, now: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = now - limits.max_age_ms;
        let tx = conn.unchecked_transaction()?;
        let mut n = tx.execute(
            "DELETE FROM versions WHERE path IN (
                SELECT v.path FROM versions v WHERE v.id = (SELECT MAX(id) FROM versions m WHERE m.path = v.path) AND v.hash IS NULL AND v.at < ?1)",
            params![cutoff],
        )?;
        n += tx.execute("DELETE FROM versions WHERE at < ?1 AND id NOT IN (SELECT MAX(id) FROM versions GROUP BY path)", params![cutoff])?;
        n += tx.execute(
            "DELETE FROM versions WHERE id IN (
                SELECT id FROM (SELECT id, ROW_NUMBER() OVER (PARTITION BY path ORDER BY id DESC) AS rn FROM versions) WHERE rn > ?1)",
            params![limits.max_versions.max(1) as i64],
        )?;
        gc_blobs(&tx)?;
        loop {
            let used: i64 = tx.query_row("SELECT COALESCE(SUM(length(data)), 0) FROM blobs", [], |r| r.get(0))?;
            if (used as u64) <= limits.max_bytes {
                break;
            }
            let removed = tx.execute(
                "DELETE FROM versions WHERE id IN (
                    SELECT id FROM versions WHERE id NOT IN (SELECT MAX(id) FROM versions GROUP BY path) ORDER BY at ASC, id ASC LIMIT 100)",
                [],
            )?;
            if removed == 0 {
                break;
            }
            n += removed;
            gc_blobs(&tx)?;
        }
        tx.commit()?;
        conn.execute_batch("PRAGMA incremental_vacuum;")?;
        Ok(n)
    }

    pub fn stats(&self) -> Result<FileHistoryStats> {
        let conn = self.conn.lock().unwrap();
        let (files, versions): (i64, i64) = conn.query_row("SELECT COUNT(DISTINCT path), COUNT(*) FROM versions", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let content: i64 = conn.query_row("SELECT COALESCE(SUM(length(data)), 0) FROM blobs", [], |r| r.get(0))?;
        Ok(FileHistoryStats { files: files as usize, versions: versions as usize, content_bytes: content as u64, disk_bytes: crate::db_size(&self.path) })
    }
}

fn gc_blobs(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM blobs WHERE hash NOT IN (SELECT hash FROM versions WHERE hash IS NOT NULL)", [])?)
}

/// Paths whose latest version is not a deletion.
fn live_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT v.path FROM versions v WHERE v.id = (SELECT MAX(id) FROM versions m WHERE m.path = v.path) AND v.hash IS NOT NULL")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Tracked files under `dir` (whose project-relative path is `rel`), skipping hidden folders.
fn collect(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let child = devall_core::fsutil::rel_join(rel, &name);
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            collect(&entry.path(), &child, out);
        } else if ft.is_file() && is_tracked(&child) {
            out.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, FileHistory) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MANIFEST), "name: P\n").unwrap();
        let h = FileHistory::open(&dir.path().join("fh.sqlite"), dir.path()).unwrap();
        (dir, h)
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn tracking_rules() {
        for yes in ["devall.yaml", "http/a.req.yaml", "http/x/_folder.yaml", "environments/dev.env.yaml", "notes/a/b.md", "notes/n.ipynb", "mocks/mappings/a.json", "mocks/servers.yaml", "load/smoke.load.yaml", "load/api/c.load.yaml", "data/users.data.yaml", "webhooks/s.webhook.yaml"] {
            assert!(is_tracked(yes), "{yes}");
        }
        for no in [".env", "environments/.env.local", ".git/HEAD", ".devall/history.sqlite", "notes/img.png", "load/runs/smoke/1.json", "load/notes.txt", "src/main.rs", "README.md", "http/.a.req.yaml.1234.tmp", "http/", "target/x"] {
            assert!(!is_tracked(no), "{no}");
        }
    }

    #[test]
    fn record_dedupes_and_stores_content_once() {
        let (_d, h) = setup();
        let big = "x".repeat(10_000);
        let a = h.record_at("http/a.req.yaml", Some(big.as_bytes()), VersionSource::Initial, 1_000).unwrap();
        assert!(a.is_some());
        assert_eq!(h.record_at("http/a.req.yaml", Some(big.as_bytes()), VersionSource::External, 2_000).unwrap(), None, "same content");
        h.record_at("http/b.req.yaml", Some(big.as_bytes()), VersionSource::Initial, 3_000).unwrap();
        let s = h.stats().unwrap();
        assert_eq!((s.files, s.versions), (2, 2));
        assert!(s.content_bytes < 1_000, "one compressed blob for both files: {}", s.content_bytes);
        assert_eq!(h.content(a.unwrap()).unwrap().unwrap(), big.as_bytes());
        // Deletion markers: once, and only for files that have history.
        assert!(h.record_at("http/a.req.yaml", None, VersionSource::External, 4_000).unwrap().is_some());
        assert_eq!(h.record_at("http/a.req.yaml", None, VersionSource::External, 5_000).unwrap(), None);
        assert_eq!(h.record_at("http/never.req.yaml", None, VersionSource::External, 5_000).unwrap(), None);
        let v = h.versions("http/a.req.yaml", 10).unwrap();
        assert!(v[0].deleted && !v[1].deleted);
        assert_eq!(h.content(v[0].id).unwrap(), None);
    }

    #[test]
    fn app_saves_merge_within_the_window() {
        let (_d, h) = setup();
        h.record_at("notes/a.md", Some(b"v0"), VersionSource::Initial, 0).unwrap();
        let first = h.record_at("notes/a.md", Some(b"v1"), VersionSource::App, 100_000).unwrap();
        let merged = h.record_at("notes/a.md", Some(b"v12"), VersionSource::App, 105_000).unwrap();
        assert_eq!(first, merged);
        h.record_at("notes/a.md", Some(b"v123"), VersionSource::App, 200_000).unwrap();
        let v = h.versions("notes/a.md", 10).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(h.content(v[1].id).unwrap().unwrap(), b"v12");
        assert_eq!(v[2].source, VersionSource::Initial);
    }

    #[test]
    fn scan_records_new_changed_and_deleted_files() {
        let (d, h) = setup();
        write(d.path(), "http/a.req.yaml", "name: A\n");
        write(d.path(), "environments/dev.env.yaml", "name: Dev\n");
        write(d.path(), ".env", "SECRET=1\n");
        write(d.path(), "notes/pic.png", "binary?");
        write(d.path(), "mocks/__files/big.json", &"x".repeat((MAX_MOCK_BODY + 1) as usize));
        let r = h.scan().unwrap();
        assert_eq!(r, ScanReport { recorded: 3, deleted: 0 });
        assert_eq!(h.versions("http/a.req.yaml", 5).unwrap()[0].source, VersionSource::Initial);
        assert_eq!(h.scan().unwrap(), ScanReport::default(), "nothing changed");

        write(d.path(), "http/a.req.yaml", "name: A2\n");
        std::fs::remove_file(d.path().join("environments/dev.env.yaml")).unwrap();
        let r = h.scan().unwrap();
        assert_eq!(r, ScanReport { recorded: 1, deleted: 1 });
        assert_eq!(h.versions("http/a.req.yaml", 5).unwrap()[0].source, VersionSource::External);
        assert!(h.versions("environments/dev.env.yaml", 5).unwrap()[0].deleted);
        let files = h.files(None).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(h.files(Some("env")).unwrap().len(), 1);
        assert_eq!(h.recent(None, 0, 100).unwrap().len(), 5);
        assert_eq!(h.recent(Some("a.req"), 0, 100).unwrap().len(), 2);
    }

    #[test]
    fn watcher_changes_are_attributed() {
        let (d, h) = setup();
        let rel = "http/a.req.yaml";
        devall_core::fsutil::atomic_write(&d.path().join(rel), b"name: Saved\n").unwrap();
        assert_eq!(h.record_change(rel).unwrap(), 1);
        assert_eq!(h.versions(rel, 1).unwrap()[0].source, VersionSource::App);
        std::fs::write(d.path().join(rel), "name: Edited elsewhere\n").unwrap();
        assert_eq!(h.record_change(rel).unwrap(), 1);
        assert_eq!(h.versions(rel, 1).unwrap()[0].source, VersionSource::External);
        // A folder renamed: the old paths get deletion markers, the new folder's files versions.
        std::fs::create_dir_all(d.path().join("http/moved")).unwrap();
        std::fs::rename(d.path().join(rel), d.path().join("http/moved/a.req.yaml")).unwrap();
        assert_eq!(h.record_change("http/a.req.yaml").unwrap(), 1);
        assert_eq!(h.record_change("http/moved").unwrap(), 1);
        assert!(h.versions(rel, 1).unwrap()[0].deleted);
        assert_eq!(h.record_change(".git/index").unwrap(), 0);
        assert_eq!(h.record_change("src/lib.rs").unwrap(), 0);
    }

    #[test]
    fn restore_writes_the_file_and_records_a_version() {
        let (d, h) = setup();
        let rel = "environments/dev.env.yaml";
        write(d.path(), rel, "name: Dev\nvariables:\n  a: '1'\n");
        h.scan().unwrap();
        let old = h.versions(rel, 1).unwrap()[0].id;
        write(d.path(), rel, "name: Dev\nvariables:\n  a: '2'\n");
        h.record_change(rel).unwrap();
        assert_eq!(h.restore(old).unwrap(), rel);
        assert_eq!(std::fs::read_to_string(d.path().join(rel)).unwrap(), "name: Dev\nvariables:\n  a: '1'\n");
        let v = h.versions(rel, 10).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].source, VersionSource::Restore);
        // The watcher event for the restore's own write is a no-op.
        assert_eq!(h.record_change(rel).unwrap(), 0);
        // Deletions can't be restored.
        std::fs::remove_file(d.path().join(rel)).unwrap();
        h.record_change(rel).unwrap();
        assert!(h.restore(h.versions(rel, 1).unwrap()[0].id).is_err());
    }

    #[test]
    fn prune_applies_count_age_and_size_limits() {
        let (_d, h) = setup();
        h.set_max_versions(1000);
        for i in 0..10 {
            h.record_at("http/a.req.yaml", Some(format!("v{i}").as_bytes()), VersionSource::External, i * DAY_MS).unwrap();
        }
        let limits = Limits { max_versions: 5, max_age_ms: 1000 * DAY_MS, max_bytes: u64::MAX };
        assert_eq!(h.prune_at(&limits, 10 * DAY_MS).unwrap(), 5);
        assert_eq!(h.versions("http/a.req.yaml", 100).unwrap().len(), 5);
        // Age: everything older than 2 days goes, except the latest version.
        let limits = Limits { max_versions: 50, max_age_ms: 2 * DAY_MS, max_bytes: u64::MAX };
        h.prune_at(&limits, 20 * DAY_MS).unwrap();
        let left = h.versions("http/a.req.yaml", 100).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(h.content(left[0].id).unwrap().unwrap(), b"v9");
        // An old deletion drops the file's history entirely.
        h.record_at("http/gone.req.yaml", Some(b"x"), VersionSource::Initial, 0).unwrap();
        h.record_at("http/gone.req.yaml", None, VersionSource::External, DAY_MS).unwrap();
        h.prune_at(&limits, 20 * DAY_MS).unwrap();
        assert!(h.versions("http/gone.req.yaml", 10).unwrap().is_empty());
        // Size: oldest versions go until the content fits.
        for i in 0..20 {
            let text: String = (0..2000).map(|j| char::from(b'a' + ((i * 7 + j * 13) % 26) as u8)).collect();
            h.record_at("notes/n.md", Some(text.as_bytes()), VersionSource::External, 30 * DAY_MS + i).unwrap();
        }
        let before = h.stats().unwrap().content_bytes;
        let limits = Limits { max_versions: 50, max_age_ms: 1000 * DAY_MS, max_bytes: before / 2 };
        assert!(h.prune_at(&limits, 30 * DAY_MS + 100).unwrap() > 0);
        let after = h.stats().unwrap();
        assert!(after.content_bytes <= before / 2, "{} > {}", after.content_bytes, before / 2);
        assert!(!h.versions("notes/n.md", 100).unwrap().is_empty());
        assert_eq!(h.versions("http/a.req.yaml", 100).unwrap().len(), 1, "latest versions survive the size cap");
    }

    #[test]
    fn clear_and_delete() {
        let (_d, h) = setup();
        let id = h.record_at("http/a.req.yaml", Some(b"a"), VersionSource::Initial, 1).unwrap().unwrap();
        h.record_at("http/b.req.yaml", Some(b"b"), VersionSource::Initial, 1).unwrap();
        assert!(h.delete_version(id).unwrap());
        assert_eq!(h.clear(Some("http/b.req.yaml")).unwrap(), 1);
        assert_eq!(h.stats().unwrap(), FileHistoryStats { disk_bytes: h.stats().unwrap().disk_bytes, ..Default::default() });
    }
}
