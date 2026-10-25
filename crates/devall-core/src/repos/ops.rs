//! Ref listing and the operations of Source control's menus: branches (create, rename, delete,
//! upstream, fast-forward), tags, checkout, merge, rebase, reset, cherry-pick, revert, the
//! working-tree operations of Changes (stage, unstage, discard, commit, pull) and push — plus the
//! merge/rebase/cherry-pick/revert-in-progress state.
//!
//! Every local mutation goes through [`undo::journaled`] (one writer per repository via
//! [`git::mutate`], journaled for undo when a journal is given). Operations that talk to a remote
//! (push, deleting a remote branch or tag) only run on explicit user actions and can't be undone
//! locally — the UI says so before running them. Their output has credentials embedded in remote
//! URLs scrubbed.

use std::ffi::OsStr;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::compare::{self, RefKind};
use super::git::{self, GitResult};
use super::undo::{self, Journal, Kind, Op, Scope};

/// The journal an operation is recorded in (`None`: not journaled — the CLI, tests).
pub type Undo<'a> = Option<&'a Journal>;

fn journaled<T>(dir: &Path, j: Undo<'_>, kind: Kind, label: String, scope: Scope, f: impl FnOnce() -> GitResult<T>) -> GitResult<T> {
    undo::journaled(dir, j, Op::new(kind, label, scope), f)
}

/// Pathspecs taken literally (file names with `*` or `[` are just names).
const LITERAL: (&str, &str) = ("GIT_LITERAL_PATHSPECS", "1");

fn run_literal(dir: &Path, args: &[&str]) -> GitResult<String> {
    git::run_env(dir, args, &[(LITERAL.0, OsStr::new(LITERAL.1))])
}

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
    pub ahead: u32,
    pub behind: u32,
    /// The upstream branch no longer exists on the remote (`[gone]`).
    pub gone: bool,
    /// Unix seconds (commit or tag date).
    pub time: i64,
    pub subject: String,
    /// The checked-out branch.
    pub current: bool,
    /// Checked out in another worktree (its path).
    pub worktree: Option<String>,
    /// An annotated tag.
    pub annotated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefList {
    pub current: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    /// Most recently updated first.
    pub refs: Vec<RefEntry>,
    pub remotes: Vec<String>,
}

/// `ahead 1, behind 2` / `gone` → (ahead, behind, gone).
pub fn parse_track(t: &str) -> (u32, u32, bool) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in t.split(',').map(str::trim) {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind, t.trim() == "gone")
}

