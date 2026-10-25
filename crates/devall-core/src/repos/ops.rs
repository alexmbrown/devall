//! Ref listing and the commit/ref operations of the graph's context menu (check out, branch or
//! tag here, cherry-pick, revert) plus the merge/rebase/cherry-pick/revert-in-progress state.
//! Every mutation goes through [`git::mutate`] (one writer per repository). Nothing here pushes
//! or touches remotes.

use std::path::Path;

use serde::Serialize;

use super::compare::{self, RefKind};
use super::git::{self, GitResult};

/// A branch, remote branch or tag.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefEntry {
    /// Short name (`main`, `origin/main`, `v1.2`).
    pub name: String,
    pub refname: String,
    /// The commit it points at (tags peeled).
    pub sha: String,
    pub kind: RefKind,
    pub upstream: Option<String>,
    /// `ahead 1, behind 2` (local branches with an upstream).
    pub track: Option<String>,
    /// Unix seconds (commit or tag date).
    pub time: i64,
    pub subject: String,
    /// The checked-out branch.
    pub current: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefList {
    pub current: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    /// Most recently updated first.
    pub refs: Vec<RefEntry>,
}

pub fn refs(dir: &Path) -> GitResult<RefList> {
    let out = git::run(
        dir,
        &[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname)%1f%(objectname)%1f%(*objectname)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(creatordate:unix)%1f%(contents:subject)%1f%(HEAD)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    let mut list = RefList { head: git::run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()), ..Default::default() };
    for line in out.lines() {
        let f: Vec<&str> = line.split('\x1f').collect();
        if f.len() < 8 {
            continue;
        }
        let refname = f[0];
        let (kind, name) = if let Some(n) = refname.strip_prefix("refs/heads/") {
            (RefKind::Branch, n)
        } else if let Some(n) = refname.strip_prefix("refs/remotes/") {
            if n.ends_with("/HEAD") {
                continue;
            }
            (RefKind::Remote, n)
        } else if let Some(n) = refname.strip_prefix("refs/tags/") {
            (RefKind::Tag, n)
        } else {
            continue;
        };
        let current = f[7].trim() == "*";
        if current {
            list.current = Some(name.to_string());
        }
        list.refs.push(RefEntry {
            name: name.to_string(),
            refname: refname.to_string(),
            sha: if f[2].is_empty() { f[1] } else { f[2] }.to_string(),
            kind,
            upstream: Some(f[3].to_string()).filter(|s| !s.is_empty()),
            track: Some(f[4].to_string()).filter(|s| !s.is_empty()),
            time: f[5].trim().parse().unwrap_or(0),
            subject: f[6].to_string(),
            current,
        });
    }
    list.detached = list.current.is_none() && list.head.is_some();
    Ok(list)
}

fn check_name(name: &str, what: &str) -> GitResult<()> {
    if git::valid_branch(name) { Ok(()) } else { Err(format!("“{name}” isn't a valid {what} name")) }
}

/// `git branch <name> <at>`, then switches to it when `checkout`.
pub fn create_branch(dir: &Path, name: &str, at: &str, checkout: bool) -> GitResult<()> {
    check_name(name, "branch")?;
    let sha = compare::resolve(dir, at)?;
    git::mutate(dir, || {
        git::run(dir, &["branch", "--no-track", name, &sha])?;
        if checkout {
            git::run(dir, &["switch", name])?;
        }
        Ok(())
    })
}

/// A lightweight tag, or an annotated one when `message` is given.
pub fn create_tag(dir: &Path, name: &str, at: &str, message: Option<&str>) -> GitResult<()> {
    check_name(name, "tag")?;
    let sha = compare::resolve(dir, at)?;
    git::mutate(dir, || match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => git::run(dir, &["tag", "-a", name, "-m", m, &sha]).map(|_| ()),
        None => git::run(dir, &["tag", name, &sha]).map(|_| ()),
    })
}

/// Checks out a commit without a branch (detached HEAD). Git refuses when uncommitted changes
/// would be overwritten.
pub fn checkout_detached(dir: &Path, rev: &str) -> GitResult<()> {
    let sha = compare::resolve(dir, rev)?;
    git::mutate(dir, || git::run(dir, &["switch", "--detach", &sha]).map(|_| ()))
}

