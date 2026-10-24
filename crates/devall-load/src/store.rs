//! Test definitions under `<project>/load/` (`*.load.yaml`, any depth except `runs/`) and run
//! summaries. New summaries go to the project data folder on this device
//! (`<project data>/load-runs/<test slug>/<UTC timestamp>.json`, see
//! `devall_core::local::project_dir`) — never into the repository. Summaries older versions wrote
//! to `load/runs/<test slug>/` are still listed and read (never deleted for you).
//!
//! Paths crossing this API are `/`-separated; test paths are relative to `load/`. Run "files" are
//! ids: `load-runs/<slug>/<name>.json` (project data folder) or `load/runs/<slug>/<name>.json`
//! (legacy, relative to the project root).

use std::fs;
use std::path::{Path, PathBuf};

use devall_core::fsutil::{atomic_write, normalize_rel, read_yaml, rel_join, safe_join, slugify, unique_name, write_yaml};
use devall_core::vars::redact_values;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{LoadError, Result};
use crate::model::{FORMAT_VERSION, LoadTestFile};
use crate::summary::{GitInfo, LoadSummary};

pub const LOAD_DIR: &str = "load";
/// Legacy run summaries inside the project (`load/runs/`), read but no longer written.
pub const RUNS_DIR: &str = "runs";
/// Run summaries in the project data folder (`<project data>/load-runs/`).
pub const LOCAL_RUNS_DIR: &str = "load-runs";
pub const TEST_EXT: &str = ".load.yaml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadTestEntry {
    /// Relative to `load/`, e.g. `smoke.load.yaml` or `api/checkout.load.yaml`.
    pub path: String,
    pub name: String,
    /// Executor type, when the file parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// Number of saved runs.
    pub runs: usize,
    /// Set when the file exists but can't be parsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunEntry {
    /// Run id: `load-runs/smoke/20261021T101500Z.json` (project data folder) or
    /// `load/runs/smoke/…` (legacy, in the project).
    pub file: String,
    pub run_id: String,
    pub started_at: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// The run targeted the project's mock server.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mock_baseline: bool,
    pub aborted: bool,
    pub thresholds_passed: bool,
    pub requests: u64,
    pub rps: f64,
    pub error_rate: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitInfo>,
}

fn load_dir(root: &Path) -> PathBuf {
    root.join(LOAD_DIR)
}

fn test_file(root: &Path, path: &str) -> Result<(String, PathBuf)> {
    let rel = normalize_rel(path);
    if !rel.ends_with(TEST_EXT) {
        return Err(LoadError::Invalid(format!("'{rel}' is not a load test (*{TEST_EXT})")));
    }
    if rel == RUNS_DIR || rel.starts_with(&format!("{RUNS_DIR}/")) {
        return Err(LoadError::Invalid("load/runs/ holds run summaries, not tests".into()));
    }
    Ok((rel.clone(), safe_join(&load_dir(root), &rel)?))
}

/// `smoke.load.yaml` → `smoke`; `api/checkout.load.yaml` → `api/checkout`.
pub fn test_slug(path: &str) -> String {
    let rel = normalize_rel(path);
    rel.strip_suffix(TEST_EXT).unwrap_or(&rel).to_string()
}

/// Legacy summaries of a test (`load/runs/<slug>/`).
fn runs_dir(root: &Path, test_path: &str) -> Result<PathBuf> {
    Ok(safe_join(&load_dir(root).join(RUNS_DIR), &test_slug(test_path))?)
}

/// Summaries of a test on this device (`<project data>/load-runs/<slug>/`).
fn local_runs_dir(root: &Path, test_path: &str) -> Result<PathBuf> {
    Ok(safe_join(&devall_core::local::project_dir(root).join(LOCAL_RUNS_DIR), &test_slug(test_path))?)
}

/// Every test under `load/`, sorted by path. Unparseable files are listed with an error.
pub fn list_tests(root: &Path) -> Result<Vec<LoadTestEntry>> {
    let mut out = Vec::new();
    walk(root, &load_dir(root), "", &mut out);
    out.sort_by_key(|a| a.path.to_lowercase());
    Ok(out)
}

fn walk(root: &Path, dir: &Path, rel: &str, out: &mut Vec<LoadTestEntry>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let child = rel_join(rel, &name);
        let path = entry.path();
        if path.is_dir() {
            if rel.is_empty() && name == RUNS_DIR {
                continue;
            }
            walk(root, &path, &child, out);
        } else if name.ends_with(TEST_EXT) {
            let runs = [runs_dir(root, &child), local_runs_dir(root, &child)].into_iter().flatten().map(|d| count_json(&d)).sum();
            match read_yaml::<LoadTestFile>(&path) {
                Ok(t) => out.push(LoadTestEntry { path: child, name: t.name, executor: Some(t.executor.kind().into()), runs, error: None }),
                Err(e) => out.push(LoadTestEntry {
                    name: name.trim_end_matches(TEST_EXT).to_string(),
                    path: child,
                    executor: None,
                    runs,
                    error: Some(e.to_string()),
                }),
            }
        }
    }
}

