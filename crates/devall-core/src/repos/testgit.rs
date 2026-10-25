//! Real, throwaway git repositories for tests (skipped when git isn't installed). The user's
//! global config can't leak in: hooks, signing and line-ending conversion are pinned locally.

use std::path::{Path, PathBuf};

use super::git;

pub struct TestRepo {
    _tmp: tempfile::TempDir,
    pub dir: PathBuf,
}

/// `None` (and a note on stderr) when git can't be started — tests then return early.
pub fn repo() -> Option<TestRepo> {
    if !git::available() {
        eprintln!("git not installed — skipping");
        return None;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    let r = TestRepo { _tmp: tmp, dir };
    r.git(&["init", "-q"]);
    r.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
    for (k, v) in [
        ("user.name", "Test"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("core.autocrlf", "false"),
        ("core.safecrlf", "false"),
        ("core.hooksPath", "no-hooks"),
        ("init.defaultBranch", "main"),
        ("merge.conflictstyle", "merge"),
    ] {
        r.git(&["config", k, v]);
    }
    Some(r)
}

impl TestRepo {
    /// Runs git and returns stdout; panics with git's message on failure.
    pub fn git(&self, args: &[&str]) -> String {
        git::run(&self.dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    pub fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let p = self.path(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.path(rel)).unwrap()
    }

    /// Stages everything and commits; returns the new commit's SHA.
    pub fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
        self.sha("HEAD")
    }

    pub fn sha(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev]).trim().to_string()
    }

    pub fn root(&self) -> &Path {
        &self.dir
    }
}
