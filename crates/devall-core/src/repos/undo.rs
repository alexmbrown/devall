//! Multi-level undo / redo of the app's git operations.
//!
//! Every mutating operation the app runs is journaled ([`record`]): the state before and after it —
//! HEAD, the branch and tag refs it moved, branch upstreams, the index tree, a working-tree snapshot
//! (tracked and untracked files, written as a tree through a temporary index, so the real index and
//! the files are never touched) and the stash list, as far as the operation's [`Scope`] reaches.
//! [`undo`] puts the "before" state back, [`redo`] the "after" state.
//!
//! Snapshots are anchored under `refs/devall/undo/<journal>/<entry>/s` (a tree: invisible to
//! `git log --all`, left out of the graph) and commits that would otherwise become unreachable (an
//! amended commit, a deleted branch's tip, a dropped stash) under `…/<entry>/k-<oid>`. The entries
//! live in a small JSON journal in the project data folder ([`FILE`], ≤ [`MAX_ENTRIES`] per
//! repository, ≤ 30 days); entries that fall off take their refs with them.
//!
//! Undo is exact or it says why not:
//! - only what the operation changed is restored (its refs, paths, index entries, stash list);
//! - if the repository moved on since (files edited, other commits), undo asks first
//!   ([`Outcome::Confirm`]); the state it replaces becomes the redo step, so nothing is lost;
//! - commits that were pushed after the operation are not taken back ([`Outcome::Refused`]);
//! - while a merge/rebase/cherry-pick/revert is paused, finish or abort it first. An operation
//!   that paused on conflicts is one entry, completed when it finishes ([`finish_pending`]).
//!
//! These functions expect to run inside [`git::mutate`] (one writer per repository); [`journaled`]
//! does that for an operation.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::git::{self, GitResult};
use super::ops;

pub const FILE: &str = "git-undo.json";
/// Entries kept per repository (undo + redo).
pub const MAX_ENTRIES: usize = 50;
/// Entries older than this are dropped.
pub const MAX_AGE_SECS: i64 = 30 * 24 * 3600;
/// Untracked files bigger than this aren't copied into working-tree snapshots.
pub const MAX_UNTRACKED_BYTES: u64 = 50 * 1024 * 1024;
pub const REF_ROOT: &str = "refs/devall/undo/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Commit,
    Amend,
    Stage,
    Unstage,
    Discard,
    Checkout,
    BranchCreate,
    BranchDelete,
    BranchRename,
    SetUpstream,
    FastForward,
    Reset,
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Pull,
    StashSave,
    StashApply,
    StashPop,
    StashDrop,
    TagCreate,
    TagDelete,
}

/// Which working-tree files a snapshot covers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Files {
    #[default]
    None,
    /// These paths only (a discard).
    Paths(Vec<String>),
    /// Every tracked and untracked (not ignored) file.
    All,
}

/// What an operation can change, so what its snapshots capture. Refs, HEAD and branch upstreams
/// are always captured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scope {
    pub index: bool,
    pub files: Files,
    pub stash: bool,
}

