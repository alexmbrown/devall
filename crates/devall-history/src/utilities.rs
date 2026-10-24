//! Utility runs, device-wide (Utilities work without a project): `<app data>/history/utilities.sqlite`.
//!
//! An entry keeps the input (up to [`MAX_INPUT`]), the arguments — values of secret-looking ones
//! masked — and a short output preview, so a past run can be restored. Recording the same op,
//! input and arguments as the latest entry within [`DEDUPE_WINDOW_MS`] only bumps its time.
//! Inputs or outputs that are themselves secrets (TOTP seeds, passwords) are never stored.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, like_pattern};

pub const DB_FILE: &str = "utilities.sqlite";
pub const MAX_INPUT: usize = 64 * 1024;
pub const MAX_OUTPUT_PREVIEW: usize = 2 * 1024;
pub const DEDUPE_WINDOW_MS: i64 = 10 * 60 * 1000;
pub const MASK: &str = "••••••";
/// Ops whose input is a secret (a TOTP seed, a password to hash or verify): the input isn't kept.
pub const SECRET_INPUT_OPS: &[&str] = &["totp.generate", "bcrypt.hash", "bcrypt.verify"];
/// Ops whose output is a secret (generated passwords, one-time codes): no output preview.
pub const SECRET_OUTPUT_OPS: &[&str] = &["password.generate", "totp.generate"];

/// Whether an argument's value should be masked, judged by its name.
pub fn is_secret_arg(name: &str) -> bool {
    let n = name.to_ascii_lowercase().replace(['_', '-'], "");
    ["secret", "password", "passwd", "passphrase", "token", "credential", "private", "apikey"].iter().any(|w| n.contains(w))
        || n == "key"
        || n.ends_with("key")
        || n == "pass"
        || n == "pwd"
}

/// A run to record, as the UI reports it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewUtilityRun {
    pub op: String,
    pub op_name: String,
    pub input: String,
    pub args: BTreeMap<String, String>,
    /// Names of arguments the UI knows are secrets (arg kind `secret`); masked like secret-looking names.
    pub secret_args: Vec<String>,
    pub output: String,
    pub error: Option<String>,
    /// Open project root, if any.
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UtilityRun {
    pub id: String,
    /// Unix milliseconds of the latest run.
    pub at: i64,
    pub op: String,
    pub op_name: String,
    pub input: String,
    pub input_truncated: bool,
    /// The input was a secret and is not stored.
    pub input_masked: bool,
    /// Secret values replaced by [`MASK`].
    pub args: BTreeMap<String, String>,
    pub output: String,
    pub output_truncated: bool,
    pub ok: bool,
    pub error: Option<String>,
    pub project: Option<String>,
}

pub struct UtilityHistory {
    conn: Mutex<Connection>,
    path: PathBuf,
}

