//! Consent before running code that came from a repository.
//!
//! Notebooks and Markdown notes are committed to git, so opening a project must never run code
//! by itself. The user trusts a notebook once; the trust is pinned to a SHA-256 of what would run
//! (every code cell's language, code and templating flag, plus the notebook's working directory),
//! so a `git pull` that changes the code asks again. Markdown code blocks are trusted one block at
//! a time (note path + block hash). Injecting secrets into a cell is a separate, per-cell grant
//! pinned to that cell's code.
//!
//! The store is a user-level file in the app data directory (`notebook-trust.json`) — never in the
//! project, so a repository can't pre-trust itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{NotebookError, Result};

pub const TRUST_FILE: &str = "notebook-trust.json";
/// Hashes remembered per notebook / note (older ones are dropped).
const KEEP: usize = 20;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustFile {
    #[serde(default)]
    version: u32,
    /// `<project id>:<notebook path>` → trusted notebook hashes, oldest first.
    #[serde(default)]
    notebooks: BTreeMap<String, Vec<String>>,
    /// `<project id>:<note path>` → trusted code-block hashes.
    #[serde(default)]
    snippets: BTreeMap<String, Vec<String>>,
    /// `<project id>:<notebook path>#<cell id>` → hash of the cell the user allowed secrets for.
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// User-level record of trusted notebooks, code blocks and secret grants.
pub struct TrustStore {
    path: Option<PathBuf>,
    data: Mutex<TrustFile>,
}

fn key(project_id: &str, path: &str) -> String {
    format!("{project_id}:{}", devall_core::fsutil::normalize_rel(path))
}

impl TrustStore {
    /// Opens `<dir>/notebook-trust.json` (missing or unreadable → empty).
    pub fn open(dir: &Path) -> Self {
        let path = dir.join(TRUST_FILE);
        let data = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path: Some(path), data: Mutex::new(data) }
    }

    /// A store that isn't persisted (tests, previews).
    pub fn in_memory() -> Self {
        Self { path: None, data: Mutex::new(TrustFile::default()) }
    }

    fn save(&self, data: &TrustFile) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let mut d = data.clone();
        d.version = 1;
        let text = serde_json::to_string_pretty(&d).map_err(|e| NotebookError::Invalid(e.to_string()))?;
        devall_core::fsutil::atomic_write(path, text.as_bytes())?;
        Ok(())
    }

    fn update(&self, f: impl FnOnce(&mut TrustFile)) -> Result<()> {
        let mut d = self.data.lock().unwrap();
        f(&mut d);
        self.save(&d)
    }

    pub fn is_trusted(&self, project_id: &str, notebook: &str, hash: &str) -> bool {
        self.data.lock().unwrap().notebooks.get(&key(project_id, notebook)).is_some_and(|v| v.iter().any(|h| h == hash))
    }

    pub fn trust(&self, project_id: &str, notebook: &str, hash: &str) -> Result<()> {
        self.update(|d| remember(d.notebooks.entry(key(project_id, notebook)).or_default(), hash))
    }

    pub fn is_snippet_trusted(&self, project_id: &str, note: &str, hash: &str) -> bool {
        self.data.lock().unwrap().snippets.get(&key(project_id, note)).is_some_and(|v| v.iter().any(|h| h == hash))
    }

    pub fn trust_snippet(&self, project_id: &str, note: &str, hash: &str) -> Result<()> {
        self.update(|d| remember(d.snippets.entry(key(project_id, note)).or_default(), hash))
    }

    /// Forgets every trust decision for a notebook or note (and its secret grants).
    pub fn revoke(&self, project_id: &str, path: &str) -> Result<()> {
        let k = key(project_id, path);
        let prefix = format!("{k}#");
        self.update(|d| {
            d.notebooks.remove(&k);
            d.snippets.remove(&k);
            d.secrets.retain(|s, _| !s.starts_with(&prefix));
        })
    }

    /// Whether the user allowed secrets for this exact cell code.
    pub fn secrets_allowed(&self, project_id: &str, notebook: &str, cell_id: &str, cell_hash: &str) -> bool {
        self.data.lock().unwrap().secrets.get(&format!("{}#{cell_id}", key(project_id, notebook))).is_some_and(|h| h == cell_hash)
    }

    /// Grants (`Some(hash)`) or revokes (`None`) secrets for a cell.
    pub fn set_secrets_allowed(&self, project_id: &str, notebook: &str, cell_id: &str, cell_hash: Option<&str>) -> Result<()> {
        let k = format!("{}#{cell_id}", key(project_id, notebook));
        self.update(|d| match cell_hash {
            Some(h) => {
                d.secrets.insert(k, h.to_string());
            }
            None => {
                d.secrets.remove(&k);
            }
        })
    }
}

fn remember(list: &mut Vec<String>, hash: &str) {
    list.retain(|h| h != hash);
    list.push(hash.to_string());
    if list.len() > KEEP {
        let extra = list.len() - KEEP;
        list.drain(..extra);
    }
}

/// Feeds one length-prefixed field (unambiguous concatenation).
fn field(h: &mut Sha256, s: &str) {
    h.update((s.len() as u64).to_le_bytes());
    h.update(s.as_bytes());
}

/// Line endings don't change what runs (git `autocrlf` must not invalidate trust).
fn normalize(code: &str) -> String {
    code.replace("\r\n", "\n")
}

