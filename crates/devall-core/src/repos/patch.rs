//! Hunk and line staging: parse `git diff -U3` for one file, build a patch from the hunks and
//! lines the user picked, and apply it with `git apply --recount` — to the index (stage), the
//! index in reverse (unstage) or the working tree in reverse (discard).
//!
//! The diff is read again (as bytes, never re-encoded) right before applying and must match the
//! fingerprint of the one the user saw, so a file that changed in the meantime is never patched
//! with stale line numbers. git does the line-ending work on both ends (`core.autocrlf`, CR bytes
//! of CRLF blobs stay in the patch), so CRLF files round-trip.
//!
//! Building a partial patch (forward, for staging): an unpicked `+` line is dropped, an unpicked
//! `-` line becomes context. In reverse (unstage/discard) it's the mirror: an unpicked `+` line
//! becomes context, an unpicked `-` line is dropped. Either way one side of each hunk stays exactly
//! what the target file holds, so the hunk still applies where it was.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::git::{self, GitResult};
use super::undo::{self, Journal, Kind, Op, Scope};

/// Patches with more lines than this are shown on request only ("load anyway").
pub const MAX_VIEW_LINES: usize = 5000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// `b' '`, `b'+'` or `b'-'`.
    pub kind: u8,
    /// Without the leading marker and the `\n` (a CRLF line keeps its `\r`).
    pub text: Vec<u8>,
    /// Followed by `\ No newline at end of file`.
    pub no_eol: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The function context after the second `@@`.
    pub section: Vec<u8>,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilePatch {
    /// `diff --git …`, mode and index lines, `---`, `+++` — verbatim.
    pub header: Vec<Vec<u8>>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

fn parse_range(s: &str) -> (u32, u32) {
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

/// `@@ -1,4 +1,5 @@ fn main()` → (old start, old count, new start, new count, section).
fn parse_hunk_header(line: &[u8]) -> Option<(u32, u32, u32, u32, Vec<u8>)> {
    let rest = line.strip_prefix(b"@@ ")?;
    let end = rest.windows(3).position(|w| w == b" @@")?;
    let ranges = std::str::from_utf8(&rest[..end]).ok()?;
    let mut parts = ranges.split(' ');
    let (os, ol) = parse_range(parts.next()?.strip_prefix('-')?);
    let (ns, nl) = parse_range(parts.next()?.strip_prefix('+')?);
    let section = rest[end + 3..].strip_prefix(b" ").unwrap_or(&rest[end + 3..]).to_vec();
    Some((os, ol, ns, nl, section))
}

/// Parses the diff of one file.
pub fn parse(diff: &[u8]) -> FilePatch {
    let mut p = FilePatch::default();
    let mut lines = diff.split(|&b| b == b'\n').peekable();
    // Lines of the current hunk still to come (old side, new side).
    let mut left = (0u32, 0u32);
    while let Some(line) = lines.next() {
        if left.0 > 0 || left.1 > 0 {
            let hunk = p.hunks.last_mut().expect("counts only set inside a hunk");
            // `diff.suppressBlankEmpty` prints blank context lines without the space.
            let (kind, text) = match line.first() {
                Some(&k @ (b' ' | b'+' | b'-')) => (k, line[1..].to_vec()),
                None => (b' ', Vec::new()),
                _ => {
                    left = (0, 0);
                    continue;
                }
            };
            match kind {
                b' ' => left = (left.0.saturating_sub(1), left.1.saturating_sub(1)),
                b'-' => left.0 = left.0.saturating_sub(1),
                _ => left.1 = left.1.saturating_sub(1),
            }
            hunk.lines.push(Line { kind, text, no_eol: false });
            if lines.peek().is_some_and(|l| l.starts_with(b"\\")) {
                lines.next();
                hunk.lines.last_mut().unwrap().no_eol = true;
            }
            continue;
        }
        if line.starts_with(b"\\") {
            if let Some(l) = p.hunks.last_mut().and_then(|h| h.lines.last_mut()) {
                l.no_eol = true;
            }
        } else if let Some((os, ol, ns, nl, section)) = parse_hunk_header(line) {
            p.hunks.push(Hunk { old_start: os, old_lines: ol, new_start: ns, new_lines: nl, section, lines: Vec::new() });
            left = (ol, nl);
        } else if p.hunks.is_empty() && !line.is_empty() {
            if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
                p.binary = true;
            }
            p.header.push(line.to_vec());
        }
    }
    p
}

/// What to take from one hunk: every line, or the given line indices (others are left alone).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub hunk: usize,
    /// `None` = the whole hunk.
    pub lines: Option<Vec<usize>>,
}

fn first_line(start: u32, count: u32) -> i64 {
    if count == 0 { i64::from(start) + 1 } else { i64::from(start) }
}

fn start_for(first: i64, count: u32) -> i64 {
    if count == 0 { first - 1 } else { first }
}