/// Checks out a local branch, or a remote branch as a new local branch tracking it (an existing
/// local branch of that name is checked out instead).
pub fn checkout_ref(dir: &Path, name: &str, kind: RefKind) -> GitResult<()> {
    check_name(name, "branch")?;
    git::mutate(dir, || match kind {
        RefKind::Branch => git::run(dir, &["switch", name]).map(|_| ()),
        RefKind::Remote => {
            let remotes = compare::remotes(dir);
            let local = remotes.iter().filter(|r| name.starts_with(&format!("{r}/"))).max_by_key(|r| r.len()).map(|r| &name[r.len() + 1..]).ok_or_else(|| format!("{name} isn't a remote branch"))?;
            if git::run(dir, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{local}")]).is_ok() {
                git::run(dir, &["switch", local]).map(|_| ())
            } else {
                git::run(dir, &["switch", "--track", name]).map(|_| ())
            }
        }
        _ => Err(format!("{name} isn't a branch")),
    })
}

/// What a cherry-pick or revert did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    /// Stopped with conflicts (resolve them, then continue or abort).
    pub conflicts: bool,
    pub message: String,
}

fn is_merge(dir: &Path, sha: &str) -> bool {
    git::run(dir, &["rev-list", "--parents", "-n1", sha]).map(|s| s.split_whitespace().count() > 2).unwrap_or(false)
}

fn pick(dir: &Path, cmd: &str, rev: &str) -> GitResult<OpResult> {
    let sha = compare::resolve(dir, rev)?;
    git::mutate(dir, || {
        let mut args = vec![cmd];
        if cmd == "revert" {
            args.push("--no-edit");
        }
        if is_merge(dir, &sha) {
            args.extend(["-m", "1"]);
        }
        args.push(&sha);
        let out = git::output(dir, &args)?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
        if out.status.success() {
            Ok(OpResult { conflicts: false, message: text })
        } else if operation(dir).is_some() {
            Ok(OpResult { conflicts: true, message: text })
        } else {
            Err(text)
        }
    })
}

/// `git cherry-pick <sha>` (`-m 1` for a merge commit).
pub fn cherry_pick(dir: &Path, rev: &str) -> GitResult<OpResult> {
    pick(dir, "cherry-pick", rev)
}

/// `git revert --no-edit <sha>` (`-m 1` for a merge commit).
pub fn revert(dir: &Path, rev: &str) -> GitResult<OpResult> {
    pick(dir, "revert", rev)
}

/// A multi-step operation git has paused (usually on conflicts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl Operation {
    fn command(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
        }
    }
}

fn git_path_exists(dir: &Path, name: &str) -> bool {
    git::run(dir, &["rev-parse", "--git-path", name]).map(|p| {
        let p = p.trim();
        if Path::new(p).is_absolute() { Path::new(p).exists() } else { dir.join(p).exists() }
    })
    .unwrap_or(false)
}

/// The operation in progress, if any.
pub fn operation(dir: &Path) -> Option<Operation> {
    if git_path_exists(dir, "rebase-merge") || git_path_exists(dir, "rebase-apply") {
        Some(Operation::Rebase)
    } else if git_path_exists(dir, "CHERRY_PICK_HEAD") {
        Some(Operation::CherryPick)
    } else if git_path_exists(dir, "REVERT_HEAD") {
        Some(Operation::Revert)
    } else if git_path_exists(dir, "MERGE_HEAD") {
        Some(Operation::Merge)
    } else {
        None
    }
}

/// `git <op> --abort`: back to where the operation started.
pub fn abort(dir: &Path) -> GitResult<()> {
    let op = operation(dir).ok_or("nothing to abort")?;
    git::mutate(dir, || git::run(dir, &[op.command(), "--abort"]).map(|_| ()))
}

