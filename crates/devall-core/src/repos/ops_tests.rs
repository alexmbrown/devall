use super::*;
use crate::repos::testgit::{TestRepo, repo};

fn text(r: &TestRepo, p: &str) -> String {
    String::from_utf8(r.read(p)).unwrap()
}

#[test]
fn lists_refs_and_creates_branches_and_tags() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "a\n");
    let first = r.commit("first");
    r.write("a.txt", "b\n");
    r.commit("second");
    create_branch(r.root(), None, "fix/one", &first, false).unwrap();
    create_tag(r.root(), None, "v1", &first, Some("release one")).unwrap();
    create_tag(r.root(), None, "light", "HEAD", None).unwrap();
    assert!(create_branch(r.root(), None, "bad name", "HEAD", false).is_err());
    assert!(create_tag(r.root(), None, "-x", "HEAD", None).is_err());
    let l = refs(r.root()).unwrap();
    assert_eq!(l.current.as_deref(), Some("main"));
    assert!(!l.detached);
    let find = |n: &str| l.refs.iter().find(|x| x.name == n).unwrap();
    assert_eq!(find("fix/one").sha, first);
    assert_eq!(find("v1").sha, first, "annotated tags are peeled");
    assert_eq!(find("v1").kind, RefKind::Tag);
    assert!(find("v1").annotated);
    assert!(!find("light").annotated);
    assert!(find("main").current);
    create_branch(r.root(), None, "work", "HEAD~1", true).unwrap();
    assert_eq!(compare::current_branch(r.root()).as_deref(), Some("work"));
    checkout_detached(r.root(), None, "main").unwrap();
    assert!(refs(r.root()).unwrap().detached);
    checkout_ref(r.root(), None, "main", RefKind::Branch).unwrap();
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
    let ok = cherry_pick(r.root(), None, &f).unwrap();
    assert!(!ok.conflicts);
    assert!(r.path("f.txt").exists());
    let picked = r.sha("HEAD");
    revert(r.root(), None, &picked).unwrap();
    assert!(!r.path("f.txt").exists());
    r.write("a.txt", "main\n");
    r.commit("edit a on main");
    let res = cherry_pick(r.root(), None, &clash).unwrap();
    assert!(res.conflicts);
    assert_eq!(operation(r.root()), Some(Operation::CherryPick));
    abort(r.root(), None).unwrap();
    assert_eq!(operation(r.root()), None);
    assert_eq!(text(&r, "a.txt"), "main\n");
    // Continue after resolving.
    cherry_pick(r.root(), None, &clash).unwrap();
    r.write("a.txt", "resolved\n");
    r.git(&["add", "a.txt"]);
    continue_op(r.root(), None).unwrap();
    assert_eq!(operation(r.root()), None);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "edit a on feature");
    // Skip drops the commit being applied.
    r.write("a.txt", "again\n");
    r.commit("another main edit");
    let res = cherry_pick(r.root(), None, &clash).unwrap();
    assert!(res.conflicts);
    skip(r.root(), None).unwrap();
    assert_eq!(operation(r.root()), None);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "another main edit");
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
    checkout_ref(&clone, None, "origin/topic", RefKind::Remote).unwrap();
    assert_eq!(compare::current_branch(&clone).as_deref(), Some("topic"));
    let l = refs(&clone).unwrap();
    assert_eq!(l.refs.iter().find(|x| x.name == "topic").and_then(|x| x.upstream.as_deref()), Some("origin/topic"));
    assert!(!l.refs.iter().any(|x| x.name.ends_with("/HEAD")));
    assert_eq!(l.remotes, ["origin"]);
}

#[test]
fn parses_tracking() {
    assert_eq!(parse_track("ahead 2, behind 3"), (2, 3, false));
    assert_eq!(parse_track("behind 1"), (0, 1, false));
    assert_eq!(parse_track("gone"), (0, 0, true));
}

#[test]
fn scrubs_credentials_from_output() {
    let t = "fatal: unable to access 'https://bob:ghp_abc123@github.com/acme/x.git/': 403\nTo https://tok@gitlab.com/a/b.git";
    let s = scrub(t);
    assert!(!s.contains("ghp_abc123") && !s.contains("tok@"), "{s}");
    assert!(s.contains("https://github.com/acme/x.git"));
    assert_eq!(scrub("git@github.com:acme/x.git"), "git@github.com:acme/x.git");
}