fn count_json(dir: &Path) -> usize {
    fs::read_dir(dir).map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".json")).count()).unwrap_or(0)
}

pub fn read_test(root: &Path, path: &str) -> Result<LoadTestFile> {
    let (rel, file) = test_file(root, path)?;
    if !file.is_file() {
        return Err(devall_core::Error::NotFound(format!("load/{rel}")).into());
    }
    Ok(read_yaml(&file)?)
}

/// Writes the test (stable key order, `version: 1`). Semantic validation is separate
/// ([`crate::validate::validate`]) so drafts can be saved while being edited.
pub fn save_test(root: &Path, path: &str, test: &LoadTestFile) -> Result<()> {
    let (_, file) = test_file(root, path)?;
    let mut t = test.clone();
    t.version = FORMAT_VERSION;
    Ok(write_yaml(&file, &t)?)
}

/// Creates `load/<slug(name)>.load.yaml` from `template` (or a default) and returns its path.
pub fn create_test(root: &Path, name: &str, template: Option<LoadTestFile>) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LoadError::Invalid("name cannot be empty".into()));
    }
    let dir = load_dir(root);
    fs::create_dir_all(&dir).map_err(|e| devall_core::Error::Io { path: dir.clone(), source: e })?;
    let mut slug = slugify(name);
    if slug == RUNS_DIR {
        slug.push_str("-test");
    }
    let file = unique_name(&dir, &slug, TEST_EXT);
    let mut test = template.unwrap_or_else(|| LoadTestFile::template(name, None));
    test.name = name.to_string();
    save_test(root, &file, &test)?;
    Ok(file)
}

/// Moves a test file to the OS trash. Its run summaries stay (they are history).
pub fn delete_test(root: &Path, path: &str) -> Result<()> {
    let (rel, file) = test_file(root, path)?;
    if !file.exists() {
        return Err(devall_core::Error::NotFound(format!("load/{rel}")).into());
    }
    trash::delete(&file).map_err(|e| devall_core::Error::Io { path: file.clone(), source: std::io::Error::other(e.to_string()) })?;
    Ok(())
}

// ---------- runs ----------

/// Writes `summary` to `<project data>/load-runs/<slug>/<UTC timestamp>.json` and returns its
/// run id (`load-runs/<slug>/<name>`; [`run_path`] resolves it). Any secret value still present
/// is masked. For CI artifacts, write [`summary_json`] wherever you need it (`devall load run --out`).
pub fn write_run(root: &Path, summary: &LoadSummary, secret_values: &std::collections::BTreeSet<String>) -> Result<String> {
    let dir = local_runs_dir(root, &summary.test_path)?;
    let stamp = chrono::DateTime::parse_from_rfc3339(&summary.started_at)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now())
        .format("%Y%m%dT%H%M%SZ")
        .to_string();
    fs::create_dir_all(&dir).map_err(|e| devall_core::Error::Io { path: dir.clone(), source: e })?;
    let name = unique_name(&dir, &stamp, ".json");
    let text = summary_json(summary, secret_values);
    atomic_write(&dir.join(&name), text.as_bytes())?;
    Ok(format!("{LOCAL_RUNS_DIR}/{}/{name}", test_slug(&summary.test_path)))
}

/// Serialized summary: readable, but leaf objects (timeline points, percentiles) stay on one
/// line so a run is a few hundred lines, not tens of thousands. Secrets are masked both raw
/// and JSON-escaped.
pub fn summary_json(summary: &LoadSummary, secret_values: &std::collections::BTreeSet<String>) -> String {
    let value = serde_json::to_value(summary).unwrap_or(Value::Null);
    let mut out = String::new();
    write_value(&value, 0, &mut out);
    out.push('\n');
    let escaped: std::collections::BTreeSet<String> =
        secret_values.iter().filter_map(|s| serde_json::to_string(s).ok().map(|q| q[1..q.len() - 1].to_string())).collect();
    redact_values(&redact_values(&out, &escaped), secret_values)
}

fn is_leaf(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.values().all(|x| !x.is_object() && !x.is_array()),
        Value::Array(a) => a.iter().all(|x| !x.is_object() && !x.is_array()),
        _ => true,
    }
}

