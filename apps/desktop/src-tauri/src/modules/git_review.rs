//! Source control, part two: compare & review (a branch against its base the way a merge request
//! shows it, with locally stored "reviewed" marks), the commit graph (lanes laid out in Rust,
//! streamed and paged), hunk/line staging and the commit operations of the graph's menu. The git
//! work lives in `devall_core::repos::{compare, graph, patch, ops, review}`.
//!
//! Like `git.rs`, every command names its repository (`repo`; `None` = the project's default) and
//! the folder is resolved from the project's repo list — never a path from the renderer.
//! Revisions are checked (`compare::valid_rev`) and resolved before they reach git. Mutations run
//! one at a time per repo (`git::mutate`). Nothing here pushes or changes remotes.
//!
//! Registration: `pub mod git_review;`, `.manage(git_review::GitGraphState::default())` and the
//! commands below.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devall_core::repos::compare::{self, ChangedFile, CommitInfo, Comparison, ConflictPrediction, FileDiff, Mode, RangeFiles, RefKind, Side};
use devall_core::repos::graph::{self, GraphJob, GraphPage};
use devall_core::repos::ops::{self, OpResult, RefList};
use devall_core::repos::patch::{self, LineAction, PatchView, Selection};
use devall_core::repos::review::{self, Reviewed};
use devall_core::repos::git;
use serde::{Deserialize, Serialize};
use tauri::State;

use super::git::{blocking, in_repo, repo_of};

type R<T> = Result<T, String>;

// ---------- compare & review ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareDefaults {
    /// The remote's default branch (`origin/main`…), else main/master/develop.
    base: Option<String>,
    /// The checked-out branch (`None` when detached).
    current: Option<String>,
    head: Option<String>,
    git_version: String,
    /// `git merge-tree --write-tree` is available (Git ≥ 2.38).
    can_predict_conflicts: bool,
}