pub fn refs(dir: &Path) -> GitResult<RefList> {
    let out = git::run(
        dir,
        &[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname)%1f%(objectname)%1f%(*objectname)%1f%(upstream:short)%1f%(upstream:track,nobracket)%1f%(creatordate:unix)%1f%(contents:subject)%1f%(HEAD)%1f%(worktreepath)%1f%(objecttype)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    let here = std::fs::canonicalize(dir).ok();
    let mut list = RefList { head: git::run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()), remotes: compare::remotes(dir), ..Default::default() };
    for line in out.lines() {
        let f: Vec<&str> = line.split('\x1f').collect();
        if f.len() < 10 {
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
        let track = Some(f[4].to_string()).filter(|s| !s.is_empty());
        let (ahead, behind, gone) = track.as_deref().map(parse_track).unwrap_or_default();
        let wt = f[8].trim();
        let elsewhere = !wt.is_empty() && !current && std::fs::canonicalize(wt).ok() != here;
        list.refs.push(RefEntry {
            name: name.to_string(),
            refname: refname.to_string(),
            sha: if f[2].is_empty() { f[1] } else { f[2] }.to_string(),
            kind,
            upstream: Some(f[3].to_string()).filter(|s| !s.is_empty()),
            track,
            ahead,
            behind,
            gone,
            time: f[5].trim().parse().unwrap_or(0),
            subject: f[6].to_string(),
            current,
            worktree: elsewhere.then(|| wt.to_string()),
            annotated: f[9].trim() == "tag",
        });
    }
    list.detached = list.current.is_none() && list.head.is_some();
    Ok(list)
}

fn check_name(name: &str, what: &str) -> GitResult<()> {
    if git::valid_branch(name) { Ok(()) } else { Err(format!("“{name}” isn't a valid {what} name")) }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn branch_exists(dir: &Path, name: &str) -> bool {
    git::run(dir, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")]).is_ok()
}

/// `origin/feature/x` → (`origin`, `feature/x`), matching the longest remote name.
pub fn split_remote_branch(dir: &Path, name: &str) -> GitResult<(String, String)> {
    compare::remotes(dir)
        .into_iter()
        .filter(|r| name.starts_with(&format!("{r}/")))
        .max_by_key(|r| r.len())
        .map(|r| {
            let b = name[r.len() + 1..].to_string();
            (r, b)
        })
        .filter(|(_, b)| !b.is_empty())
        .ok_or_else(|| format!("{name} isn't a remote branch"))
}

/// Removes credentials embedded in URLs anywhere in git's output (`https://user:token@host`).
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail.find(|c: char| c.is_whitespace() || c == '/' || c == '\'' || c == '"').unwrap_or(tail.len());
        match tail[..end].rfind('@') {
            Some(at) => rest = &tail[at + 1..],
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// Runs a network operation (push, fetch, pull): git's output with credentials scrubbed, both on
/// success and failure.
pub(crate) fn network(dir: &Path, args: &[&str]) -> GitResult<String> {
    let out = git::output(dir, args)?;
    let text = scrub(&git::combined(&out));
    if out.status.success() { Ok(text) } else { Err(text) }
}

// ---------- branches ----------

/// `git branch <name> <at>`, then switches to it when `checkout`.
pub fn create_branch(dir: &Path, j: Undo<'_>, name: &str, at: &str, checkout: bool) -> GitResult<()> {
    check_name(name, "branch")?;
    let sha = compare::resolve(dir, at)?;
    let (kind, scope, label) = if checkout {
        (Kind::Checkout, Scope::full(), format!("Create branch {name} at {} and check it out", short(&sha)))
    } else {
        (Kind::BranchCreate, Scope::refs(), format!("Create branch {name} at {}", short(&sha)))
    };
    journaled(dir, j, kind, label, scope, || {
        git::run(dir, &["branch", "--no-track", name, &sha])?;
        if checkout {
            git::run(dir, &["switch", name])?;
        }
        Ok(())
    })
}

/// `git branch -m <old> <new>` (its upstream and reflog move along).
pub fn rename_branch(dir: &Path, j: Undo<'_>, old: &str, new: &str) -> GitResult<()> {
    check_name(old, "branch")?;
    check_name(new, "branch")?;
    if !branch_exists(dir, old) {
        return Err(format!("no branch {old}"));
    }
    journaled(dir, j, Kind::BranchRename, format!("Rename branch {old} to {new}"), Scope::refs(), || git::run(dir, &["branch", "-m", old, new]).map(|_| ()))
}

/// `git branch -d` (refuses unmerged work) or, with `force`, `-D`. The tip stays in the undo
/// journal.
pub fn delete_branch(dir: &Path, j: Undo<'_>, name: &str, force: bool) -> GitResult<()> {
    check_name(name, "branch")?;
    let sha = compare::resolve(dir, &format!("refs/heads/{name}")).map_err(|_| format!("no branch {name}"))?;
    journaled(dir, j, Kind::BranchDelete, format!("Delete branch {name} (was {})", short(&sha)), Scope::refs(), || git::run(dir, &["branch", if force { "-D" } else { "-d" }, name]).map(|_| ()))
}

/// Commits on `name` that no other branch, tag or remote branch has (what a forced delete loses
/// from every ref — the undo journal still keeps them).
pub fn unmerged_count(dir: &Path, name: &str) -> GitResult<u32> {
    check_name(name, "branch")?;
    let out = git::run(dir, &["rev-list", "--count", &format!("refs/heads/{name}"), "--not", &format!("--exclude={name}"), "--branches", "--tags", "--remotes"])?;
    Ok(out.trim().parse().unwrap_or(0))
}

/// `git push <remote> --delete <branch>` for a remote branch (`origin/feature`). Network; can't be
/// undone locally (the UI confirms naming the remote branch).
pub fn delete_remote_branch(dir: &Path, remote_branch: &str) -> GitResult<String> {
    let (remote, branch) = split_remote_branch(dir, remote_branch)?;
    check_name(&branch, "branch")?;
    git::mutate(dir, || network(dir, &["push", &remote, "--delete", &format!("refs/heads/{branch}")]))
}

/// Sets (`Some("origin/main")`) or removes the upstream of a local branch.
pub fn set_upstream(dir: &Path, j: Undo<'_>, branch: &str, upstream: Option<&str>) -> GitResult<()> {
    check_name(branch, "branch")?;
    if let Some(u) = upstream {
        check_name(u, "upstream")?;
    }
    let label = match upstream {
        Some(u) => format!("Set upstream of {branch} to {u}"),
        None => format!("Remove the upstream of {branch}"),
    };
    journaled(dir, j, Kind::SetUpstream, label, Scope::refs(), || match upstream {
        Some(u) => git::run(dir, &["branch", &format!("--set-upstream-to={u}"), branch]).map(|_| ()),
        None => git::run(dir, &["branch", "--unset-upstream", branch]).map(|_| ()),
    })
}

/// Fast-forwards `branch` to `from` (default: its upstream) — without checking it out
/// (`git fetch . <from>:<branch>`, refused when it isn't a fast-forward); the checked-out branch
/// uses `git merge --ff-only`.
pub fn fast_forward(dir: &Path, j: Undo<'_>, branch: &str, from: Option<&str>) -> GitResult<String> {
    check_name(branch, "branch")?;
    let src = match from {
        Some(f) => f.to_string(),
        None => git::run(dir, &["rev-parse", "--abbrev-ref", &format!("{branch}@{{upstream}}")]).map(|s| s.trim().to_string()).map_err(|_| format!("{branch} has no upstream to fast-forward to"))?,
    };
    let sha = compare::resolve(dir, &src)?;
    let current = compare::current_branch(dir).as_deref() == Some(branch);
    let label = format!("Fast-forward {branch} to {src}");
    if current {
        journaled(dir, j, Kind::FastForward, label, Scope::full(), || {
            let out = git::output(dir, &["merge", "--ff-only", &sha])?;
            if out.status.success() { Ok(git::combined(&out)) } else { Err(git::combined(&out)) }
        })
    } else {
        journaled(dir, j, Kind::FastForward, label, Scope::refs(), || {
            let out = git::output(dir, &["fetch", ".", &format!("{sha}:refs/heads/{branch}")])?;
            let text = git::combined(&out);
            if out.status.success() {
                Ok(text)
            } else if text.contains("non-fast-forward") || text.contains("rejected") {
                Err(format!("{branch} can't be fast-forwarded to {src}: they have diverged (merge or rebase instead)"))
            } else {
                Err(text)
            }
        })
    }
}

/// Points a branch that isn't checked out at another commit (`git branch -f`).
pub fn reset_branch(dir: &Path, j: Undo<'_>, branch: &str, rev: &str) -> GitResult<()> {
    check_name(branch, "branch")?;
    if compare::current_branch(dir).as_deref() == Some(branch) {
        return Err(format!("{branch} is checked out — reset it with Reset instead"));
    }
    let sha = compare::resolve(dir, rev)?;
    journaled(dir, j, Kind::Reset, format!("Move {branch} to {}", short(&sha)), Scope::refs(), || git::run(dir, &["branch", "-f", branch, &sha]).map(|_| ()))
}

// ---------- tags ----------

/// A lightweight tag, or an annotated one when `message` is given.
pub fn create_tag(dir: &Path, j: Undo<'_>, name: &str, at: &str, message: Option<&str>) -> GitResult<()> {
    check_name(name, "tag")?;
    let sha = compare::resolve(dir, at)?;
    journaled(dir, j, Kind::TagCreate, format!("Create tag {name} at {}", short(&sha)), Scope::refs(), || match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => git::run(dir, &["tag", "-a", name, "-m", m, &sha]).map(|_| ()),
        None => git::run(dir, &["tag", name, &sha]).map(|_| ()),
    })
}

pub fn delete_tag(dir: &Path, j: Undo<'_>, name: &str) -> GitResult<()> {
    check_name(name, "tag")?;
    journaled(dir, j, Kind::TagDelete, format!("Delete tag {name}"), Scope::refs(), || git::run(dir, &["tag", "-d", name]).map(|_| ()))
}

fn check_remote(dir: &Path, remote: &str) -> GitResult<()> {
    if compare::remotes(dir).iter().any(|r| r == remote) { Ok(()) } else { Err(format!("no remote “{remote}”")) }
}

/// `git push <remote> refs/tags/<name>`. Network.
pub fn push_tag(dir: &Path, remote: &str, name: &str) -> GitResult<String> {
    check_name(name, "tag")?;
    check_remote(dir, remote)?;
    git::mutate(dir, || network(dir, &["push", remote, &format!("refs/tags/{name}")]))
}

/// `git push <remote> --delete refs/tags/<name>`. Network; can't be undone locally.
pub fn delete_remote_tag(dir: &Path, remote: &str, name: &str) -> GitResult<String> {
    check_name(name, "tag")?;
    check_remote(dir, remote)?;
    git::mutate(dir, || network(dir, &["push", remote, "--delete", &format!("refs/tags/{name}")]))
}

// ---------- checkout ----------

/// Checks out a commit without a branch (detached HEAD). Git refuses when uncommitted changes
/// would be overwritten.
pub fn checkout_detached(dir: &Path, j: Undo<'_>, rev: &str) -> GitResult<()> {
    let sha = compare::resolve(dir, rev)?;
    journaled(dir, j, Kind::Checkout, format!("Check out {} (detached)", short(&sha)), Scope::full(), || git::run(dir, &["switch", "--detach", &sha]).map(|_| ()))
}

/// Checks out a local branch, or a remote branch as a new local branch tracking it (an existing
/// local branch of that name is checked out instead).
pub fn checkout_ref(dir: &Path, j: Undo<'_>, name: &str, kind: RefKind) -> GitResult<()> {
    check_name(name, "branch")?;
    match kind {
        RefKind::Branch => journaled(dir, j, Kind::Checkout, format!("Check out {name}"), Scope::full(), || git::run(dir, &["switch", name]).map(|_| ())),
        RefKind::Remote => {
            let (_, local) = split_remote_branch(dir, name)?;
            journaled(dir, j, Kind::Checkout, format!("Check out {name} as {local}"), Scope::full(), || {
                if branch_exists(dir, &local) { git::run(dir, &["switch", &local]).map(|_| ()) } else { git::run(dir, &["switch", "--track", name]).map(|_| ()) }
            })
        }
        _ => Err(format!("{name} isn't a branch")),
    }
}

/// Switches to `branch` (or creates it from HEAD with `create`), like the branch menu.
pub fn switch_branch(dir: &Path, j: Undo<'_>, branch: &str, create: bool) -> GitResult<()> {
    check_name(branch, "branch")?;
    let label = if create { format!("Create branch {branch} and check it out") } else { format!("Check out {branch}") };
    journaled(dir, j, Kind::Checkout, label, Scope::full(), || git::run(dir, &if create { vec!["switch", "-c", branch] } else { vec!["switch", branch] }).map(|_| ()))
}

// ---------- history-changing operations ----------

/// What a merge, rebase, cherry-pick, revert or stash apply did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    /// Stopped with conflicts (resolve them, then continue or abort).
    pub conflicts: bool,
    /// Stopped without conflicts (an interactive rebase's `edit`).
    #[serde(default)]
    pub paused: bool,
    pub message: String,
}

fn has_unmerged(dir: &Path) -> bool {
    git::run(dir, &["ls-files", "-u", "-z"]).is_ok_and(|s| !s.is_empty())
}

/// Turns git's exit into an [`OpResult`]: success, stopped (conflicts / edit) or an error.
pub(crate) fn op_result(dir: &Path, out: &std::process::Output) -> GitResult<OpResult> {
    let text = git::combined(out);
    let conflicts = has_unmerged(dir);
    if out.status.success() {
        let paused = operation(dir).is_some();
        Ok(OpResult { conflicts, paused: paused && !conflicts, message: text })
    } else if conflicts || operation(dir).is_some() {
        Ok(OpResult { conflicts, paused: !conflicts, message: text })
    } else {
        Err(text)
    }
}

fn is_merge(dir: &Path, sha: &str) -> bool {
    git::run(dir, &["rev-list", "--parents", "-n1", sha]).map(|s| s.split_whitespace().count() > 2).unwrap_or(false)
}

fn subject(dir: &Path, sha: &str) -> String {
    let s = git::run(dir, &["log", "-1", "--format=%s", sha]).unwrap_or_default().trim().to_string();
    if s.chars().count() > 50 { format!("{}…", s.chars().take(49).collect::<String>()) } else { s }
}

fn head_name(dir: &Path) -> String {
    compare::current_branch(dir).unwrap_or_else(|| "HEAD".into())
}

fn pick(dir: &Path, j: Undo<'_>, cmd: &str, rev: &str) -> GitResult<OpResult> {
    let sha = compare::resolve(dir, rev)?;
    let (kind, label) = if cmd == "revert" {
        (Kind::Revert, format!("Revert {} “{}”", short(&sha), subject(dir, &sha)))
    } else {
        (Kind::CherryPick, format!("Cherry-pick {} “{}” onto {}", short(&sha), subject(dir, &sha), head_name(dir)))
    };
    journaled(dir, j, kind, label, Scope::full(), || {
        let mut args = vec![cmd];
        if cmd == "revert" {
            args.push("--no-edit");
        }
        if is_merge(dir, &sha) {
            args.extend(["-m", "1"]);
        }
        args.push(&sha);
        op_result(dir, &git::output(dir, &args)?)
    })
}

/// `git cherry-pick <sha>` (`-m 1` for a merge commit).
pub fn cherry_pick(dir: &Path, j: Undo<'_>, rev: &str) -> GitResult<OpResult> {
    pick(dir, j, "cherry-pick", rev)
}

/// `git revert --no-edit <sha>` (`-m 1` for a merge commit).
pub fn revert(dir: &Path, j: Undo<'_>, rev: &str) -> GitResult<OpResult> {
    pick(dir, j, "revert", rev)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeMode {
    /// Fast-forward when possible, else a merge commit (git's default).
    Ff,
    /// Always a merge commit.
    NoFf,
    /// Only a fast-forward.
    FfOnly,
    /// The changes staged as one commit to make (no merge recorded).
    Squash,
}

/// Merges `rev` into the checked-out branch.
pub fn merge(dir: &Path, j: Undo<'_>, rev: &str, mode: MergeMode) -> GitResult<OpResult> {
    if !compare::valid_rev(rev) {
        return Err(format!("“{rev}” isn't a branch, tag or commit"));
    }
    let sha = compare::resolve(dir, rev)?;
    let into = head_name(dir);
    let label = match mode {
        MergeMode::Squash => format!("Squash {rev} into {into}"),
        MergeMode::FfOnly => format!("Fast-forward {into} to {rev}"),
        _ => format!("Merge {rev} into {into}"),
    };
    journaled(dir, j, Kind::Merge, label, Scope::full(), || {
        let flag = match mode {
            MergeMode::Ff => "--ff",
            MergeMode::NoFf => "--no-ff",
            MergeMode::FfOnly => "--ff-only",
            MergeMode::Squash => "--squash",
        };
        // The name (checked above, resolved to a commit) so git's merge message names it.
        let _ = &sha;
        let args = vec!["merge", flag, "--no-edit", rev];
        let out = git::output_env(dir, &args, &[("GIT_EDITOR", OsStr::new("true"))])?;
        op_result(dir, &out)
    })
}

/// `git rebase <onto>`: replays the checked-out branch's commits on top of `onto`.
pub fn rebase(dir: &Path, j: Undo<'_>, onto: &str) -> GitResult<OpResult> {
    let sha = compare::resolve(dir, onto)?;
    let branch = compare::current_branch(dir).ok_or("check out the branch to rebase first (HEAD is detached)")?;
    journaled(dir, j, Kind::Rebase, format!("Rebase {branch} onto {onto}"), Scope::full(), || {
        let out = git::output_env(dir, &["rebase", &sha], &[("GIT_EDITOR", OsStr::new("true"))])?;
        op_result(dir, &out)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    /// Move the branch; keep the index and files (the undone commits' changes stay staged).
    Soft,
    /// Move the branch and reset the index; keep the files.
    Mixed,
    /// Move the branch and update files, refusing to lose local changes.
    Keep,
    /// Move the branch and throw away every uncommitted change to tracked files.
    Hard,
}

/// Resets the checked-out branch (or detached HEAD) to `rev`.
pub fn reset(dir: &Path, j: Undo<'_>, rev: &str, mode: ResetMode) -> GitResult<()> {
    let sha = compare::resolve(dir, rev)?;
    let (flag, scope) = match mode {
        ResetMode::Soft => ("--soft", Scope::refs()),
        ResetMode::Mixed => ("--mixed", Scope::index()),
        ResetMode::Keep => ("--keep", Scope::full()),
        ResetMode::Hard => ("--hard", Scope::full()),
    };
    let label = format!("Reset {} to {} ({})", head_name(dir), short(&sha), &flag[2..]);
    journaled(dir, j, Kind::Reset, label, scope, || {
        // Fresh stat info, or `--keep` takes a file touched since the last status as modified.
        let _ = git::output(dir, &["update-index", "-q", "--refresh"]);
        let out = git::output(dir, &["reset", "-q", flag, &sha])?;
        if out.status.success() { Ok(()) } else { Err(git::combined(&out)) }
    })
}

// ---------- working tree (Changes) ----------

fn files_label(verb: &str, paths: &[String]) -> String {
    match paths {
        [one] => format!("{verb} {one}"),
        many => format!("{verb} {} files", many.len()),
    }
}

fn check_paths(paths: &[String]) -> GitResult<()> {
    if paths.is_empty() {
        return Err("no files".into());
    }
    for p in paths {
        if p.is_empty() || p.starts_with('-') || p.contains('\0') || p.split(['/', '\\']).any(|s| s == "..") {
            return Err(format!("invalid path {p}"));
        }
    }
    Ok(())
}

fn with_paths<'a>(head: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    let mut args = head.to_vec();
    args.push("--");
    args.extend(paths.iter().map(String::as_str));
    args
}

pub fn stage(dir: &Path, j: Undo<'_>, paths: &[String]) -> GitResult<()> {
    check_paths(paths)?;
    journaled(dir, j, Kind::Stage, files_label("Stage", paths), Scope::index(), || run_literal(dir, &with_paths(&["add", "-A"], paths)).map(|_| ()))
}

pub fn unstage(dir: &Path, j: Undo<'_>, paths: &[String]) -> GitResult<()> {
    check_paths(paths)?;
    let unborn = git::run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err();
    journaled(dir, j, Kind::Unstage, files_label("Unstage", paths), Scope::index(), || {
        if unborn { run_literal(dir, &with_paths(&["rm", "--cached", "-r", "-q"], paths)) } else { run_literal(dir, &with_paths(&["restore", "--staged"], paths)) }.map(|_| ())
    })
}

/// Throws away working-tree changes: tracked files go back to their staged (or committed)
/// version, untracked files go to the OS trash. Journaled, so undo brings them back.
pub fn discard(dir: &Path, j: Undo<'_>, paths: &[String]) -> GitResult<()> {
    check_paths(paths)?;
    journaled(dir, j, Kind::Discard, files_label("Discard changes to", paths), Scope::paths(paths.to_vec()), || {
        let untracked: Vec<String> = run_literal(dir, &with_paths(&["ls-files", "--others", "--exclude-standard", "-z"], paths))?.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect();
        let tracked: Vec<String> = paths.iter().filter(|p| !untracked.iter().any(|u| u == *p || u.starts_with(&format!("{}/", p.trim_end_matches('/'))))).cloned().collect();
        if !tracked.is_empty() {
            run_literal(dir, &with_paths(&["restore", "--worktree"], &tracked))?;
        }
        for u in &untracked {
            let abs = crate::fsutil::safe_join(dir, u).map_err(|e| e.to_string())?;
            super::to_trash(&abs)?;
        }
        Ok(())
    })
}

/// `git commit -m <message>` (`--amend`: an empty message keeps the old one).
pub fn commit(dir: &Path, j: Undo<'_>, message: &str, amend: bool) -> GitResult<String> {
    if message.trim().is_empty() && !amend {
        return Err("commit message is empty".into());
    }
    let first = message.lines().next().unwrap_or_default().trim();
    let first = if first.chars().count() > 50 { format!("{}…", first.chars().take(49).collect::<String>()) } else { first.to_string() };
    let label = if amend {
        if first.is_empty() { "Amend the last commit".to_string() } else { format!("Amend the last commit: “{first}”") }
    } else {
        format!("Commit “{first}”")
    };
    journaled(dir, j, if amend { Kind::Amend } else { Kind::Commit }, label, Scope::refs(), || {
        let mut args = vec!["commit"];
        if amend {
            args.push("--amend");
        }
        if message.trim().is_empty() {
            args.push("--no-edit");
        } else {
            args.extend(["-m", message]);
        }
        let out = git::output_env(dir, &args, &[("GIT_EDITOR", OsStr::new("true"))])?;
        if out.status.success() { Ok(git::combined(&out)) } else { Err(git::combined(&out)) }
    })
}

/// `git pull --ff-only` (journaled: undo puts the branch back where it was).
pub fn pull(dir: &Path, j: Undo<'_>) -> GitResult<String> {
    let label = format!("Pull {} (fast-forward)", head_name(dir));
    journaled(dir, j, Kind::Pull, label, Scope::full(), || network(dir, &["pull", "--ff-only"]))
}

// ---------- push ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PushMode {
    /// `git push` to the upstream.
    Normal,
    /// `git push -u <remote> <branch>`.
    SetUpstream,
    /// `git push --force-with-lease --force-if-includes <remote> <branch>` — only overwrites the
    /// remote branch if it is still where this clone last saw it.
    ForceWithLease,
}

/// What the push button needs to decide what to offer.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushInfo {
    pub branch: Option<String>,
    /// `origin/main`.
    pub upstream: Option<String>,
    pub remote: Option<String>,
    /// The branch's name on the remote.
    pub remote_branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub remotes: Vec<String>,
    /// `--force-if-includes` is available (Git ≥ 2.30).
    pub force_if_includes: bool,
}

pub fn push_info(dir: &Path) -> GitResult<PushInfo> {
    let s = git::status(dir, false)?;
    let remotes = compare::remotes(dir);
    let (remote, remote_branch) = match (&s.branch, &s.upstream) {
        (Some(_), Some(u)) => split_remote_branch(dir, u).map(|(r, b)| (Some(r), Some(b))).unwrap_or((None, None)),
        (Some(b), None) => (compare::main_remote(dir), Some(b.clone())),
        _ => (None, None),
    };
    Ok(PushInfo { branch: s.branch, upstream: s.upstream, remote, remote_branch, ahead: s.ahead, behind: s.behind, remotes, force_if_includes: git::version_at_least(2, 30) })
}

/// Pushes the checked-out branch. Network; not journaled (it changes the remote, not this clone).
pub fn push(dir: &Path, mode: PushMode, remote: Option<&str>, branch: Option<&str>) -> GitResult<String> {
    if mode == PushMode::Normal {
        return git::mutate(dir, || network(dir, &["push"]));
    }
    let info = push_info(dir)?;
    let local = info.branch.clone().ok_or("HEAD is detached — check out a branch to push")?;
    let remote = remote.map(str::to_string).or(info.remote.clone()).ok_or("this repository has no remote")?;
    check_remote(dir, &remote)?;
    let target = branch.map(str::to_string).or(info.remote_branch.clone()).unwrap_or_else(|| local.clone());
    check_name(&target, "branch")?;
    let refspec = format!("refs/heads/{local}:refs/heads/{target}");
    git::mutate(dir, || match mode {
        PushMode::SetUpstream => network(dir, &["push", "-u", &remote, &refspec]),
        _ => {
            let lease = format!("--force-with-lease=refs/heads/{target}");
            let mut args = vec!["push", lease.as_str()];
            if git::version_at_least(2, 30) {
                args.push("--force-if-includes");
            }
            args.extend([remote.as_str(), refspec.as_str()]);
            network(dir, &args)
        }
    })
}

/// `git push -u <remote> <branch>` for any local branch (before opening a pull request for it).
pub fn push_branch(dir: &Path, remote: &str, branch: &str) -> GitResult<String> {
    check_name(branch, "branch")?;
    check_remote(dir, remote)?;
    if !branch_exists(dir, branch) {
        return Err(format!("no local branch {branch} to push"));
    }
    git::mutate(dir, || network(dir, &["push", "-u", remote, &format!("refs/heads/{branch}:refs/heads/{branch}")]))
}

// ---------- operations in progress ----------

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
    pub fn command(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
        }
    }
}

