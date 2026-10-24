//! On-device state that never lands in the repository.
//!
//! Every project gets a data folder in the app data directory —
//! `<app data>/projects/<folder slug>-<hash of the canonical root>/` — for its response history,
//! event timeline, webhook events, local file history, semantic index, proxy captures and load-run
//! summaries. The desktop app and the `devall` CLI resolve the same folder, so they share history.
//! Older versions kept this in `<project>/.devall/`; [`project_dir`] moves it there once.
//!
//! Also here: the history settings (`<app data>/history/settings.json`, read by the app and the
//! CLI) and a process-wide registry of files this process just wrote, so the file watcher can tell
//! "saved in /dev/all" from "changed outside".

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{IoContext, Result};
use crate::project::LOCAL_DIR;

/// Folder name of the desktop app's data directory (its Tauri identifier).
pub const APP_DIR_NAME: &str = "dev.devall.app";
pub const PROJECTS_DIR: &str = "projects";
/// `{ root, name, lastOpened }` in each project data folder, so the folders can be told apart.
pub const PROJECT_FILE: &str = "project.json";
/// Device-global history (utilities) and the history settings.
pub const HISTORY_DIR: &str = "history";
pub const SETTINGS_FILE: &str = "settings.json";

/// SQLite databases older versions kept in `<project>/.devall/`.
pub const LEGACY_DBS: &[&str] = &["history.sqlite", "events.sqlite", "webhooks.sqlite"];
/// Folders older versions kept in `<project>/.devall/`.
pub const LEGACY_DIRS: &[&str] = &["embeddings", "relay-inbox", "captures"];

static APP_DATA_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// The desktop app's data directory (`dev.devall.app` under the platform data dir):
/// `%APPDATA%` on Windows, `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or
/// `~/.local/share` elsewhere — what Tauri's `app_data_dir()` returns.
pub fn default_app_data_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
    }?;
    Some(base.join(APP_DIR_NAME))
}

/// Uses `dir` as the app data directory for this process (the desktop app passes Tauri's).
pub fn set_app_data_dir(dir: impl Into<PathBuf>) {
    *APP_DATA_OVERRIDE.write().unwrap() = Some(dir.into());
}

/// The app data directory: the one set with [`set_app_data_dir`], else [`default_app_data_dir`] —
/// except in test binaries, which always get a throwaway one (see [`is_test_binary`]).
pub fn app_data_dir() -> Option<PathBuf> {
    if let Some(dir) = APP_DATA_OVERRIDE.read().unwrap().clone() {
        return Some(dir);
    }
    if is_test_binary() {
        return Some(isolate_for_tests());
    }
    default_app_data_dir()
}

/// Cargo runs unit and integration tests from `target/<profile>/deps/`; `cargo run` and installed
/// binaries never live there. A safety net so no crate's tests write into the user's real app data
/// (history, embeddings, `project.json` of temp projects), whether or not they isolate themselves.
fn is_test_binary() -> bool {
    static IS_TEST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IS_TEST.get_or_init(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.ends_with("deps"))).unwrap_or(false))
}

/// Points this process at a throwaway app data directory (unit tests of crates that open
/// project stores, so they never touch the real one). Returns it.
#[doc(hidden)]
pub fn isolate_for_tests() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devall-test-data-{}", std::process::id()));
    let mut cur = APP_DATA_OVERRIDE.write().unwrap();
    if cur.as_deref() != Some(dir.as_path()) {
        *cur = Some(dir.clone());
    }
    dir
}

/// `<app data>/history` (utilities history, settings).
pub fn history_dir() -> Option<PathBuf> {
    app_data_dir().map(|d| d.join(HISTORY_DIR))
}

/// Normalizes an absolute path string: `/` separators, no Windows verbatim prefix, lower-cased on
/// Windows and macOS (case-insensitive file systems).
fn normalize_key(p: &Path) -> String {
    let mut s = p.to_string_lossy().replace('\\', "/");
    if let Some(rest) = s.strip_prefix("//?/UNC/") {
        s = format!("//{rest}");
    } else if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    while s.len() > 1 && s.ends_with('/') && !s.ends_with(":/") {
        s.pop();
    }
    if cfg!(any(windows, target_os = "macos")) { s.to_lowercase() } else { s }
}