#[test]
fn rename_delete_and_unmerged_count() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "a\n");
    r.commit("base");
    r.git(&["branch", "merged"]);
    r.git(&["switch", "-q", "-c", "work"]);
    r.write("w.txt", "w\n");
    r.commit("w1");
    r.write("w.txt", "w2\n");
    r.commit("w2");
    r.git(&["switch", "-q", "main"]);
    assert_eq!(unmerged_count(r.root(), "work").unwrap(), 2);
    assert_eq!(unmerged_count(r.root(), "merged").unwrap(), 0);
    rename_branch(r.root(), None, "work", "work2").unwrap();
    assert!(rename_branch(r.root(), None, "nope", "x").is_err());
    let err = delete_branch(r.root(), None, "work2", false).unwrap_err();
    assert!(err.contains("not fully merged"), "{err}");
    delete_branch(r.root(), None, "work2", true).unwrap();
    delete_branch(r.root(), None, "merged", false).unwrap();
    assert_eq!(r.git(&["branch", "--format=%(refname:short)"]).trim(), "main");
}

/// A clone of a bare repository made from `up`, with `main` tracking `origin/main`.
fn clone_of(up: &TestRepo, tmp: &Path) -> (std::path::PathBuf, TestRepo) {
    let bare = tmp.join("bare.git");
    git::run(tmp, &["clone", "-q", "--bare", &up.root().display().to_string(), &bare.display().to_string()]).unwrap();
    let r = repo().unwrap();
    r.git(&["remote", "add", "origin", &bare.display().to_string()]);
    r.git(&["fetch", "-q", "origin"]);
    r.git(&["reset", "-q", "--hard", "origin/main"]);
    r.git(&["branch", "-q", "--set-upstream-to=origin/main"]);
    (bare, r)
}

#[test]
fn upstreams_fast_forward_and_pushes() {
    let Some(up) = repo() else { return };
    up.write("a.txt", "a\n");
    up.commit("base");
    let tmp = tempfile::tempdir().unwrap();
    let (bare, r) = clone_of(&up, tmp.path());
    // A second clone pushes a commit; fast-forward main without checking it out.
    let other = repo().unwrap();
    other.git(&["remote", "add", "origin", &bare.display().to_string()]);
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["reset", "-q", "--hard", "origin/main"]);
    other.write("a.txt", "b\n");
    let upstream_tip = other.commit("upstream work");
    other.git(&["push", "-q", "origin", "main"]);
    r.git(&["fetch", "-q"]);
    r.git(&["switch", "-q", "-c", "side"]);
    fast_forward(r.root(), None, "main", None).unwrap();
    assert_eq!(r.sha("main"), upstream_tip);
    assert_eq!(compare::current_branch(r.root()).as_deref(), Some("side"), "still on side");
    // Diverged: refused.
    r.git(&["switch", "-q", "main"]);
    r.write("c.txt", "c\n");
    r.commit("local");
    other.write("a.txt", "c\n");
    other.commit("more upstream");
    other.git(&["push", "-q", "origin", "main"]);
    r.git(&["fetch", "-q"]);
    let err = fast_forward(r.root(), None, "main", None).unwrap_err();
    assert!(err.contains("Not possible") || err.contains("fast-forward") || err.contains("diverged"), "{err}");
    // Upstream set/unset.
    set_upstream(r.root(), None, "side", Some("origin/main")).unwrap();
    assert_eq!(refs(r.root()).unwrap().refs.iter().find(|x| x.name == "side").unwrap().upstream.as_deref(), Some("origin/main"));
    set_upstream(r.root(), None, "side", None).unwrap();
    assert_eq!(refs(r.root()).unwrap().refs.iter().find(|x| x.name == "side").unwrap().upstream, None);
    // Push a new branch with upstream, force-with-lease after rewriting it.
    r.git(&["switch", "-q", "-c", "topic"]);
    r.write("t.txt", "1\n");
    r.commit("t1");
    let info = push_info(r.root()).unwrap();
    assert_eq!(info.upstream, None);
    assert_eq!(info.remote.as_deref(), Some("origin"));
    push(r.root(), PushMode::SetUpstream, None, None).unwrap();
    let info = push_info(r.root()).unwrap();
    assert_eq!(info.upstream.as_deref(), Some("origin/topic"));
    r.write("t.txt", "2\n");
    r.git(&["commit", "-q", "-am", "t1 rewritten", "--amend"]);
    assert!(push(r.root(), PushMode::Normal, None, None).is_err(), "non-fast-forward");
    push(r.root(), PushMode::ForceWithLease, None, None).unwrap();
    assert_eq!(git::run(&bare, &["rev-parse", "topic"]).unwrap().trim(), r.sha("HEAD"));
    // Someone else updates topic: the lease protects their work.
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["switch", "-q", "-c", "topic", "origin/topic"]);
    other.write("o.txt", "o\n");
    other.commit("their topic work");
    other.git(&["push", "-q", "origin", "topic"]);
    r.git(&["commit", "-q", "--allow-empty", "-m", "mine"]);
    let err = push(r.root(), PushMode::ForceWithLease, None, None).unwrap_err();
    assert!(err.contains("stale info") || err.contains("rejected"), "{err}");
    // Tags and remote deletes.
    create_tag(r.root(), None, "v9", "HEAD", Some("nine")).unwrap();
    push_tag(r.root(), "origin", "v9").unwrap();
    assert!(git::run(&bare, &["rev-parse", "--verify", "refs/tags/v9"]).is_ok());
    delete_remote_tag(r.root(), "origin", "v9").unwrap();
    assert!(git::run(&bare, &["rev-parse", "--verify", "--quiet", "refs/tags/v9"]).is_err());
    assert!(push_tag(r.root(), "nope", "v9").is_err());
    delete_remote_branch(r.root(), "origin/topic").unwrap();
    assert!(git::run(&bare, &["rev-parse", "--verify", "--quiet", "refs/heads/topic"]).is_err());
    assert!(delete_remote_branch(r.root(), "topic").is_err(), "not a remote branch");
}