/// Hash of one piece of code in a language (Markdown code blocks, secret grants).
pub fn code_hash(language: &str, code: &str) -> String {
    let mut h = Sha256::new();
    field(&mut h, "devall-code/v1");
    field(&mut h, devall_core::notebook::normalize_language(language).unwrap_or(language));
    field(&mut h, &normalize(code));
    hex::encode(h.finalize())
}

/// Hash of everything in a notebook that decides what runs: the working directory and, for each
/// code cell in order, its language, code and templating flag. Markdown cells, outputs and other
/// metadata don't count.
pub fn notebook_hash(json_text: &str) -> Result<String> {
    let nb: Value = serde_json::from_str(json_text).map_err(|e| NotebookError::Invalid(format!("not a notebook: {e}")))?;
    let mut h = Sha256::new();
    field(&mut h, "devall-notebook/v1");
    field(&mut h, nb.pointer("/metadata/devall/cwd").and_then(Value::as_str).unwrap_or(""));
    for cell in nb.get("cells").and_then(Value::as_array).into_iter().flatten() {
        if cell.get("cell_type").and_then(Value::as_str) != Some("code") {
            continue;
        }
        let (lang, code) = devall_core::notebook::cell_language(cell, &nb);
        field(&mut h, lang.unwrap_or(""));
        field(&mut h, &normalize(&code));
        let templating = cell.pointer("/metadata/devall/templating").and_then(Value::as_bool).unwrap_or(false);
        field(&mut h, if templating { "templating" } else { "" });
    }
    Ok(hex::encode(h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn nb(cells: Value) -> String {
        json!({ "nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells": cells }).to_string()
    }

    #[test]
    fn hash_tracks_code_only() {
        let a = nb(json!([
            { "cell_type": "markdown", "metadata": {}, "source": "# Title" },
            { "cell_type": "code", "metadata": { "devall": { "language": "shell" } }, "outputs": [], "source": ["echo hi\n", "ls"] }
        ]));
        let h = notebook_hash(&a).unwrap();
        // Markdown, outputs and line endings don't matter.
        let b = nb(json!([
            { "cell_type": "markdown", "metadata": {}, "source": "# Renamed" },
            { "cell_type": "code", "metadata": { "devall": { "language": "shell" } }, "outputs": [{ "output_type": "stream", "name": "stdout", "text": "hi" }], "source": "echo hi\r\nls" }
        ]));
        assert_eq!(notebook_hash(&b).unwrap(), h);
        // Code, language, templating and cwd do.
        let c = nb(json!([{ "cell_type": "code", "metadata": { "devall": { "language": "shell" } }, "source": "echo hi\nrm -rf /" }]));
        assert_ne!(notebook_hash(&c).unwrap(), h);
        let d = nb(json!([{ "cell_type": "code", "metadata": { "devall": { "language": "python" } }, "source": "echo hi\nls" }]));
        assert_ne!(notebook_hash(&d).unwrap(), h);
        let e = nb(json!([{ "cell_type": "code", "metadata": { "devall": { "language": "shell", "templating": true } }, "source": "echo hi\nls" }]));
        assert_ne!(notebook_hash(&e).unwrap(), h);
        let mut f: Value = serde_json::from_str(&a).unwrap();
        f["metadata"]["devall"] = json!({ "cwd": "../.." });
        assert_ne!(notebook_hash(&f.to_string()).unwrap(), h);
        assert!(notebook_hash("{oops").is_err());

        assert_eq!(code_hash("bash", "ls\r\n"), code_hash("shell", "ls\n"));
        assert_ne!(code_hash("shell", "ls"), code_hash("python", "ls"));
    }

    #[test]
    fn store_persists_and_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let s = TrustStore::open(dir.path());
        assert!(!s.is_trusted("p1", "run.ipynb", "h1"));
        s.trust("p1", "run.ipynb", "h1").unwrap();
        s.trust_snippet("p1", "readme.md", "b1").unwrap();
        s.set_secrets_allowed("p1", "run.ipynb", "cell-a", Some("c1")).unwrap();

        let s = TrustStore::open(dir.path());
        assert!(s.is_trusted("p1", "run.ipynb", "h1"));
        assert!(s.is_trusted("p1", "./run.ipynb", "h1"));
        assert!(!s.is_trusted("p2", "run.ipynb", "h1"), "other project");
        assert!(!s.is_trusted("p1", "run.ipynb", "h2"), "changed code");
        assert!(s.is_snippet_trusted("p1", "readme.md", "b1"));
        assert!(!s.is_snippet_trusted("p1", "other.md", "b1"));
        assert!(s.secrets_allowed("p1", "run.ipynb", "cell-a", "c1"));
        assert!(!s.secrets_allowed("p1", "run.ipynb", "cell-a", "c2"), "grant is pinned to the code");
        s.revoke("p1", "run.ipynb").unwrap();
        assert!(!s.is_trusted("p1", "run.ipynb", "h1"));
        assert!(!s.secrets_allowed("p1", "run.ipynb", "cell-a", "c1"));

        for i in 0..30 {
            s.trust("p1", "x.ipynb", &format!("h{i}")).unwrap();
        }
        assert!(!s.is_trusted("p1", "x.ipynb", "h0"));
        assert!(s.is_trusted("p1", "x.ipynb", "h29"));
    }
}
