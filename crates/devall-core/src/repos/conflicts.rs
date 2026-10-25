//! The merge-conflict editor's git side: which files conflict (`git ls-files -u`), the three
//! versions of one (index stages `:1:` base, `:2:` ours, `:3:` theirs) plus the working-tree file
//! with its markers, and the resolutions — save the edited result (and mark it resolved with
//! `git add`), take one side for the whole file (`git checkout --ours|--theirs`, also for binary
//! files), resolve as deleted (`git rm`), and re-create the markers (`git checkout --conflict`).
//!
//! These run while git has paused an operation (or after a stash apply that conflicted); they are
//! part of that operation's undo entry, not journaled one by one.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::compare;
use super::git::{self, GitResult};
use super::ops::{self, Operation};

/// Largest version read into the editor.
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Pathspecs taken literally (file names with `*` or `[` are just names).
fn literal_env() -> [(&'static str, &'static std::ffi::OsStr); 1] {
    [("GIT_LITERAL_PATHSPECS", std::ffi::OsStr::new("1"))]
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFile {
    pub path: String,
    pub base: bool,
    pub ours: bool,
    pub theirs: bool,
    /// "both modified", "deleted by them", "added by us"…
    pub description: String,
}

/// git's wording for a conflict from the stages present.
pub fn describe(base: bool, ours: bool, theirs: bool) -> &'static str {
    match (base, ours, theirs) {
        (true, true, true) => "both modified",
        (false, true, true) => "both added",
        (true, true, false) => "deleted by them",
        (true, false, true) => "deleted by us",
        (false, true, false) => "added by us",
        (false, false, true) => "added by them",
        (true, false, false) => "both deleted",
        _ => "conflict",
    }
}

/// Parses `git ls-files -u -z` (`<mode> <oid> <stage>\t<path>\0`).
pub fn parse_unmerged(out: &str) -> Vec<ConflictFile> {
    let mut files: Vec<ConflictFile> = Vec::new();
    for rec in out.split('\0').filter(|r| !r.is_empty()) {
        let Some((meta, path)) = rec.split_once('\t') else { continue };
        let stage = meta.split(' ').nth(2).unwrap_or("0");
        let f = match files.iter_mut().find(|f| f.path == path) {
            Some(f) => f,
            None => {
                files.push(ConflictFile { path: path.to_string(), base: false, ours: false, theirs: false, description: String::new() });
                files.last_mut().expect("just pushed")
            }
        };
        match stage {
            "1" => f.base = true,
            "2" => f.ours = true,
            "3" => f.theirs = true,
            _ => {}
        }
    }
    for f in &mut files {
        f.description = describe(f.base, f.ours, f.theirs).to_string();
    }
    files
}

pub fn list(dir: &Path) -> GitResult<Vec<ConflictFile>> {
    Ok(parse_unmerged(&git::run(dir, &["ls-files", "-u", "-z"])?))
}

fn check_path(p: &str) -> GitResult<()> {
    if p.is_empty() || p.starts_with('-') || p.contains('\0') || p.split(['/', '\\']).any(|s| s == "..") { Err(format!("invalid path {p}")) } else { Ok(()) }
}

fn is_binary(b: &[u8]) -> bool {
    b[..b.len().min(8000)].contains(&0)
}

/// One version of a conflicted file.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub text: Option<String>,
    pub exists: bool,
    pub binary: bool,
    pub too_large: bool,
    pub size: u64,
}

fn version_of(bytes: Option<Vec<u8>>, size: u64) -> Version {
    match bytes {
        None if size > MAX_BYTES => Version { exists: true, too_large: true, size, ..Default::default() },
        None => Version::default(),
        Some(b) => {
            let binary = is_binary(&b);
            Version { text: (!binary).then(|| String::from_utf8_lossy(&b).into_owned()), exists: true, binary, size, too_large: false }
        }
    }
}

fn stage(dir: &Path, n: u8, path: &str) -> Version {
    let spec = format!(":{n}:{path}");
    let Ok(size) = git::run(dir, &["cat-file", "-s", &spec]).map(|s| s.trim().parse::<u64>().unwrap_or(0)) else { return Version::default() };
    if size > MAX_BYTES {
        return version_of(None, size);
    }
    version_of(git::run_bytes(dir, &["cat-file", "blob", &spec]).ok(), size)
}

/// Who "ours" and "theirs" are in this operation (they swap in a rebase).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Labels {
    pub ours: String,
    pub theirs: String,
    pub base: String,
}

fn describe_commit(dir: &Path, rev: &str) -> Option<String> {
    let sha = git::run(dir, &["rev-parse", "--verify", "--quiet", rev]).ok()?.trim().to_string();
    if sha.is_empty() {
        return None;
    }
    let name = git::run(dir, &["name-rev", "--name-only", "--no-undefined", "--refs=refs/heads/*", "--refs=refs/remotes/*", &sha])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains('~') && !s.contains('^'));
    let subject = git::run(dir, &["log", "-1", "--format=%s", &sha]).unwrap_or_default().trim().to_string();
    Some(match name {
        Some(n) => format!("{n} ({})", &sha[..7.min(sha.len())]),
        None => format!("{} {subject}", &sha[..7.min(sha.len())]),
    })
}

fn read_git_file(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(git::git_path(dir, name).ok()?).ok().map(|s| s.trim().to_string())
}