/// Takes every changed line of every hunk.
fn whole_file(patch: &FilePatch, selection: &[Selection]) -> bool {
    patch.hunks.iter().enumerate().all(|(i, h)| {
        selection.iter().any(|s| {
            s.hunk == i
                && s.lines.as_ref().is_none_or(|l| {
                    let set: HashSet<usize> = l.iter().copied().collect();
                    h.lines.iter().enumerate().all(|(j, line)| line.kind == b' ' || set.contains(&j))
                })
        })
    })
}

/// A partial change of an added/deleted file is a change *of* the file (it exists on both
/// sides afterwards): drop the new/deleted mode line and name the file on both sides.
fn as_modification(header: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let path = header.iter().find_map(|l| l.strip_prefix(b"+++ b/").or_else(|| l.strip_prefix(b"--- a/"))).map(<[u8]>::to_vec).unwrap_or_default();
    header
        .iter()
        .filter(|l| !l.starts_with(b"new file mode") && !l.starts_with(b"deleted file mode"))
        .map(|l| match l.as_slice() {
            b"--- /dev/null" => [b"--- a/".as_slice(), &path].concat(),
            b"+++ /dev/null" => [b"+++ b/".as_slice(), &path].concat(),
            _ => l.clone(),
        })
        .collect()
}

