//! Worktrees: list (`git worktree list --porcelain`), add (a new branch, an existing one, or a
//! detached commit), lock/unlock, remove, prune. Operations on an existing worktree take its path
//! and act only when it is one `git worktree list` reports for this repository.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::compare;
use super::git::{self, GitResult};

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub head: Option<String>,
    /// Short branch name (`None`: detached or bare).
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    /// Locked, with the reason when one was given (`Some("")` without).
    pub locked: Option<String>,
    /// Its folder is gone (`git worktree prune` would remove it), with git's reason.
    pub prunable: Option<String>,
    /// The main work tree (listed first).
    pub main: bool,
    /// The work tree this list was read from.
    pub current: bool,
}

/// Parses `git worktree list --porcelain` (with `-z`: NUL-separated lines; without: `\n`).
pub fn parse_porcelain(out: &str, z: bool) -> Vec<Worktree> {
    let sep = if z { '\0' } else { '\n' };
    let mut list: Vec<Worktree> = Vec::new();
    let mut cur: Option<Worktree> = None;
    for line in out.split(sep) {
        if line.is_empty() {
            if let Some(w) = cur.take() {
                list.push(w);
            }
            continue;
        }
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        if k == "worktree" {
            if let Some(w) = cur.take() {
                list.push(w);
            }
            cur = Some(Worktree { path: v.to_string(), ..Default::default() });
            continue;
        }
        let Some(w) = cur.as_mut() else { continue };
        match k {
            "HEAD" => w.head = Some(v.to_string()),
            "branch" => w.branch = Some(v.trim_start_matches("refs/heads/").to_string()),
            "detached" => w.detached = true,
            "bare" => w.bare = true,
            "locked" => w.locked = Some(v.to_string()),
            "prunable" => w.prunable = Some(v.to_string()),
            _ => {}
        }
    }
    if let Some(w) = cur {
        list.push(w);
    }
    if let Some(first) = list.first_mut() {
        first.main = true;
    }
    list
}

fn same(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => {
            let n = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
            n(a) == n(b)
        }
    }
}

pub fn list(dir: &Path) -> GitResult<Vec<Worktree>> {
    let z = git::version_at_least(2, 36);
    let out = if z { git::run(dir, &["worktree", "list", "--porcelain", "-z"])? } else { git::run(dir, &["worktree", "list", "--porcelain"])? };
    let mut list = parse_porcelain(&out, z);
    for w in &mut list {
        w.current = same(Path::new(&w.path), dir);
    }
    Ok(list)
}

/// The listed worktree at `path` (anything else is refused).
fn listed(dir: &Path, path: &str) -> GitResult<Worktree> {
    list(dir)?.into_iter().find(|w| same(Path::new(&w.path), Path::new(path))).ok_or_else(|| format!("{path} isn't a worktree of this repository"))
}

/// What a new worktree checks out.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Checkout {
    /// A new branch `name` starting at `from`.
    NewBranch { name: String, from: String },
    /// An existing local branch (not checked out anywhere else).
    Branch { name: String },
    /// A commit, detached.
    Detached { rev: String },
}

/// `<repo folder>-<branch>` next to the repository (a suggestion for the add dialog).
pub fn suggest_path(dir: &Path, branch: &str) -> PathBuf {
    let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
    let slug: String = branch.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
    dir.parent().unwrap_or(dir).join(format!("{name}-{}", slug.trim_matches('-')))
}

/// `git worktree add`. `path` must be absolute and not an existing non-empty folder.
pub fn add(dir: &Path, path: &str, checkout: &Checkout) -> GitResult<Worktree> {
    let p = Path::new(path.trim());
    if !p.is_absolute() || path.trim().starts_with('-') {
        return Err("choose an absolute folder for the worktree".into());
    }
    if p.exists() && std::fs::read_dir(p).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!("{} already exists and isn't empty", p.display()));
    }
    let target = p.to_string_lossy().to_string();
    git::mutate(dir, || {
        match checkout {
            Checkout::NewBranch { name, from } => {
                if !git::valid_branch(name) {
                    return Err(format!("“{name}” isn't a valid branch name"));
                }
                let sha = compare::resolve(dir, from)?;
                git::run(dir, &["worktree", "add", "-b", name, "--", &target, &sha])?;
            }
            Checkout::Branch { name } => {
                if !git::valid_branch(name) {
                    return Err(format!("“{name}” isn't a valid branch name"));
                }
                git::run(dir, &["worktree", "add", "--", &target, name])?;
            }
            Checkout::Detached { rev } => {
                let sha = compare::resolve(dir, rev)?;
                git::run(dir, &["worktree", "add", "--detach", "--", &target, &sha])?;
            }
        }
        Ok(())
    })?;
    listed(dir, &target)
}

pub fn lock(dir: &Path, path: &str, reason: Option<&str>) -> GitResult<()> {
    let w = listed(dir, path)?;
    if w.main {
        return Err("the main work tree can't be locked".into());
    }
    git::mutate(dir, || match reason.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => git::run(dir, &["worktree", "lock", "--reason", r, "--", &w.path]).map(|_| ()),
        None => git::run(dir, &["worktree", "lock", "--", &w.path]).map(|_| ()),
    })
}

