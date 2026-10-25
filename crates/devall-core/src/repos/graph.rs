//! The commit graph: lanes computed in one streaming pass over
//! `git log --all --topo-order -z` (no `--graph` ASCII parsing), served to the UI in pages.
//!
//! [`Layout`] keeps one slot per open line ("lane"), each waiting for the commit it leads to.
//! A commit takes the leftmost lane waiting for it (other lanes waiting for it end there, which
//! draws a fork); its first parent continues in that lane, further parents (merges) join a lane
//! already waiting for them or open one in the leftmost free slot. Cost is linear in commits ×
//! open lanes. Each row carries the segments to draw inside its own band, so the UI can render
//! any page without the rows above it.
//!
//! [`GraphJob`] runs the pass on a thread and keeps the rows; pages wait until their rows exist,
//! so the first screen shows while git is still walking a 100k-commit history.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::ser::{SerializeTuple, Serializer};
use sha2::{Digest, Sha256};

use super::git::{self, GitResult};

/// Hash of the synthetic "uncommitted changes" row.
pub const WIP: &str = "WIP";

/// A line inside one row's band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seg {
    pub kind: SegKind,
    /// Column at the top edge (pass, in) or the node's column (out).
    pub from: u16,
    /// Column at the bottom edge (pass, out) or the node's column (in).
    pub to: u16,
    pub color: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegKind {
    /// A lane passing by: top of `from` to bottom of `to` (same column).
    Pass = 0,
    /// A lane ending in this commit: top of `from` to the node.
    In = 1,
    /// A parent line leaving the node: the node to the bottom of `to`.
    Out = 2,
}

/// Serialized compactly as `[kind, from, to, color]`.
impl Serialize for Seg {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut t = s.serialize_tuple(4)?;
        t.serialize_element(&(self.kind as u8))?;
        t.serialize_element(&self.from)?;
        t.serialize_element(&self.to)?;
        t.serialize_element(&self.color)?;
        t.end()
    }
}

/// Where a commit sits and what to draw around it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub col: u16,
    pub color: u16,
    pub segs: Vec<Seg>,
    /// Columns in use in this row (top or bottom).
    pub width: u16,
}

#[derive(Debug, Clone)]
struct Lane {
    expect: String,
    color: u16,
}

/// The lane state of a graph being laid out, newest commit first (children before parents).
#[derive(Debug, Default)]
pub struct Layout {
    lanes: Vec<Option<Lane>>,
    next_color: u16,
}

impl Layout {
    fn color(&mut self) -> u16 {
        let c = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        c
    }

    fn free_slot(&mut self) -> usize {
        match self.lanes.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    /// Places the next commit (in topological order) and opens lanes for its parents.
    pub fn place(&mut self, hash: &str, parents: &[String]) -> Placement {
        let matching: Vec<usize> = self.lanes.iter().enumerate().filter(|(_, l)| l.as_ref().is_some_and(|l| l.expect == hash)).map(|(i, _)| i).collect();
        let top_width = self.lanes.len();
        let (col, color) = match matching.first() {
            Some(&i) => (i, self.lanes[i].as_ref().map_or(0, |l| l.color)),
            None => {
                let slot = self.free_slot();
                (slot, self.color())
            }
        };
        let mut segs = Vec::new();
        for (i, lane) in self.lanes.iter().enumerate() {
            if let Some(l) = lane {
                let kind = if matching.contains(&i) { SegKind::In } else { SegKind::Pass };
                segs.push(Seg { kind, from: i as u16, to: if kind == SegKind::In { col as u16 } else { i as u16 }, color: l.color });
            }
        }
        for &i in &matching {
            self.lanes[i] = None;
        }
        if let Some(first) = parents.first() {
            self.lanes[col] = Some(Lane { expect: first.clone(), color });
            segs.push(Seg { kind: SegKind::Out, from: col as u16, to: col as u16, color });
        }
        for p in parents.iter().skip(1) {
            let existing = self.lanes.iter().position(|l| l.as_ref().is_some_and(|l| &l.expect == p));
            let (j, c) = match existing {
                Some(j) => (j, self.lanes[j].as_ref().map_or(0, |l| l.color)),
                None => {
                    let j = self.free_slot();
                    let c = self.color();
                    self.lanes[j] = Some(Lane { expect: p.clone(), color: c });
                    (j, c)
                }
            };
            segs.push(Seg { kind: SegKind::Out, from: col as u16, to: j as u16, color: c });
        }
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        let width = top_width.max(self.lanes.len()).max(col + 1) as u16;
        Placement { col: col as u16, color, segs, width }
    }

    /// Lanes still open (waiting for a commit further down).
    pub fn open_lanes(&self) -> usize {
        self.lanes.iter().filter(|l| l.is_some()).count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefLabelKind {
    /// A detached `HEAD`.
    Head,
    Branch,
    Remote,
    Tag,
    Other,
}

/// A ref pill on a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    /// Short name (`main`, `origin/main`, `v1.2`).
    pub name: String,
    pub kind: RefLabelKind,
    /// The checked-out branch (or the detached HEAD).
    pub current: bool,
}

/// Parses `%D` with `--decorate=full` (`HEAD -> refs/heads/main, refs/remotes/origin/main,
/// tag: refs/tags/v1`). Remote `HEAD` pointers are dropped (noise).
pub fn parse_refs(d: &str) -> Vec<RefLabel> {
    let mut out = Vec::new();
    for item in d.split(", ").map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(target) = item.strip_prefix("HEAD -> ") {
            if let Some(b) = target.strip_prefix("refs/heads/") {
                out.push(RefLabel { name: b.into(), kind: RefLabelKind::Branch, current: true });
            }
        } else if item == "HEAD" {
            out.push(RefLabel { name: "HEAD".into(), kind: RefLabelKind::Head, current: true });
        } else if let Some(t) = item.strip_prefix("tag: ") {
            out.push(RefLabel { name: t.strip_prefix("refs/tags/").unwrap_or(t).into(), kind: RefLabelKind::Tag, current: false });
        } else if let Some(b) = item.strip_prefix("refs/heads/") {
            out.push(RefLabel { name: b.into(), kind: RefLabelKind::Branch, current: false });
        } else if let Some(r) = item.strip_prefix("refs/remotes/") {
            if !r.ends_with("/HEAD") {
                out.push(RefLabel { name: r.into(), kind: RefLabelKind::Remote, current: false });
            }
        } else {
            out.push(RefLabel { name: item.strip_prefix("refs/").unwrap_or(item).into(), kind: RefLabelKind::Other, current: false });
        }
    }
    out
}

/// One row of the graph.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    /// [`WIP`] for the uncommitted-changes row.
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, Unix seconds.
    pub time: i64,
    pub subject: String,
    pub refs: Vec<RefLabel>,
    #[serde(flatten)]
    pub place: Placement,
}

