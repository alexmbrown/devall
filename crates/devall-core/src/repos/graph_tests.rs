use std::collections::BTreeSet;

use super::*;
use crate::repos::testgit::repo;

fn p(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Columns entering a row from above / leaving it below.
fn top(pl: &Placement) -> BTreeSet<u16> {
    pl.segs.iter().filter(|s| s.kind != SegKind::Out).map(|s| s.from).collect()
}
fn bottom(pl: &Placement) -> BTreeSet<u16> {
    pl.segs.iter().filter(|s| s.kind != SegKind::In).map(|s| s.to).collect()
}

/// Every line leaving a row continues into the next one (the drawing has no gaps or dangling ends),
/// and the last row leaves nothing open.
fn assert_continuous(rows: &[Placement]) {
    for (i, w) in rows.windows(2).enumerate() {
        assert_eq!(bottom(&w[0]), top(&w[1]), "rows {i} → {}", i + 1);
    }
    if let Some(last) = rows.last() {
        assert!(bottom(last).is_empty(), "lines left open after the last row");
    }
    for r in rows {
        assert!(r.segs.iter().all(|s| s.from < r.width && s.to < r.width && r.col < r.width));
    }
}

#[test]
fn linear_history_stays_in_one_lane() {
    let mut l = Layout::default();
    let rows = vec![l.place("c", &p(&["b"])), l.place("b", &p(&["a"])), l.place("a", &[])];
    assert!(rows.iter().all(|r| r.col == 0 && r.width == 1 && r.color == 0));
    assert_eq!(rows[0].segs, [Seg { kind: SegKind::Out, from: 0, to: 0, color: 0 }]);
    assert_eq!(rows[1].segs.len(), 2);
    assert_continuous(&rows);
}

#[test]
fn branch_and_merge() {
    // M merges A and B; both branched from O.
    let mut l = Layout::default();
    let m = l.place("M", &p(&["A", "B"]));
    assert_eq!((m.col, m.width), (0, 2));
    assert_eq!(m.segs, [Seg { kind: SegKind::Out, from: 0, to: 0, color: 0 }, Seg { kind: SegKind::Out, from: 0, to: 1, color: 1 }]);
    let a = l.place("A", &p(&["O"]));
    assert_eq!(a.col, 0);
    let b = l.place("B", &p(&["O"]));
    assert_eq!((b.col, b.color), (1, 1));
    let o = l.place("O", &[]);
    assert_eq!(o.col, 0);
    assert_eq!(o.segs.iter().filter(|s| s.kind == SegKind::In).count(), 2, "the fork: both lanes end in O");
    assert_continuous(&[m, a, b, o]);
    assert_eq!(l.open_lanes(), 0);
}

#[test]
fn octopus_merge_opens_a_lane_per_parent() {
    let mut l = Layout::default();
    let m = l.place("M", &p(&["A", "B", "C", "D"]));
    assert_eq!(m.width, 4);
    assert_eq!(m.segs.iter().filter(|s| s.kind == SegKind::Out).map(|s| s.to).collect::<Vec<_>>(), [0, 1, 2, 3]);
    let rows = vec![m, l.place("A", &p(&["R"])), l.place("B", &p(&["R"])), l.place("C", &p(&["R"])), l.place("D", &p(&["R"])), l.place("R", &[])];
    assert_eq!(rows[5].segs.iter().filter(|s| s.kind == SegKind::In).count(), 4);
    assert_continuous(&rows);
}

#[test]
fn many_branch_tips_and_slot_reuse() {
    let mut l = Layout::default();
    let mut rows = Vec::new();
    // 12 branch tips on top of a shared root, newest first.
    for i in 0..12 {
        rows.push(l.place(&format!("t{i}"), &p(&["root"])));
    }
    assert_eq!(rows.iter().map(|r| r.col).collect::<Vec<_>>(), (0..12).collect::<Vec<u16>>());
    assert_eq!(rows[11].width, 12);
    let colors: BTreeSet<u16> = rows.iter().map(|r| r.color).collect();
    assert_eq!(colors.len(), 12, "each branch gets its own colour");
    rows.push(l.place("root", &p(&["older"])));
    assert_eq!(rows[12].col, 0);
    assert_eq!(rows[12].width, 12);
    // Lanes closed: a new tip reuses the leftmost free slot.
    rows.push(l.place("older", &[]));
    assert_continuous(&rows);
    let mut l2 = Layout::default();
    l2.place("x", &p(&["y"]));
    l2.place("z", &p(&["w"]));
    l2.place("y", &[]);
    let n = l2.place("n", &[]);
    assert_eq!(n.col, 0, "slot 0 was freed by y");
}

#[test]
fn a_merge_into_an_existing_lane_draws_a_join() {
    let mut l = Layout::default();
    let rows = vec![
        l.place("F", &p(&["E"])),      // feature tip
        l.place("M", &p(&["D", "E"])), // main merges E (already awaited by F's lane)
        l.place("E", &p(&["D"])),
        l.place("D", &[]),
    ];
    assert_eq!(rows[1].col, 1);
    assert!(rows[1].segs.contains(&Seg { kind: SegKind::Out, from: 1, to: 0, color: rows[0].color }));
    assert_continuous(&rows);
}

#[test]
fn parses_decorations() {
    let refs = parse_refs("HEAD -> refs/heads/main, refs/remotes/origin/main, refs/remotes/origin/HEAD, tag: refs/tags/v1.0, refs/heads/feature/x, refs/notes/commits");
    let got: Vec<(&str, RefLabelKind, bool)> = refs.iter().map(|r| (r.name.as_str(), r.kind, r.current)).collect();
    assert_eq!(
        got,
        [
            ("main", RefLabelKind::Branch, true),
            ("origin/main", RefLabelKind::Remote, false),
            ("v1.0", RefLabelKind::Tag, false),
            ("feature/x", RefLabelKind::Branch, false),
            ("notes/commits", RefLabelKind::Other, false),
        ]
    );
    assert_eq!(parse_refs("HEAD")[0].kind, RefLabelKind::Head);
    assert!(parse_refs("").is_empty());
}

fn collect(dir: &Path, wip: bool) -> Vec<GraphRow> {
    let mut rows = Vec::new();
    stream(dir, wip, &AtomicBool::new(false), |r| rows.push(r)).unwrap();
    rows
}

fn assert_topological(rows: &[GraphRow]) {
    let index: HashMap<&str, usize> = rows.iter().enumerate().map(|(i, r)| (r.hash.as_str(), i)).collect();
    for (i, r) in rows.iter().enumerate() {
        for parent in &r.parents {
            assert!(index[parent.as_str()] > i, "{} is listed before its child {}", parent, r.hash);
        }
    }
}

#[test]
fn streams_a_real_repository_with_merges_and_tags() {
    let Some(r) = repo() else { return };
    r.write("a.txt", "a\n");
    r.commit("root");
    r.git(&["tag", "v0"]);
    for b in ["one", "two", "three"] {
        r.git(&["switch", "-q", "-c", b, "main"]);
        r.write(&format!("{b}.txt"), b);
        r.commit(&format!("work on {b}"));
    }
    r.git(&["switch", "-q", "main"]);
    r.write("m.txt", "m\n");
    r.commit("main work");
    r.git(&["merge", "-q", "--no-edit", "one"]);
    r.git(&["merge", "-q", "--no-edit", "two", "three"]); // octopus
    r.git(&["switch", "-q", "-c", "side", "v0"]);
    r.write("s.txt", "s\n");
    r.commit("side work");
    r.write("dirty.txt", "x");
    let rows = collect(r.root(), true);
    let total: usize = r.git(&["rev-list", "--count", "--all"]).trim().parse().unwrap();
    assert_eq!(rows.len(), total + 1);
    assert_eq!(rows[0].hash, WIP);
    assert_eq!(rows[0].parents, [r.sha("HEAD")]);
    assert_topological(&rows);
    let places: Vec<Placement> = rows.iter().map(|r| r.place.clone()).collect();
    assert_continuous(&places);
    let octopus = rows.iter().find(|x| x.parents.len() == 3).unwrap();
    assert!(octopus.refs.iter().any(|l| l.name == "main" && l.kind == RefLabelKind::Branch && !l.current));
    let side = rows.iter().find(|x| x.subject == "side work").unwrap();
    assert!(side.refs.iter().any(|l| l.name == "side" && l.current));
    let root = rows.iter().find(|x| x.subject == "root").unwrap();
    assert!(root.refs.iter().any(|l| l.name == "v0" && l.kind == RefLabelKind::Tag));
    // Search and lookup over the read rows.
    assert_eq!(search(&rows, "WORK ON T").len(), 2);
    assert_eq!(search(&rows, &rows[3].hash[..8]), [3]);
    assert!(search(&rows, "").is_empty());
}

#[test]
fn the_job_serves_pages_while_streaming() {
    let Some(r) = repo() else { return };
    for i in 0..30 {
        r.write("f.txt", format!("{i}"));
        r.commit(&format!("c{i}"));
    }
    let job = GraphJob::start(r.root().to_path_buf(), false, signature(r.root()));
    let first = job.page(0, 10, Duration::from_secs(20));
    assert_eq!(first.rows.len(), 10);
    assert_eq!(first.rows[0].subject, "c29");
    assert!(job.wait_done(Duration::from_secs(20)));
    let rest = job.page(25, 10, Duration::from_secs(1));
    assert_eq!((rest.rows.len(), rest.loaded, rest.done), (5, 30, true));
    assert!(job.page(40, 10, Duration::ZERO).rows.is_empty());
    assert_eq!(job.find(&r.sha("HEAD~2")), Some(2));
    assert_eq!(job.find(&r.sha("HEAD~2")[..10]), Some(2));
    assert_eq!(job.search("c1").len(), 11);
    let before = signature(r.root());
    r.git(&["branch", "x"]);
    assert_ne!(signature(r.root()), before, "a new ref changes the signature");
}

#[test]
fn an_empty_repository_has_an_empty_graph() {
    let Some(r) = repo() else { return };
    assert!(collect(r.root(), true).is_empty());
}

#[test]
fn lays_out_this_repositorys_own_history() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !git::available() || !here.join(".git").exists() {
        return;
    }
    let rows = collect(&here, false);
    let mut args = vec!["rev-list", "--count"];
    args.extend(EXCLUDE);
    args.push("--all");
    let total: usize = git::run(&here, &args).unwrap().trim().parse().unwrap();
    assert_eq!(rows.len(), total);
    assert_topological(&rows);
    assert_continuous(&rows.iter().map(|r| r.place.clone()).collect::<Vec<_>>());
}

