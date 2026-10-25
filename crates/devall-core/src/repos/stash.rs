//! The stash: list (with each entry's files for a preview), save (message, untracked files,
//! staged changes only), apply, pop and drop — all journaled for undo (a dropped stash comes back).
//! Entries are addressed by index *and* commit id, so a list that changed meanwhile (a stash made
//! in a terminal) never drops or applies the wrong one.

use std::path::Path;

use serde::Serialize;

use super::compare::{self, ChangedFile};
use super::git::{self, GitResult};
use super::ops::{self, OpResult, Undo};
use super::undo::{self, Kind, Op, Scope};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    pub index: usize,
    /// `stash@{0}`.
    pub name: String,
    pub oid: String,
    /// `On main: wip` / `WIP on main: abc1234 subject`.
    pub message: String,
    /// The message without git's `On <branch>:` prefix.
    pub title: String,
    pub branch: Option<String>,
    /// Unix seconds.
    pub time: i64,
    /// The commit it was made on.
    pub base: String,
    /// Holds untracked files (a third parent).
    pub untracked: bool,
}

/// `On main: wip` → (`Some("main")`, `wip`); `WIP on main: abc subject` → (`main`, `abc subject`).
pub fn split_message(m: &str) -> (Option<String>, String) {
    for prefix in ["WIP on ", "On "] {
        if let Some(rest) = m.strip_prefix(prefix)
            && let Some((b, t)) = rest.split_once(": ")
        {
            return (Some(b.to_string()), t.to_string());
        }
    }
    (None, m.to_string())
}

pub fn list(dir: &Path) -> GitResult<Vec<StashEntry>> {
    let out = git::run(dir, &["stash", "list", "--format=%gd%x1f%H%x1f%gs%x1f%ct%x1f%P"])?;
    Ok(out
        .lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let f: Vec<&str> = l.split('\x1f').collect();
            if f.len() < 5 {
                return None;
            }
            let parents: Vec<&str> = f[4].split_whitespace().collect();
            let (branch, title) = split_message(f[2]);
            Some(StashEntry {
                index: i,
                name: f[0].to_string(),
                oid: f[1].to_string(),
                message: f[2].to_string(),
                title,
                branch,
                time: f[3].trim().parse().unwrap_or(0),
                base: parents.first().map(|s| s.to_string()).unwrap_or_default(),
                untracked: parents.len() > 2,
            })
        })
        .collect())
}

