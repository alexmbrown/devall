use super::*;
use crate::repos::testgit::{TestRepo, repo};

// (`concat!` rather than `\` line continuations, which would eat the context lines' leading space.)
const DIFF: &str = concat!(
    "diff --git a/f.txt b/f.txt\n",
    "index 1111111..2222222 100644\n",
    "--- a/f.txt\n",
    "+++ b/f.txt\n",
    "@@ -1,3 +1,4 @@ intro\n",
    " one\n",
    "-two\n",
    "+TWO\n",
    "+two and a half\n",
    " three\n",
    "@@ -10,3 +11,2 @@\n",
    " ten\n",
    "-eleven\n",
    " twelve\n",
);

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

#[test]
fn parses_hunks_and_line_numbers() {
    let p = parse(DIFF.as_bytes());
    assert_eq!(p.header.len(), 4);
    assert_eq!(p.hunks.len(), 2);
    let h = &p.hunks[0];
    assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (1, 3, 1, 4));
    assert_eq!(h.section, b"intro");
    assert_eq!(h.lines.iter().map(|l| l.kind as char).collect::<String>(), " -++ ");
    let v = view("f.txt", false, DIFF.as_bytes(), false);
    let nums: Vec<(Option<u32>, Option<u32>)> = v.hunks[0].lines.iter().map(|l| (l.old, l.new)).collect();
    assert_eq!(nums, [(Some(1), Some(1)), (Some(2), None), (None, Some(2)), (None, Some(3)), (Some(3), Some(4))]);
    assert_eq!(v.hunks[1].header, "@@ -10,3 +11,2 @@");
    assert_eq!(v.line_count, 8);
    assert!(!v.too_large && !v.binary && !v.crlf);
}

#[test]
fn builds_partial_patches_both_ways() {
    let p = parse(DIFF.as_bytes());
    // Stage only "+TWO" (line 2) of hunk 0: "-two" stays as context, the other "+" is dropped.
    let fwd = text(&build(&p, &[Selection { hunk: 0, lines: Some(vec![2]) }], false).unwrap());
    assert!(fwd.ends_with("@@ -1,3 +1,4 @@ intro\n one\n two\n+TWO\n three\n"), "{fwd}");
    // Unstage only "-two": the "+" lines become context, so the index side still matches.
    let rev = text(&build(&p, &[Selection { hunk: 0, lines: Some(vec![1]) }], true).unwrap());
    assert!(rev.ends_with("@@ -1,5 +1,4 @@ intro\n one\n-two\n TWO\n two and a half\n three\n"), "{rev}");
    // Whole second hunk only; context-only selections produce nothing.
    let second = text(&build(&p, &[Selection { hunk: 1, lines: None }], false).unwrap());
    assert!(second.contains("@@ -10,3 +10,2 @@\n ten\n-eleven\n twelve\n"), "{second}");
    assert!(build(&p, &[Selection { hunk: 0, lines: Some(vec![0, 4]) }], false).is_none());
    assert!(build(&p, &[Selection { hunk: 7, lines: None }], false).is_none());
    // Both hunks: the second's new start follows the first's growth (+1).
    let both = text(&build(&p, &[Selection { hunk: 1, lines: None }, Selection { hunk: 0, lines: None }], false).unwrap());
    assert!(both.contains("@@ -1,3 +1,4 @@") && both.contains("@@ -10,3 +11,2 @@"), "{both}");
}

#[test]
fn keeps_no_newline_markers_and_pure_additions() {
    let d = "diff --git a/n b/n\n--- a/n\n+++ b/n\n@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n";
    let p = parse(d.as_bytes());
    assert!(p.hunks[0].lines[1].no_eol);
    let only_b = text(&build(&p, &[Selection { hunk: 0, lines: Some(vec![1]) }], false).unwrap());
    assert!(only_b.ends_with("@@ -0,0 +1,1 @@\n+b\n\\ No newline at end of file\n"), "{only_b}");
}