const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D";

/// Refs the graph leaves out (passed before `--all`): the stash, notes, maintenance prefetch refs
/// and the app's own refs.
pub const EXCLUDE: &[&str] = &["--exclude=refs/stash", "--exclude=refs/notes/*", "--exclude=refs/prefetch/*", "--exclude=refs/devall/*"];

/// A `git log` record without its placement yet.
fn parse_record(rec: &[u8]) -> Option<GraphRow> {
    let s = String::from_utf8_lossy(rec);
    let s = s.trim_start_matches('\n');
    let f: Vec<&str> = s.splitn(7, '\x1f').collect();
    if f.len() < 7 || f[0].is_empty() {
        return None;
    }
    Some(GraphRow {
        hash: f[0].to_string(),
        parents: f[1].split_whitespace().map(str::to_string).collect(),
        author: f[2].to_string(),
        email: f[3].to_string(),
        time: f[4].trim().parse().unwrap_or(0),
        subject: f[5].to_string(),
        refs: parse_refs(f[6].trim()),
        place: Placement::default(),
    })
}

/// Streams the whole graph of `dir` (every branch, tag and remote branch, plus `HEAD`; see
/// [`EXCLUDE`] for what's left out) to `on_row`, newest first. With `wip`, a [`WIP`] row whose
/// parent is `HEAD` comes first. Stops early when `cancel` is set.
pub fn stream(dir: &Path, wip: bool, cancel: &AtomicBool, mut on_row: impl FnMut(GraphRow)) -> GitResult<()> {
    let mut layout = Layout::default();
    if wip && let Ok(head) = git::run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        let parents = vec![head.trim().to_string()];
        let place = layout.place(WIP, &parents);
        on_row(GraphRow { hash: WIP.into(), parents, author: String::new(), email: String::new(), time: 0, subject: String::new(), refs: vec![], place });
    }
    let mut child = git::command(dir)
        .args(["log", "--topo-order", "--decorate=full", "-z", FORMAT])
        .args(EXCLUDE)
        .args(["--all", "--"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git is not installed or not on PATH ({e})"))?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let errs = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut rows = 0usize;
    for rec in BufReader::with_capacity(1 << 16, stdout).split(0) {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
        let Ok(rec) = rec else { break };
        let Some(mut row) = parse_record(&rec) else { continue };
        row.place = layout.place(&row.hash, &row.parents);
        on_row(row);
        rows += 1;
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let err = errs.join().unwrap_or_default();
    if !status.success() && rows == 0 {
        let err = err.trim();
        // An empty repository has nothing to show — not an error.
        if err.contains("does not have any commits") || err.is_empty() {
            return Ok(());
        }
        return Err(err.to_string());
    }
    Ok(())
}

/// Changes when anything the graph shows changes: HEAD, any ref.
pub fn signature(dir: &Path) -> String {
    let mut h = Sha256::new();
    h.update(git::run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).unwrap_or_default());
    h.update(git::run(dir, &["symbolic-ref", "--quiet", "HEAD"]).unwrap_or_default());
    h.update(git::run(dir, &["for-each-ref", "--format=%(objectname) %(refname)"]).unwrap_or_default());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Rows whose subject, author, e-mail (case-insensitive) or hash (prefix) match `q`.
pub fn search(rows: &[GraphRow], q: &str) -> Vec<usize> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let hexish = q.len() >= 4 && q.chars().all(|c| c.is_ascii_hexdigit());
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.hash != WIP)
        .filter(|(_, r)| (hexish && r.hash.starts_with(&q)) || r.subject.to_lowercase().contains(&q) || r.author.to_lowercase().contains(&q) || r.email.to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

#[derive(Default)]
struct JobState {
    rows: Vec<GraphRow>,
    index: HashMap<String, usize>,
    done: bool,
    error: Option<String>,
    max_width: u16,
    elapsed_ms: u64,
}

/// A page of rows (and how far the stream got).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPage {
    pub offset: usize,
    pub rows: Vec<GraphRow>,
    /// Rows read so far (the total once `done`).
    pub loaded: usize,
    pub done: bool,
    pub error: Option<String>,
    /// Widest row so far (columns).
    pub max_width: u16,
    /// How long the whole pass took (once done).
    pub elapsed_ms: u64,
}

/// A graph being (or done being) read on a background thread.
pub struct GraphJob {
    state: Mutex<JobState>,
    cv: Condvar,
    cancel: AtomicBool,
    pub signature: String,
    pub wip: bool,
}

impl GraphJob {
    pub fn start(dir: PathBuf, wip: bool, signature: String) -> Arc<GraphJob> {
        let job = Arc::new(GraphJob { state: Mutex::new(JobState::default()), cv: Condvar::new(), cancel: AtomicBool::new(false), signature, wip });
        let j = job.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut batch: Vec<GraphRow> = Vec::with_capacity(256);
            let flush = |batch: &mut Vec<GraphRow>| {
                let mut s = j.state.lock().unwrap_or_else(|e| e.into_inner());
                for r in batch.drain(..) {
                    s.max_width = s.max_width.max(r.place.width);
                    let i = s.rows.len();
                    s.index.insert(r.hash.clone(), i);
                    s.rows.push(r);
                }
                drop(s);
                j.cv.notify_all();
            };
            let res = stream(&dir, wip, &j.cancel, |row| {
                batch.push(row);
                if batch.len() >= 256 {
                    flush(&mut batch);
                }
            });
            flush(&mut batch);
            let mut s = j.state.lock().unwrap_or_else(|e| e.into_inner());
            s.done = true;
            s.error = res.err();
            s.elapsed_ms = started.elapsed().as_millis() as u64;
            drop(s);
            j.cv.notify_all();
        });
        job
    }

    /// Rows `offset..offset + limit`, waiting up to `wait` for the stream to get there.
    pub fn page(&self, offset: usize, limit: usize, wait: Duration) -> GraphPage {
        let deadline = Instant::now() + wait;
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !s.done && s.rows.len() < offset + limit {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
            s = self.cv.wait_timeout(s, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        let end = (offset + limit).min(s.rows.len());
        GraphPage {
            offset,
            rows: s.rows.get(offset.min(end)..end).map(<[GraphRow]>::to_vec).unwrap_or_default(),
            loaded: s.rows.len(),
            done: s.done,
            error: s.error.clone(),
            max_width: s.max_width,
            elapsed_ms: s.elapsed_ms,
        }
    }

    /// Blocks until the whole graph is read (or `wait` passes); true when done.
    pub fn wait_done(&self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !s.done {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { return false };
            s = self.cv.wait_timeout(s, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        true
    }

    /// Matching row indices among the rows read so far.
    pub fn search(&self, q: &str) -> Vec<usize> {
        search(&self.state.lock().unwrap_or_else(|e| e.into_inner()).rows, q)
    }

    /// Row of a commit (full hash, or a unique prefix of 4+ characters) among the rows read so far.
    pub fn find(&self, hash: &str) -> Option<usize> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&i) = s.index.get(hash) {
            return Some(i);
        }
        if hash.len() < 4 {
            return None;
        }
        let mut hits = s.rows.iter().enumerate().filter(|(_, r)| r.hash.starts_with(hash)).map(|(i, _)| i);
        let first = hits.next();
        if hits.next().is_some() { None } else { first }
    }

    /// Stops the reader (its thread holds the job until it exits, so replacing a job must cancel it).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
