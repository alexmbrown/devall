//! Interactive rebase driven by a plan the UI made: reorder, pick, reword, squash, fixup, drop,
//! edit — `--update-refs` keeps branches stacked on the rewritten commits pointing at them.
//!
//! git asks an editor for the todo list (`GIT_SEQUENCE_EDITOR`); here that "editor" copies the
//! todo we wrote (`cp '<plan>'`, run by git's own shell on every platform). New commit messages
//! (reword, a squash's combined message) are applied by an `exec git commit --amend -F <file>`
//! line after the commit, so no editor ever opens. The plan and message files live in
//! `<app data>/git-rebase/<id>/` until the rebase is done (old folders are cleaned up).
//!
//! The rebase is one undo entry (journaled as it starts; completed when it finishes — conflicts
//! and `edit` stops go through the usual paused-operation banner and conflict editor).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::compare;
use super::git::{self, GitResult};
use super::ops::{self, OpResult, Undo};
use super::undo::{self, Kind, Op, Scope};

/// A commit the rebase would replay, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoCommit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub body: String,
    pub author: String,
    /// ISO 8601.
    pub date: String,
    /// Other local branches pointing at it (moved along with `--update-refs`).
    pub branches: Vec<String>,
    pub merge: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebaseTodo {
    /// The commit the plan is replayed onto.
    pub base: String,
    pub base_short: String,
    pub branch: Option<String>,
    pub commits: Vec<TodoCommit>,
    /// Merge commits in the range: not supported here.
    pub has_merges: bool,
    /// `--update-refs` is available (Git ≥ 2.38).
    pub can_update_refs: bool,
}