#[test]
fn merge_modes_rebase_and_reset_branch() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "a\n");
    let base = r.commit("base");
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("f.txt", "f\n");
    r.commit("f");
    r.git(&["switch", "-q", "main"]);
    let res = merge(r.root(), None, "feature", MergeMode::NoFf).unwrap();
    assert!(!res.conflicts);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "Merge branch 'feature'");
    r.git(&["reset", "-q", "--hard", &base]);
    merge(r.root(), None, "feature", MergeMode::Ff).unwrap();
    assert_eq!(r.sha("HEAD"), r.sha("feature"), "fast-forwarded");
    r.git(&["reset", "-q", "--hard", &base]);
    r.write("m.txt", "m\n");
    r.commit("m");
    assert!(merge(r.root(), None, "feature", MergeMode::FfOnly).is_err());
    assert!(merge(r.root(), None, "--evil", MergeMode::Ff).is_err());
    r.git(&["switch", "-q", "feature"]);
    rebase(r.root(), None, "main").unwrap();
    assert!(r.path("m.txt").exists());
    reset_branch(r.root(), None, "main", &base).unwrap();
    assert_eq!(r.sha("main"), base);
    assert!(reset_branch(r.root(), None, "feature", &base).is_err(), "checked out");
}

#[test]
fn working_tree_operations() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "a\n");
    r.commit("base");
    r.write("a.txt", "b\n");
    r.write("we[i]rd.txt", "glob chars\n");
    r.write("weird.txt", "matched by the glob, not the name\n");
    stage(r.root(), None, &["we[i]rd.txt".into()]).unwrap();
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]).trim(), "we[i]rd.txt", "literal pathspec");
    unstage(r.root(), None, &["we[i]rd.txt".into()]).unwrap();
    assert!(stage(r.root(), None, &["../x".into()]).is_err());
    assert!(stage(r.root(), None, &[]).is_err());
    discard(r.root(), None, &["a.txt".into(), "we[i]rd.txt".into()]).unwrap();
    assert!(r.path("weird.txt").exists(), "only the literal name went");
    assert_eq!(text(&r, "a.txt"), "a\n");
    assert!(!r.path("we[i]rd.txt").exists());
    assert!(commit(r.root(), None, "  ", false).is_err());
    r.write("a.txt", "c\n");
    stage(r.root(), None, &["a.txt".into()]).unwrap();
    commit(r.root(), None, "second", false).unwrap();
    commit(r.root(), None, "", true).unwrap();
    assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "second", "amend without a message keeps it");
}
