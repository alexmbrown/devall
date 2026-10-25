//! Which files of a comparison the user has reviewed — local to this machine (the project data
//! folder's `git-review.json`), never in the repository.
//!
//! Keyed by repository name, then `<base>...<head>` as the user named them (`origin/main...feature/x`,
//! not SHAs, so the marks survive new commits), then the file path → the blob id the file had when
//! it was marked. The UI compares that id with the file's current one: a file that changed since
//! shows as "changed since review" and can be diffed against the reviewed version.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::fsutil::atomic_write;

pub const FILE: &str = "git-review.json";
/// Comparisons kept per repository (the least recently touched go first).
pub const MAX_PER_REPO: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewed {
    /// The file's blob id when it was marked (`-<old blob>` for a deleted file).
    pub blob: String,
    /// Unix seconds.
    pub at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Entry {
    updated: i64,
    files: BTreeMap<String, Reviewed>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    version: u32,
    repos: BTreeMap<String, BTreeMap<String, Entry>>,
}

/// The store of a project (`<project data>/git-review.json`).
pub fn path_for(root: &Path) -> PathBuf {
    crate::local::project_dir(root).join(FILE)
}

fn key(base: &str, head: &str) -> String {
    format!("{base}...{head}")
}

fn read(path: &Path) -> Store {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reviewed files of one comparison.
pub fn get(path: &Path, repo: &str, base: &str, head: &str) -> BTreeMap<String, Reviewed> {
    let _l = lock();
    read(path).repos.get(repo).and_then(|m| m.get(&key(base, head))).map(|e| e.files.clone()).unwrap_or_default()
}

/// Marks (`Some(blob)`) or unmarks (`None`) files; returns the comparison's marks after the change.
pub fn set(path: &Path, repo: &str, base: &str, head: &str, changes: &[(String, Option<String>)], now: i64) -> Result<BTreeMap<String, Reviewed>> {
    let _l = lock();
    let mut store = read(path);
    store.version = 1;
    let repo_map = store.repos.entry(repo.to_string()).or_default();
    let entry = repo_map.entry(key(base, head)).or_default();
    for (file, blob) in changes {
        match blob {
            Some(b) => {
                entry.files.insert(file.clone(), Reviewed { blob: b.clone(), at: now });
            }
            None => {
                entry.files.remove(file);
            }
        }
    }
    entry.updated = now;
    let files = entry.files.clone();
    repo_map.retain(|_, e| !e.files.is_empty());
    while repo_map.len() > MAX_PER_REPO {
        let oldest = repo_map.iter().min_by_key(|(_, e)| e.updated).map(|(k, _)| k.clone());
        match oldest {
            Some(k) => repo_map.remove(&k),
            None => break,
        };
    }
    store.repos.retain(|_, m| !m.is_empty());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).at(dir)?;
    }
    let json = serde_json::to_vec_pretty(&store).map_err(|e| Error::Invalid(e.to_string()))?;
    atomic_write(path, &json)?;
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_survive_and_unmark() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("data").join(FILE);
        assert!(get(&p, "web", "main", "feature/x").is_empty());
        let m = set(&p, "web", "main", "feature/x", &[("a.ts".into(), Some("abc".into())), ("b.ts".into(), Some("def".into()))], 10).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(get(&p, "web", "main", "feature/x")["a.ts"], Reviewed { blob: "abc".into(), at: 10 });
        // Other comparisons and repos are separate.
        assert!(get(&p, "web", "main", "feature/y").is_empty());
        assert!(get(&p, "api", "main", "feature/x").is_empty());
        let m = set(&p, "web", "main", "feature/x", &[("a.ts".into(), None), ("b.ts".into(), Some("new".into()))], 20).unwrap();
        assert_eq!(m.keys().collect::<Vec<_>>(), ["b.ts"]);
        assert_eq!(m["b.ts"].blob, "new");
        // Emptied comparisons disappear from the file.
        set(&p, "web", "main", "feature/x", &[("b.ts".into(), None)], 30).unwrap();
        assert!(!std::fs::read_to_string(&p).unwrap().contains("feature/x"));
    }

    #[test]
    fn keeps_a_bounded_number_of_comparisons() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(FILE);
        for i in 0..(MAX_PER_REPO + 5) {
            set(&p, "web", "main", &format!("b{i}"), &[("f".into(), Some("x".into()))], i as i64).unwrap();
        }
        assert!(get(&p, "web", "main", "b0").is_empty(), "the oldest went");
        assert!(!get(&p, "web", "main", &format!("b{}", MAX_PER_REPO + 4)).is_empty());
    }
}