/// The project root as one stable string: absolute, symlinks resolved when the folder exists.
pub fn root_key(root: &Path) -> String {
    let abs = fs::canonicalize(root).unwrap_or_else(|_| std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()));
    normalize_key(&abs)
}

/// `<app_data>/projects/<folder slug>-<12 hex chars of SHA-256(root key)>`. Pure apart from
/// resolving `root` (so two spellings of the same folder share one data folder).
pub fn project_data_dir(app_data: &Path, root: &Path) -> PathBuf {
    let key = root_key(root);
    let folder = key.rsplit('/').find(|s| !s.is_empty() && !s.ends_with(':')).unwrap_or("project");
    let mut slug = crate::fsutil::slugify(folder);
    slug.truncate(40);
    let slug = slug.trim_end_matches('-');
    let hash = Sha256::digest(key.as_bytes());
    let short: String = hash.iter().take(6).map(|b| format!("{b:02x}")).collect();
    app_data.join(PROJECTS_DIR).join(format!("{slug}-{short}"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub root: String,
    pub name: String,
    /// Unix milliseconds.
    #[serde(default)]
    pub last_opened: i64,
}

fn prepared() -> &'static Mutex<HashSet<PathBuf>> {
    static SET: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    SET.get_or_init(Default::default)
}

/// The project's data folder (stores create it when they first write; [`touch_project`] creates
/// it with its `project.json`). The first call for a project in this process also moves what an
/// older version left in `<project>/.devall/` (see [`migrate_legacy`]) — before any store can
/// open, since every store resolves its location through here. Falls back to `<project>/.devall`
/// when the platform has no user data directory.
pub fn project_dir(root: &Path) -> PathBuf {
    let Some(app) = app_data_dir() else { return root.join(LOCAL_DIR) };
    let dir = project_data_dir(&app, root);
    // Held while migrating so a concurrent caller can't open (and create) a store first.
    let mut done = prepared().lock().unwrap();
    if done.insert(dir.clone()) && root.join(LOCAL_DIR).is_dir() {
        let report = migrate_legacy(&root.join(LOCAL_DIR), &dir);
        for e in &report.errors {
            eprintln!("/dev/all: moving local history out of {}: {e}", root.display());
        }
        if !report.moved.is_empty() && !dir.join(PROJECT_FILE).is_file() {
            let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            write_record(&dir, root, &name, 0);
        }
    }
    dir
}

fn write_record(dir: &Path, root: &Path, name: &str, last_opened: i64) {
    let rec = ProjectRecord { root: std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()).display().to_string(), name: name.to_string(), last_opened };
    if let Ok(json) = serde_json::to_vec_pretty(&rec) {
        let _ = crate::fsutil::atomic_write(&dir.join(PROJECT_FILE), &json);
    }
}

/// Call when a project is opened (app or CLI): prepares its data folder (migrating once) and
/// records its name and the time in `project.json`. Returns the folder.
pub fn touch_project(root: &Path, name: &str) -> PathBuf {
    let dir = project_dir(root);
    if dir.ends_with(LOCAL_DIR) && dir.parent() == Some(root) {
        return dir;
    }
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("/dev/all: cannot create {}: {e}", dir.display());
    }
    write_record(&dir, root, name, now_ms());
    dir
}

/// Every project data folder with its `project.json` (unreadable records are skipped).
pub fn known_projects() -> Vec<(PathBuf, ProjectRecord)> {
    let Some(app) = app_data_dir() else { return Vec::new() };
    let Ok(rd) = fs::read_dir(app.join(PROJECTS_DIR)) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let dir = e.path();
            let rec: ProjectRecord = serde_json::from_slice(&fs::read(dir.join(PROJECT_FILE)).ok()?).ok()?;
            Some((dir, rec))
        })
        .collect()
}

/// Total size of the files under `path` (0 when it doesn't exist).
pub fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else { return 0 };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    fs::read_dir(path).map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---------- migration from <project>/.devall ----------

