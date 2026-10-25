use super::*;
use crate::repos::testgit::{TestRepo, repo};
use crate::repos::undo::{Journal, Outcome};

fn c(hash: &str, subject: &str, branches: &[&str]) -> TodoCommit {
    TodoCommit { hash: hash.into(), short: hash[..7.min(hash.len())].into(), subject: subject.into(), body: String::new(), author: "A".into(), date: String::new(), branches: branches.iter().map(|b| b.to_string()).collect(), merge: false }
}

fn step(action: Action, hash: &str, message: Option<&str>) -> Step {
    Step { action, hash: hash.into(), message: message.map(str::to_string) }
}

#[test]
fn builds_todos() {
    let commits = [c("aaaaaaa1", "one", &[]), c("bbbbbbb2", "two", &["stack/a"]), c("ccccccc3", "three", &[]), c("ddddddd4", "four", &["stack/b"])];
    let q = |n: &str| format!("'/plans/{n}'");
    // Reorder, reword, squash with a combined message, drop.
    let steps = [step(Action::Reword, "ccccccc3", Some("Three, reworded")), step(Action::Pick, "aaaaaaa1", Some("One and two")), step(Action::Squash, "bbbbbbb2", None), step(Action::Drop, "ddddddd4", None)];
    let b = build(&commits, &steps, true, q).unwrap();
    assert_eq!(
        b.todo,
        "pick ccccccc3 three\nexec git commit --amend --allow-empty --only --no-verify -F '/plans/msg-0.txt'\n\
         pick aaaaaaa1 one\nfixup bbbbbbb2 two\nexec git commit --amend --allow-empty --only --no-verify -F '/plans/msg-1.txt'\nupdate-ref refs/heads/stack/a\n\
         drop ddddddd4 four\nupdate-ref refs/heads/stack/b\n"
    );
    assert_eq!(b.messages, [("msg-0.txt".to_string(), "Three, reworded\n".to_string()), ("msg-1.txt".to_string(), "One and two\n".to_string())]);
    // Plain fixups and edits; no update-refs.
    let steps = [step(Action::Edit, "aaaaaaa1", None), step(Action::Fixup, "bbbbbbb2", None), step(Action::Pick, "ccccccc3", None), step(Action::Squash, "ddddddd4", None)];
    let b = build(&commits, &steps, false, q).unwrap();
    assert_eq!(b.todo, "edit aaaaaaa1 one\nfixup bbbbbbb2 two\npick ccccccc3 three\nsquash ddddddd4 four\n");
    assert!(b.messages.is_empty());
    // Invalid plans.
    assert!(build(&commits, &steps[..3], false, q).is_err(), "missing commit");
    let dup = [step(Action::Pick, "aaaaaaa1", None), step(Action::Pick, "aaaaaaa1", None), step(Action::Pick, "ccccccc3", None), step(Action::Pick, "ddddddd4", None)];
    assert!(build(&commits, &dup, false, q).is_err());
    let first_squash = [step(Action::Drop, "aaaaaaa1", None), step(Action::Squash, "bbbbbbb2", None), step(Action::Pick, "ccccccc3", None), step(Action::Pick, "ddddddd4", None)];
    assert!(build(&commits, &first_squash, false, q).unwrap_err().contains("first commit"));
    let no_msg = [step(Action::Reword, "aaaaaaa1", None), step(Action::Pick, "bbbbbbb2", None), step(Action::Pick, "ccccccc3", None), step(Action::Pick, "ddddddd4", None)];
    assert!(build(&commits, &no_msg, false, q).is_err());
    let all_drop: Vec<Step> = commits.iter().map(|x| step(Action::Drop, &x.hash, None)).collect();
    assert!(build(&commits, &all_drop, false, q).is_err());
    let foreign = [step(Action::Pick, "eeeeeee5", None), step(Action::Pick, "bbbbbbb2", None), step(Action::Pick, "ccccccc3", None), step(Action::Pick, "ddddddd4", None)];
    assert!(build(&commits, &foreign, false, q).is_err());
}

#[test]
fn quotes_paths_for_the_shell() {
    assert_eq!(sh_quote(Path::new("C:\\Users\\O'Brien\\plan")), "'C:/Users/O'\\''Brien/plan'");
}

#[test]
fn autosquash_moves_fixups_after_their_target() {
    let commits = [c("aaaaaaa1", "Add login", &[]), c("bbbbbbb2", "Add logout", &[]), c("ccccccc3", "fixup! Add login", &[]), c("ddddddd4", "squash! Add logout", &[])];
    let plan: Vec<Step> = commits.iter().map(|x| step(Action::Pick, &x.hash, None)).collect();
    let out = autosquash(&commits, &plan);
    let got: Vec<(Action, &str)> = out.iter().map(|s| (s.action, s.hash.as_str())).collect();
    assert_eq!(got, [(Action::Pick, "aaaaaaa1"), (Action::Fixup, "ccccccc3"), (Action::Pick, "bbbbbbb2"), (Action::Squash, "ddddddd4")]);
}