/// The entry at `index`, which must still be `oid`.
fn entry(dir: &Path, index: usize, oid: &str) -> GitResult<StashEntry> {
    list(dir)?.into_iter().find(|e| e.index == index && e.oid == oid).ok_or_else(|| "the stash list changed — look at it again".to_string())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SaveOptions {
    pub message: Option<String>,
    /// `-u`: untracked files too.
    pub include_untracked: bool,
    /// `--staged` (Git ≥ 2.35): only what is staged.
    pub staged_only: bool,
    /// `--keep-index`: staged changes stay staged in the working tree too.
    pub keep_index: bool,
}

/// `git stash push`. "No local changes to save" comes back as a message, not an error.
pub fn save(dir: &Path, j: Undo<'_>, opts: &SaveOptions) -> GitResult<String> {
    if opts.staged_only && !git::version_at_least(2, 35) {
        return Err(format!("stashing only staged changes needs Git 2.35 or newer (this is {})", git::version_text()));
    }
    if opts.staged_only && opts.include_untracked {
        return Err("--staged can't include untracked files".into());
    }
    let msg = opts.message.as_deref().map(str::trim).filter(|m| !m.is_empty());
    let label = match msg {
        Some(m) => format!("Stash “{m}”"),
        None => "Stash changes".to_string(),
    };
    undo::journaled(dir, j, Op::new(Kind::StashSave, label, Scope::full()), || {
        let mut args = vec!["stash", "push"];
        if opts.include_untracked {
            args.push("--include-untracked");
        }
        if opts.staged_only {
            args.push("--staged");
        }
        if opts.keep_index {
            args.push("--keep-index");
        }
        if let Some(m) = msg {
            args.extend(["-m", m]);
        }
        let out = git::output(dir, &args)?;
        if out.status.success() { Ok(git::combined(&out)) } else { Err(git::combined(&out)) }
    })
}

/// `git stash apply` / `pop` of one entry (`--index`: restore what was staged as staged). On
/// conflicts the entry stays in the list (git keeps it on a failed pop too).
pub fn apply(dir: &Path, j: Undo<'_>, index: usize, oid: &str, pop: bool) -> GitResult<OpResult> {
    let e = entry(dir, index, oid)?;
    let (kind, verb) = if pop { (Kind::StashPop, "Pop") } else { (Kind::StashApply, "Apply") };
    undo::journaled(dir, j, Op::new(kind, format!("{verb} stash “{}”", e.title), Scope::full()), || {
        let name = e.name.clone();
        let out = git::output(dir, &["stash", if pop { "pop" } else { "apply" }, "--index", &name])?;
        if !out.status.success() && git::combined(&out).contains("--index") {
            // The staged state can't be restored (conflicts in the index): apply without it.
            let out = git::output(dir, &["stash", if pop { "pop" } else { "apply" }, &name])?;
            return ops::op_result(dir, &out);
        }
        ops::op_result(dir, &out)
    })
}

/// `git stash drop` of one entry (the undo journal keeps its commit).
pub fn drop(dir: &Path, j: Undo<'_>, index: usize, oid: &str) -> GitResult<()> {
    let e = entry(dir, index, oid)?;
    let scope = Scope { stash: true, ..Default::default() };
    undo::journaled(dir, j, Op::new(Kind::StashDrop, format!("Drop stash “{}”", e.title), scope), || git::run(dir, &["stash", "drop", "-q", &e.name]).map(|_| ()))
}

/// What a stash entry holds, for the preview: changed tracked files (base → stash) and, when
/// saved with untracked files, those.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashFiles {
    pub base: String,
    pub commit: String,
    pub files: Vec<ChangedFile>,
    /// The commit holding the untracked files.
    pub untracked_commit: Option<String>,
}

