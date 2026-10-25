use super::*;
use crate::repos::ops::{self, MergeMode};
use crate::repos::testgit::{TestRepo, repo};
use crate::repos::undo::{self, Journal, Outcome};

fn text(r: &TestRepo, p: &str) -> String {
    String::from_utf8(r.read(p)).unwrap()
}

/// main and feature both edit a.txt (and feature deletes d.txt that main edits).
fn diverged() -> Option<TestRepo> {
    let r = repo()?;
    r.write("a.txt", "one\ntwo\nthree\n");
    r.write("d.txt", "dee\n");
    r.commit("base");
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("a.txt", "one\nTWO (feature)\nthree\n");
    std::fs::remove_file(r.path("d.txt")).unwrap();
    r.commit("feature edits");
    r.git(&["switch", "-q", "main"]);
    r.write("a.txt", "one\ntwo (main)\nthree\n");
    r.write("d.txt", "dee, edited on main\n");
    r.commit("main edits");
    Some(r)
}

#[test]
fn describes_stages() {
    let out = ["100644 aaa 1\ta.txt", "100644 bbb 2\ta.txt", "100644 ccc 3\ta.txt", "100644 ddd 1\td.txt", "100644 eee 2\td.txt", ""].join("\0");
    let l = parse_unmerged(&out);
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].description, "both modified");
    assert_eq!(l[1].description, "deleted by them");
    assert!(l[1].base && l[1].ours && !l[1].theirs);
}

#[test]
fn merge_conflict_resolved_in_the_editor_then_undone_as_one() {
    let Some(r) = diverged() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let j = Journal::new(tmp.path().join("u.json"), "r");
    let main_tip = r.sha("HEAD");
    let res = ops::merge(r.root(), Some(&j), "feature", MergeMode::NoFf).unwrap();
    assert!(res.conflicts);
    let files = list(r.root()).unwrap();
    assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["a.txt", "d.txt"]);
    let c = read(r.root(), "a.txt").unwrap();
    assert_eq!(c.operation, Some(Operation::Merge));
    assert_eq!(c.base.text.as_deref(), Some("one\ntwo\nthree\n"));
    assert_eq!(c.ours.text.as_deref(), Some("one\ntwo (main)\nthree\n"));
    assert_eq!(c.theirs.text.as_deref(), Some("one\nTWO (feature)\nthree\n"));
    assert!(c.result.text.as_deref().unwrap().contains("<<<<<<<"));
    assert!(c.labels.ours.contains("main"), "{:?}", c.labels);
    assert!(c.labels.theirs.contains("feature"), "{:?}", c.labels);
    // Re-create markers after editing by hand.
    r.write("a.txt", "garbage\n");
    recreate(r.root(), "a.txt").unwrap();
    assert!(text(&r, "a.txt").contains("<<<<<<<"));
    // Resolve a.txt by hand, d.txt by keeping ours (main's edit).
    save(r.root(), "a.txt", "one\ntwo (both)\nthree\n", true).unwrap();
    let c = read(r.root(), "a.txt").unwrap();
    assert!(c.resolved);
    assert!(recreate(r.root(), "a.txt").is_err(), "already resolved");
    assert_eq!(read(r.root(), "d.txt").unwrap().description, "deleted by them");
    take(r.root(), "d.txt", Pick::Ours).unwrap();
    assert!(list(r.root()).unwrap().is_empty());
    ops::continue_op(r.root(), Some(&j)).unwrap();
    assert_eq!(ops::operation(r.root()), None);
    assert_eq!(text(&r, "a.txt"), "one\ntwo (both)\nthree\n");
    assert_eq!(text(&r, "d.txt"), "dee, edited on main\n");
    let h = undo::history(r.root(), &j).unwrap();
    assert_eq!(h.undo.len(), 1);
    assert!(matches!(git::mutate(r.root(), || undo::undo(r.root(), &j, false)).unwrap(), Outcome::Done { .. }));
    assert_eq!(r.sha("HEAD"), main_tip);
    assert_eq!(text(&r, "a.txt"), "one\ntwo (main)\nthree\n");
}