fn git_path_exists(dir: &Path, name: &str) -> bool {
    git::git_path(dir, name).is_ok_and(|p| p.exists())
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

/// `git <op> --abort`: back to where the operation started (its journal entry goes too).
pub fn abort(dir: &Path, j: Undo<'_>) -> GitResult<()> {
    let op = operation(dir).ok_or("nothing to abort")?;
    git::mutate(dir, || {
        git::run(dir, &[op.command(), "--abort"])?;
        if let Some(j) = j {
            undo::finish_pending(dir, j)?;
        }
        Ok(())
    })
}

fn resume(dir: &Path, j: Undo<'_>, flag: &str) -> GitResult<OpResult> {
    let op = operation(dir).ok_or_else(|| format!("nothing to {}", flag.trim_start_matches('-')))?;
    if flag == "--skip" && op == Operation::Merge {
        return Err("a merge can't skip — resolve the conflicts or abort it".into());
    }
    git::mutate(dir, || {
        let out = git::output_env(dir, &[op.command(), flag], &[("GIT_EDITOR", OsStr::new("true"))])?;
        let res = op_result(dir, &out);
        if let Some(j) = j {
            undo::finish_pending(dir, j)?;
        }
        res
    })
}

/// `git <op> --continue` with git's prepared message (no editor). May stop again (the next
/// conflict of a rebase); the journal entry completes when the operation finishes.
pub fn continue_op(dir: &Path, j: Undo<'_>) -> GitResult<OpResult> {
    resume(dir, j, "--continue")
}

/// `git <rebase|cherry-pick|revert> --skip`: drops the commit being applied.
pub fn skip(dir: &Path, j: Undo<'_>) -> GitResult<OpResult> {
    resume(dir, j, "--skip")
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
