use super::*;
use crate::repos::testgit::{TestRepo, repo};

fn paths(files: &[ChangedFile]) -> Vec<(&str, &str)> {
    files.iter().map(|f| (f.path.as_str(), f.status.as_str())).collect()
}

/// main: a.txt → feature branches off (adds f.txt, edits a.txt) → main moves on (adds m.txt).
fn diverged() -> Option<TestRepo> {
    let r = repo()?;
    r.write("a.txt", "one\ntwo\nthree\n");
    r.commit("base");
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("f.txt", "feature\n");
    r.write("a.txt", "one\ntwo\nthree\nfour\n");
    r.commit("feature work");
    r.git(&["switch", "-q", "main"]);
    r.write("m.txt", "main\n");
    r.commit("main moves on");
    Some(r)
}

#[test]
fn revs_from_the_ui_are_checked() {
    for ok in ["main", "origin/feature/x", "v1.2.0", "HEAD~2", "abc1234", "@{u}", "release_1+2"] {
        assert!(valid_rev(ok), "{ok}");
    }
    for bad in ["", "-x", "--output=x", "a..b", "a...b", "HEAD:file", "a b", "x\n", "a\\b", "*"] {
        assert!(!valid_rev(bad), "{bad}");
    }
    assert!(valid_oid("4b825dc642cb6eb9a060e54bf8d69288fbee4904"));
    assert!(!valid_oid("xyz1234"));
}

#[test]
fn parses_raw_and_numstat_with_renames() {
    let raw = ":100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 M\0src/a b.ts\0\
               :000000 100644 0000000000000000000000000000000000000000 3333333333333333333333333333333333333333 A\0new.txt\0\
               :100644 000000 4444444444444444444444444444444444444444 0000000000000000000000000000000000000000 D\0gone.txt\0\
               :100644 100644 5555555555555555555555555555555555555555 6666666666666666666666666666666666666666 R087\0old/name.rs\0new/name.rs\0\
               :100644 100644 7777777777777777777777777777777777777777 0000000000000000000000000000000000000000 M\0wip.txt\0";
    let files = parse_raw(raw);
    assert_eq!(paths(&files), [("src/a b.ts", "M"), ("new.txt", "A"), ("gone.txt", "D"), ("new/name.rs", "R"), ("wip.txt", "M")]);
    assert_eq!(files[1].old_blob, None);
    assert_eq!(files[2].new_blob, None);
    assert_eq!(files[3].old_path.as_deref(), Some("old/name.rs"));
    assert_eq!(files[3].similarity, Some(87));
    assert_eq!(files[4].new_blob, None, "working-tree side has no blob yet");
    let num = parse_numstat("3\t1\tsrc/a b.ts\0-\t-\timg.png\0\x32\t0\t\0old/name.rs\0new/name.rs\0");
    assert_eq!(num["src/a b.ts"], Some((3, 1)));
    assert_eq!(num["img.png"], None);
    assert_eq!(num["new/name.rs"], Some((2, 0)));
}

#[test]
fn merge_base_mode_shows_only_the_branch_changes() {
    let Some(r) = diverged() else { return };
    let c = compare(r.root(), "main", "feature", Mode::MergeBase, false).unwrap();
    assert_eq!(paths(&c.files), [("a.txt", "M"), ("f.txt", "A")], "main's m.txt doesn't show up reversed");
    assert_eq!((c.ahead, c.behind), (1, 1));
    assert_eq!(c.merge_base.as_deref(), Some(r.sha("main~1").as_str()));
    assert_eq!(c.from, r.sha("main~1"));
    assert_eq!(c.to.as_deref(), Some(r.sha("feature").as_str()));
    assert_eq!(c.commits.len(), 1);
    assert_eq!(c.commits[0].subject, "feature work");
    assert_eq!(c.files[0].added, Some(1));
    assert_eq!(c.files[0].deleted, Some(0));
    assert_eq!(c.base.kind, RefKind::Branch);
    assert!(!c.unrelated && c.merge_bases == 1);
}