fn write_value(v: &Value, indent: usize, out: &mut String) {
    let pad = |n: usize| " ".repeat(n);
    match v {
        Value::Object(m) if !m.is_empty() && !(is_leaf(v) && serde_json::to_string(v).map(|s| s.len() <= 200).unwrap_or(false)) => {
            out.push_str("{\n");
            let n = m.len();
            for (i, (k, child)) in m.iter().enumerate() {
                out.push_str(&pad(indent + 2));
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push_str(": ");
                write_value(child, indent + 2, out);
                if i + 1 < n {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(indent));
            out.push('}');
        }
        Value::Array(a) if !a.is_empty() && !(is_leaf(v) && serde_json::to_string(v).map(|s| s.len() <= 200).unwrap_or(false)) => {
            out.push_str("[\n");
            let n = a.len();
            for (i, child) in a.iter().enumerate() {
                out.push_str(&pad(indent + 2));
                write_value(child, indent + 2, out);
                if i + 1 < n {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(indent));
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
    }
}

/// Where a run id points: the project data folder (`load-runs/…`) or, for legacy summaries,
/// the project (`load/runs/…`).
pub fn run_path(root: &Path, file: &str) -> Result<PathBuf> {
    let rel = normalize_rel(file);
    let legacy = format!("{LOAD_DIR}/{RUNS_DIR}/");
    let local = format!("{LOCAL_RUNS_DIR}/");
    if rel.ends_with(".json") && rel.starts_with(&local) {
        return Ok(safe_join(&devall_core::local::project_dir(root), &rel)?);
    }
    if !rel.starts_with(&legacy) || !rel.ends_with(".json") {
        return Err(LoadError::Invalid(format!("'{rel}' is not a run summary (load-runs/**/*.json or load/runs/**/*.json)")));
    }
    Ok(safe_join(root, &rel)?)
}

pub fn read_run(root: &Path, file: &str) -> Result<LoadSummary> {
    let path = run_path(root, file)?;
    let text = fs::read_to_string(&path).map_err(|e| devall_core::Error::Io { path: path.clone(), source: e })?;
    serde_json::from_str(&text).map_err(|e| devall_core::Error::Parse { path, message: e.to_string() }.into())
}

pub fn delete_run(root: &Path, file: &str) -> Result<()> {
    let path = run_path(root, file)?;
    trash::delete(&path).map_err(|e| devall_core::Error::Io { path: path.clone(), source: std::io::Error::other(e.to_string()) })?;
    Ok(())
}

/// Saved runs of a test (on this device and legacy ones in `load/runs/`), newest first.
/// Unreadable files are skipped.
pub fn list_runs(root: &Path, test_path: &str) -> Result<Vec<RunEntry>> {
    let slug = test_slug(test_path);
    let mut files = Vec::new();
    for (dir, prefix) in [(local_runs_dir(root, test_path)?, LOCAL_RUNS_DIR.to_string()), (runs_dir(root, test_path)?, format!("{LOAD_DIR}/{RUNS_DIR}"))] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") && !name.starts_with('.') {
                files.push(format!("{prefix}/{slug}/{name}"));
            }
        }
    }
    let mut out = Vec::new();
    for file in files {
        let Ok(s) = read_run(root, &file) else { continue };
        out.push(RunEntry {
            file,
            run_id: s.run_id,
            started_at: s.started_at,
            duration_ms: s.duration_ms,
            environment: s.environment,
            mock_baseline: s.mock_baseline.is_some(),
            aborted: s.aborted,
            thresholds_passed: s.thresholds_passed,
            requests: s.totals.requests,
            rps: s.totals.rps,
            error_rate: s.totals.error_rate,
            p50: s.latency.p50,
            p95: s.latency.p95,
            p99: s.latency.p99,
            git: s.git,
        });
    }
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at).then_with(|| b.file.cmp(&a.file)));
    Ok(out)
}

/// Branch and commit of the repository containing `root`, read from `.git` files (no git binary).
pub fn git_info(root: &Path) -> Option<GitInfo> {
    let mut dir = Some(root);
    let git_dir = loop {
        let d = dir?;
        let candidate = d.join(".git");
        if candidate.is_dir() {
            break candidate;
        }
        if candidate.is_file() {
            // Worktrees and submodules: `gitdir: <path>`.
            let text = fs::read_to_string(&candidate).ok()?;
            let p = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
            break if p.is_absolute() { p } else { d.join(p) };
        }
        dir = d.parent();
    };
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        let branch = r.strip_prefix("refs/heads/").map(str::to_string);
        // Linked worktrees keep refs in the common dir.
        let common = fs::read_to_string(git_dir.join("commondir")).ok().map(|c| git_dir.join(c.trim())).unwrap_or_else(|| git_dir.clone());
        let commit = [&git_dir, &common]
            .iter()
            .find_map(|g| fs::read_to_string(g.join(r)).ok().map(|s| s.trim().to_string()))
            .or_else(|| {
                let packed = fs::read_to_string(common.join("packed-refs")).ok()?;
                packed.lines().find_map(|l| l.strip_suffix(r).map(|sha| sha.trim().to_string()))
            })?;
        return Some(GitInfo { branch, commit: short(&commit) });
    }
    (head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit())).then(|| GitInfo { branch: None, commit: short(head) })
}

fn short(sha: &str) -> String {
    sha.chars().take(12).collect()
}
