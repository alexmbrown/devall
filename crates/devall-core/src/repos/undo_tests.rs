use super::*;
use crate::repos::compare::RefKind;
use crate::repos::ops::{self, MergeMode, ResetMode};
use crate::repos::patch::{self, LineAction, Selection};
use crate::repos::testgit::{TestRepo, repo};

fn journal() -> (tempfile::TempDir, Journal) {
    let tmp = tempfile::tempdir().unwrap();
    let j = Journal::new(tmp.path().join(FILE), "repo");
    (tmp, j)
}

fn text(r: &TestRepo, p: &str) -> String {
    String::from_utf8(r.read(p)).unwrap()
}

fn done(o: Outcome) {
    assert!(matches!(o, Outcome::Done { .. }), "{o:?}");
}

fn undo_ok(r: &TestRepo, j: &Journal) {
    git::mutate(r.root(), || undo(r.root(), j, false)).map(done).unwrap();
}

fn redo_ok(r: &TestRepo, j: &Journal) {
    git::mutate(r.root(), || redo(r.root(), j, false)).map(done).unwrap();
}

fn staged(r: &TestRepo) -> String {
    r.git(&["diff", "--cached", "--name-only"]).trim().to_string()
}

/// Expires every reflog and prunes unreachable objects: only what the journal anchors survives.
fn gc_hard(r: &TestRepo) {
    r.git(&["reflog", "expire", "--expire=now", "--expire-unreachable=now", "--all"]);
    r.git(&["gc", "--prune=now", "-q"]);
}

fn base(r: &TestRepo) -> String {
    r.write("a.txt", "one\n");
    r.write("b.txt", "bee\n");
    r.commit("base")
}

#[test]
fn commit_and_amend_round_trip() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    let first = base(&r);
    r.write("a.txt", "two\n");
    ops::stage(r.root(), Some(&j), &["a.txt".into()]).unwrap();
    ops::commit(r.root(), Some(&j), "second", false).unwrap();
    let second = r.sha("HEAD");
    ops::commit(r.root(), Some(&j), "second, better", true).unwrap();
    let amended = r.sha("HEAD");
    assert_ne!(amended, second);
    let h = history(r.root(), &j).unwrap();
    assert_eq!(h.undo.iter().map(|e| e.kind).collect::<Vec<_>>(), [Kind::Amend, Kind::Commit, Kind::Stage]);
    // Only the journal keeps the amended-away commit now.
    gc_hard(&r);
    undo_ok(&r, &j);
    assert_eq!(r.sha("HEAD"), second, "amend undone");
    undo_ok(&r, &j);
    assert_eq!(r.sha("HEAD"), first, "commit undone");
    assert_eq!(staged(&r), "a.txt", "its changes are staged again");
    undo_ok(&r, &j);
    assert_eq!(staged(&r), "");
    assert_eq!(text(&r, "a.txt"), "two\n", "unstaging keeps the file");
    redo_ok(&r, &j);
    redo_ok(&r, &j);
    redo_ok(&r, &j);
    assert_eq!(r.sha("HEAD"), amended, "redo brings the amended commit back");
    assert_eq!(git::mutate(r.root(), || redo(r.root(), &j, false)).unwrap(), Outcome::Nothing);
    // A new operation clears the redo list.
    undo_ok(&r, &j);
    ops::create_tag(r.root(), Some(&j), "t1", "HEAD", None).unwrap();
    assert!(history(r.root(), &j).unwrap().redo.is_empty());
}

#[test]
fn snapshots_are_invisible_to_log_all() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    base(&r);
    r.write("a.txt", "x\n");
    ops::discard(r.root(), Some(&j), &["a.txt".into()]).unwrap();
    let all = r.git(&["log", "--all", "--format=%s"]);
    assert_eq!(all.trim(), "base", "no snapshot commits in git log --all");
    assert!(r.git(&["for-each-ref", "refs/devall/undo/"]).contains("/s"), "the snapshot tree is anchored");
}