pub fn files(dir: &Path, index: usize, oid: &str) -> GitResult<StashFiles> {
    let e = entry(dir, index, oid)?;
    let mut files = compare::range_files(dir, &e.base, Some(&e.oid), false)?;
    let untracked_commit = if e.untracked { git::run(dir, &["rev-parse", "--verify", "--quiet", &format!("{}^3", e.oid)]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) } else { None };
    if let Some(u) = &untracked_commit {
        let empty = compare::empty_tree(dir);
        let mut extra = compare::range_files(dir, &empty, Some(u), false)?;
        for f in &mut extra {
            f.untracked = true;
        }
        files.extend(extra);
        files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(StashFiles { base: e.base, commit: e.oid, files, untracked_commit })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::testgit::repo;
    use crate::repos::undo::{self, Journal, Outcome};

    #[test]
    fn splits_messages() {
        assert_eq!(split_message("On main: wip"), (Some("main".into()), "wip".into()));
        assert_eq!(split_message("WIP on feature/x: abc123 Fix it"), (Some("feature/x".into()), "abc123 Fix it".into()));
        assert_eq!(split_message("custom"), (None, "custom".into()));
    }

    #[test]
    fn save_list_preview_apply_pop_drop_and_undo() {
        let Some(r) = repo() else { return };
        let tmp = tempfile::tempdir().unwrap();
        let j = Journal::new(tmp.path().join("undo.json"), "r");
        r.write("a.txt", "one\n");
        r.commit("base");
        r.write("a.txt", "two\n");
        r.write("new.txt", "untracked\n");
        let msg = save(r.root(), Some(&j), &SaveOptions { message: Some("first".into()), include_untracked: true, ..Default::default() }).unwrap();
        assert!(msg.contains("first") || msg.contains("Saved"), "{msg}");
        assert!(!r.path("new.txt").exists());
        assert_eq!(String::from_utf8(r.read("a.txt")).unwrap(), "one\n");
        let l = list(r.root()).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].title, "first");
        assert!(l[0].untracked);
        let f = files(r.root(), 0, &l[0].oid).unwrap();
        let names: Vec<&str> = f.files.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(names, ["a.txt", "new.txt"]);
        assert!(f.files[1].untracked);
        // Undo the save: files back, stash gone.
        assert!(matches!(undo::undo(r.root(), &j, false).unwrap(), Outcome::Done { .. }));
        assert_eq!(String::from_utf8(r.read("a.txt")).unwrap(), "two\n");
        assert_eq!(String::from_utf8(r.read("new.txt")).unwrap(), "untracked\n");
        assert!(list(r.root()).unwrap().is_empty());
        // Redo: stashed again.
        assert!(matches!(undo::redo(r.root(), &j, false).unwrap(), Outcome::Done { .. }));
        let l = list(r.root()).unwrap();
        assert_eq!(l.len(), 1);
        assert!(!r.path("new.txt").exists());
        // Apply keeps it, a wrong oid is refused, pop removes it.
        assert!(apply(r.root(), Some(&j), 0, "deadbeef", false).is_err());
        let res = apply(r.root(), Some(&j), 0, &l[0].oid, false).unwrap();
        assert!(!res.conflicts);
        assert_eq!(String::from_utf8(r.read("a.txt")).unwrap(), "two\n");
        assert_eq!(list(r.root()).unwrap().len(), 1);
        assert!(matches!(undo::undo(r.root(), &j, false).unwrap(), Outcome::Done { .. }));
        assert_eq!(String::from_utf8(r.read("a.txt")).unwrap(), "one\n");
        apply(r.root(), Some(&j), 0, &l[0].oid, true).unwrap();
        assert!(list(r.root()).unwrap().is_empty());
        assert!(matches!(undo::undo(r.root(), &j, false).unwrap(), Outcome::Done { .. }));
        assert_eq!(list(r.root()).unwrap().len(), 1, "pop undone: the stash is back");
        // Drop and undo the drop.
        r.write("b.txt", "b\n");
        save(r.root(), Some(&j), &SaveOptions { message: Some("second".into()), include_untracked: true, ..Default::default() }).unwrap();
        let l = list(r.root()).unwrap();
        assert_eq!(l.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["second", "first"]);
        drop(r.root(), Some(&j), 1, &l[1].oid).unwrap();
        assert_eq!(list(r.root()).unwrap().len(), 1);
        assert!(matches!(undo::undo(r.root(), &j, false).unwrap(), Outcome::Done { .. }));
        let back = list(r.root()).unwrap();
        assert_eq!(back.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["second", "first"], "order restored");
        assert_eq!(back[1].oid, l[1].oid);
        // Nothing to stash is a message, not an entry.
        git::run(r.root(), &["stash", "clear"]).unwrap();
        git::run(r.root(), &["checkout", "--", "."]).ok();
        git::run(r.root(), &["clean", "-fdq"]).ok();
        let before = undo::history(r.root(), &j).unwrap().undo.len();
        save(r.root(), Some(&j), &SaveOptions::default()).unwrap();
        assert_eq!(undo::history(r.root(), &j).unwrap().undo.len(), before);
    }

    #[test]
    fn staged_only() {
        let Some(r) = repo() else { return };
        if !git::version_at_least(2, 35) {
            return;
        }
        r.write("a.txt", "a\n");
        r.write("b.txt", "b\n");
        r.commit("base");
        r.write("a.txt", "A\n");
        r.write("b.txt", "B\n");
        r.git(&["add", "a.txt"]);
        save(r.root(), None, &SaveOptions { message: Some("staged".into()), staged_only: true, ..Default::default() }).unwrap();
        assert_eq!(String::from_utf8(r.read("a.txt")).unwrap(), "a\n");
        assert_eq!(String::from_utf8(r.read("b.txt")).unwrap(), "B\n", "unstaged change stays");
        assert!(save(r.root(), None, &SaveOptions { staged_only: true, include_untracked: true, ..Default::default() }).is_err());
    }
}