/// The commits of `base..HEAD` (what `git rebase -i <base>` would list).
pub fn todo(dir: &Path, base: &str) -> GitResult<RebaseTodo> {
    let base_sha = compare::resolve(dir, base)?;
    let head = compare::resolve(dir, "HEAD")?;
    if base_sha == head {
        return Err("nothing to rebase: the base is HEAD itself".into());
    }
    let out = git::run(dir, &["log", "--reverse", "-z", "--format=%H%x1f%h%x1f%P%x1f%an%x1f%aI%x1f%s%x1f%b", &format!("{base_sha}..{head}"), "--"])?;
    let current = compare::current_branch(dir);
    let tips = git::run(dir, &["for-each-ref", "--format=%(objectname) %(refname:short)", "refs/heads"]).unwrap_or_default();
    let mut commits = Vec::new();
    for rec in out.split('\0').filter(|r| !r.trim().is_empty()) {
        let f: Vec<&str> = rec.trim_start_matches('\n').splitn(7, '\x1f').collect();
        if f.len() < 7 {
            continue;
        }
        let branches = tips.lines().filter_map(|l| l.split_once(' ')).filter(|(o, b)| *o == f[0] && Some(*b) != current.as_deref()).map(|(_, b)| b.to_string()).collect();
        commits.push(TodoCommit {
            hash: f[0].to_string(),
            short: f[1].to_string(),
            merge: f[2].split_whitespace().count() > 1,
            author: f[3].to_string(),
            date: f[4].to_string(),
            subject: f[5].to_string(),
            body: f[6].trim_end().to_string(),
            branches,
        });
    }
    if commits.is_empty() {
        return Err(format!("{} has no commits after {}", current.as_deref().unwrap_or("HEAD"), &base_sha[..7]));
    }
    Ok(RebaseTodo {
        base_short: base_sha[..7.min(base_sha.len())].to_string(),
        base: base_sha,
        branch: current,
        has_merges: commits.iter().any(|c| c.merge),
        commits,
        can_update_refs: git::version_at_least(2, 38),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    Drop,
}

/// One line of the plan. `message` is the new message of a reword, or of a pick/reword/edit
/// followed by squashes (the combined message).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub action: Action,
    pub hash: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// A todo list for git plus the message files it refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub todo: String,
    /// (file name, message).
    pub messages: Vec<(String, String)>,
}

/// `'…'` for git's shell (forward slashes; `'` escaped).
pub fn sh_quote(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Builds the todo for `steps` over `commits` (see the module docs). Every commit must appear
/// exactly once; the first kept commit can't be a squash/fixup; a reword needs a message.
/// `msg_path(name)` gives the shell-quoted path of a message file.
pub fn build(commits: &[TodoCommit], steps: &[Step], update_refs: bool, msg_path: impl Fn(&str) -> String) -> GitResult<Built> {
    if steps.len() != commits.len() {
        return Err(format!("the plan has {} commits, the branch {}", steps.len(), commits.len()));
    }
    let mut seen = std::collections::HashSet::new();
    for s in steps {
        if !commits.iter().any(|c| c.hash == s.hash) {
            return Err(format!("{} isn't one of the commits being rebased", &s.hash[..s.hash.len().min(7)]));
        }
        if !seen.insert(&s.hash) {
            return Err(format!("{} is in the plan twice", &s.hash[..s.hash.len().min(7)]));
        }
    }
    if let Some(first) = steps.iter().find(|s| s.action != Action::Drop)
        && matches!(first.action, Action::Squash | Action::Fixup)
    {
        return Err("the first commit can't be squashed or fixed up into a previous one".into());
    }
    if steps.iter().all(|s| s.action == Action::Drop) {
        return Err("dropping every commit would leave the branch at the base — use Reset instead".into());
    }
    let commit = |h: &str| commits.iter().find(|c| c.hash == h).expect("checked above");
    // Groups: a kept head plus the squashes/fixups that follow; drops on their own.
    let mut out = String::new();
    let mut messages = Vec::new();
    let mut pending_refs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < steps.len() {
        let s = &steps[i];
        let c = commit(&s.hash);
        if s.action == Action::Drop {
            out.push_str(&format!("drop {} {}\n", c.hash, c.subject));
            pending_refs.extend(c.branches.iter().cloned());
            i += 1;
            continue;
        }
        // A drop between a head and its squashes doesn't break the group (it's written after
        // it); trailing drops stand on their own.
        let mut j = i + 1;
        while j < steps.len() && matches!(steps[j].action, Action::Squash | Action::Fixup | Action::Drop) {
            j += 1;
        }
        while j > i + 1 && steps[j - 1].action == Action::Drop {
            j -= 1;
        }
        let group = &steps[i..j];
        let msg = s.message.as_deref().map(str::trim_end).filter(|m| !m.trim().is_empty());
        if s.action == Action::Reword && msg.is_none() {
            return Err(format!("{} needs a new message", c.short));
        }
        let has_squash = group.iter().any(|g| g.action == Action::Squash);
        let custom = msg.is_some() && (s.action == Action::Reword || has_squash);
        // Refs of commits dropped right before this group point at the commit before it: emit
        // them now (git moves them to HEAD at this point).
        if update_refs {
            for b in pending_refs.drain(..) {
                out.push_str(&format!("update-ref refs/heads/{b}\n"));
            }
        }
        let head_cmd = match s.action {
            Action::Edit => "edit",
            _ => "pick",
        };
        out.push_str(&format!("{head_cmd} {} {}\n", c.hash, c.subject));
        let mut refs: Vec<String> = c.branches.clone();
        let mut drops = String::new();
        for g in &group[1..] {
            let gc = commit(&g.hash);
            refs.extend(gc.branches.iter().cloned());
            if g.action == Action::Drop {
                drops.push_str(&format!("drop {} {}\n", gc.hash, gc.subject));
                continue;
            }
            let cmd = if g.action == Action::Squash && !custom { "squash" } else { "fixup" };
            out.push_str(&format!("{cmd} {} {}\n", gc.hash, gc.subject));
        }
        if custom {
            let name = format!("msg-{}.txt", messages.len());
            out.push_str(&format!("exec git commit --amend --allow-empty --only --no-verify -F {}\n", msg_path(&name)));
            messages.push((name, msg.unwrap_or_default().to_string() + "\n"));
        }
        if update_refs {
            for b in refs {
                out.push_str(&format!("update-ref refs/heads/{b}\n"));
            }
        }
        out.push_str(&drops);
        i = j;
    }
    if update_refs {
        for b in pending_refs {
            out.push_str(&format!("update-ref refs/heads/{b}\n"));
        }
    }
    Ok(Built { todo: out, messages })
}

/// Where plans are written: `<app data>/git-rebase` (the system temp folder without one).
fn plans_dir() -> PathBuf {
    crate::local::app_data_dir().unwrap_or_else(std::env::temp_dir).join("git-rebase")
}

/// Removes plan folders older than a week (a rebase still paused that long is abandoned).
fn clean_old(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 7 * 86400);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Runs `git rebase -i <base>` with `steps` (see [`build`]).
pub fn run(dir: &Path, j: Undo<'_>, base: &str, steps: &[Step], update_refs: bool) -> GitResult<OpResult> {
    if ops::operation(dir).is_some() {
        return Err("another operation is in progress — finish or abort it first".into());
    }
    let t = todo(dir, base)?;
    if t.has_merges {
        return Err("the range contains merge commits — interactive rebase here handles straight histories only".into());
    }
    let update_refs = update_refs && t.can_update_refs;
    let root = plans_dir();
    clean_old(&root);
    let work = root.join(ulid::Ulid::generate().to_string().to_lowercase());
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let built = build(&t.commits, steps, update_refs, |name| sh_quote(&work.join(name)))?;
    for (name, text) in &built.messages {
        std::fs::write(work.join(name), text).map_err(|e| e.to_string())?;
    }
    let plan = work.join("git-rebase-todo");
    std::fs::write(&plan, &built.todo).map_err(|e| e.to_string())?;
    let seq = format!("cp {}", sh_quote(&plan));
    let label = format!("Interactive rebase of {} onto {}", t.branch.as_deref().unwrap_or("HEAD"), t.base_short);
    undo::journaled(dir, j, Op::new(Kind::Rebase, label, Scope::full()), || {
        let mut args = vec!["rebase", "-i", "--no-autosquash"];
        if t.can_update_refs {
            args.push(if update_refs { "--update-refs" } else { "--no-update-refs" });
        }
        args.push(&t.base);
        let out = git::output_env(dir, &args, &[("GIT_SEQUENCE_EDITOR", std::ffi::OsStr::new(&seq)), ("GIT_EDITOR", std::ffi::OsStr::new("true"))])?;
        ops::op_result(dir, &out)
    })
}

/// Reorders a plan the way `--autosquash` would: each `fixup! <subject>` / `squash! <subject>` /
/// `amend! <subject>` commit moves right after the commit it names (by subject or hash prefix),
/// as a fixup/squash.
pub fn autosquash(commits: &[TodoCommit], steps: &[Step]) -> Vec<Step> {
    let subject = |h: &str| commits.iter().find(|c| c.hash == h).map(|c| c.subject.clone()).unwrap_or_default();
    let mut out: Vec<Step> = Vec::new();
    let mut moved: Vec<(usize, Step)> = Vec::new();
    for (i, s) in steps.iter().enumerate() {
        let subj = subject(&s.hash);
        let target = ["fixup! ", "squash! ", "amend! "].iter().find_map(|p| subj.strip_prefix(p).map(|t| (*p, t.trim().to_string())));
        match target {
            Some((p, t)) => {
                let found = steps[..i].iter().position(|x| {
                    let xs = subject(&x.hash);
                    xs == t || (t.len() >= 4 && x.hash.starts_with(&t)) || xs.strip_prefix("fixup! ").is_some_and(|y| y == t)
                });
                match found {
                    Some(k) => moved.push((k, Step { action: if p == "squash! " { Action::Squash } else { Action::Fixup }, hash: s.hash.clone(), message: None })),
                    None => out.push(s.clone()),
                }
            }
            None => out.push(s.clone()),
        }
    }
    // Insert each moved step after its target (and after earlier fixups of the same target).
    for (k, step) in moved {
        let target_hash = &steps[k].hash;
        let pos = out.iter().position(|x| &x.hash == target_hash).map(|p| {
            let mut q = p + 1;
            while q < out.len() && matches!(out[q].action, Action::Fixup | Action::Squash) {
                q += 1;
            }
            q
        });
        match pos {
            Some(q) => out.insert(q, step),
            None => out.push(step),
        }
    }
    out
}

#[cfg(test)]
#[path = "rebase_tests.rs"]
mod tests;
