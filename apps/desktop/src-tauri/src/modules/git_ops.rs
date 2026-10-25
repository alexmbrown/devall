//! Source control, part three: branch / tag / remote / stash management, push, multi-level undo
//! and redo, the merge-conflict editor, interactive rebase, blame and file history, worktrees and
//! the staged diff for an AI commit message. The git work lives in
//! `devall_core::repos::{ops, stash, remotes, undo, conflicts, rebase, blame, worktree}`.
//!
//! Like the other git modules, every command names its repository (`repo`; `None` = the project's
//! default) and the folder is resolved from the project's repo list — never a path from the
//! renderer (worktree paths are checked against `git worktree list`; a new worktree's folder is
//! one the user picked). Local mutations are journaled for undo (`<project data>/git-undo.json`
//! and `refs/devall/undo/*`). Commands that talk to a remote (push, fetch, prune, deleting a
//! remote branch or tag) only run on explicit, confirmed user actions.
//!
//! Registration: `pub mod git_ops;` and the commands below.

use std::path::Path;

use devall_core::repos::blame::{self, Blame, FileVersion};
use devall_core::repos::conflicts::{self, Conflict, ConflictFile, Pick};
use devall_core::repos::ops::{self, MergeMode, OpResult, PushInfo, PushMode, ResetMode};
use devall_core::repos::rebase::{self, RebaseTodo, Step};
use devall_core::repos::remotes::{self, RemoteInfo};
use devall_core::repos::stash::{self, SaveOptions, StashEntry, StashFiles};
use devall_core::repos::undo::{self, History, Outcome};
use devall_core::repos::worktree::{self, Checkout, Worktree};
use devall_core::repos::{self, compare, git};
use serde::Serialize;

use super::git::{blocking, in_repo, in_repo_j, repo_of};

type R<T> = Result<T, String>;

// ---------- branches ----------

#[tauri::command]
pub async fn git_branch_rename(root: String, repo: Option<String>, name: String, new_name: String) -> R<()> {
    in_repo_j(root, repo, move |top, j| ops::rename_branch(top, Some(j), &name, &new_name)).await
}

/// `git branch -d` (or `-D` with `force`, after the UI named the commits it drops).
#[tauri::command]
pub async fn git_branch_delete(root: String, repo: Option<String>, name: String, force: bool) -> R<()> {
    in_repo_j(root, repo, move |top, j| ops::delete_branch(top, Some(j), &name, force)).await
}

/// Commits only this branch has (what a forced delete takes off every branch).
#[tauri::command]
pub async fn git_branch_unmerged(root: String, repo: Option<String>, name: String) -> R<u32> {
    in_repo(root, repo, move |top| ops::unmerged_count(top, &name)).await
}

/// `git push <remote> --delete <branch>` (network; the UI confirms naming the remote branch).
#[tauri::command]
pub async fn git_branch_delete_remote(root: String, repo: Option<String>, remote_branch: String) -> R<String> {
    in_repo(root, repo, move |top| ops::delete_remote_branch(top, &remote_branch)).await
}

#[tauri::command]
pub async fn git_branch_upstream(root: String, repo: Option<String>, branch: String, upstream: Option<String>) -> R<()> {
    in_repo_j(root, repo, move |top, j| ops::set_upstream(top, Some(j), &branch, upstream.as_deref())).await
}

/// Fast-forwards a branch (default: to its upstream) without checking it out.
#[tauri::command]
pub async fn git_branch_ff(root: String, repo: Option<String>, branch: String, from: Option<String>) -> R<String> {
    in_repo_j(root, repo, move |top, j| ops::fast_forward(top, Some(j), &branch, from.as_deref())).await
}

/// Points a branch that isn't checked out at another commit.
#[tauri::command]
pub async fn git_branch_move(root: String, repo: Option<String>, branch: String, rev: String) -> R<()> {
    in_repo_j(root, repo, move |top, j| ops::reset_branch(top, Some(j), &branch, &rev)).await
}

#[tauri::command]
pub async fn git_merge(root: String, repo: Option<String>, rev: String, mode: MergeMode) -> R<OpResult> {
    in_repo_j(root, repo, move |top, j| ops::merge(top, Some(j), &rev, mode)).await
}

#[tauri::command]
pub async fn git_rebase(root: String, repo: Option<String>, onto: String) -> R<OpResult> {
    in_repo_j(root, repo, move |top, j| ops::rebase(top, Some(j), &onto)).await
}

/// Resets the checked-out branch (`soft`, `mixed`, `keep`; `hard` after a strong confirmation —
/// journaled, so undo brings local changes back).
#[tauri::command]
pub async fn git_reset(root: String, repo: Option<String>, rev: String, mode: ResetMode) -> R<()> {
    in_repo_j(root, repo, move |top, j| ops::reset(top, Some(j), &rev, mode)).await
}