#[test]
fn direct_mode_compares_tip_to_tip() {
    let Some(r) = diverged() else { return };
    let c = compare(r.root(), "main", "feature", Mode::Direct, false).unwrap();
    assert_eq!(paths(&c.files), [("a.txt", "M"), ("f.txt", "A"), ("m.txt", "D")]);
    assert_eq!(c.from, r.sha("main"));
    assert_eq!(c.mode, Mode::Direct);
}

#[test]
fn uncommitted_changes_only_on_the_checked_out_head() {
    let Some(r) = diverged() else { return };
    r.git(&["switch", "-q", "feature"]);
    r.write("a.txt", "one\nTWO\nthree\nfour\n");
    r.write("notes/u.txt", "untracked\nfile\n");
    let c = compare(r.root(), "main", "feature", Mode::MergeBase, true).unwrap();
    assert!(c.head_is_current && c.uncommitted && c.to.is_none());
    assert_eq!(paths(&c.files), [("a.txt", "M"), ("f.txt", "A"), ("notes/u.txt", "A")]);
    let a = &c.files[0];
    assert_eq!(a.new_blob.as_deref(), Some(r.git(&["hash-object", "a.txt"]).trim()), "working-tree files are keyed by the blob git would store");
    let u = &c.files[2];
    assert!(u.untracked);
    assert_eq!((u.added, u.deleted), (Some(2), Some(0)));
    assert!(u.new_blob.is_some());
    // Without the flag, or for a head that isn't checked out, the working tree stays out.
    let committed = compare(r.root(), "main", "feature", Mode::MergeBase, false).unwrap();
    assert_eq!(paths(&committed.files), [("a.txt", "M"), ("f.txt", "A")]);
    let other = compare(r.root(), "feature", "main", Mode::MergeBase, true).unwrap();
    assert!(!other.head_is_current && !other.uncommitted);
    assert_eq!(paths(&other.files), [("m.txt", "A")]);
}

#[test]
fn renames_are_detected() {
    let Some(r) = repo() else { return };
    let body: String = (1..40).map(|i| format!("line {i}\n")).collect();
    r.write("src/old.rs", &body);
    r.commit("add");
    r.git(&["switch", "-q", "-c", "move"]);
    r.git(&["mv", "src/old.rs", "src/new.rs"]);
    r.write("src/new.rs", format!("{body}one more\n"));
    r.commit("rename");
    let c = compare(r.root(), "main", "move", Mode::MergeBase, false).unwrap();
    assert_eq!(c.files.len(), 1);
    let f = &c.files[0];
    assert_eq!((f.path.as_str(), f.status.as_str(), f.old_path.as_deref()), ("src/new.rs", "R", Some("src/old.rs")));
    assert!(f.similarity.unwrap() > 80);
    assert_eq!((f.added, f.deleted), (Some(1), Some(0)));
    let d = file_diff(r.root(), &Side::Rev { rev: c.from.clone(), path: "src/old.rs".into() }, &Side::Rev { rev: c.to.clone().unwrap(), path: "src/new.rs".into() }, "src/new.rs", false).unwrap();
    assert_eq!(d.original, body);
    assert!(d.modified.ends_with("one more\n"));
}