#[derive(Debug, Default, Clone, PartialEq)]
pub struct MigrationReport {
    /// Entries moved (`history.sqlite`, `embeddings`, …).
    pub moved: Vec<String>,
    /// Entries left in place because the destination already has them.
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
    /// `.devall/` was empty afterwards and got removed.
    pub removed_legacy_dir: bool,
}

/// Moves the databases and folders an older version kept in `legacy` (`<project>/.devall`) into
/// `dest`, skipping anything `dest` already has. SQLite databases are checkpointed first (their
/// `-wal` is folded in) when the `sqlite` feature is on; leftover `-wal`/`-shm` files move with
/// them. Works across volumes (copy + delete), never fails as a whole (errors are reported), and
/// removes `legacy` when it ends up empty. Running it again is a no-op.
pub fn migrate_legacy(legacy: &Path, dest: &Path) -> MigrationReport {
    let mut report = MigrationReport::default();
    if !legacy.is_dir() || legacy == dest {
        return report;
    }
    if let Err(e) = fs::create_dir_all(dest) {
        report.errors.push(format!("{}: {e}", dest.display()));
        return report;
    }
    for db in LEGACY_DBS {
        let src = legacy.join(db);
        if !src.is_file() {
            continue;
        }
        let to = dest.join(db);
        if to.exists() {
            report.skipped.push(db.to_string());
            continue;
        }
        if let Err(e) = checkpoint(&src) {
            report.errors.push(format!("{db}: checkpoint: {e}"));
        }
        match move_path(&src, &to) {
            Ok(()) => {
                report.moved.push(db.to_string());
                for suffix in ["-wal", "-shm"] {
                    let side = sidecar(&src, suffix);
                    let Ok(meta) = fs::metadata(&side) else { continue };
                    // An empty WAL (checkpointed) or any shared-memory index is safe to drop.
                    let res = if meta.len() == 0 || suffix == "-shm" { fs::remove_file(&side) } else { move_path(&side, &sidecar(&to, suffix)) };
                    if let Err(e) = res {
                        report.errors.push(format!("{db}{suffix}: {e}"));
                    }
                }
            }
            Err(e) => report.errors.push(format!("{db}: {e}")),
        }
    }
    for d in LEGACY_DIRS {
        let src = legacy.join(d);
        if !src.is_dir() {
            continue;
        }
        let to = dest.join(d);
        if to.exists() {
            report.skipped.push(d.to_string());
            continue;
        }
        match move_path(&src, &to) {
            Ok(()) => report.moved.push(d.to_string()),
            Err(e) => report.errors.push(format!("{d}: {e}")),
        }
    }
    if fs::read_dir(legacy).map(|mut rd| rd.next().is_none()).unwrap_or(false) && fs::remove_dir(legacy).is_ok() {
        report.removed_legacy_dir = true;
    }
    report
}

fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Folds the WAL into the database file (`PRAGMA wal_checkpoint(TRUNCATE)`) and closes it.
#[cfg(feature = "sqlite")]
fn checkpoint(db: &Path) -> std::result::Result<(), String> {
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_secs(2)).map_err(|e| e.to_string())?;
    let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    conn.close().map_err(|(_, e)| e.to_string())?;
    if busy != 0 { Err("database is in use".into()) } else { Ok(()) }
}

/// Without SQLite the `-wal` file moves along with the database (SQLite replays it on open).
#[cfg(not(feature = "sqlite"))]
fn checkpoint(_db: &Path) -> std::result::Result<(), String> {
    Ok(())
}

/// Renames `src` to `to`, falling back to copy + delete (another volume, or a rename the OS
/// refuses). A failed copy is cleaned up and leaves `src` untouched.
fn move_path(src: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(src, to).is_ok() {
        return Ok(());
    }
    if let Err(e) = copy_recursive(src, to) {
        let _ = if to.is_dir() { fs::remove_dir_all(to) } else { fs::remove_file(to) };
        return Err(e);
    }
    if src.is_dir() { fs::remove_dir_all(src) } else { fs::remove_file(src) }
}