fn lines(n: usize, f: impl Fn(usize) -> String) -> String {
    (1..=n).map(|i| f(i) + "\n").collect()
}

/// A tracked file with two far-apart changes in the working tree.
fn two_hunks(r: &TestRepo) {
    r.write("f.txt", lines(30, |i| format!("line {i}")));
    r.commit("base");
    r.write("f.txt", lines(30, |i| match i {
        2 => "line 2 changed".into(),
        25 => "line 25 changed".into(),
        _ => format!("line {i}"),
    }));
}

fn staged_diff(r: &TestRepo) -> String {
    r.git(&["diff", "--cached", "--no-color", "-U0"])
}

#[test]
fn stages_unstages_and_discards_hunks() {
    let Some(r) = repo() else { return };
    two_hunks(&r);
    let v = file_view(r.root(), "f.txt", false, false, false).unwrap();
    assert_eq!(v.hunks.len(), 2);
    apply_selection(r.root(), None, "f.txt", LineAction::Stage, &v.fingerprint, &[Selection { hunk: 1, lines: None }], false).unwrap();
    let staged = staged_diff(&r);
    assert!(staged.contains("+line 25 changed") && !staged.contains("line 2 changed"), "{staged}");
    // The unstaged diff now has only the first hunk.
    let left = file_view(r.root(), "f.txt", false, false, false).unwrap();
    assert_eq!(left.hunks.len(), 1);
    // Unstage it again.
    let s = file_view(r.root(), "f.txt", true, false, false).unwrap();
    apply_selection(r.root(), None, "f.txt", LineAction::Unstage, &s.fingerprint, &[Selection { hunk: 0, lines: None }], false).unwrap();
    assert!(staged_diff(&r).is_empty());
    // Discard the first hunk from the working tree: line 25's change stays.
    let w = file_view(r.root(), "f.txt", false, false, false).unwrap();
    apply_selection(r.root(), None, "f.txt", LineAction::Discard, &w.fingerprint, &[Selection { hunk: 0, lines: None }], false).unwrap();
    let now = text(&r.read("f.txt"));
    assert!(now.contains("line 2\n") && now.contains("line 25 changed\n"), "{now}");
}

#[test]
fn stages_single_lines() {
    let Some(r) = repo() else { return };
    r.write("f.txt", "a\nb\nc\n");
    r.commit("base");
    r.write("f.txt", "a\nB1\nB2\nc\n");
    let v = file_view(r.root(), "f.txt", false, false, false).unwrap();
    // Lines: " a", "-b", "+B1", "+B2", " c" → stage "-b" and "+B2".
    assert_eq!(v.hunks[0].lines.iter().map(|l| l.kind.as_str()).collect::<String>(), " -++ ");
    apply_selection(r.root(), None, "f.txt", LineAction::Stage, &v.fingerprint, &[Selection { hunk: 0, lines: Some(vec![1, 3]) }], false).unwrap();
    assert_eq!(r.git(&["show", ":f.txt"]), "a\nB2\nc\n");
    assert_eq!(text(&r.read("f.txt")), "a\nB1\nB2\nc\n", "the working tree is untouched");
    // Unstage just "+B2" (index: a, B2, c vs HEAD: a, b, c → "-b", "+B2").
    let s = file_view(r.root(), "f.txt", true, false, false).unwrap();
    let plus = s.hunks[0].lines.iter().position(|l| l.kind == "+").unwrap();
    apply_selection(r.root(), None, "f.txt", LineAction::Unstage, &s.fingerprint, &[Selection { hunk: 0, lines: Some(vec![plus]) }], false).unwrap();
    assert_eq!(r.git(&["show", ":f.txt"]), "a\nc\n");
    // Discard just "+B1" from the working tree.
    let w = file_view(r.root(), "f.txt", false, false, false).unwrap();
    let b1 = w.hunks[0].lines.iter().position(|l| l.text == "B1").unwrap();
    apply_selection(r.root(), None, "f.txt", LineAction::Discard, &w.fingerprint, &[Selection { hunk: 0, lines: Some(vec![b1]) }], false).unwrap();
    assert_eq!(text(&r.read("f.txt")), "a\nB2\nc\n");
}