pub fn unlock(dir: &Path, path: &str) -> GitResult<()> {
    let w = listed(dir, path)?;
    git::mutate(dir, || git::run(dir, &["worktree", "unlock", "--", &w.path]).map(|_| ()))
}

/// `git worktree remove` (git refuses with uncommitted changes, and a locked one). With `force`
/// the folder — uncommitted and untracked files included — goes to the OS trash instead of being
/// deleted, then git forgets the worktree (`git worktree prune`). The branch it had checked out
/// stays.
pub fn remove(dir: &Path, path: &str, force: bool) -> GitResult<()> {
    let w = listed(dir, path)?;
    if w.main {
        return Err("the main work tree can't be removed".into());
    }
    if w.current {
        return Err("open the repository from another work tree to remove this one".into());
    }
    if w.locked.is_some() {
        return Err(format!("{} is locked — unlock it first", w.path));
    }
    git::mutate(dir, || {
        if !force {
            return git::run(dir, &["worktree", "remove", "--", w.path.as_str()]).map(|_| ());
        }
        let folder = Path::new(&w.path);
        if folder.exists() {
            super::to_trash(folder)?;
        }
        git::run(dir, &["worktree", "prune"])?;
        if list(dir)?.iter().any(|x| same(Path::new(&x.path), folder)) {
            return Err(format!("{} went to the trash but git still lists the worktree — run Prune", w.path));
        }
        Ok(())
    })
}

/// `git worktree prune`: forgets worktrees whose folders are gone. Returns what it pruned.
pub fn prune(dir: &Path) -> GitResult<String> {
    git::mutate(dir, || {
        let out = git::output(dir, &["worktree", "prune", "-v"])?;
        if out.status.success() { Ok(git::combined(&out)) } else { Err(git::combined(&out)) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::testgit::repo;

    #[test]
    fn parses_both_formats() {
        let z = "worktree /r\0HEAD abc\0branch refs/heads/main\0\0worktree /r-x\0HEAD def\0detached\0locked moving disks\0\0worktree /gone\0HEAD 123\0branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0";
        let l = parse_porcelain(z, true);
        assert_eq!(l.len(), 3);
        assert!(l[0].main && !l[1].main);
        assert_eq!(l[0].branch.as_deref(), Some("main"));
        assert!(l[1].detached);
        assert_eq!(l[1].locked.as_deref(), Some("moving disks"));
        assert!(l[2].prunable.is_some());
        let nl = z.replace('\0', "\n");
        assert_eq!(parse_porcelain(&nl, false), l);
    }

    #[test]
    fn add_lock_remove_prune() {
        let Some(r) = repo() else { return };
        r.write("a.txt", "a\n");
        r.commit("base");
        r.git(&["branch", "existing"]);
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("wt-new");
        let w = add(r.root(), &p1.display().to_string(), &Checkout::NewBranch { name: "feature/wt".into(), from: "HEAD".into() }).unwrap();
        assert_eq!(w.branch.as_deref(), Some("feature/wt"));
        assert!(p1.join("a.txt").exists());
        let p2 = tmp.path().join("wt-existing");
        add(r.root(), &p2.display().to_string(), &Checkout::Branch { name: "existing".into() }).unwrap();
        let p3 = tmp.path().join("wt-detached");
        let d = add(r.root(), &p3.display().to_string(), &Checkout::Detached { rev: "HEAD".into() }).unwrap();
        assert!(d.detached);
        assert!(add(r.root(), "relative/path", &Checkout::Detached { rev: "HEAD".into() }).is_err());
        assert!(add(r.root(), &p1.display().to_string(), &Checkout::Detached { rev: "HEAD".into() }).is_err(), "not empty");
        let l = list(r.root()).unwrap();
        assert_eq!(l.len(), 4);
        assert!(l[0].main && l[0].current);
        lock(r.root(), &p1.display().to_string(), Some("keep")).unwrap();
        assert_eq!(list(r.root()).unwrap().iter().find(|w| w.branch.as_deref() == Some("feature/wt")).unwrap().locked.as_deref(), Some("keep"));
        assert!(remove(r.root(), &p1.display().to_string(), false).is_err(), "locked");
        unlock(r.root(), &p1.display().to_string()).unwrap();
        remove(r.root(), &p1.display().to_string(), false).unwrap();
        assert!(!p1.exists());
        // Dirty: refused without force.
        std::fs::write(p2.join("a.txt"), "dirty\n").unwrap();
        assert!(remove(r.root(), &p2.display().to_string(), false).is_err());
        remove(r.root(), &p2.display().to_string(), true).unwrap();
        assert!(remove(r.root(), &r.root().display().to_string(), false).is_err(), "main");
        assert!(lock(r.root(), "/not/a/worktree", None).is_err());
        // A worktree whose folder vanished is pruned.
        std::fs::remove_dir_all(&p3).unwrap();
        assert!(list(r.root()).unwrap().iter().any(|w| w.prunable.is_some()));
        prune(r.root()).unwrap();
        assert_eq!(list(r.root()).unwrap().len(), 1);
        assert!(suggest_path(r.root(), "feature/x").to_string_lossy().ends_with("repo-feature-x"));
    }
}