#[test]
fn discard_file_untracked_and_lines_come_back() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    base(&r);
    r.write("a.txt", "changed\n");
    r.write("new.txt", "fresh\n");
    r.write("b.txt", "bee\nline2\nline3\n");
    ops::discard(r.root(), Some(&j), &["a.txt".into(), "new.txt".into()]).unwrap();
    assert_eq!(text(&r, "a.txt"), "one\n");
    assert!(!r.path("new.txt").exists());
    let v = patch::file_view(r.root(), "b.txt", false, false, false).unwrap();
    patch::apply_selection(r.root(), Some(&j), "b.txt", LineAction::Discard, &v.fingerprint, &[Selection { hunk: 0, lines: Some(vec![2]) }], false).unwrap();
    assert_eq!(text(&r, "b.txt"), "bee\nline2\n");
    undo_ok(&r, &j);
    assert_eq!(text(&r, "b.txt"), "bee\nline2\nline3\n", "line discard undone");
    undo_ok(&r, &j);
    assert_eq!(text(&r, "a.txt"), "changed\n");
    assert_eq!(text(&r, "new.txt"), "fresh\n", "the untracked file is back");
    redo_ok(&r, &j);
    assert!(!r.path("new.txt").exists());
}

#[test]
fn stage_unstage_hunks() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    base(&r);
    r.write("a.txt", "one\ntwo\n");
    let v = patch::file_view(r.root(), "a.txt", false, false, false).unwrap();
    patch::apply_selection(r.root(), Some(&j), "a.txt", LineAction::Stage, &v.fingerprint, &[Selection { hunk: 0, lines: None }], false).unwrap();
    assert_eq!(staged(&r), "a.txt");
    ops::unstage(r.root(), Some(&j), &["a.txt".into()]).unwrap();
    assert_eq!(staged(&r), "");
    undo_ok(&r, &j);
    assert_eq!(staged(&r), "a.txt");
    undo_ok(&r, &j);
    assert_eq!(staged(&r), "");
    assert_eq!(text(&r, "a.txt"), "one\ntwo\n");
}

#[test]
fn checkout_branches_and_upstreams() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    let first = base(&r);
    ops::create_branch(r.root(), Some(&j), "feature", "HEAD", true).unwrap();
    r.write("f.txt", "f\n");
    r.commit("on feature");
    // Uncommitted change carried across the checkout.
    r.write("b.txt", "dirty\n");
    ops::checkout_ref(r.root(), Some(&j), "main", RefKind::Branch).unwrap();
    assert!(!r.path("f.txt").exists());
    assert_eq!(text(&r, "b.txt"), "dirty\n");
    undo_ok(&r, &j);
    assert_eq!(crate::repos::compare::current_branch(r.root()).as_deref(), Some("feature"));
    assert!(r.path("f.txt").exists());
    assert_eq!(text(&r, "b.txt"), "dirty\n");
    r.write("b.txt", "bee\n");
    ops::checkout_ref(r.root(), Some(&j), "main", RefKind::Branch).unwrap();
    // Rename with an upstream; delete; undo both.
    r.git(&["config", "branch.feature.remote", "origin"]);
    r.git(&["config", "branch.feature.merge", "refs/heads/feature"]);
    ops::rename_branch(r.root(), Some(&j), "feature", "feat").unwrap();
    assert_eq!(r.git(&["config", "branch.feat.remote"]).trim(), "origin");
    let tip = r.sha("feat");
    ops::delete_branch(r.root(), Some(&j), "feat", false).unwrap_err();
    ops::delete_branch(r.root(), Some(&j), "feat", true).unwrap();
    assert!(r.git(&["branch", "--list", "feat"]).trim().is_empty());
    gc_hard(&r);
    undo_ok(&r, &j);
    assert_eq!(r.sha("feat"), tip, "deleted branch restored");
    assert_eq!(r.git(&["config", "branch.feat.merge"]).trim(), "refs/heads/feature", "with its upstream");
    undo_ok(&r, &j);
    assert_eq!(r.sha("feature"), tip);
    assert!(r.git(&["branch", "--list", "feat"]).trim().is_empty());
    assert_eq!(r.git(&["config", "branch.feature.remote"]).trim(), "origin");
    // Detached checkout.
    ops::checkout_detached(r.root(), Some(&j), &first).unwrap();
    undo_ok(&r, &j);
    assert_eq!(crate::repos::compare::current_branch(r.root()).as_deref(), Some("main"));
}

#[test]
fn resets_of_every_kind() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    let first = base(&r);
    r.write("a.txt", "two\n");
    let second = r.commit("second");
    r.write("b.txt", "local edit\n");
    r.write("u.txt", "untracked\n");
    for mode in [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard] {
        ops::reset(r.root(), Some(&j), &first, mode).unwrap();
        assert_eq!(r.sha("HEAD"), first);
        undo_ok(&r, &j);
        assert_eq!(r.sha("HEAD"), second, "{mode:?}");
        assert_eq!(staged(&r), "", "{mode:?}");
        assert_eq!(text(&r, "b.txt"), "local edit\n", "{mode:?}: local edit restored");
        assert_eq!(text(&r, "a.txt"), "two\n", "{mode:?}");
        assert_eq!(text(&r, "u.txt"), "untracked\n");
    }
    // --keep refuses to overwrite local changes to files it would update.
    r.write("a.txt", "mine\n");
    assert!(ops::reset(r.root(), Some(&j), &first, ResetMode::Keep).is_err());
    r.write("a.txt", "two\n");
    ops::reset(r.root(), Some(&j), &first, ResetMode::Keep).unwrap();
    assert_eq!(text(&r, "b.txt"), "local edit\n");
    undo_ok(&r, &j);
    assert_eq!(r.sha("HEAD"), second);
}