fn copy_recursive(src: &Path, to: &Path) -> io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, to).map(|_| ())
    }
}

// ---------- history settings ----------

/// What is recorded and for how long. Stored in `<app data>/history/settings.json` so the app and
/// the CLI honour the same choices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HistorySettings {
    /// Keep response bodies in the request history (status, timing and headers are always kept).
    pub response_bodies: bool,
    /// Responses kept per request.
    pub responses_per_request: usize,
    /// Record utility runs.
    pub utilities: bool,
    /// Utility runs kept (device-wide).
    pub utilities_max: usize,
    /// Keep versions of project files (local file history).
    pub file_versions: bool,
    /// Versions kept per file.
    pub file_max_versions: usize,
    /// Versions older than this are pruned (the latest of each file is always kept).
    pub file_max_days: u32,
    /// Size cap of a project's file history, in MB.
    pub file_max_mb: u64,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            response_bodies: true,
            responses_per_request: 100,
            utilities: true,
            utilities_max: 2000,
            file_versions: true,
            file_max_versions: 50,
            file_max_days: 180,
            file_max_mb: 200,
        }
    }
}

impl HistorySettings {
    /// Clamps values a hand-edited file could set out of range.
    pub fn sanitized(mut self) -> Self {
        self.responses_per_request = self.responses_per_request.clamp(1, 100_000);
        self.utilities_max = self.utilities_max.clamp(10, 100_000);
        self.file_max_versions = self.file_max_versions.clamp(1, 10_000);
        self.file_max_days = self.file_max_days.clamp(1, 36_500);
        self.file_max_mb = self.file_max_mb.clamp(1, 100_000);
        self
    }
}

pub fn load_settings_from(dir: &Path) -> HistorySettings {
    fs::read(dir.join(SETTINGS_FILE)).ok().and_then(|b| serde_json::from_slice::<HistorySettings>(&b).ok()).unwrap_or_default().sanitized()
}

pub fn save_settings_to(dir: &Path, settings: &HistorySettings) -> Result<()> {
    fs::create_dir_all(dir).at(dir)?;
    let json = serde_json::to_vec_pretty(&settings.clone().sanitized()).map_err(|e| crate::Error::Invalid(e.to_string()))?;
    crate::fsutil::atomic_write(&dir.join(SETTINGS_FILE), &json)
}

/// The current settings (defaults when there is no file or no app data directory).
pub fn load_settings() -> HistorySettings {
    history_dir().map(|d| load_settings_from(&d)).unwrap_or_default()
}

pub fn save_settings(settings: &HistorySettings) -> Result<()> {
    let dir = history_dir().ok_or_else(|| crate::Error::Invalid("no user data directory on this system".into()))?;
    save_settings_to(&dir, settings)
}

// ---------- recent self-writes (file history attribution) ----------

/// Who wrote a project file, as far as this process knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WriteSource {
    /// Saved by the app (any write through [`crate::fsutil::atomic_write`]).
    App,
    /// Edited by a coding agent through the MCP server.
    Agent,
    /// A local-history version restored by the user.
    Restore,
}

impl WriteSource {
    pub fn as_str(self) -> &'static str {
        match self {
            WriteSource::App => "app",
            WriteSource::Agent => "agent",
            WriteSource::Restore => "restore",
        }
    }
}

struct Mark {
    at: Instant,
    source: WriteSource,
    /// Fingerprint of the content written; `None` when unknown (matches any content briefly).
    fingerprint: Option<u64>,
}

/// How long a write with a known fingerprint is attributed.
const MARK_TTL: Duration = Duration::from_secs(120);
/// How long a mark without a fingerprint (or a sticky agent/restore mark) applies.
const LOOSE_TTL: Duration = Duration::from_secs(15);
const MAX_FINGERPRINT_BYTES: usize = 8 * 1024 * 1024;

fn marks() -> &'static Mutex<HashMap<String, Mark>> {
    static MARKS: OnceLock<Mutex<HashMap<String, Mark>>> = OnceLock::new();
    MARKS.get_or_init(Default::default)
}