#[tauri::command]
pub async fn git_compare_defaults(root: String, repo: Option<String>) -> R<CompareDefaults> {
    in_repo(root, repo, |top| {
        Ok(CompareDefaults {
            base: compare::default_base(top),
            current: compare::current_branch(top),
            head: git::run(top, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            git_version: git::version_text(),
            can_predict_conflicts: git::version_at_least(2, 38),
        })
    })
    .await
}

/// Branches, remote branches and tags (newest first) for the ref picker and the branches list.
#[tauri::command]
pub async fn git_refs(root: String, repo: Option<String>) -> R<RefList> {
    in_repo(root, repo, ops::refs).await
}

/// Files, commits and header facts of `head` compared with `base`.
#[tauri::command]
pub async fn git_compare(root: String, repo: Option<String>, base: String, head: String, mode: Mode, uncommitted: bool) -> R<Comparison> {
    in_repo(root, repo, move |top| compare::compare(top, &base, &head, mode, uncommitted)).await
}

/// Would merging `head` into `base` conflict, and where (Git ≥ 2.38; read-only).
#[tauri::command]
pub async fn git_compare_conflicts(root: String, repo: Option<String>, base: String, head: String) -> R<ConflictPrediction> {
    in_repo(root, repo, move |top| compare::predict_conflicts(top, &base, &head)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareHint {
    base: String,
    head: String,
    /// Commits on the current branch the base doesn't have.
    ahead: u32,
    behind: u32,
}

/// For the "Review N commits before opening an MR" hint: how far the checked-out branch is ahead
/// of `base` (default: the remote's default branch). `None` when detached, on the base itself or
/// with nothing to review.
#[tauri::command]
pub async fn git_compare_hint(root: String, repo: Option<String>, base: Option<String>) -> R<Option<CompareHint>> {
    in_repo(root, repo, move |top| {
        let Some(head) = compare::current_branch(top) else { return Ok(None) };
        let Some(base) = base.filter(|b| compare::valid_rev(b)).or_else(|| compare::default_base(top)) else { return Ok(None) };
        let (Ok(b), Ok(h)) = (compare::resolve(top, &base), compare::resolve(top, &head)) else { return Ok(None) };
        if b == h || base == head {
            return Ok(None);
        }
        let (behind, ahead) = compare::ahead_behind(top, &b, &h)?;
        Ok((ahead > 0).then_some(CompareHint { base, head, ahead, behind }))
    })
    .await
}

/// The files changed by one commit or a run of commits (oldest's parent → newest).
#[tauri::command]
pub async fn git_commit_range(root: String, repo: Option<String>, oldest: String, newest: String) -> R<RangeFiles> {
    in_repo(root, repo, move |top| compare::commit_range(top, &oldest, &newest)).await
}

/// Both sides of one file for the diff view (text, or data URLs for images).
#[tauri::command]
pub async fn git_file_diff(root: String, repo: Option<String>, old: Side, new: Side, path: String, force: Option<bool>) -> R<FileDiff> {
    in_repo(root, repo, move |top| compare::file_diff(top, &old, &new, &path, force.unwrap_or(false))).await
}

#[tauri::command]
pub async fn git_commit_info(root: String, repo: Option<String>, rev: String) -> R<CommitInfo> {
    in_repo(root, repo, move |top| compare::commit_info(top, &rev)).await
}

/// The files changed between two trees (e.g. a commit's files), for callers that have both ends.
#[tauri::command]
pub async fn git_range_files(root: String, repo: Option<String>, from: String, to: Option<String>, untracked: Option<bool>) -> R<Vec<ChangedFile>> {
    in_repo(root, repo, move |top| compare::range_files(top, &from, to.as_deref(), untracked.unwrap_or(false))).await
}

fn review_store(root: &str) -> PathBuf {
    review::path_for(std::path::Path::new(root))
}

/// The resolved repo's name (marks are keyed by it, not by the renderer's spelling).
fn repo_name(root: &str, repo: Option<&str>) -> R<String> {
    repo_of(root, repo).map(|r| r.name)
}

/// Reviewed files of a comparison (`path → blob id when marked`), from this machine's data folder.
#[tauri::command]
pub async fn git_review_get(root: String, repo: Option<String>, base: String, head: String) -> R<BTreeMap<String, Reviewed>> {
    blocking(move || {
        let name = repo_name(&root, repo.as_deref())?;
        Ok(review::get(&review_store(&root), &name, &base, &head))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewChange {
    path: String,
    /// The blob id to remember; `None` unmarks.
    blob: Option<String>,
}

#[tauri::command]
pub async fn git_review_set(root: String, repo: Option<String>, base: String, head: String, changes: Vec<ReviewChange>) -> R<BTreeMap<String, Reviewed>> {
    blocking(move || {
        let name = repo_name(&root, repo.as_deref())?;
        let changes: Vec<(String, Option<String>)> = changes.into_iter().map(|c| (c.path, c.blob)).collect();
        review::set(&review_store(&root), &name, &base, &head, &changes, chrono::Utc::now().timestamp()).map_err(|e| e.to_string())
    })
    .await
}

// ---------- commit graph ----------

struct Open {
    generation: u64,
    job: Arc<GraphJob>,
}

/// The graphs being read or kept (one per repository folder, a few at most).
#[derive(Default)]
pub struct GitGraphState {
    jobs: Mutex<HashMap<PathBuf, Open>>,
    next: AtomicU64,
}

const KEEP_GRAPHS: usize = 4;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphOpened {
    /// Pass back with page/search requests; a new one means the graph was re-read.
    generation: u64,
    /// The graph was already read and nothing changed.
    reused: bool,
}

/// Starts reading the graph of a repo (every branch, tag and remote branch) — or keeps the one
/// already read when HEAD and the refs are unchanged (and `refresh` isn't set). `wip` adds the
/// uncommitted-changes row on top.
#[tauri::command]
pub async fn git_graph_open(state: State<'_, GitGraphState>, root: String, repo: Option<String>, wip: bool, refresh: Option<bool>) -> R<GraphOpened> {
    let (dir, sig) = blocking(move || {
        let r = repo_of(&root, repo.as_deref())?;
        let sig = graph::signature(&r.path);
        Ok((r.path, sig))
    })
    .await?;
    let mut jobs = state.jobs.lock().unwrap_or_else(|e| e.into_inner());
    if !refresh.unwrap_or(false)
        && let Some(o) = jobs.get(&dir)
        && o.job.signature == sig
        && o.job.wip == wip
    {
        return Ok(GraphOpened { generation: o.generation, reused: true });
    }
    if let Some(old) = jobs.remove(&dir) {
        old.job.cancel();
    }
    while jobs.len() >= KEEP_GRAPHS {
        let Some(k) = jobs.iter().min_by_key(|(_, o)| o.generation).map(|(k, _)| k.clone()) else { break };
        if let Some(o) = jobs.remove(&k) {
            o.job.cancel();
        }
    }
    let generation = state.next.fetch_add(1, Ordering::Relaxed) + 1;
    jobs.insert(dir.clone(), Open { generation, job: GraphJob::start(dir, wip, sig) });
    Ok(GraphOpened { generation, reused: false })
}

/// The graph a page/search request is for (the repo is resolved off the async runtime: reading
/// the project's repo list touches the disk).
async fn job_for(state: &GitGraphState, root: String, repo: Option<String>, generation: u64) -> R<Arc<GraphJob>> {
    let dir = blocking(move || repo_of(&root, repo.as_deref()).map(|r| r.path)).await?;
    let jobs = state.jobs.lock().unwrap_or_else(|e| e.into_inner());
    match jobs.get(&dir) {
        Some(o) if o.generation == generation => Ok(o.job.clone()),
        _ => Err("stale graph — reopen it".into()),
    }
}

/// Rows `offset..offset+limit` (waits up to 15 s for the reader to get there).
#[tauri::command]
pub async fn git_graph_rows(state: State<'_, GitGraphState>, root: String, repo: Option<String>, generation: u64, offset: usize, limit: usize) -> R<GraphPage> {
    let job = job_for(&state, root, repo, generation).await?;
    blocking(move || Ok(job.page(offset, limit.min(2000), Duration::from_secs(15)))).await
}

/// Row indices whose message, author or hash match (among the rows read so far).
#[tauri::command]
pub async fn git_graph_search(state: State<'_, GitGraphState>, root: String, repo: Option<String>, generation: u64, query: String) -> R<Vec<usize>> {
    let job = job_for(&state, root, repo, generation).await?;
    blocking(move || Ok(job.search(&query))).await
}

/// The row of a commit (full hash or unique prefix), waiting for the reader to finish if needed.
#[tauri::command]
pub async fn git_graph_find(state: State<'_, GitGraphState>, root: String, repo: Option<String>, generation: u64, hash: String) -> R<Option<usize>> {
    let job = job_for(&state, root, repo, generation).await?;
    blocking(move || {
        if let Some(i) = job.find(&hash) {
            return Ok(Some(i));
        }
        job.wait_done(Duration::from_secs(20));
        Ok(job.find(&hash))
    })
    .await
}

// ---------- hunk / line staging ----------

/// One changed file's diff as hunks (unstaged: index ↔ working tree; `staged`: HEAD ↔ index).
#[tauri::command]
pub async fn git_patch(root: String, repo: Option<String>, path: String, staged: bool, ignore_ws: Option<bool>, force: Option<bool>) -> R<PatchView> {
    in_repo(root, repo, move |top| patch::file_view(top, &path, staged, ignore_ws.unwrap_or(false), force.unwrap_or(false))).await
}

/// Stages, unstages or discards the picked hunks/lines of one file. `fingerprint` is the shown
/// diff's: when the file changed since, nothing is applied. Discard is destructive — the UI
/// confirms with the exact number of lines first.
#[tauri::command]
pub async fn git_apply_lines(root: String, repo: Option<String>, path: String, action: LineAction, fingerprint: String, selection: Vec<Selection>, ignore_ws: Option<bool>) -> R<()> {
    in_repo(root, repo, move |top| patch::apply_selection(top, &path, action, &fingerprint, &selection, ignore_ws.unwrap_or(false))).await
}

// ---------- commit & ref operations ----------

#[tauri::command]
pub async fn git_branch_create(root: String, repo: Option<String>, name: String, at: String, checkout: bool) -> R<()> {
    in_repo(root, repo, move |top| ops::create_branch(top, &name, &at, checkout)).await
}

#[tauri::command]
pub async fn git_tag_create(root: String, repo: Option<String>, name: String, at: String, message: Option<String>) -> R<()> {
    in_repo(root, repo, move |top| ops::create_tag(top, &name, &at, message.as_deref())).await
}

/// Detached checkout of a commit (the UI confirms; git refuses to overwrite local changes).
#[tauri::command]
pub async fn git_checkout_commit(root: String, repo: Option<String>, rev: String) -> R<()> {
    in_repo(root, repo, move |top| ops::checkout_detached(top, &rev)).await
}

/// Checks out a branch (`kind` branch) or a remote branch as a tracking branch (`kind` remote).
#[tauri::command]
pub async fn git_checkout_ref(root: String, repo: Option<String>, name: String, kind: String) -> R<()> {
    let kind = match kind.as_str() {
        "branch" => RefKind::Branch,
        "remote" => RefKind::Remote,
        other => return Err(format!("can't check out a {other}")),
    };
    in_repo(root, repo, move |top| ops::checkout_ref(top, &name, kind)).await
}

#[tauri::command]
pub async fn git_cherry_pick(root: String, repo: Option<String>, rev: String) -> R<OpResult> {
    in_repo(root, repo, move |top| ops::cherry_pick(top, &rev)).await
}

#[tauri::command]
pub async fn git_revert(root: String, repo: Option<String>, rev: String) -> R<OpResult> {
    in_repo(root, repo, move |top| ops::revert(top, &rev)).await
}

/// `git <merge|rebase|cherry-pick|revert> --abort` (the UI confirms).
#[tauri::command]
pub async fn git_operation_abort(root: String, repo: Option<String>) -> R<()> {
    in_repo(root, repo, ops::abort).await
}

#[tauri::command]
pub async fn git_operation_continue(root: String, repo: Option<String>) -> R<String> {
    in_repo(root, repo, ops::continue_op).await
}

/// Removes an `index.lock` older than two minutes (left behind by a git that crashed).
#[tauri::command]
pub async fn git_clear_lock(root: String, repo: Option<String>) -> R<bool> {
    in_repo(root, repo, git::clear_stale_lock).await
}