#[test]
fn merge_pauses_as_one_entry() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    base(&r);
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("a.txt", "feature\n");
    r.commit("feature edit");
    r.git(&["switch", "-q", "main"]);
    r.write("a.txt", "main\n");
    let main_tip = r.commit("main edit");
    let res = ops::merge(r.root(), Some(&j), "feature", MergeMode::NoFf).unwrap();
    assert!(res.conflicts);
    let h = history(r.root(), &j).unwrap();
    assert!(h.undo[0].pending);
    // Undo is refused while paused.
    let o = git::mutate(r.root(), || undo(r.root(), &j, false)).unwrap();
    assert!(matches!(o, Outcome::Refused { .. }), "{o:?}");
    // Staging while paused isn't journaled on its own.
    r.write("a.txt", "resolved\n");
    ops::stage(r.root(), Some(&j), &["a.txt".into()]).unwrap();
    ops::continue_op(r.root(), Some(&j)).unwrap();
    let h = history(r.root(), &j).unwrap();
    assert_eq!(h.undo.len(), 1, "{h:?}");
    assert!(!h.undo[0].pending);
    assert_eq!(r.git(&["rev-list", "--count", "--merges", "HEAD"]).trim(), "1");
    undo_ok(&r, &j);
    assert_eq!(r.sha("HEAD"), main_tip, "the whole merge is undone");
    assert_eq!(text(&r, "a.txt"), "main\n");
    redo_ok(&r, &j);
    assert_eq!(text(&r, "a.txt"), "resolved\n");
    // An aborted merge leaves no entry.
    undo_ok(&r, &j);
    ops::merge(r.root(), Some(&j), "feature", MergeMode::Ff).unwrap();
    ops::abort(r.root(), Some(&j)).unwrap();
    let h = history(r.root(), &j).unwrap();
    assert!(h.undo.is_empty(), "{h:?}");
    // Fast-forward and squash.
    r.git(&["switch", "-q", "-c", "ahead"]);
    r.write("c.txt", "c\n");
    r.commit("ahead");
    r.git(&["switch", "-q", "main"]);
    ops::merge(r.root(), Some(&j), "ahead", MergeMode::FfOnly).unwrap();
    assert!(r.path("c.txt").exists());
    undo_ok(&r, &j);
    assert!(!r.path("c.txt").exists());
    ops::merge(r.root(), Some(&j), "ahead", MergeMode::Squash).unwrap();
    assert_eq!(staged(&r), "c.txt");
    undo_ok(&r, &j);
    assert_eq!(staged(&r), "");
    assert!(!r.path("c.txt").exists());
}

#[test]
fn rebase_cherry_pick_revert() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    base(&r);
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("f.txt", "f\n");
    let f = r.commit("f");
    r.git(&["switch", "-q", "main"]);
    r.write("m.txt", "m\n");
    let m = r.commit("m");
    r.git(&["switch", "-q", "feature"]);
    ops::rebase(r.root(), Some(&j), "main").unwrap();
    assert!(r.path("m.txt").exists());
    assert_ne!(r.sha("feature"), f);
    undo_ok(&r, &j);
    assert_eq!(r.sha("feature"), f);
    assert!(!r.path("m.txt").exists());
    ops::cherry_pick(r.root(), Some(&j), &m).unwrap();
    assert!(r.path("m.txt").exists());
    undo_ok(&r, &j);
    assert_eq!(r.sha("HEAD"), f);
    ops::revert(r.root(), Some(&j), &f).unwrap();
    assert!(!r.path("f.txt").exists());
    undo_ok(&r, &j);
    assert!(r.path("f.txt").exists());
    assert_eq!(r.sha("HEAD"), f);
}