/// `git <op> --continue` with git's prepared message (no editor).
pub fn continue_op(dir: &Path) -> GitResult<String> {
    let op = operation(dir).ok_or("nothing to continue")?;
    git::mutate(dir, || {
        let out = git::command(dir).args([op.command(), "--continue"]).env("GIT_EDITOR", "true").stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
        if out.status.success() { Ok(text) } else { Err(text) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::testgit::repo;

    #[test]
    fn lists_refs_and_creates_branches_and_tags() {
        let Some(r) = repo() else { return };
        r.write("a.txt", "a\n");
        let first = r.commit("first");
        r.write("a.txt", "b\n");
        r.commit("second");
        create_branch(r.root(), "fix/one", &first, false).unwrap();
        create_tag(r.root(), "v1", &first, Some("release one")).unwrap();
        create_tag(r.root(), "light", "HEAD", None).unwrap();
        assert!(create_branch(r.root(), "bad name", "HEAD", false).is_err());
        assert!(create_tag(r.root(), "-x", "HEAD", None).is_err());
        let l = refs(r.root()).unwrap();
        assert_eq!(l.current.as_deref(), Some("main"));
        assert!(!l.detached);
        let find = |n: &str| l.refs.iter().find(|x| x.name == n).unwrap();
        assert_eq!(find("fix/one").sha, first);
        assert_eq!(find("v1").sha, first, "annotated tags are peeled");
        assert_eq!(find("v1").kind, RefKind::Tag);
        assert!(find("main").current);
        create_branch(r.root(), "work", "HEAD~1", true).unwrap();
        assert_eq!(compare::current_branch(r.root()).as_deref(), Some("work"));
        checkout_detached(r.root(), "main").unwrap();
        assert!(refs(r.root()).unwrap().detached);
        checkout_ref(r.root(), "main", RefKind::Branch).unwrap();
        assert_eq!(compare::current_branch(r.root()).as_deref(), Some("main"));
    }

    #[test]
    fn cherry_pick_revert_and_conflicts() {
        let Some(r) = repo() else { return };
        r.write("a.txt", "one\n");
        r.commit("base");
        r.git(&["switch", "-q", "-c", "feature"]);
        r.write("f.txt", "f\n");
        let f = r.commit("add f");
        r.write("a.txt", "feature\n");
        let clash = r.commit("edit a on feature");
        r.git(&["switch", "-q", "main"]);
        let ok = cherry_pick(r.root(), &f).unwrap();
        assert!(!ok.conflicts);
        assert!(r.path("f.txt").exists());
        let picked = r.sha("HEAD");
        revert(r.root(), &picked).unwrap();
        assert!(!r.path("f.txt").exists());
        r.write("a.txt", "main\n");
        r.commit("edit a on main");
        let res = cherry_pick(r.root(), &clash).unwrap();
        assert!(res.conflicts);
        assert_eq!(operation(r.root()), Some(Operation::CherryPick));
        abort(r.root()).unwrap();
        assert_eq!(operation(r.root()), None);
        assert_eq!(String::from_utf8(r.read("a.txt")).unwrap(), "main\n");
        // Continue after resolving.
        cherry_pick(r.root(), &clash).unwrap();
        r.write("a.txt", "resolved\n");
        r.git(&["add", "a.txt"]);
        continue_op(r.root()).unwrap();
        assert_eq!(operation(r.root()), None);
        assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "edit a on feature");
    }

    #[test]
    fn checks_out_remote_branches_as_tracking_branches() {
        let Some(up) = repo() else { return };
        up.write("a.txt", "a\n");
        up.commit("base");
        up.git(&["branch", "topic"]);
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("c");
        git::run(tmp.path(), &["clone", "-q", &up.root().display().to_string(), &clone.display().to_string()]).unwrap();
        checkout_ref(&clone, "origin/topic", RefKind::Remote).unwrap();
        assert_eq!(compare::current_branch(&clone).as_deref(), Some("topic"));
        let l = refs(&clone).unwrap();
        assert_eq!(l.refs.iter().find(|x| x.name == "topic").and_then(|x| x.upstream.as_deref()), Some("origin/topic"));
        assert!(!l.refs.iter().any(|x| x.name.ends_with("/HEAD")));
    }
}