fn path_key(path: &Path) -> String {
    normalize_key(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// FNV-1a: cheap and stable, enough to recognise content this process wrote.
fn fingerprint(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn put_mark(path: &Path, source: WriteSource, fp: Option<u64>) {
    let now = Instant::now();
    let mut map = marks().lock().unwrap();
    map.retain(|_, m| now.duration_since(m.at) < MARK_TTL);
    let key = path_key(path);
    // An agent or restore mark set just before the write keeps its source when the write lands.
    let source = match map.get(&key) {
        Some(m) if source == WriteSource::App && m.source != WriteSource::App && now.duration_since(m.at) < LOOSE_TTL => m.source,
        _ => source,
    };
    map.insert(key, Mark { at: now, source, fingerprint: fp });
}

/// Records that `path` was just written by `source` (call it right after — or right before — the
/// write). The MCP server marks agent edits with [`WriteSource::Agent`].
pub fn mark_write(path: &Path, source: WriteSource) {
    let fp = fs::read(path).ok().filter(|b| b.len() <= MAX_FINGERPRINT_BYTES).map(|b| fingerprint(&b));
    put_mark(path, source, fp);
}

/// Called by [`crate::fsutil::atomic_write`] after each write.
pub(crate) fn note_app_write(path: &Path, data: &[u8]) {
    put_mark(path, WriteSource::App, (data.len() <= MAX_FINGERPRINT_BYTES).then(|| fingerprint(data)));
}

/// Who wrote `path` with exactly `content` recently, if it was this process.
pub fn recent_write(path: &Path, content: &[u8]) -> Option<WriteSource> {
    let map = marks().lock().unwrap();
    let m = map.get(&path_key(path))?;
    let age = m.at.elapsed();
    match m.fingerprint {
        Some(fp) if age < MARK_TTL && content.len() <= MAX_FINGERPRINT_BYTES && fp == fingerprint(content) => Some(m.source),
        None if age < LOOSE_TTL => Some(m.source),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_data_dir_is_stable_and_distinct() {
        let app = Path::new("/data/dev.devall.app");
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("My Api");
        let b = tmp.path().join("other");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let da = project_data_dir(app, &a);
        assert_eq!(da, project_data_dir(app, &a));
        assert_ne!(da, project_data_dir(app, &b));
        assert!(da.starts_with(app.join(PROJECTS_DIR)));
        let name = da.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("my-api-"), "{name}");
        assert_eq!(name.len(), "my-api-".len() + 12);
        // Another spelling of the same folder (trailing separator, `.` segment) maps to the same place.
        assert_eq!(project_data_dir(app, &a.join(".")), da);
        let mut trailing = a.as_os_str().to_owned();
        trailing.push(std::path::MAIN_SEPARATOR_STR);
        assert_eq!(project_data_dir(app, Path::new(&trailing)), da);
        #[cfg(any(windows, target_os = "macos"))]
        assert_eq!(project_data_dir(app, &tmp.path().join("MY API")), da, "case-insensitive file systems");
    }

    #[test]
    fn root_key_normalizes() {
        assert!(!root_key(Path::new(".")).contains('\\'));
        assert!(!root_key(Path::new(".")).starts_with("//?/"));
        assert!(!root_key(Path::new(".")).ends_with('/') || root_key(Path::new(".")).len() <= 3);
    }

    #[test]
    fn migration_moves_once_and_removes_empty_legacy_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("proj").join(LOCAL_DIR);
        let dest = tmp.path().join("data").join("proj-1");
        fs::create_dir_all(legacy.join("embeddings")).unwrap();
        fs::create_dir_all(legacy.join("captures")).unwrap();
        fs::write(legacy.join("history.sqlite"), b"db").unwrap();
        fs::write(legacy.join("history.sqlite-wal"), b"wal").unwrap();
        fs::write(legacy.join("history.sqlite-shm"), b"shm").unwrap();
        fs::write(legacy.join("embeddings").join("vectors.f32"), b"v").unwrap();
        fs::write(legacy.join("captures").join("a.har"), b"{}").unwrap();
        // Already present at the destination: left in place.
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("events.sqlite"), b"new").unwrap();
        fs::write(legacy.join("events.sqlite"), b"old").unwrap();

        let r = migrate_legacy(&legacy, &dest);
        // With the `sqlite` feature the fake database can't be checkpointed: reported, still moved.
        assert!(r.errors.iter().all(|e| e.contains("checkpoint")), "{:?}", r.errors);
        assert_eq!(r.moved, vec!["history.sqlite", "embeddings", "captures"]);
        assert_eq!(r.skipped, vec!["events.sqlite"]);
        assert_eq!(fs::read(dest.join("history.sqlite")).unwrap(), b"db");
        // Without the sqlite feature the WAL travels with the db (SQLite replays it on open). With
        // it, SQLite itself may discard a WAL that doesn't match the (fake) database — see
        // devall-history's `migration_checkpoints_wal_databases` for a real one.
        #[cfg(not(feature = "sqlite"))]
        assert_eq!(fs::read(dest.join("history.sqlite-wal")).unwrap(), b"wal");
        assert!(!legacy.join("history.sqlite-wal").exists());
        assert!(!dest.join("history.sqlite-shm").exists() && !legacy.join("history.sqlite-shm").exists());
        assert!(dest.join("embeddings").join("vectors.f32").is_file());
        assert_eq!(fs::read(dest.join("events.sqlite")).unwrap(), b"new");
        assert!(!r.removed_legacy_dir, "events.sqlite is still there");
        assert!(legacy.join("events.sqlite").is_file());

        // Idempotent; removes the folder once nothing is left.
        fs::remove_file(legacy.join("events.sqlite")).unwrap();
        let again = migrate_legacy(&legacy, &dest);
        assert!(again.moved.is_empty() && again.errors.is_empty());
        assert!(again.removed_legacy_dir);
        assert!(!legacy.exists());
        assert_eq!(migrate_legacy(&legacy, &dest), MigrationReport::default());
    }

    #[test]
    fn copy_fallback_moves_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("nested").join("f"), b"x").unwrap();
        let to = tmp.path().join("b");
        copy_recursive(&src, &to).unwrap();
        assert_eq!(fs::read(to.join("nested").join("f")).unwrap(), b"x");
        move_path(&src, &tmp.path().join("c")).unwrap();
        assert!(!src.exists() && tmp.path().join("c").join("nested").join("f").is_file());
    }

    #[test]
    fn settings_round_trip_and_sanitize() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_settings_from(tmp.path()), HistorySettings::default());
        let s = HistorySettings { utilities: false, file_max_versions: 0, ..Default::default() };
        save_settings_to(tmp.path(), &s).unwrap();
        let back = load_settings_from(tmp.path());
        assert!(!back.utilities);
        assert_eq!(back.file_max_versions, 1, "clamped");
        // Unknown / missing fields fall back to defaults.
        fs::write(tmp.path().join(SETTINGS_FILE), r#"{"fileMaxMb": 5, "future": true}"#).unwrap();
        let partial = load_settings_from(tmp.path());
        assert_eq!(partial.file_max_mb, 5);
        assert!(partial.response_bodies);
    }

    #[test]
    fn write_registry_attributes_matching_content() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.req.yaml");
        crate::fsutil::atomic_write(&p, b"one").unwrap();
        assert_eq!(recent_write(&p, b"one"), Some(WriteSource::App));
        assert_eq!(recent_write(&p, b"edited elsewhere"), None);
        // The agent marks after writing; the source sticks for that content.
        crate::fsutil::atomic_write(&p, b"two").unwrap();
        mark_write(&p, WriteSource::Agent);
        assert_eq!(recent_write(&p, b"two"), Some(WriteSource::Agent));
        // Marked before the write: the write keeps the agent source.
        mark_write(&p, WriteSource::Restore);
        crate::fsutil::atomic_write(&p, b"three").unwrap();
        assert_eq!(recent_write(&p, b"three"), Some(WriteSource::Restore));
        assert_eq!(recent_write(&tmp.path().join("other"), b"three"), None);
    }
}