#[test]
fn tags_round_trip() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    base(&r);
    ops::create_tag(r.root(), Some(&j), "v1", "HEAD", Some("release")).unwrap();
    let tag_obj = r.git(&["rev-parse", "refs/tags/v1"]).trim().to_string();
    ops::delete_tag(r.root(), Some(&j), "v1").unwrap();
    gc_hard(&r);
    undo_ok(&r, &j);
    assert_eq!(r.git(&["rev-parse", "refs/tags/v1"]).trim(), tag_obj, "the annotated tag object survived gc");
    undo_ok(&r, &j);
    assert!(r.git(&["tag", "--list"]).trim().is_empty());
}

#[test]
fn diverged_state_asks_first_and_redo_restores_it() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    base(&r);
    r.write("a.txt", "edit\n");
    ops::discard(r.root(), Some(&j), &["a.txt".into()]).unwrap();
    // Edited again since the discard.
    r.write("a.txt", "newer\n");
    let o = git::mutate(r.root(), || undo(r.root(), &j, false)).unwrap();
    match o {
        Outcome::Confirm { reasons, .. } => assert!(reasons.iter().any(|x| x.contains("a.txt")), "{reasons:?}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(text(&r, "a.txt"), "newer\n", "nothing changed yet");
    git::mutate(r.root(), || undo(r.root(), &j, true)).map(done).unwrap();
    assert_eq!(text(&r, "a.txt"), "edit\n");
    redo_ok(&r, &j);
    assert_eq!(text(&r, "a.txt"), "newer\n", "redo brings back what the forced undo replaced");
}

#[test]
fn pushed_commits_are_not_taken_back_but_pulls_are() {
    let Some(up) = repo() else { return };
    up.write("a.txt", "a\n");
    up.commit("base");
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("bare.git");
    git::run(tmp.path(), &["clone", "-q", "--bare", &up.root().display().to_string(), &bare.display().to_string()]).unwrap();
    let Some(r) = repo() else { return };
    r.git(&["remote", "add", "origin", &bare.display().to_string()]);
    r.git(&["fetch", "-q", "origin"]);
    r.git(&["reset", "-q", "--hard", "origin/main"]);
    r.git(&["branch", "-q", "--set-upstream-to=origin/main"]);
    let (_t, j) = journal();
    r.write("a.txt", "b\n");
    r.git(&["add", "a.txt"]);
    ops::commit(r.root(), Some(&j), "local", false).unwrap();
    ops::push(r.root(), ops::PushMode::Normal, None, None).unwrap();
    let o = git::mutate(r.root(), || undo(r.root(), &j, false)).unwrap();
    match o {
        Outcome::Refused { reason, .. } => assert!(reason.contains("pushed"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // A pull is undoable (its commits were on the remote already).
    let Some(other) = repo() else { return };
    other.git(&["remote", "add", "origin", &bare.display().to_string()]);
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["reset", "-q", "--hard", "origin/main"]);
    other.write("a.txt", "c\n");
    other.commit("from other");
    other.git(&["push", "-q", "origin", "main"]);
    let before = r.sha("HEAD");
    r.git(&["fetch", "-q"]);
    clear(r.root(), &j).unwrap();
    ops::pull(r.root(), Some(&j)).unwrap();
    assert_ne!(r.sha("HEAD"), before);
    undo_ok(&r, &j);
    assert_eq!(r.sha("HEAD"), before);
}

#[test]
fn bounded_history_drops_old_entries_and_their_refs() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    base(&r);
    for i in 0..(MAX_ENTRIES + 3) {
        ops::create_tag(r.root(), Some(&j), &format!("t{i}"), "HEAD", None).unwrap();
    }
    let h = history(r.root(), &j).unwrap();
    assert_eq!(h.undo.len(), MAX_ENTRIES);
    assert_eq!(h.undo.last().unwrap().label, "Create tag t3 at ".to_string() + &r.sha("HEAD")[..7]);
    clear(r.root(), &j).unwrap();
    assert!(r.git(&["for-each-ref", "refs/devall/"]).trim().is_empty());
    assert!(history(r.root(), &j).unwrap().undo.is_empty());
}

#[test]
fn first_commit_and_unborn_head() {
    let Some(r) = repo() else { return };
    let (_t, j) = journal();
    r.write("a.txt", "a\n");
    ops::stage(r.root(), Some(&j), &["a.txt".into()]).unwrap();
    ops::commit(r.root(), Some(&j), "root", false).unwrap();
    undo_ok(&r, &j);
    assert!(git::run(r.root(), &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err(), "back to an unborn branch");
    assert_eq!(staged(&r), "a.txt");
    undo_ok(&r, &j);
    assert_eq!(staged(&r), "");
    assert!(r.path("a.txt").exists());
}