/// 50k commits (fast-import): a main line with a side branch merged every 40 commits and a tag
/// every 1000. The whole pass must stay quick and the drawing continuous.
#[test]
fn a_large_synthetic_history_streams_quickly() {
    let Some(r) = repo() else { return };
    const N: usize = 50_000;
    let mut s = String::with_capacity(N * 140);
    let mut mark = 0usize;
    let mut main_tip = 0usize;
    let mut t = 1_600_000_000u64;
    let mut commit = |s: &mut String, refname: &str, from: usize, merge: Option<usize>, msg: &str| -> usize {
        mark += 1;
        t += 60;
        s.push_str(&format!("commit {refname}\nmark :{mark}\ncommitter Bot <bot@example.com> {t} +0000\ndata {}\n{msg}\n", msg.len()));
        if from > 0 {
            s.push_str(&format!("from :{from}\n"));
        }
        if let Some(m) = merge {
            s.push_str(&format!("merge :{m}\n"));
        }
        s.push('\n');
        mark
    };
    let mut made = 0usize;
    let mut branch = 0usize;
    while made < N {
        if made > 0 && made.is_multiple_of(40) && made + 6 <= N {
            branch += 1;
            let name = format!("refs/heads/b{branch}");
            let mut tip = main_tip;
            for k in 0..4 {
                tip = commit(&mut s, &name, tip, None, &format!("b{branch} work {k}"));
            }
            main_tip = commit(&mut s, "refs/heads/main", main_tip, None, &format!("main {made}"));
            main_tip = commit(&mut s, "refs/heads/main", main_tip, Some(tip), &format!("merge b{branch}"));
            made += 6;
        } else {
            main_tip = commit(&mut s, "refs/heads/main", main_tip, None, &format!("main {made}"));
            made += 1;
        }
        if made.is_multiple_of(1000) {
            s.push_str(&format!("reset refs/tags/t{made}\nfrom :{main_tip}\n\n"));
        }
    }
    git::run_input(r.root(), &["fast-import", "--quiet"], s.as_bytes()).unwrap();
    let started = Instant::now();
    let job = GraphJob::start(r.root().to_path_buf(), false, String::new());
    let first = job.page(0, 500, Duration::from_secs(60));
    assert_eq!(first.rows.len(), 500);
    assert!(job.wait_done(Duration::from_secs(120)));
    let elapsed = started.elapsed();
    let page = job.page(0, N + 10, Duration::ZERO);
    assert_eq!(page.loaded, N);
    assert!(page.error.is_none());
    assert_topological(&page.rows);
    assert_continuous(&page.rows.iter().map(|r| r.place.clone()).collect::<Vec<_>>());
    assert!(page.max_width <= 3, "side branches reuse lanes (width {})", page.max_width);
    eprintln!("graph of {N} commits in {elapsed:?} (first page after {} rows)", first.loaded);
    let last = job.page(N - 100, 500, Duration::ZERO);
    assert_eq!(last.rows.len(), 100);
}