#[test]
fn refuses_a_stale_view() {
    let Some(r) = repo() else { return };
    two_hunks(&r);
    let v = file_view(r.root(), "f.txt", false, false, false).unwrap();
    r.write("f.txt", "something else entirely\n");
    let err = apply_selection(r.root(), None, "f.txt", LineAction::Stage, &v.fingerprint, &[Selection { hunk: 0, lines: None }], false).unwrap_err();
    assert!(err.contains("changed"), "{err}");
    assert!(staged_diff(&r).is_empty());
}

#[test]
fn crlf_working_tree_with_autocrlf() {
    let Some(r) = repo() else { return };
    r.git(&["config", "core.autocrlf", "true"]);
    r.write("w.txt", lines(20, |i| format!("row {i}")).replace('\n', "\r\n"));
    r.commit("crlf file");
    assert!(!r.git(&["show", "HEAD:w.txt"]).contains('\r'), "stored with LF");
    r.write("w.txt", lines(20, |i| match i {
        3 => "row 3 new".into(),
        18 => "row 18 new".into(),
        _ => format!("row {i}"),
    })
    .replace('\n', "\r\n"));
    let v = file_view(r.root(), "w.txt", false, false, false).unwrap();
    assert_eq!(v.hunks.len(), 2, "only the real changes, not every line ending");
    apply_selection(r.root(), None, "w.txt", LineAction::Stage, &v.fingerprint, &[Selection { hunk: 0, lines: None }], false).unwrap();
    let index = r.git(&["show", ":w.txt"]);
    assert!(index.contains("row 3 new\n") && !index.contains("row 18 new") && !index.contains('\r'), "{index:?}");
    let w = file_view(r.root(), "w.txt", false, false, false).unwrap();
    apply_selection(r.root(), None, "w.txt", LineAction::Discard, &w.fingerprint, &[Selection { hunk: 0, lines: None }], false).unwrap();
    let now = text(&r.read("w.txt"));
    assert!(now.contains("row 3 new\r\n") && now.contains("row 18\r\n"), "{now:?}");
    assert!(!now.replace("\r\n", "").contains('\n'), "still CRLF everywhere: {now:?}");
}

#[test]
fn crlf_blobs_keep_their_carriage_returns() {
    let Some(r) = repo() else { return };
    r.write("c.txt", "one\r\ntwo\r\nthree\r\n");
    r.commit("crlf in the blob");
    r.write("c.txt", "one\r\nTWO\r\n2.5\r\nthree\r\n");
    let v = file_view(r.root(), "c.txt", false, false, false).unwrap();
    assert!(v.crlf);
    assert_eq!(v.hunks[0].lines[2].text, "TWO", "shown without the CR");
    let plus_two = v.hunks[0].lines.iter().position(|l| l.text == "TWO").unwrap();
    let minus = v.hunks[0].lines.iter().position(|l| l.kind == "-").unwrap();
    apply_selection(r.root(), None, "c.txt", LineAction::Stage, &v.fingerprint, &[Selection { hunk: 0, lines: Some(vec![minus, plus_two]) }], false).unwrap();
    assert_eq!(r.git(&["show", ":c.txt"]), "one\r\nTWO\r\nthree\r\n");
}