/// The longest prefix of `s` that is at most `max` bytes and ends on a char boundary.
fn truncate(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

const COLUMNS: &str = "id, at, op, op_name, input, input_truncated, input_masked, args, output, output_truncated, ok, error, project";

fn run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<UtilityRun> {
    let args: String = r.get(7)?;
    Ok(UtilityRun {
        id: r.get(0)?,
        at: r.get(1)?,
        op: r.get(2)?,
        op_name: r.get(3)?,
        input: r.get(4)?,
        input_truncated: r.get(5)?,
        input_masked: r.get(6)?,
        args: serde_json::from_str(&args).unwrap_or_default(),
        output: r.get(8)?,
        output_truncated: r.get(9)?,
        ok: r.get(10)?,
        error: r.get(11)?,
        project: r.get(12)?,
    })
}

impl UtilityHistory {
    /// Opens (creating if needed) `<app data>/history/utilities.sqlite`.
    pub fn open_default() -> Result<Self> {
        let dir = devall_core::local::history_dir().ok_or_else(|| crate::HistoryError::Invalid("no user data directory on this system".into()))?;
        std::fs::create_dir_all(&dir)?;
        Self::open(&dir.join(DB_FILE))
    }

    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                at INTEGER NOT NULL,
                op TEXT NOT NULL,
                op_name TEXT NOT NULL,
                input TEXT NOT NULL,
                input_truncated INTEGER NOT NULL,
                input_masked INTEGER NOT NULL,
                args TEXT NOT NULL,
                output TEXT NOT NULL,
                output_truncated INTEGER NOT NULL,
                ok INTEGER NOT NULL,
                error TEXT,
                project TEXT,
                fingerprint TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS runs_by_time ON runs(at DESC);
            CREATE INDEX IF NOT EXISTS runs_by_op ON runs(op, at DESC);",
        )?;
        Ok(Self { conn: Mutex::new(conn), path: path.to_path_buf() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records a run (see the module docs) and keeps the newest `max_entries`. Returns the id
    /// of the new or bumped entry.
    pub fn record(&self, run: NewUtilityRun, max_entries: usize) -> Result<String> {
        self.record_at(run, max_entries, devall_core::local::now_ms())
    }

    pub fn record_at(&self, run: NewUtilityRun, max_entries: usize, now: i64) -> Result<String> {
        let args: BTreeMap<String, String> = run
            .args
            .iter()
            .map(|(k, v)| {
                let secret = !v.is_empty() && (is_secret_arg(k) || run.secret_args.iter().any(|s| s == k));
                (k.clone(), if secret { MASK.to_string() } else { truncate(v, MAX_INPUT).0.to_string() })
            })
            .collect();
        let input_masked = SECRET_INPUT_OPS.contains(&run.op.as_str()) && !run.input.is_empty();
        let (input, input_truncated) = if input_masked { ("", false) } else { truncate(&run.input, MAX_INPUT) };
        let (output, output_truncated) = if SECRET_OUTPUT_OPS.contains(&run.op.as_str()) { ("", false) } else { truncate(&run.output, MAX_OUTPUT_PREVIEW) };
        let args_json = serde_json::to_string(&args).unwrap_or_else(|_| "{}".into());
        let mut h = Sha256::new();
        for part in [run.op.as_str(), input, args_json.as_str()] {
            h.update(part.as_bytes());
            h.update([0]);
        }
        let fingerprint: String = h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect();
        let ok = run.error.is_none();

        let conn = self.conn.lock().unwrap();
        let last: Option<(String, String, i64)> =
            conn.query_row("SELECT id, fingerprint, at FROM runs ORDER BY at DESC, id DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
        if let Some((id, fp, at)) = last
            && fp == fingerprint
            && now - at <= DEDUPE_WINDOW_MS
        {
            conn.execute(
                "UPDATE runs SET at = ?2, output = ?3, output_truncated = ?4, ok = ?5, error = ?6, project = COALESCE(?7, project) WHERE id = ?1",
                params![id, now.max(at), output, output_truncated, ok, run.error, run.project],
            )?;
            return Ok(id);
        }
        let id = ulid::Ulid::generate().to_string();
        conn.execute(
            "INSERT INTO runs (id, at, op, op_name, input, input_truncated, input_masked, args, output, output_truncated, ok, error, project, fingerprint)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![id, now, run.op, run.op_name, input, input_truncated, input_masked, args_json, output, output_truncated, ok, run.error, run.project, fingerprint],
        )?;
        conn.execute("DELETE FROM runs WHERE id NOT IN (SELECT id FROM runs ORDER BY at DESC, id DESC LIMIT ?1)", params![max_entries.max(1) as i64])?;
        Ok(id)
    }

    /// Newest first; `op` limits to one utility, `query` matches the op, its name and the input.
    pub fn list(&self, op: Option<&str>, query: Option<&str>, offset: usize, limit: usize) -> Result<Vec<UtilityRun>> {
        let conn = self.conn.lock().unwrap();
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(like_pattern);
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM runs WHERE (?1 IS NULL OR op = ?1)
               AND (?2 IS NULL OR op LIKE ?2 ESCAPE '!' OR op_name LIKE ?2 ESCAPE '!' OR input LIKE ?2 ESCAPE '!')
             ORDER BY at DESC, id DESC LIMIT ?3 OFFSET ?4"
        ))?;
        let rows = stmt.query_map(params![op, pattern, limit as i64, offset as i64], run_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn get(&self, id: &str) -> Result<Option<UtilityRun>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(&format!("SELECT {COLUMNS} FROM runs WHERE id = ?1"), params![id], run_row).optional().map_err(Into::into)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        Ok(self.conn.lock().unwrap().execute("DELETE FROM runs WHERE id = ?1", params![id])? > 0)
    }

    /// Forgets one utility's runs, or all of them.
    pub fn clear(&self, op: Option<&str>) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM runs WHERE (?1 IS NULL OR op = ?1)", params![op])?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        if op.is_none() {
            conn.execute_batch("VACUUM;")?;
        }
        Ok(n)
    }

    /// Number of runs and the database size on disk.
    pub fn stats(&self) -> Result<(usize, u64)> {
        let n: i64 = self.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))?;
        Ok((n as usize, crate::db_size(&self.path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(op: &str, input: &str, args: &[(&str, &str)]) -> NewUtilityRun {
        NewUtilityRun {
            op: op.into(),
            op_name: op.into(),
            input: input.into(),
            args: args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            output: "out".into(),
            ..Default::default()
        }
    }

    fn open() -> (tempfile::TempDir, UtilityHistory) {
        let d = tempfile::tempdir().unwrap();
        let h = UtilityHistory::open(&d.path().join(DB_FILE)).unwrap();
        (d, h)
    }

    #[test]
    fn secret_arg_names() {
        for yes in ["key", "secret", "password", "apiKey", "api_key", "privateKey", "signingKey", "token", "passphrase", "pwd"] {
            assert!(is_secret_arg(yes), "{yes}");
        }
        for no in ["alg", "indent", "sortKeys", "keyword", "count", "pattern", "encoding"] {
            assert!(!is_secret_arg(no), "{no}");
        }
    }

    #[test]
    fn masks_secrets_and_truncates() {
        let (_d, h) = open();
        let mut r = run("jwt.sign", &"x".repeat(MAX_INPUT + 10), &[("key", "hunter2"), ("alg", "HS256"), ("custom", "s3"), ("empty", "")]);
        r.secret_args = vec!["custom".into()];
        r.output = "é".repeat(MAX_OUTPUT_PREVIEW);
        let id = h.record_at(r, 100, 1).unwrap();
        let e = h.get(&id).unwrap().unwrap();
        assert_eq!(e.args["key"], MASK);
        assert_eq!(e.args["custom"], MASK);
        assert_eq!(e.args["alg"], "HS256");
        assert_eq!(e.args["empty"], "", "nothing to mask");
        assert!(e.input_truncated && e.input.len() == MAX_INPUT);
        assert!(e.output_truncated && e.output.len() <= MAX_OUTPUT_PREVIEW && e.output.chars().all(|c| c == 'é'));

        let id = h.record_at(run("totp.generate", "JBSWY3DPEHPK3PXP", &[]), 100, 2).unwrap();
        let e = h.get(&id).unwrap().unwrap();
        assert!(e.input_masked && e.input.is_empty() && e.output.is_empty());
        let id = h.record_at(run("password.generate", "", &[("length", "20")]), 100, 3).unwrap();
        assert!(h.get(&id).unwrap().unwrap().output.is_empty());
    }

    #[test]
    fn dedupes_the_latest_entry_within_the_window() {
        let (_d, h) = open();
        let a = h.record_at(run("base64.encode", "hi", &[]), 100, 1_000).unwrap();
        let b = h.record_at(run("base64.encode", "hi", &[]), 100, 2_000).unwrap();
        assert_eq!(a, b);
        assert_eq!(h.get(&a).unwrap().unwrap().at, 2_000);
        // Different args → a new entry; same as an older (not latest) entry → also new.
        h.record_at(run("base64.encode", "hi", &[("urlSafe", "yes")]), 100, 3_000).unwrap();
        let c = h.record_at(run("base64.encode", "hi", &[]), 100, 4_000).unwrap();
        assert_ne!(a, c);
        // Outside the window → new.
        let d = h.record_at(run("base64.encode", "hi", &[]), 100, 4_000 + DEDUPE_WINDOW_MS + 1).unwrap();
        assert_ne!(c, d);
        assert_eq!(h.list(None, None, 0, 100).unwrap().len(), 4);
    }

    #[test]
    fn list_search_cap_and_clear() {
        let (_d, h) = open();
        for i in 0..12 {
            h.record_at(run(if i % 2 == 0 { "jq" } else { "url.encode" }, &format!("input {i}"), &[]), 10, i).unwrap();
        }
        assert_eq!(h.list(None, None, 0, 100).unwrap().len(), 10, "capped");
        assert_eq!(h.list(Some("jq"), None, 0, 100).unwrap().len(), 5);
        assert_eq!(h.list(None, Some("input 11"), 0, 100).unwrap()[0].op, "url.encode");
        assert_eq!(h.list(None, Some("URL"), 0, 100).unwrap().len(), 5, "LIKE is case-insensitive for ASCII");
        assert_eq!(h.list(None, None, 8, 100).unwrap().len(), 2);
        assert_eq!(h.clear(Some("jq")).unwrap(), 5);
        assert_eq!(h.stats().unwrap().0, 5);
        assert_eq!(h.clear(None).unwrap(), 5);
    }
}