#[tauri::command]
pub async fn git_operation_skip(root: String, repo: Option<String>) -> R<OpResult> {
    in_repo_j(root, repo, |top, j| ops::skip(top, Some(j))).await
}

// ---------- tags ----------

#[tauri::command]
pub async fn git_tag_delete(root: String, repo: Option<String>, name: String) -> R<()> {
    in_repo_j(root, repo, move |top, j| ops::delete_tag(top, Some(j), &name)).await
}

#[tauri::command]
pub async fn git_tag_push(root: String, repo: Option<String>, remote: String, name: String) -> R<String> {
    in_repo(root, repo, move |top| ops::push_tag(top, &remote, &name)).await
}

#[tauri::command]
pub async fn git_tag_delete_remote(root: String, repo: Option<String>, remote: String, name: String) -> R<String> {
    in_repo(root, repo, move |top| ops::delete_remote_tag(top, &remote, &name)).await
}

// ---------- remotes ----------

#[tauri::command]
pub async fn git_remotes(root: String, repo: Option<String>) -> R<Vec<RemoteInfo>> {
    in_repo(root, repo, remotes::list).await
}

#[tauri::command]
pub async fn git_remote_add(root: String, repo: Option<String>, name: String, url: String) -> R<()> {
    in_repo(root, repo, move |top| remotes::add(top, &name, &url)).await
}

#[tauri::command]
pub async fn git_remote_rename(root: String, repo: Option<String>, name: String, new_name: String) -> R<()> {
    in_repo(root, repo, move |top| remotes::rename(top, &name, &new_name)).await
}

#[tauri::command]
pub async fn git_remote_set_url(root: String, repo: Option<String>, name: String, url: String) -> R<()> {
    in_repo(root, repo, move |top| remotes::set_url(top, &name, &url)).await
}

#[tauri::command]
pub async fn git_remote_remove(root: String, repo: Option<String>, name: String) -> R<()> {
    in_repo(root, repo, move |top| remotes::remove(top, &name)).await
}

#[tauri::command]
pub async fn git_remote_prune(root: String, repo: Option<String>, name: String) -> R<String> {
    in_repo(root, repo, move |top| remotes::prune(top, &name)).await
}

/// `git fetch [--prune] <remote>` (`None`: every remote).
#[tauri::command]
pub async fn git_fetch(root: String, repo: Option<String>, remote: Option<String>, prune: bool) -> R<String> {
    in_repo(root, repo, move |top| remotes::fetch(top, remote.as_deref(), prune)).await
}

// ---------- stash ----------

#[tauri::command]
pub async fn git_stash_list(root: String, repo: Option<String>) -> R<Vec<StashEntry>> {
    in_repo(root, repo, stash::list).await
}

#[tauri::command]
pub async fn git_stash_save(root: String, repo: Option<String>, options: SaveOptions) -> R<String> {
    in_repo_j(root, repo, move |top, j| stash::save(top, Some(j), &options)).await
}

/// Applies (or pops) the entry at `index`, which must still be `oid`.
#[tauri::command]
pub async fn git_stash_apply(root: String, repo: Option<String>, index: usize, oid: String, pop: bool) -> R<OpResult> {
    in_repo_j(root, repo, move |top, j| stash::apply(top, Some(j), index, &oid, pop)).await
}

#[tauri::command]
pub async fn git_stash_drop(root: String, repo: Option<String>, index: usize, oid: String) -> R<()> {
    in_repo_j(root, repo, move |top, j| stash::drop(top, Some(j), index, &oid)).await
}

#[tauri::command]
pub async fn git_stash_files(root: String, repo: Option<String>, index: usize, oid: String) -> R<StashFiles> {
    in_repo(root, repo, move |top| stash::files(top, index, &oid)).await
}

// ---------- push ----------

#[tauri::command]
pub async fn git_push_info(root: String, repo: Option<String>) -> R<PushInfo> {
    in_repo(root, repo, ops::push_info).await
}

/// Pushes the checked-out branch: normal, with upstream, or `--force-with-lease
/// --force-if-includes` (the UI names the remote branch it overwrites first).
#[tauri::command]
pub async fn git_push(root: String, repo: Option<String>, mode: PushMode, remote: Option<String>, branch: Option<String>) -> R<String> {
    in_repo(root, repo, move |top| ops::push(top, mode, remote.as_deref(), branch.as_deref())).await
}

// ---------- undo / redo ----------

#[tauri::command]
pub async fn git_undo_history(root: String, repo: Option<String>) -> R<History> {
    in_repo_j(root, repo, |top, j| git::mutate(top, || undo::history(top, j))).await
}