#[test]
fn predicts_conflicts_without_touching_the_work_tree() {
    let Some(r) = repo() else { return };
    if !git::version_at_least(2, 38) {
        let p = predict_conflicts(r.root(), "HEAD", "HEAD").unwrap_or_else(|_| ConflictPrediction { supported: false, clean: false, files: vec![], message: None, git_version: String::new() });
        assert!(!p.supported);
        return;
    }
    r.write("a.txt", "one\ntwo\nthree\n");
    r.write("b.txt", "b\n");
    r.commit("base");
    r.git(&["switch", "-q", "-c", "clash"]);
    r.write("a.txt", "one\nTWO (feature)\nthree\n");
    r.commit("feature edit");
    r.git(&["switch", "-q", "-c", "clean", "main"]);
    r.write("c.txt", "c\n");
    r.commit("unrelated file");
    r.git(&["switch", "-q", "main"]);
    r.write("a.txt", "one\nTWO (main)\nthree\n");
    r.commit("main edit");
    let before = r.git(&["status", "--porcelain"]);
    let p = predict_conflicts(r.root(), "main", "clash").unwrap();
    assert!(p.supported && !p.clean);
    assert_eq!(p.files, ["a.txt"]);
    let ok = predict_conflicts(r.root(), "main", "clean").unwrap();
    assert!(ok.supported && ok.clean && ok.files.is_empty());
    assert_eq!(r.git(&["status", "--porcelain"]), before);
    assert_eq!(r.sha("HEAD"), r.sha("main"));
}

#[test]
fn marks_commits_already_on_the_base() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "a\n");
    r.commit("base");
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("fix.txt", "fix\n");
    let fix = r.commit("hotfix");
    r.write("more.txt", "more\n");
    r.commit("more work");
    r.git(&["switch", "-q", "main"]);
    // (Another commit first: picked onto the same parent within the same second, the copy would
    // be byte-identical — the same commit, not an equivalent one.)
    r.write("main.txt", "main\n");
    r.commit("main work");
    r.git(&["cherry-pick", fix.as_str()]);
    let (commits, truncated) = range_commits(r.root(), "main", "feature", 100, true).unwrap();
    assert!(!truncated);
    let marks: Vec<(&str, bool)> = commits.iter().map(|c| (c.subject.as_str(), c.equivalent)).collect();
    assert_eq!(marks, [("more work", false), ("hotfix", true)]);
    let (_, cut) = range_commits(r.root(), "main", "feature", 1, true).unwrap();
    assert!(cut);
    let (plain, _) = range_commits(r.root(), "main", "feature", 100, false).unwrap();
    assert_eq!(plain.len(), 2);
    assert!(plain.iter().all(|c| !c.equivalent));
}

#[test]
fn unrelated_histories_fall_back_to_direct() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "a\n");
    r.commit("main");
    r.git(&["checkout", "-q", "--orphan", "other"]);
    r.git(&["rm", "-rfq", "--cached", "."]);
    std::fs::remove_file(r.path("a.txt")).unwrap();
    r.write("o.txt", "o\n");
    r.commit("other root");
    let c = compare(r.root(), "main", "other", Mode::MergeBase, false).unwrap();
    assert!(c.unrelated);
    assert_eq!(c.mode, Mode::Direct);
    assert_eq!(paths(&c.files), [("a.txt", "D"), ("o.txt", "A")]);
    // A root commit's own changes are diffed against the empty tree.
    let range = commit_range(r.root(), "other", "other").unwrap();
    assert_eq!(range.from, empty_tree(r.root()));
    assert_eq!(paths(&range.files), [("o.txt", "A")]);
}

#[test]
fn default_base_prefers_the_remote_default_branch() {
    let Some(upstream) = diverged() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("clone");
    git::run(tmp.path(), &["clone", "-q", &upstream.root().display().to_string(), &clone.display().to_string()]).unwrap();
    assert_eq!(default_base(&clone).as_deref(), Some("origin/main"));
    git::run(&clone, &["switch", "-q", "feature"]).unwrap();
    let c = compare(&clone, "origin/main", "feature", Mode::MergeBase, false).unwrap();
    assert_eq!(c.base.kind, RefKind::Remote);
    assert_eq!(c.base_remote.as_deref(), Some("origin"));
    assert_eq!(c.base_remote_branch.as_deref(), Some("main"));
    assert_eq!(c.head_remote_branch.as_deref(), Some("feature"));
    assert!(c.head_pushed);
    assert!(c.remote_url.is_some());
    git::run(&clone, &["switch", "-q", "-c", "local-only"]).unwrap();
    assert!(!compare(&clone, "origin/main", "local-only", Mode::MergeBase, false).unwrap().head_pushed);
    // No remote: a local default branch.
    assert_eq!(default_base(upstream.root()).as_deref(), Some("main"));
}