impl Scope {
    /// Branches, tags, HEAD, upstreams (commit, branch and tag operations).
    pub fn refs() -> Self {
        Self::default()
    }
    /// + the index (stage / unstage).
    pub fn index() -> Self {
        Self { index: true, ..Default::default() }
    }
    /// + these working-tree paths (discard).
    pub fn paths(paths: Vec<String>) -> Self {
        Self { files: Files::Paths(paths), ..Default::default() }
    }
    /// Everything: index, working tree, stash list (checkout, reset, merge, rebase, stash…).
    pub fn full() -> Self {
        Self { index: true, files: Files::All, stash: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    pub remote: String,
    pub merge: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashItem {
    pub oid: String,
    /// The reflog subject (`On main: wip`), restored as is.
    pub message: String,
}

/// Everything captured at one moment.
#[derive(Debug, Clone, Default, PartialEq)]
struct Snapshot {
    head_ref: Option<String>,
    head: Option<String>,
    refs: BTreeMap<String, String>,
    upstreams: BTreeMap<String, Upstream>,
    index: Option<String>,
    unmerged: bool,
    worktree: Option<String>,
    stash: Option<Vec<StashItem>>,
}

/// One side of a journal entry: only what differs between before and after (refs, upstreams,
/// stash), plus HEAD and the trees.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Side {
    pub head_ref: Option<String>,
    pub head: Option<String>,
    /// `None` = the ref doesn't exist on this side.
    pub refs: BTreeMap<String, Option<String>>,
    pub upstreams: BTreeMap<String, Option<Upstream>>,
    pub index: Option<String>,
    /// The index had unresolved conflicts (no tree).
    pub unmerged: bool,
    pub worktree: Option<String>,
    pub stash: Option<Vec<StashItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    /// Unix seconds.
    pub at: i64,
    #[serde(default)]
    pub scope: Scope,
    pub before: Side,
    pub after: Side,
    /// Branches whose new tip was already on a remote when the operation ran (a pull): taking
    /// them back doesn't rewrite anything published.
    #[serde(default)]
    pub published: BTreeSet<String>,
    /// The operation paused (conflicts, an `edit` stop): completed when it finishes.
    #[serde(default)]
    pub pending: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Stack {
    #[serde(default)]
    undo: Vec<Entry>,
    /// Undone entries, the next one to redo last.
    #[serde(default)]
    redo: Vec<Entry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    version: u32,
    repos: BTreeMap<String, Stack>,
}

/// Where a repository's entries are kept: the project's journal file, keyed by repository name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Journal {
    pub file: PathBuf,
    pub repo: String,
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

impl Journal {
    pub fn new(file: PathBuf, repo: impl Into<String>) -> Self {
        Self { file, repo: repo.into() }
    }

    /// `<project data>/git-undo.json`, entries of `repo`.
    pub fn for_project(root: &Path, repo: &str) -> Self {
        Self::new(crate::local::project_dir(root).join(FILE), repo)
    }

    /// Short id of this journal + repository in ref names (two projects sharing a repository
    /// never touch each other's anchors).
    fn key(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.file.to_string_lossy().as_bytes());
        h.update([0]);
        h.update(self.repo.as_bytes());
        h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect()
    }

    fn prefix(&self) -> String {
        format!("{REF_ROOT}{}/", self.key())
    }

    fn read_store(&self) -> Store {
        std::fs::read(&self.file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn load(&self) -> Stack {
        let _l = lock();
        self.read_store().repos.remove(&self.repo).unwrap_or_default()
    }

    fn save(&self, stack: Stack) -> GitResult<()> {
        let _l = lock();
        let mut store = self.read_store();
        store.version = 1;
        if stack.undo.is_empty() && stack.redo.is_empty() {
            store.repos.remove(&self.repo);
        } else {
            store.repos.insert(self.repo.clone(), stack);
        }
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let json = serde_json::to_vec_pretty(&store).map_err(|e| e.to_string())?;
        crate::fsutil::atomic_write(&self.file, &json).map_err(|e| e.to_string())
    }
}

/// An operation about to be journaled.
#[derive(Debug, Clone)]
pub struct Op {
    pub kind: Kind,
    pub label: String,
    pub scope: Scope,
}

impl Op {
    pub fn new(kind: Kind, label: impl Into<String>, scope: Scope) -> Self {
        Self { kind, label: label.into(), scope }
    }
}

/// Runs `f` as one writer of the repository (see [`git::mutate`]) and, with a journal, records it
/// for undo.
pub fn journaled<T>(dir: &Path, j: Option<&Journal>, op: Op, f: impl FnOnce() -> GitResult<T>) -> GitResult<T> {
    git::mutate(dir, || match j {
        Some(j) => record(dir, j, op, f),
        None => f(),
    })
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---------- capturing ----------

fn rev(dir: &Path, spec: &str) -> Option<String> {
    git::run(dir, &["rev-parse", "--verify", "--quiet", spec]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn head_ref(dir: &Path) -> Option<String> {
    git::output(dir, &["symbolic-ref", "-q", "HEAD"]).ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn list_refs(dir: &Path) -> GitResult<BTreeMap<String, String>> {
    let out = git::run(dir, &["for-each-ref", "--format=%(objectname) %(refname)", "refs/heads", "refs/tags"])?;
    Ok(out.lines().filter_map(|l| l.split_once(' ')).map(|(o, r)| (r.to_string(), o.to_string())).collect())
}

/// `branch.<name>.remote` / `.merge` of every branch (names may contain dots).
fn upstreams(dir: &Path) -> BTreeMap<String, Upstream> {
    let out = git::run(dir, &["config", "-z", "--get-regexp", r"^branch\..*\.(remote|merge)$"]).unwrap_or_default();
    let mut remote: BTreeMap<String, String> = BTreeMap::new();
    let mut merge: BTreeMap<String, String> = BTreeMap::new();
    for rec in out.split('\0').filter(|r| !r.is_empty()) {
        let (key, value) = rec.split_once('\n').unwrap_or((rec, ""));
        let Some(rest) = key.strip_prefix("branch.") else { continue };
        if let Some(b) = rest.strip_suffix(".remote") {
            remote.insert(b.to_string(), value.to_string());
        } else if let Some(b) = rest.strip_suffix(".merge") {
            merge.insert(b.to_string(), value.to_string());
        }
    }
    remote.into_iter().filter_map(|(b, r)| merge.get(&b).map(|m| (b, Upstream { remote: r, merge: m.clone() }))).collect()
}

pub(crate) fn stash_list(dir: &Path) -> Vec<StashItem> {
    git::run(dir, &["stash", "list", "--format=%H%x1f%gs"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\x1f'))
        .map(|(o, m)| StashItem { oid: o.to_string(), message: m.to_string() })
        .collect()
}

fn unique() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

/// A temporary index file in the git dir, removed on drop.
struct TempIndex(PathBuf);

impl TempIndex {
    /// A copy of the repository's index (or an empty one).
    fn copy_of_index(dir: &Path) -> GitResult<Self> {
        let index = git::git_path(dir, "index")?;
        let tmp = git::git_path(dir, &format!("devall-undo-{}.index", unique()))?;
        if index.is_file() {
            std::fs::copy(&index, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        }
        Ok(Self(tmp))
    }
    fn empty(dir: &Path) -> GitResult<Self> {
        Ok(Self(git::git_path(dir, &format!("devall-undo-{}.index", unique()))?))
    }
    fn env(&self) -> [(&'static str, &OsStr); 1] {
        [("GIT_INDEX_FILE", self.0.as_os_str())]
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("index.lock"));
    }
}

/// The working tree as a tree object: the index with the working-tree state of every file (or of
/// `paths`) added, untracked files included (ignored ones and files over
/// [`MAX_UNTRACKED_BYTES`] left out).
fn worktree_tree(dir: &Path, paths: Option<&[String]>) -> GitResult<String> {
    let tmp = TempIndex::copy_of_index(dir)?;
    let env = tmp.env();
    match paths {
        Some(paths) => {
            let (present, gone): (Vec<&String>, Vec<&String>) = paths.iter().partition(|p| crate::fsutil::safe_join(dir, p).is_ok_and(|a| a.symlink_metadata().is_ok()));
            if !present.is_empty() {
                let mut args = vec!["add", "-A", "--ignore-errors", "--"];
                args.extend(present.iter().map(|p| p.as_str()));
                git::run_env(dir, &args, &env)?;
            }
            if !gone.is_empty() {
                let mut args = vec!["update-index", "--force-remove", "--"];
                args.extend(gone.iter().map(|p| p.as_str()));
                git::run_env(dir, &args, &env)?;
            }
        }
        None => {
            let untracked = git::run(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
            let big: Vec<String> = untracked
                .split('\0')
                .filter(|p| !p.is_empty())
                .filter(|p| crate::fsutil::safe_join(dir, p).ok().and_then(|a| a.metadata().ok()).is_some_and(|m| m.len() > MAX_UNTRACKED_BYTES))
                .map(|p| format!(":(exclude,literal){p}"))
                .collect();
            let mut args = vec!["add", "-A", "--ignore-errors", "--", "."];
            args.extend(big.iter().map(String::as_str));
            git::run_env(dir, &args, &env)?;
        }
    }
    Ok(git::run_env(dir, &["write-tree"], &env)?.trim().to_string())
}

fn capture(dir: &Path, scope: &Scope) -> GitResult<Snapshot> {
    let (index, unmerged) = if scope.index {
        match git::run(dir, &["write-tree"]) {
            Ok(t) => (Some(t.trim().to_string()), false),
            Err(_) => (None, true),
        }
    } else {
        (None, false)
    };
    let worktree = match &scope.files {
        Files::None => None,
        Files::Paths(p) => Some(worktree_tree(dir, Some(p))?),
        Files::All => Some(worktree_tree(dir, None)?),
    };
    Ok(Snapshot {
        head_ref: head_ref(dir),
        head: rev(dir, "HEAD"),
        refs: list_refs(dir)?,
        upstreams: upstreams(dir),
        index,
        unmerged,
        worktree,
        stash: scope.stash.then(|| stash_list(dir)),
    })
}

/// Before/after sides keeping only what differs (plus `keep` refs/upstreams).
fn sides(a: &Snapshot, b: &Snapshot, keep_refs: &BTreeSet<String>, keep_up: &BTreeSet<String>) -> (Side, Side) {
    let ref_keys: BTreeSet<&String> = a.refs.keys().chain(b.refs.keys()).filter(|k| keep_refs.contains(*k) || a.refs.get(*k) != b.refs.get(*k)).collect();
    let up_keys: BTreeSet<&String> = a.upstreams.keys().chain(b.upstreams.keys()).filter(|k| keep_up.contains(*k) || a.upstreams.get(*k) != b.upstreams.get(*k)).collect();
    let stash_differs = a.stash != b.stash;
    let side = |s: &Snapshot| Side {
        head_ref: s.head_ref.clone(),
        head: s.head.clone(),
        refs: ref_keys.iter().map(|k| ((*k).clone(), s.refs.get(*k).cloned())).collect(),
        upstreams: up_keys.iter().map(|k| ((*k).clone(), s.upstreams.get(*k).cloned())).collect(),
        index: s.index.clone(),
        unmerged: s.unmerged,
        worktree: s.worktree.clone(),
        stash: if stash_differs { s.stash.clone() } else { None },
    };
    (side(a), side(b))
}

/// A side with every ref and upstream (pending entries: the refs a paused operation will move
/// aren't known until it finishes).
fn full_side(s: &Snapshot) -> Side {
    Side {
        head_ref: s.head_ref.clone(),
        head: s.head.clone(),
        refs: s.refs.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect(),
        upstreams: s.upstreams.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect(),
        index: s.index.clone(),
        unmerged: s.unmerged,
        worktree: s.worktree.clone(),
        stash: s.stash.clone(),
    }
}

fn snapshot_of(side: &Side) -> Snapshot {
    Snapshot {
        head_ref: side.head_ref.clone(),
        head: side.head.clone(),
        refs: side.refs.iter().filter_map(|(k, v)| v.clone().map(|v| (k.clone(), v))).collect(),
        upstreams: side.upstreams.iter().filter_map(|(k, v)| v.clone().map(|v| (k.clone(), v))).collect(),
        index: side.index.clone(),
        unmerged: side.unmerged,
        worktree: side.worktree.clone(),
        stash: side.stash.clone(),
    }
}

fn unchanged(a: &Snapshot, b: &Snapshot) -> bool {
    a.head_ref == b.head_ref && a.head == b.head && a.refs == b.refs && a.upstreams == b.upstreams && a.index == b.index && a.unmerged == b.unmerged && a.worktree == b.worktree && a.stash == b.stash
}

// ---------- anchoring ----------

fn object_type(dir: &Path, oid: &str) -> Option<String> {
    git::run(dir, &["cat-file", "-t", oid]).ok().map(|s| s.trim().to_string())
}

fn exists(dir: &Path, oid: &str) -> bool {
    git::run(dir, &["cat-file", "-e", oid]).is_ok()
}

fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    git::output(dir, &["merge-base", "--is-ancestor", a, b]).is_ok_and(|o| o.status.success())
}

/// The remote-tracking branch of `branch` (its upstream, or `<remote>/<branch>` on any remote)
/// that contains `oid` — the commit is published. Targeted checks, so huge repositories with
/// thousands of remote branches stay quick.
fn on_remote(dir: &Path, oid: &str, branch: &str) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(u) = upstreams(dir).get(branch) {
        candidates.push(format!("refs/remotes/{}/{}", u.remote, u.merge.trim_start_matches("refs/heads/")));
    }
    for r in super::compare::remotes(dir) {
        candidates.push(format!("refs/remotes/{r}/{branch}"));
    }
    candidates.dedup();
    candidates.into_iter().filter(|c| rev(dir, c).is_some()).find(|c| is_ancestor(dir, oid, c)).map(|c| c.trim_start_matches("refs/remotes/").to_string())
}

/// `oid` stays reachable from the state the repository moved to (`tips`: its refs and HEAD).
fn reachable_from(dir: &Path, oid: &str, tips: &[String]) -> bool {
    tips.iter().any(|t| t == oid || is_ancestor(dir, oid, t))
}

/// Ref values and HEAD of a side.
fn tips(side: &Side) -> Vec<String> {
    let mut t: Vec<String> = side.refs.values().flatten().cloned().collect();
    t.extend(side.head.iter().cloned());
    t.sort();
    t.dedup();
    t
}

/// Objects of `side` that nothing else may keep alive once the repository has left that state.
fn keep_candidates(side: &Side, other: &Side) -> Vec<String> {
    let mut out: Vec<String> = side.refs.values().flatten().cloned().collect();
    if side.head_ref.is_none()
        && let Some(h) = &side.head
    {
        out.push(h.clone());
    }
    if let Some(st) = &side.stash {
        let others: BTreeSet<&String> = other.stash.iter().flatten().map(|s| &s.oid).collect();
        out.extend(st.iter().filter(|s| !others.contains(&s.oid)).map(|s| s.oid.clone()));
    }
    out.sort();
    out.dedup();
    out
}

/// Points `…/<id>/s` at a tree holding the entry's snapshot trees and `…/<id>/k-<oid>` at each
/// object of `keep` that isn't reachable from `now` (the side the repository is in now) — an
/// object anchored needlessly costs a ref, never a lost commit.
fn anchor(dir: &Path, j: &Journal, e: &Entry, keep: &[String], now: &Side) -> GitResult<()> {
    let now_tips = tips(now);
    let base = format!("{}{}/", j.prefix(), e.id);
    let mut cmds = String::new();
    let mut mk = String::new();
    for (name, t) in [("ai", &e.after.index), ("aw", &e.after.worktree), ("bi", &e.before.index), ("bw", &e.before.worktree)] {
        if let Some(t) = t {
            mk.push_str(&format!("040000 tree {t}\t{name}\n"));
        }
    }
    if !mk.is_empty() {
        let tree = String::from_utf8_lossy(&git::run_input(dir, &["mktree"], mk.as_bytes())?).trim().to_string();
        cmds.push_str(&format!("update {base}s {tree}\n"));
    }
    for oid in keep {
        let ty = object_type(dir, oid);
        let needed = match ty.as_deref() {
            Some("commit") => !reachable_from(dir, oid, &now_tips),
            Some("tag") => true,
            _ => false,
        };
        if needed {
            cmds.push_str(&format!("update {base}k-{oid} {oid}\n"));
        }
    }
    if !cmds.is_empty() {
        git::run_input(dir, &["update-ref", "--stdin"], cmds.as_bytes())?;
    }
    Ok(())
}

fn delete_refs_under(dir: &Path, prefix: &str) -> GitResult<()> {
    let refs = git::run(dir, &["for-each-ref", "--format=%(refname)", prefix])?;
    let cmds: String = refs.lines().filter(|l| !l.is_empty()).map(|r| format!("delete {r}\n")).collect();
    if !cmds.is_empty() {
        git::run_input(dir, &["update-ref", "--stdin"], cmds.as_bytes())?;
    }
    Ok(())
}

fn drop_entry_refs(dir: &Path, j: &Journal, id: &str) {
    let _ = delete_refs_under(dir, &format!("{}{id}/", j.prefix()));
}

/// Drops entries over the count/age limits and anchors no entry uses any more.
fn prune(dir: &Path, j: &Journal, stack: &mut Stack) {
    let cutoff = now() - MAX_AGE_SECS;
    let mut dropped: Vec<String> = Vec::new();
    stack.undo.retain(|e| {
        let keep = e.at >= cutoff || e.pending;
        if !keep {
            dropped.push(e.id.clone());
        }
        keep
    });
    stack.redo.retain(|e| {
        let keep = e.at >= cutoff;
        if !keep {
            dropped.push(e.id.clone());
        }
        keep
    });
    while stack.undo.len() + stack.redo.len() > MAX_ENTRIES && !stack.undo.is_empty() {
        dropped.push(stack.undo.remove(0).id);
    }
    for id in &dropped {
        drop_entry_refs(dir, j, id);
    }
    // Orphans (a crash between writing refs and the journal).
    let live: BTreeSet<&str> = stack.undo.iter().chain(&stack.redo).map(|e| e.id.as_str()).collect();
    let prefix = j.prefix();
    if let Ok(refs) = git::run(dir, &["for-each-ref", "--format=%(refname)", &prefix]) {
        let orphans: BTreeSet<&str> = refs.lines().filter_map(|r| r.strip_prefix(&prefix)).filter_map(|r| r.split('/').next()).filter(|id| !live.contains(id)).collect();
        for id in orphans {
            drop_entry_refs(dir, j, id);
        }
    }
}

// ---------- recording ----------

fn changed_branches_published(dir: &Path, before: &Side, after: &Side) -> BTreeSet<String> {
    after
        .refs
        .iter()
        .filter(|(r, a)| r.starts_with("refs/heads/") && a.is_some() && before.refs.get(*r) != Some(*a))
        .filter(|(r, a)| on_remote(dir, a.as_deref().unwrap_or_default(), r.trim_start_matches("refs/heads/")).is_some())
        .map(|(r, _)| r.clone())
        .collect()
}

fn new_id() -> String {
    ulid::Ulid::generate().to_string().to_lowercase()
}

/// Runs `f` and journals what it changed (call inside [`git::mutate`]). Not journaled: operations
/// run while a merge/rebase/… is paused (they belong to it), and operations that changed nothing.
/// When `f` fails but left changes behind, they are journaled too.
pub fn record<T>(dir: &Path, j: &Journal, op: Op, f: impl FnOnce() -> GitResult<T>) -> GitResult<T> {
    if ops::operation(dir).is_some() {
        return f();
    }
    let mut stack = j.load();
    reconcile(dir, j, &mut stack);
    let before = match capture(dir, &op.scope) {
        Ok(s) => s,
        Err(_) => return f(),
    };
    let result = f();
    let paused = ops::operation(dir).is_some();
    let Ok(after) = capture(dir, &op.scope) else { return result };
    if !paused && unchanged(&before, &after) {
        let _ = j.save(stack);
        return result;
    }
    let (bs, as_) = if paused { (full_side(&before), full_side(&after)) } else { sides(&before, &after, &BTreeSet::new(), &BTreeSet::new()) };
    let entry = Entry {
        id: new_id(),
        kind: op.kind,
        label: op.label,
        at: now(),
        published: if paused { BTreeSet::new() } else { changed_branches_published(dir, &bs, &as_) },
        scope: op.scope,
        before: bs,
        after: as_,
        pending: paused,
    };
    // Only what changed can become unreachable (a pending entry stores every ref).
    let (pb, pa) = sides(&before, &after, &BTreeSet::new(), &BTreeSet::new());
    let keep = keep_candidates(&pb, &pa);
    if anchor(dir, j, &entry, &keep, &pa).is_err() {
        return result;
    }
    for e in stack.redo.drain(..) {
        drop_entry_refs(dir, j, &e.id);
    }
    stack.undo.push(entry);
    prune(dir, j, &mut stack);
    let _ = j.save(stack);
    result
}

/// Completes the paused entry on top once its operation is no longer in progress: finished (the
/// entry gets its "after" state) or aborted (back where it started: the entry goes).
fn reconcile(dir: &Path, j: &Journal, stack: &mut Stack) {
    let Some(top) = stack.undo.last() else { return };
    if !top.pending || ops::operation(dir).is_some() {
        return;
    }
    let Ok(cur) = capture(dir, &top.scope) else { return };
    let mut e = stack.undo.pop().expect("checked");
    let start = snapshot_of(&e.before);
    if cur.head == start.head && cur.head_ref == start.head_ref && cur.refs == start.refs {
        drop_entry_refs(dir, j, &e.id);
        return;
    }
    let (bs, as_) = sides(&start, &cur, &BTreeSet::new(), &BTreeSet::new());
    e.published = changed_branches_published(dir, &bs, &as_);
    e.before = bs;
    e.after = as_;
    e.pending = false;
    let keep = keep_candidates(&e.before, &e.after);
    let _ = anchor(dir, j, &e, &keep, &e.after);
    stack.undo.push(e);
}

/// After `git <op> --continue/--skip/--abort`: completes or drops the paused entry (call inside
/// [`git::mutate`]).
pub fn finish_pending(dir: &Path, j: &Journal) -> GitResult<()> {
    let mut stack = j.load();
    reconcile(dir, j, &mut stack);
    j.save(stack)
}

// ---------- history ----------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryView {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    pub at: i64,
    pub pending: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    /// Newest first: the next undo is `undo[0]`.
    pub undo: Vec<EntryView>,
    /// The next redo first.
    pub redo: Vec<EntryView>,
}

fn view(e: &Entry) -> EntryView {
    EntryView { id: e.id.clone(), kind: e.kind, label: e.label.clone(), at: e.at, pending: e.pending }
}

/// The journal of a repository (completing a paused entry whose operation finished meanwhile).
pub fn history(dir: &Path, j: &Journal) -> GitResult<History> {
    let mut stack = j.load();
    if stack.undo.last().is_some_and(|e| e.pending) {
        reconcile(dir, j, &mut stack);
        j.save(stack.clone())?;
    }
    Ok(History { undo: stack.undo.iter().rev().map(view).collect(), redo: stack.redo.iter().rev().map(view).collect() })
}

/// Forgets every entry of the repository and removes its anchors.
pub fn clear(dir: &Path, j: &Journal) -> GitResult<()> {
    delete_refs_under(dir, &j.prefix())?;
    j.save(Stack::default())
}

// ---------- undo / redo ----------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Outcome {
    /// Restored.
    Done { label: String, kind: Kind },
    /// The repository changed since; run again with `force` to go ahead (what it replaces becomes
    /// the redo / undo step).
    Confirm { label: String, reasons: Vec<String> },
    /// Can't be done exactly; nothing changed.
    Refused { label: String, reason: String },
    /// Nothing to undo / redo.
    Nothing,
}

fn short(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

fn ref_label(r: &str) -> String {
    r.strip_prefix("refs/heads/").map(str::to_string).or_else(|| r.strip_prefix("refs/tags/").map(|t| format!("tag {t}"))).unwrap_or_else(|| r.to_string())
}

fn diff_names(dir: &Path, a: &str, b: &str, limit: Option<&[String]>) -> GitResult<Vec<(char, String)>> {
    let mut args = vec!["diff-tree", "-r", "-z", "--no-renames", "--name-status", a, b];
    if let Some(l) = limit {
        args.push("--");
        args.extend(l.iter().map(String::as_str));
    }
    let out = git::run(dir, &args)?;
    let mut it = out.split('\0').filter(|s| !s.is_empty());
    let mut v = Vec::new();
    while let (Some(st), Some(p)) = (it.next(), it.next()) {
        v.push((st.chars().next().unwrap_or('M'), p.to_string()));
    }
    Ok(v)
}

fn scope_paths(scope: &Scope) -> Option<&[String]> {
    match &scope.files {
        Files::Paths(p) => Some(p.as_slice()),
        _ => None,
    }
}

/// How the current state differs from what the entry expects (`want`), in words.
fn divergence(dir: &Path, scope: &Scope, want: &Side, cur: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    if want.head_ref != cur.head_ref {
        out.push(match &cur.head_ref {
            Some(r) => format!("HEAD is on {} now", ref_label(r)),
            None => format!("HEAD is detached at {} now", cur.head.as_deref().map(short).unwrap_or("?")),
        });
    } else if want.head_ref.is_none() && want.head != cur.head {
        out.push(format!("HEAD moved to {}", cur.head.as_deref().map(short).unwrap_or("nothing")));
    }
    for (r, v) in &want.refs {
        let c = cur.refs.get(r);
        if c != v.as_ref() {
            out.push(match (c, v) {
                (Some(c), Some(_)) => format!("{} is at {} now", ref_label(r), short(c)),
                (Some(_), None) => format!("{} exists again", ref_label(r)),
                (None, _) => format!("{} was deleted", ref_label(r)),
            });
        }
    }
    for (b, u) in &want.upstreams {
        if cur.upstreams.get(b) != u.as_ref() {
            out.push(format!("the upstream of {b} changed"));
        }
    }
    if scope.index && (want.unmerged != cur.unmerged || want.index != cur.index) {
        out.push(if cur.unmerged { "the index has unresolved conflicts".into() } else { "the staged changes are different now".into() });
    }
    if let (Some(w), Some(c)) = (&want.worktree, &cur.worktree)
        && w != c
    {
        let names = diff_names(dir, w, c, scope_paths(scope)).unwrap_or_default();
        if !names.is_empty() {
            let list: Vec<&str> = names.iter().take(4).map(|(_, p)| p.as_str()).collect();
            let more = if names.len() > 4 { format!(" and {} more", names.len() - 4) } else { String::new() };
            out.push(format!("files changed since: {}{more}", list.join(", ")));
        }
    }
    if let Some(s) = &want.stash
        && cur.stash.as_ref() != Some(s)
    {
        out.push("the stash list changed".into());
    }
    out
}

fn restore_refs(dir: &Path, target: &Side, cur: &Snapshot, msg: &str) -> GitResult<()> {
    let mut cmds = String::new();
    for (r, t) in &target.refs {
        match (t, cur.refs.get(r)) {
            (Some(t), Some(c)) if t == c => {}
            (Some(t), _) => cmds.push_str(&format!("update {r} {t}\n")),
            (None, Some(_)) => cmds.push_str(&format!("delete {r}\n")),
            (None, None) => {}
        }
    }
    if !cmds.is_empty() {
        git::run_input(dir, &["update-ref", "-m", msg, "--stdin"], cmds.as_bytes())?;
    }
    Ok(())
}

fn restore_upstreams(dir: &Path, target: &Side, cur: &Snapshot) -> GitResult<()> {
    for (b, u) in &target.upstreams {
        if cur.upstreams.get(b) == u.as_ref() {
            continue;
        }
        match u {
            Some(u) => {
                git::run(dir, &["config", &format!("branch.{b}.remote"), &u.remote])?;
                git::run(dir, &["config", &format!("branch.{b}.merge"), &u.merge])?;
            }
            None => {
                let _ = git::run(dir, &["config", "--unset", &format!("branch.{b}.remote")]);
                let _ = git::run(dir, &["config", "--unset", &format!("branch.{b}.merge")]);
            }
        }
    }
    Ok(())
}

fn restore_head(dir: &Path, target: &Side, cur: &Snapshot, msg: &str) -> GitResult<()> {
    match (&target.head_ref, &target.head) {
        (Some(r), _) if cur.head_ref.as_ref() != Some(r) => git::run(dir, &["symbolic-ref", "-m", msg, "HEAD", r]).map(|_| ()),
        (None, Some(h)) if cur.head_ref.is_some() || cur.head.as_ref() != Some(h) => git::run(dir, &["update-ref", "--no-deref", "-m", msg, "HEAD", h]).map(|_| ()),
        _ => Ok(()),
    }
}

/// Puts the files that differ between `from` and `to` (trees) back as `to` has them; files `to`
/// doesn't have go to the OS trash.
fn restore_files(dir: &Path, from: &str, to: &str, limit: Option<&[String]>) -> GitResult<()> {
    let changed = diff_names(dir, from, to, limit)?;
    if changed.is_empty() {
        return Ok(());
    }
    type Changed<'a> = Vec<&'a (char, String)>;
    let (gone, write): (Changed, Changed) = changed.iter().partition(|(s, _)| *s == 'D');
    for (_, p) in gone {
        let abs = crate::fsutil::safe_join(dir, p).map_err(|e| e.to_string())?;
        if abs.symlink_metadata().is_ok() {
            super::to_trash(&abs)?;
        }
    }
    if !write.is_empty() {
        let tmp = TempIndex::empty(dir)?;
        let env = tmp.env();
        git::run_env(dir, &["read-tree", to], &env)?;
        let input: Vec<u8> = write.iter().flat_map(|(_, p)| p.bytes().chain(std::iter::once(0))).collect();
        git::run_input_env(dir, &["checkout-index", "-f", "-z", "--stdin"], &input, &env)?;
    }
    Ok(())
}

/// Applies the index entries that differ between `from` and `to` (trees); unknown `from` (the
/// index had conflicts) resets the whole index to `to`.
fn restore_index(dir: &Path, from: Option<&str>, to: &str, cur_unmerged: bool) -> GitResult<()> {
    let Some(from) = from.filter(|_| !cur_unmerged) else {
        return git::run(dir, &["read-tree", "--reset", to]).map(|_| ());
    };
    let raw = git::run(dir, &["diff-tree", "-r", "-z", "--no-renames", "--raw", from, to])?;
    let mut input: Vec<u8> = Vec::new();
    let mut it = raw.split('\0').filter(|s| !s.is_empty());
    while let (Some(meta), Some(path)) = (it.next(), it.next()) {
        // :<old mode> <new mode> <old oid> <new oid> <status>
        let f: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        if f.len() < 5 {
            continue;
        }
        let null = "0".repeat(f[2].len());
        input.extend_from_slice(format!("0 {null}\t{path}\0").as_bytes());
        if !f[4].starts_with('D') {
            input.extend_from_slice(format!("{} {}\t{path}\0", f[1], f[3]).as_bytes());
        }
    }
    if !input.is_empty() {
        git::run_input(dir, &["update-index", "-z", "--index-info"], &input)?;
    }
    Ok(())
}

fn restore_stash(dir: &Path, target: &[StashItem], cur: &[StashItem]) -> GitResult<()> {
    if target == cur {
        return Ok(());
    }
    if git::run(dir, &["show-ref", "--verify", "--quiet", "refs/stash"]).is_ok() {
        git::run(dir, &["update-ref", "-d", "refs/stash"])?;
    }
    for s in target.iter().rev() {
        git::run(dir, &["stash", "store", "-m", &s.message, &s.oid])?;
    }
    Ok(())
}

/// Moves the repository from `expect` (where the entry says it is) to `target`, only for what
/// the entry covers.
fn apply(dir: &Path, scope: &Scope, target: &Side, expect: &Side, cur: &Snapshot, msg: &str) -> GitResult<()> {
    for oid in target.refs.values().flatten().chain(target.head.iter()).chain(target.stash.iter().flatten().map(|s| &s.oid)) {
        if !exists(dir, oid) {
            return Err(format!("{} is no longer in the repository (garbage-collected)", short(oid)));
        }
    }
    restore_refs(dir, target, cur, msg)?;
    restore_head(dir, target, cur, msg)?;
    restore_upstreams(dir, target, cur)?;
    if let (Some(to), Some(from)) = (&target.worktree, &expect.worktree) {
        restore_files(dir, from, to, scope_paths(scope))?;
    }
    if scope.index
        && let Some(to) = &target.index
    {
        restore_index(dir, if expect.unmerged { None } else { expect.index.as_deref() }, to, cur.unmerged)?;
    }
    if let (Some(to), Some(c)) = (&target.stash, &cur.stash) {
        restore_stash(dir, to, c)?;
    }
    // Files written above have new timestamps: refresh the index's stat info so the next
    // operation doesn't take them for modified.
    let _ = git::output(dir, &["update-index", "-q", "--refresh"]);
    Ok(())
}

fn pending_refusal(dir: &Path) -> Option<String> {
    ops::operation(dir).map(|op| {
        let name = match op {
            ops::Operation::Merge => "merge",
            ops::Operation::Rebase => "rebase",
            ops::Operation::CherryPick => "cherry-pick",
            ops::Operation::Revert => "revert",
        };
        format!("A {name} is in progress — finish it (Continue) or abort it first. Abort puts everything back as it was before the {name} started.")
    })
}

/// Undoes the newest entry (call inside [`git::mutate`]).
pub fn undo(dir: &Path, j: &Journal, force: bool) -> GitResult<Outcome> {
    step(dir, j, force, true)
}

/// Redoes the most recently undone entry (call inside [`git::mutate`]).
pub fn redo(dir: &Path, j: &Journal, force: bool) -> GitResult<Outcome> {
    step(dir, j, force, false)
}

fn step(dir: &Path, j: &Journal, force: bool, back: bool) -> GitResult<Outcome> {
    let mut stack = j.load();
    reconcile(dir, j, &mut stack);
    let Some(entry) = (if back { stack.undo.last() } else { stack.redo.last() }).cloned() else {
        j.save(stack)?;
        return Ok(Outcome::Nothing);
    };
    if let Some(reason) = pending_refusal(dir) {
        j.save(stack)?;
        return Ok(Outcome::Refused { label: entry.label, reason });
    }
    let (target, expect) = if back { (&entry.before, &entry.after) } else { (&entry.after, &entry.before) };
    if back {
        // Commits the operation made that were pushed afterwards stay: undoing them would rewrite
        // published history.
        for (r, a) in &entry.after.refs {
            let Some(a) = a else { continue };
            if !r.starts_with("refs/heads/") || entry.before.refs.get(r) == Some(&Some(a.clone())) || entry.published.contains(r) {
                continue;
            }
            if let Some(remote) = on_remote(dir, a, r.trim_start_matches("refs/heads/")) {
                j.save(stack)?;
                return Ok(Outcome::Refused {
                    label: entry.label,
                    reason: format!("{} has been pushed ({remote} contains it), so taking it back would rewrite published history. Revert the commit instead.", short(a)),
                });
            }
        }
    }
    let cur = capture(dir, &entry.scope)?;
    let reasons = divergence(dir, &entry.scope, expect, &cur);
    if !reasons.is_empty() && !force {
        j.save(stack)?;
        return Ok(Outcome::Confirm { label: entry.label, reasons });
    }
    let msg = format!("devall: {} {}", if back { "undo" } else { "redo" }, entry.label);
    apply(dir, &entry.scope, target, expect, &cur, &msg)?;
    let post = capture(dir, &entry.scope)?;
    // The entry moves to the other stack with the states as they really are now (forced: the
    // state it replaced, so the next redo/undo brings it back).
    let keep_refs: BTreeSet<String> = entry.before.refs.keys().cloned().collect();
    let keep_up: BTreeSet<String> = entry.before.upstreams.keys().cloned().collect();
    let mut moved = entry.clone();
    let (s_cur, s_post) = sides(&cur, &post, &keep_refs, &keep_up);
    if back {
        moved.before = s_post;
        moved.after = s_cur;
    } else {
        moved.before = s_cur;
        moved.after = s_post;
    }
    let left = if back { &moved.after } else { &moved.before };
    let other = if back { &moved.before } else { &moved.after };
    let keep = keep_candidates(left, other);
    anchor(dir, j, &moved, &keep, other)?;
    if back {
        stack.undo.pop();
        stack.redo.push(moved);
    } else {
        stack.redo.pop();
        stack.undo.push(moved);
    }
    j.save(stack)?;
    Ok(Outcome::Done { label: entry.label, kind: entry.kind })
}

#[cfg(test)]
#[path = "undo_tests.rs"]
mod tests;