/// Undoes the newest journaled operation. `force` goes ahead after a `confirm` outcome.
#[tauri::command]
pub async fn git_undo(root: String, repo: Option<String>, force: Option<bool>) -> R<Outcome> {
    in_repo_j(root, repo, move |top, j| git::mutate(top, || undo::undo(top, j, force.unwrap_or(false)))).await
}

#[tauri::command]
pub async fn git_redo(root: String, repo: Option<String>, force: Option<bool>) -> R<Outcome> {
    in_repo_j(root, repo, move |top, j| git::mutate(top, || undo::redo(top, j, force.unwrap_or(false)))).await
}

/// Forgets the repository's undo history (and removes its `refs/devall/undo/*` anchors).
#[tauri::command]
pub async fn git_undo_clear(root: String, repo: Option<String>) -> R<()> {
    in_repo_j(root, repo, |top, j| git::mutate(top, || undo::clear(top, j))).await
}

// ---------- merge conflicts ----------

#[tauri::command]
pub async fn git_conflicts(root: String, repo: Option<String>) -> R<Vec<ConflictFile>> {
    in_repo(root, repo, conflicts::list).await
}

#[tauri::command]
pub async fn git_conflict_read(root: String, repo: Option<String>, path: String) -> R<Conflict> {
    in_repo(root, repo, move |top| conflicts::read(top, &path)).await
}

/// Writes the edited result and (with `markResolved`) stages it.
#[tauri::command]
pub async fn git_conflict_save(root: String, repo: Option<String>, path: String, content: String, mark_resolved: bool) -> R<()> {
    in_repo(root, repo, move |top| conflicts::save(top, &path, &content, mark_resolved)).await
}

/// Resolves the whole file with one side (text or binary).
#[tauri::command]
pub async fn git_conflict_take(root: String, repo: Option<String>, path: String, side: Pick) -> R<()> {
    in_repo(root, repo, move |top| conflicts::take(top, &path, side)).await
}

/// Resolves the file as deleted.
#[tauri::command]
pub async fn git_conflict_remove(root: String, repo: Option<String>, path: String) -> R<()> {
    in_repo(root, repo, move |top| conflicts::remove(top, &path)).await
}

/// Puts the conflict markers back (`git checkout --conflict=zdiff3`).
#[tauri::command]
pub async fn git_conflict_recreate(root: String, repo: Option<String>, path: String) -> R<()> {
    in_repo(root, repo, move |top| conflicts::recreate(top, &path)).await
}

#[tauri::command]
pub async fn git_conflict_mark(root: String, repo: Option<String>, paths: Vec<String>) -> R<()> {
    in_repo(root, repo, move |top| conflicts::mark_resolved(top, &paths)).await
}

// ---------- interactive rebase ----------

/// The commits `git rebase -i <base>` would list (oldest first).
#[tauri::command]
pub async fn git_rebase_todo(root: String, repo: Option<String>, base: String) -> R<RebaseTodo> {
    in_repo(root, repo, move |top| rebase::todo(top, &base)).await
}

/// The plan reordered the way `--autosquash` would (fixup!/squash! commits after their target).
#[tauri::command]
pub async fn git_rebase_autosquash(root: String, repo: Option<String>, base: String, steps: Vec<Step>) -> R<Vec<Step>> {
    in_repo(root, repo, move |top| Ok(rebase::autosquash(&rebase::todo(top, &base)?.commits, &steps))).await
}

/// Runs the plan (one undo entry; conflicts and `edit` stops pause it).
#[tauri::command]
pub async fn git_rebase_interactive(root: String, repo: Option<String>, base: String, steps: Vec<Step>, update_refs: bool) -> R<OpResult> {
    in_repo_j(root, repo, move |top, j| rebase::run(top, Some(j), &base, &steps, update_refs)).await
}

// ---------- blame & file history ----------

#[tauri::command]
pub async fn git_blame(root: String, repo: Option<String>, path: String, rev: Option<String>) -> R<Blame> {
    in_repo(root, repo, move |top| blame::blame(top, &path, rev.as_deref())).await
}

#[tauri::command]
pub async fn git_file_history(root: String, repo: Option<String>, path: String, limit: Option<usize>) -> R<Vec<FileVersion>> {
    in_repo(root, repo, move |top| blame::file_history(top, &path, limit.unwrap_or(300))).await
}

// ---------- worktrees ----------

#[tauri::command]
pub async fn git_worktrees(root: String, repo: Option<String>) -> R<Vec<Worktree>> {
    in_repo(root, repo, worktree::list).await
}

/// A folder next to the repository for a new worktree of `branch`.
#[tauri::command]
pub async fn git_worktree_suggest(root: String, repo: Option<String>, branch: String) -> R<String> {
    in_repo(root, repo, move |top| Ok(worktree::suggest_path(top, &branch).display().to_string())).await
}