/// The patch for `selection` (see the module docs); `None` when it changes nothing.
pub fn build(patch: &FilePatch, selection: &[Selection], reverse: bool) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let adds_or_deletes = patch.header.iter().any(|l| l.starts_with(b"new file mode") || l.starts_with(b"deleted file mode"));
    let header = if adds_or_deletes && !whole_file(patch, selection) { as_modification(&patch.header) } else { patch.header.clone() };
    for h in &header {
        out.extend_from_slice(h);
        out.push(b'\n');
    }
    let mut delta: i64 = 0;
    let mut any = false;
    let mut picked: Vec<&Selection> = selection.iter().filter(|s| s.hunk < patch.hunks.len()).collect();
    picked.sort_by_key(|s| s.hunk);
    picked.dedup_by_key(|s| s.hunk);
    for sel in picked {
        let h = &patch.hunks[sel.hunk];
        let chosen: Option<HashSet<usize>> = sel.lines.as_ref().map(|l| l.iter().copied().collect());
        let mut body: Vec<(u8, &Line)> = Vec::new();
        for (i, l) in h.lines.iter().enumerate() {
            let on = chosen.as_ref().is_none_or(|c| c.contains(&i));
            match (l.kind, on, reverse) {
                (b' ', _, _) | (b'+' | b'-', true, _) => body.push((l.kind, l)),
                (b'+', false, false) | (b'-', false, true) => {}
                (b'+', false, true) | (b'-', false, false) => body.push((b' ', l)),
                _ => {}
            }
        }
        if !body.iter().any(|(k, _)| *k != b' ') {
            continue;
        }
        let old_count = body.iter().filter(|(k, _)| *k != b'+').count() as u32;
        let new_count = body.iter().filter(|(k, _)| *k != b'-').count() as u32;
        let (old_start, new_start) = if reverse {
            let nf = first_line(h.new_start, h.new_lines);
            (start_for(nf - delta, old_count), start_for(nf, new_count))
        } else {
            let of = first_line(h.old_start, h.old_lines);
            (start_for(of, old_count), start_for(of + delta, new_count))
        };
        delta += i64::from(new_count) - i64::from(old_count);
        out.extend_from_slice(format!("@@ -{},{} +{},{} @@", old_start.max(0), old_count, new_start.max(0), new_count).as_bytes());
        if !h.section.is_empty() {
            out.push(b' ');
            out.extend_from_slice(&h.section);
        }
        out.push(b'\n');
        for (k, l) in body {
            out.push(k);
            out.extend_from_slice(&l.text);
            out.push(b'\n');
            if l.no_eol {
                out.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
        any = true;
    }
    any.then_some(out)
}

/// `git diff -U3` of one file as raw bytes: staged = HEAD ↔ index, else index ↔ working tree.
/// External diff drivers, textconv, colours and custom prefixes are switched off so the output is
/// a patch `git apply` takes back.
pub fn file_diff_raw(dir: &Path, path: &str, staged: bool, ignore_ws: bool) -> GitResult<Vec<u8>> {
    if path.is_empty() || path.starts_with('-') {
        return Err(format!("invalid path {path}"));
    }
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U3", "--src-prefix=a/", "--dst-prefix=b/"];
    if staged {
        args.push("--cached");
    }
    if ignore_ws {
        args.push("-w");
    }
    args.extend(["--", path]);
    git::run_bytes(dir, &args)
}

/// Identifies the exact diff the user saw.
pub fn fingerprint(raw: &[u8]) -> String {
    Sha256::digest(raw).iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// A diff line for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    /// `" "`, `"+"` or `"-"`.
    pub kind: String,
    /// Without the line ending.
    pub text: String,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub no_eol: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HunkView {
    /// `@@ -1,4 +1,5 @@ fn main()`.
    pub header: String,
    pub lines: Vec<LineView>,
}

/// One file's diff for the Changes view (hunks with line numbers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchView {
    pub path: String,
    pub staged: bool,
    pub binary: bool,
    /// Pass back with a selection: the diff must not have changed since.
    pub fingerprint: String,
    pub hunks: Vec<HunkView>,
    /// Total diff lines; over [`MAX_VIEW_LINES`] the hunks are left out unless forced.
    pub line_count: usize,
    pub too_large: bool,
    /// The file's lines end in CRLF.
    pub crlf: bool,
}

pub fn view(path: &str, staged: bool, raw: &[u8], force: bool) -> PatchView {
    let p = parse(raw);
    let line_count: usize = p.hunks.iter().map(|h| h.lines.len()).sum();
    let too_large = line_count > MAX_VIEW_LINES && !force;
    let crlf = p.hunks.iter().flat_map(|h| &h.lines).any(|l| l.text.ends_with(b"\r"));
    let hunks = if too_large {
        vec![]
    } else {
        p.hunks
            .iter()
            .map(|h| {
                let (mut o, mut n) = (h.old_start, h.new_start);
                let lines = h
                    .lines
                    .iter()
                    .map(|l| {
                        let (old, new) = match l.kind {
                            b' ' => {
                                o += 1;
                                n += 1;
                                (Some(o - 1), Some(n - 1))
                            }
                            b'-' => {
                                o += 1;
                                (Some(o - 1), None)
                            }
                            _ => {
                                n += 1;
                                (None, Some(n - 1))
                            }
                        };
                        let text = l.text.strip_suffix(b"\r").unwrap_or(&l.text);
                        LineView { kind: (l.kind as char).to_string(), text: String::from_utf8_lossy(text).into_owned(), old, new, no_eol: l.no_eol }
                    })
                    .collect();
                let mut header = format!("@@ -{},{} +{},{} @@", h.old_start, h.old_lines, h.new_start, h.new_lines);
                if !h.section.is_empty() {
                    header.push(' ');
                    header.push_str(&String::from_utf8_lossy(&h.section));
                }
                HunkView { header, lines }
            })
            .collect()
    };
    PatchView { path: path.to_string(), staged, binary: p.binary, fingerprint: fingerprint(raw), hunks, line_count, too_large, crlf }
}

/// The diff of one changed file for the Changes view.
pub fn file_view(dir: &Path, path: &str, staged: bool, ignore_ws: bool, force: bool) -> GitResult<PatchView> {
    Ok(view(path, staged, &file_diff_raw(dir, path, staged, ignore_ws)?, force))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LineAction {
    /// Unstaged diff → index.
    Stage,
    /// Staged diff → out of the index (back to unstaged).
    Unstage,
    /// Unstaged diff → out of the working tree (gone).
    Discard,
}

/// Applies the picked hunks/lines of one file. `fingerprint` is the [`PatchView`]'s: if the diff
/// changed since, nothing happens and the user is asked to look again. Journaled for undo (a
/// discard snapshots the file first).
pub fn apply_selection(dir: &Path, j: Option<&Journal>, path: &str, action: LineAction, fingerprint_seen: &str, selection: &[Selection], ignore_ws: bool) -> GitResult<()> {
    let lines: usize = selection.iter().map(|s| s.lines.as_ref().map_or(0, Vec::len)).sum();
    let what = if lines > 0 { format!("{lines} line{} in {path}", if lines == 1 { "" } else { "s" }) } else { format!("{} hunk{} in {path}", selection.len(), if selection.len() == 1 { "" } else { "s" }) };
    let op = match action {
        LineAction::Stage => Op::new(Kind::Stage, format!("Stage {what}"), Scope::index()),
        LineAction::Unstage => Op::new(Kind::Unstage, format!("Unstage {what}"), Scope::index()),
        LineAction::Discard => Op::new(Kind::Discard, format!("Discard {what}"), Scope::paths(vec![path.to_string()])),
    };
    undo::journaled(dir, j, op, || {
        let raw = file_diff_raw(dir, path, action == LineAction::Unstage, ignore_ws)?;
        if fingerprint(&raw) != fingerprint_seen {
            return Err("the file changed since its diff was shown — look at it again and retry".into());
        }
        let patch = parse(&raw);
        if patch.binary {
            return Err("binary files can only be staged, unstaged or discarded as a whole".into());
        }
        let reverse = action != LineAction::Stage;
        let bytes = build(&patch, selection, reverse).ok_or("nothing selected to change")?;
        let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
        if action != LineAction::Discard {
            args.push("--cached");
        }
        if reverse {
            args.push("-R");
        }
        if ignore_ws {
            args.push("--ignore-whitespace");
        }
        args.push("-");
        git::run_input(dir, &args, &bytes).map(|_| ())
    })
}

#[cfg(test)]
#[path = "patch_tests.rs"]
mod tests;