pub fn labels(dir: &Path, op: Option<Operation>) -> Labels {
    let head = compare::current_branch(dir).map(|b| format!("{b} (HEAD)")).unwrap_or_else(|| "HEAD".into());
    match op {
        Some(Operation::Merge) => Labels { ours: head, theirs: describe_commit(dir, "MERGE_HEAD").unwrap_or_else(|| "the merged branch".into()), base: "common ancestor".into() },
        Some(Operation::Rebase) => {
            let onto = read_git_file(dir, "rebase-merge/onto").or_else(|| read_git_file(dir, "rebase-apply/onto"));
            let branch = read_git_file(dir, "rebase-merge/head-name").map(|h| h.trim_start_matches("refs/heads/").to_string());
            Labels {
                ours: format!("{} — being rebased onto", onto.as_deref().and_then(|o| describe_commit(dir, o)).unwrap_or_else(|| "upstream".into())),
                theirs: format!("{} — your commit{}", describe_commit(dir, "REBASE_HEAD").unwrap_or_else(|| "the commit".into()), branch.map(|b| format!(" on {b}")).unwrap_or_default()),
                base: "parent of your commit".into(),
            }
        }
        Some(Operation::CherryPick) => Labels { ours: head, theirs: describe_commit(dir, "CHERRY_PICK_HEAD").unwrap_or_else(|| "the picked commit".into()), base: "parent of the picked commit".into() },
        Some(Operation::Revert) => Labels { ours: head, theirs: format!("revert of {}", describe_commit(dir, "REVERT_HEAD").unwrap_or_else(|| "the commit".into())), base: "the reverted commit".into() },
        None => Labels { ours: format!("{head} — updated upstream"), theirs: "stashed changes".into(), base: "where the stash was made".into() },
    }
}

/// Everything the editor shows for one conflicted file.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub path: String,
    pub description: String,
    pub base: Version,
    pub ours: Version,
    pub theirs: Version,
    /// The working-tree file (with git's conflict markers).
    pub result: Version,
    pub binary: bool,
    pub labels: Labels,
    pub operation: Option<Operation>,
    /// No longer unmerged (already marked resolved).
    pub resolved: bool,
}

pub fn read(dir: &Path, path: &str) -> GitResult<Conflict> {
    check_path(path)?;
    let all = list(dir)?;
    let entry = all.iter().find(|f| f.path == path);
    let abs = crate::fsutil::safe_join(dir, path).map_err(|e| e.to_string())?;
    let result = match std::fs::metadata(&abs) {
        Ok(m) if m.len() > MAX_BYTES => version_of(None, m.len()),
        Ok(m) => version_of(std::fs::read(&abs).ok(), m.len()),
        Err(_) => Version::default(),
    };
    let (base, ours, theirs) = (stage(dir, 1, path), stage(dir, 2, path), stage(dir, 3, path));
    let binary = base.binary || ours.binary || theirs.binary || result.binary;
    let op = ops::operation(dir);
    Ok(Conflict {
        path: path.to_string(),
        description: entry.map(|e| e.description.clone()).unwrap_or_else(|| "resolved".into()),
        base,
        ours,
        theirs,
        result,
        binary,
        labels: labels(dir, op),
        operation: op,
        resolved: entry.is_none(),
    })
}

/// Writes the resolved text to the working-tree file and, with `mark_resolved`, stages it.
pub fn save(dir: &Path, path: &str, content: &str, mark_resolved: bool) -> GitResult<()> {
    check_path(path)?;
    let abs = crate::fsutil::safe_join(dir, path).map_err(|e| e.to_string())?;
    git::mutate(dir, || {
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&abs, content.as_bytes()).map_err(|e| format!("{}: {e}", abs.display()))?;
        if mark_resolved {
            git::run_env(dir, &["add", "--", path], &literal_env())?;
        }
        Ok(())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Pick {
    Ours,
    Theirs,
}

/// Resolves the whole file with one side (text or binary). A side that deleted the file resolves
/// it as deleted.
pub fn take(dir: &Path, path: &str, side: Pick) -> GitResult<()> {
    check_path(path)?;
    let f = list(dir)?.into_iter().find(|f| f.path == path).ok_or_else(|| format!("{path} isn't conflicted"))?;
    let present = match side {
        Pick::Ours => f.ours,
        Pick::Theirs => f.theirs,
    };
    git::mutate(dir, || {
        if present {
            git::run_env(dir, &["checkout", if side == Pick::Ours { "--ours" } else { "--theirs" }, "--", path], &literal_env())?;
            git::run_env(dir, &["add", "--", path], &literal_env())?;
        } else {
            git::run_env(dir, &["rm", "-q", "--", path], &literal_env())?;
        }
        Ok(())
    })
}

/// Resolves the file as deleted (`git rm`).
pub fn remove(dir: &Path, path: &str) -> GitResult<()> {
    check_path(path)?;
    git::mutate(dir, || git::run_env(dir, &["rm", "-q", "--", path], &literal_env()).map(|_| ()))
}

/// Puts git's conflict markers back into the file (`git checkout --conflict=zdiff3`; `diff3` on
/// Git before 2.35). Only while the file is still unmerged.
pub fn recreate(dir: &Path, path: &str) -> GitResult<()> {
    check_path(path)?;
    if !list(dir)?.iter().any(|f| f.path == path) {
        return Err(format!("{path} was already marked resolved — its conflict can't be re-created"));
    }
    let style = if git::version_at_least(2, 35) { "--conflict=zdiff3" } else { "--conflict=diff3" };
    git::mutate(dir, || git::run_env(dir, &["checkout", style, "--", path], &literal_env()).map(|_| ()))
}

/// `git add` — marks files resolved.
pub fn mark_resolved(dir: &Path, paths: &[String]) -> GitResult<()> {
    for p in paths {
        check_path(p)?;
    }
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    git::mutate(dir, || git::run_env(dir, &args, &literal_env()).map(|_| ()))
}

#[cfg(test)]
#[path = "conflicts_tests.rs"]
mod tests;