fn line_repo() -> Option<(TestRepo, String)> {
    let r = repo()?;
    r.write("base.txt", "base\n");
    let base = r.commit("base");
    for (i, name) in ["one", "two", "three"].iter().enumerate() {
        r.write(&format!("{name}.txt"), format!("{i}\n"));
        r.commit(name);
    }
    Some((r, base))
}

fn subjects(r: &TestRepo, base: &str) -> Vec<String> {
    r.git(&["log", "--reverse", "--format=%s", &format!("{base}..HEAD")]).lines().map(str::to_string).collect()
}

#[test]
fn runs_reorder_reword_squash_drop_and_undo() {
    let Some((r, base)) = line_repo() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let j = Journal::new(tmp.path().join("u.json"), "r");
    let before = r.sha("HEAD");
    let t = todo(r.root(), &base).unwrap();
    assert_eq!(t.commits.iter().map(|x| x.subject.as_str()).collect::<Vec<_>>(), ["one", "two", "three"]);
    let [one, two, three] = [&t.commits[0].hash, &t.commits[1].hash, &t.commits[2].hash];
    let steps = [step(Action::Reword, three, Some("Three first")), step(Action::Pick, one, Some("One + two")), step(Action::Squash, two, None)];
    let res = run(r.root(), Some(&j), &base, &steps, false).unwrap();
    assert!(!res.conflicts && !res.paused, "{res:?}");
    assert_eq!(subjects(&r, &base), ["Three first", "One + two"]);
    assert!(r.path("two.txt").exists(), "squashed changes kept");
    assert!(matches!(git::mutate(r.root(), || crate::repos::undo::undo(r.root(), &j, false)).unwrap(), Outcome::Done { .. }));
    assert_eq!(r.sha("HEAD"), before, "undo restores the original branch");
    // Drop.
    let t = todo(r.root(), &base).unwrap();
    let steps: Vec<Step> = t.commits.iter().map(|x| step(if x.subject == "two" { Action::Drop } else { Action::Pick }, &x.hash, None)).collect();
    run(r.root(), None, &base, &steps, false).unwrap();
    assert_eq!(subjects(&r, &base), ["one", "three"]);
    assert!(!r.path("two.txt").exists());
}

#[test]
fn edit_pauses_and_update_refs_moves_stacked_branches() {
    let Some((r, base)) = line_repo() else { return };
    if !git::version_at_least(2, 38) {
        return;
    }
    let t = todo(r.root(), &base).unwrap();
    r.git(&["branch", "stack", &t.commits[1].hash]);
    let t = todo(r.root(), &base).unwrap();
    assert_eq!(t.commits[1].branches, ["stack"]);
    let steps = [step(Action::Pick, &t.commits[0].hash, Some("One!")), step(Action::Edit, &t.commits[1].hash, None), step(Action::Pick, &t.commits[2].hash, None)];
    // `pick` with a message doesn't reword (only reword/squash groups do).
    let res = run(r.root(), None, &base, &steps, true).unwrap();
    assert!(res.paused && !res.conflicts, "{res:?}");
    assert_eq!(crate::repos::ops::operation(r.root()), Some(crate::repos::ops::Operation::Rebase));
    r.write("two.txt", "edited during the rebase\n");
    r.git(&["add", "two.txt"]);
    r.git(&["commit", "-q", "--amend", "--no-edit"]);
    crate::repos::ops::continue_op(r.root(), None).unwrap();
    assert_eq!(crate::repos::ops::operation(r.root()), None);
    assert_eq!(subjects(&r, &base), ["one", "two", "three"]);
    let stack_tip = r.sha("stack");
    assert_ne!(stack_tip, t.commits[1].hash, "the stacked branch moved to the rewritten commit");
    assert_eq!(r.git(&["log", "-1", "--format=%s", "stack"]).trim(), "two");
    assert_eq!(r.git(&["show", "stack:two.txt"]).trim(), "edited during the rebase");
}

#[test]
fn conflicting_reorder_stops_for_the_editor() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "0\n");
    let base = r.commit("base");
    r.write("a.txt", "1\n");
    r.commit("one");
    r.write("a.txt", "2\n");
    r.commit("two");
    let t = todo(r.root(), &base).unwrap();
    let steps = [step(Action::Pick, &t.commits[1].hash, None), step(Action::Pick, &t.commits[0].hash, None)];
    let res = run(r.root(), None, &base, &steps, false).unwrap();
    assert!(res.conflicts, "{res:?}");
    assert!(run(r.root(), None, &base, &steps, false).is_err(), "one at a time");
    crate::repos::ops::abort(r.root(), None).unwrap();
    assert_eq!(subjects(&r, &base), ["one", "two"]);
    assert!(todo(r.root(), "HEAD").is_err(), "nothing after HEAD");
}