#[test]
fn generated_files_are_flagged() {
    let Some(r) = repo() else { return };
    r.write("README.md", "x\n");
    r.commit("base");
    r.git(&["switch", "-q", "-c", "deps"]);
    r.write(".gitattributes", "gen/** linguist-generated\nyarn.lock -linguist-generated\n");
    r.write("package-lock.json", "{}\n");
    r.write("yarn.lock", "# keep visible\n");
    r.write("gen/api.ts", "export {}\n");
    r.write("dist/app.min.js", "x\n");
    r.write("src/app.ts", "x\n");
    r.commit("deps");
    let c = compare(r.root(), "main", "deps", Mode::MergeBase, false).unwrap();
    let flagged: Vec<&str> = c.files.iter().filter(|f| f.generated).map(|f| f.path.as_str()).collect();
    assert_eq!(flagged, ["dist/app.min.js", "gen/api.ts", "package-lock.json"]);
}

#[test]
fn reads_each_kind_of_side() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "v1\n");
    r.commit("one");
    let blob = r.git(&["rev-parse", "HEAD:a.txt"]).trim().to_string();
    r.write("a.txt", "v2\n");
    r.git(&["add", "a.txt"]);
    r.write("a.txt", "v3\n");
    let read = |s: Side| String::from_utf8(read_side(r.root(), &s, MAX_TEXT_BYTES).unwrap().bytes).unwrap();
    assert_eq!(read(Side::Rev { rev: "HEAD".into(), path: "a.txt".into() }), "v1\n");
    assert_eq!(read(Side::Blob { id: blob }), "v1\n");
    assert_eq!(read(Side::Index { path: "a.txt".into() }), "v2\n");
    assert_eq!(read(Side::Worktree { path: "a.txt".into() }), "v3\n");
    assert!(read_side(r.root(), &Side::Rev { rev: "HEAD".into(), path: "nope.txt".into() }, 10).unwrap().missing);
    assert!(read_side(r.root(), &Side::Worktree { path: "../x".into() }, 10).is_err());
    assert!(read_side(r.root(), &Side::Rev { rev: "--output=x".into(), path: "a.txt".into() }, 10).is_err());
    let big = read_side(r.root(), &Side::Worktree { path: "a.txt".into() }, 1).unwrap();
    assert!(big.too_large && big.bytes.is_empty());
    // Images come back as data URLs.
    r.write("logo.png", [0x89u8, b'P', b'N', b'G', 0, 1, 2]);
    let d = file_diff(r.root(), &Side::Empty, &Side::Worktree { path: "logo.png".into() }, "logo.png", false).unwrap();
    let img = d.image.unwrap();
    assert!(img.old.is_none());
    assert!(img.new.unwrap().starts_with("data:image/png;base64,iVBORw"));
}

#[test]
fn base64_matches_the_standard() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
}

#[test]
fn merge_tree_output_parsing() {
    assert_eq!(parse_merge_tree("abc123\0a.txt\0src/b.rs\0\0"), ["a.txt", "src/b.rs"]);
    assert!(parse_merge_tree("abc123\0").is_empty());
}

#[test]
fn version_parsing() {
    assert_eq!(git::parse_version("git version 2.48.1.windows.1"), Some((2, 48, 1)));
    assert_eq!(git::parse_version("git version 2.39.3 (Apple Git-146)"), Some((2, 39, 3)));
    assert_eq!(git::parse_version("git version 2.30"), Some((2, 30, 0)));
    assert_eq!(git::parse_version("nope"), None);
}