#[test]
fn whitespace_only_changes_can_be_ignored() {
    let Some(r) = repo() else { return };
    r.write("s.txt", "fn a() {\n  x();\n}\n\nfn b() {}\n");
    r.commit("base");
    r.write("s.txt", "fn a() {\n    x();\n}\n\nfn b() { y(); }\n");
    let all = file_view(r.root(), "s.txt", false, false, false).unwrap();
    let ws = file_view(r.root(), "s.txt", false, true, false).unwrap();
    let changed = |v: &PatchView| v.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind != " ").count();
    assert!(changed(&ws) < changed(&all));
    apply_selection(r.root(), None, "s.txt", LineAction::Stage, &ws.fingerprint, &[Selection { hunk: 0, lines: None }], true).unwrap();
    let index = r.git(&["show", ":s.txt"]);
    assert!(index.contains("fn b() { y(); }"), "{index}");
}

#[test]
fn binary_files_are_whole_file_only() {
    let Some(r) = repo() else { return };
    r.write("b.bin", [0u8, 1, 2, 3]);
    r.commit("bin");
    r.write("b.bin", [0u8, 9, 9, 9]);
    let v = file_view(r.root(), "b.bin", false, false, false).unwrap();
    assert!(v.binary && v.hunks.is_empty());
    assert!(apply_selection(r.root(), None, "b.bin", LineAction::Stage, &v.fingerprint, &[Selection { hunk: 0, lines: None }], false).is_err());
}

#[test]
fn huge_diffs_wait_for_load_anyway() {
    let big: String = (0..MAX_VIEW_LINES + 10).map(|i| format!("+l{i}\n")).collect();
    let d = format!("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -0,0 +1,{} @@\n{big}", MAX_VIEW_LINES + 10);
    let v = view("x", false, d.as_bytes(), false);
    assert!(v.too_large && v.hunks.is_empty());
    assert_eq!(v.line_count, MAX_VIEW_LINES + 10);
    assert_eq!(view("x", false, d.as_bytes(), true).hunks.len(), 1);
}

#[test]
fn partially_unstages_a_new_file_and_stages_part_of_a_deletion() {
    let Some(r) = repo() else { return };
    r.write("keep.txt", "k\n");
    r.write("old.txt", "x\ny\nz\n");
    r.commit("base");
    r.write("new.txt", "a\nb\nc\n");
    r.git(&["add", "new.txt"]);
    let s = file_view(r.root(), "new.txt", true, false, false).unwrap();
    let b = s.hunks[0].lines.iter().position(|l| l.text == "b").unwrap();
    apply_selection(r.root(), None, "new.txt", LineAction::Unstage, &s.fingerprint, &[Selection { hunk: 0, lines: Some(vec![b]) }], false).unwrap();
    assert_eq!(r.git(&["show", ":new.txt"]), "a\nc\n", "still added, minus one line");
    // Unstaging all of it makes it untracked again.
    let s = file_view(r.root(), "new.txt", true, false, false).unwrap();
    apply_selection(r.root(), None, "new.txt", LineAction::Unstage, &s.fingerprint, &[Selection { hunk: 0, lines: None }], false).unwrap();
    assert!(r.git(&["ls-files", "new.txt"]).trim().is_empty());
    // A deleted file: stage the removal of one line only.
    std::fs::remove_file(r.path("old.txt")).unwrap();
    let w = file_view(r.root(), "old.txt", false, false, false).unwrap();
    let y = w.hunks[0].lines.iter().position(|l| l.text == "y").unwrap();
    apply_selection(r.root(), None, "old.txt", LineAction::Stage, &w.fingerprint, &[Selection { hunk: 0, lines: Some(vec![y]) }], false).unwrap();
    assert_eq!(r.git(&["show", ":old.txt"]), "x\nz\n");
}

#[test]
fn appends_to_a_file_without_a_final_newline() {
    let Some(r) = repo() else { return };
    r.write("e.txt", "a\nb");
    r.commit("no eol");
    r.write("e.txt", "a\nb\nc\n");
    let v = file_view(r.root(), "e.txt", false, false, false).unwrap();
    apply_selection(r.root(), None, "e.txt", LineAction::Stage, &v.fingerprint, &[Selection { hunk: 0, lines: None }], false).unwrap();
    assert_eq!(r.git(&["show", ":e.txt"]), "a\nb\nc\n");
}