#[test]
fn deleted_by_them_resolved_as_deleted() {
    let Some(r) = diverged() else { return };
    ops::merge(r.root(), None, "feature", MergeMode::NoFf).unwrap();
    take(r.root(), "d.txt", Pick::Theirs).unwrap();
    assert!(!r.path("d.txt").exists(), "theirs deleted it");
    take(r.root(), "a.txt", Pick::Theirs).unwrap();
    assert_eq!(text(&r, "a.txt"), "one\nTWO (feature)\nthree\n");
    ops::continue_op(r.root(), None).unwrap();
    assert_eq!(ops::operation(r.root()), None);
}

#[test]
fn rebase_conflict_swaps_ours_and_theirs() {
    let Some(r) = diverged() else { return };
    r.git(&["switch", "-q", "feature"]);
    let res = ops::rebase(r.root(), None, "main").unwrap();
    assert!(res.conflicts);
    let c = read(r.root(), "a.txt").unwrap();
    assert_eq!(c.operation, Some(Operation::Rebase));
    // In a rebase "ours" is what we replay onto (main) and "theirs" the commit being replayed.
    assert_eq!(c.ours.text.as_deref(), Some("one\ntwo (main)\nthree\n"));
    assert!(c.labels.ours.contains("rebased onto"), "{:?}", c.labels);
    assert!(c.labels.theirs.contains("feature edits") || c.labels.theirs.contains("your commit"), "{:?}", c.labels);
    save(r.root(), "a.txt", "one\nmerged\nthree\n", true).unwrap();
    remove(r.root(), "d.txt").unwrap();
    let res = ops::continue_op(r.root(), None).unwrap();
    assert!(!res.conflicts);
    assert_eq!(ops::operation(r.root()), None);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "feature edits");
    assert_eq!(text(&r, "a.txt"), "one\nmerged\nthree\n");
}

#[test]
fn cherry_pick_conflict_take_theirs() {
    let Some(r) = diverged() else { return };
    let pick = r.sha("feature");
    ops::cherry_pick(r.root(), None, &pick).unwrap();
    let c = read(r.root(), "a.txt").unwrap();
    assert_eq!(c.operation, Some(Operation::CherryPick));
    assert!(c.labels.theirs.contains(&pick[..7]), "{:?}", c.labels);
    take(r.root(), "a.txt", Pick::Theirs).unwrap();
    take(r.root(), "d.txt", Pick::Ours).unwrap();
    mark_resolved(r.root(), &[]).ok();
    ops::continue_op(r.root(), None).unwrap();
    assert_eq!(text(&r, "a.txt"), "one\nTWO (feature)\nthree\n");
}

#[test]
fn binary_conflicts_pick_a_side() {
    let Some(r) = repo() else { return };
    r.write("img.bin", [0u8, 1, 2, 3]);
    r.commit("base");
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("img.bin", [0u8, 9, 9, 9]);
    r.commit("feature image");
    r.git(&["switch", "-q", "main"]);
    r.write("img.bin", [0u8, 7, 7, 7]);
    r.commit("main image");
    let res = ops::merge(r.root(), None, "feature", MergeMode::NoFf).unwrap();
    assert!(res.conflicts);
    let c = read(r.root(), "img.bin").unwrap();
    assert!(c.binary);
    assert!(c.ours.text.is_none() && c.ours.exists);
    take(r.root(), "img.bin", Pick::Theirs).unwrap();
    assert_eq!(r.read("img.bin"), [0u8, 9, 9, 9]);
    ops::continue_op(r.root(), None).unwrap();
    assert_eq!(ops::operation(r.root()), None);
}

#[test]
fn stash_apply_conflicts_without_an_operation() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "one\n");
    r.commit("base");
    r.write("a.txt", "stashed\n");
    r.git(&["stash", "push", "-q", "-m", "wip"]);
    r.write("a.txt", "committed\n");
    r.commit("moved on");
    let e = crate::repos::stash::list(r.root()).unwrap().remove(0);
    let res = crate::repos::stash::apply(r.root(), None, 0, &e.oid, false).unwrap();
    assert!(res.conflicts);
    assert_eq!(ops::operation(r.root()), None);
    let c = read(r.root(), "a.txt").unwrap();
    assert_eq!(c.operation, None);
    assert_eq!(c.labels.theirs, "stashed changes");
    save(r.root(), "a.txt", "both\n", true).unwrap();
    assert!(list(r.root()).unwrap().is_empty());
}