/// `git worktree add` into a folder the user picked (absolute, empty or new).
#[tauri::command]
pub async fn git_worktree_add(root: String, repo: Option<String>, path: String, checkout: Checkout) -> R<Worktree> {
    in_repo(root, repo, move |top| worktree::add(top, &path, &checkout)).await
}

#[tauri::command]
pub async fn git_worktree_lock(root: String, repo: Option<String>, path: String, reason: Option<String>) -> R<()> {
    in_repo(root, repo, move |top| worktree::lock(top, &path, reason.as_deref())).await
}

#[tauri::command]
pub async fn git_worktree_unlock(root: String, repo: Option<String>, path: String) -> R<()> {
    in_repo(root, repo, move |top| worktree::unlock(top, &path)).await
}

/// `git worktree remove` (`force`: even with uncommitted changes — the UI says so first). A
/// worktree opened in the repository list leaves it too.
#[tauri::command]
pub async fn git_worktree_remove(root: String, repo: Option<String>, path: String, force: bool) -> R<()> {
    blocking(move || {
        let r = repo_of(&root, repo.as_deref())?;
        worktree::remove(&r.path, &path, force)?;
        let p = devall_core::Project::open(&root).map_err(|e| e.to_string())?;
        for l in repos::read_local_repos(p.root()) {
            if Path::new(&l.path) == Path::new(&path) {
                let _ = repos::remove_local_repo(&p, &l.name);
            }
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn git_worktree_prune(root: String, repo: Option<String>) -> R<String> {
    in_repo(root, repo, worktree::prune).await
}

/// Shows a worktree of the repository in the project's repository list, on this machine only
/// (not written to devall.yaml). Returns its name there.
#[tauri::command]
pub async fn git_worktree_open(root: String, repo: Option<String>, path: String) -> R<String> {
    blocking(move || {
        let r = repo_of(&root, repo.as_deref())?;
        let w = worktree::list(&r.path)?.into_iter().find(|w| Path::new(&w.path) == Path::new(&path) || std::fs::canonicalize(&w.path).ok() == std::fs::canonicalize(&path).ok()).ok_or_else(|| format!("{path} isn't a worktree of {}", r.name))?;
        let p = devall_core::Project::open(&root).map_err(|e| e.to_string())?;
        let name = format!("{}-{}", r.name, w.branch.as_deref().unwrap_or("detached").replace('/', "-"));
        repos::add_local_repo(&p, Path::new(&w.path), Some(&name)).map_err(|e| e.to_string())
    })
    .await
}

/// Stops showing a repository opened on this machine only (its files stay).
#[tauri::command]
pub async fn git_local_repo_close(root: String, name: String) -> R<()> {
    blocking(move || {
        let p = devall_core::Project::open(&root).map_err(|e| e.to_string())?;
        repos::remove_local_repo(&p, &name).map_err(|e| e.to_string())
    })
    .await
}

// ---------- AI commit message ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedForAi {
    /// `git diff --cached` (lockfiles left out, capped).
    diff: String,
    branch: Option<String>,
    /// Recent commit subjects, for the repository's style.
    recent: Vec<String>,
    truncated: bool,
}

/// Largest staged diff handed to the AI feature (it clips further for the prompt).
const AI_DIFF_CAP: usize = 60_000;

/// What the "Suggest message" button sends to the AI feature (which masks secrets before any
/// model sees it). Read-only.
#[tauri::command]
pub async fn git_staged_for_ai(root: String, repo: Option<String>) -> R<StagedForAi> {
    in_repo(root, repo, |top| {
        let mut args: Vec<String> = ["diff", "--cached", "--no-color", "--no-ext-diff", "--stat", "--patch", "-U2", "--", "."].map(String::from).to_vec();
        for lock in ["package-lock.json", "yarn.lock", "pnpm-lock.yaml", "Cargo.lock", "bun.lock", "Gemfile.lock", "composer.lock", "poetry.lock", "go.sum", "uv.lock"] {
            args.push(format!(":(exclude,glob)**/{lock}"));
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut diff = git::run(top, &refs)?;
        let truncated = diff.len() > AI_DIFF_CAP;
        if truncated {
            let mut cut = AI_DIFF_CAP;
            while !diff.is_char_boundary(cut) {
                cut -= 1;
            }
            diff.truncate(cut);
        }
        let recent = git::run(top, &["log", "-12", "--no-merges", "--format=%s"]).unwrap_or_default().lines().map(str::to_string).collect();
        Ok(StagedForAi { diff, branch: compare::current_branch(top), recent, truncated })
    })
    .await
}
