//! Blame and file history.
//!
//! - Blame: `git blame --porcelain -M -C -w` (moved/copied lines followed, whitespace-only
//!   changes ignored), honouring `.git-blame-ignore-revs` at the repository root (and the user's
//!   `blame.ignoreRevsFile`). Lines not committed yet come back with an all-zero commit.
//! - File history: `git log --follow` with each version's path (renames followed) and its parent,
//!   so the UI can diff any version against the one before.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use super::compare;
use super::git::{self, GitResult};

/// Files bigger than this aren't blamed.
pub const MAX_BLAME_BYTES: u64 = 4 * 1024 * 1024;
pub const IGNORE_REVS_FILE: &str = ".git-blame-ignore-revs";

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameCommit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub email: String,
    /// Unix seconds.
    pub time: i64,
    pub summary: String,
    /// The line's previous version: (commit, path) — "blame the line before this change".
    pub previous: Option<(String, String)>,
    /// The path in that commit (renames).
    pub filename: String,
    /// Not committed yet.
    pub uncommitted: bool,
    pub boundary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameLine {
    /// Index into [`Blame::commits`].
    pub commit: usize,
    /// Line number in that commit's version.
    pub orig_line: u32,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blame {
    pub path: String,
    pub rev: Option<String>,
    pub commits: Vec<BlameCommit>,
    pub lines: Vec<BlameLine>,
    /// `.git-blame-ignore-revs` was applied.
    pub ignored_revs: bool,
}

/// Parses `git blame --porcelain` output.
pub fn parse_porcelain(out: &str) -> (Vec<BlameCommit>, Vec<BlameLine>) {
    let mut commits: Vec<BlameCommit> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut lines = Vec::new();
    let mut cur: Option<usize> = None;
    let mut orig = 0u32;
    for line in out.split('\n') {
        if let Some(text) = line.strip_prefix('\t') {
            if let Some(c) = cur {
                lines.push(BlameLine { commit: c, orig_line: orig, text: text.strip_suffix('\r').unwrap_or(text).to_string() });
            }
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let first = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default();
        if first.len() >= 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            let i = *index.entry(first.to_string()).or_insert_with(|| {
                commits.push(BlameCommit { hash: first.to_string(), short: first[..7].to_string(), uncommitted: first.chars().all(|c| c == '0'), ..Default::default() });
                commits.len() - 1
            });
            cur = Some(i);
            orig = rest.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            continue;
        }
        let Some(c) = cur.map(|i| &mut commits[i]) else { continue };
        match first {
            "author" => c.author = rest.to_string(),
            "author-mail" => c.email = rest.trim_matches(['<', '>']).to_string(),
            "author-time" => c.time = rest.trim().parse().unwrap_or(0),
            "summary" => c.summary = rest.to_string(),
            "filename" => c.filename = rest.to_string(),
            "boundary" => c.boundary = true,
            "previous" => {
                if let Some((sha, path)) = rest.split_once(' ') {
                    c.previous = Some((sha.to_string(), path.to_string()));
                }
            }
            _ => {}
        }
    }
    (commits, lines)
}

fn check_path(p: &str) -> GitResult<()> {
    if p.is_empty() || p.starts_with('-') || p.contains('\0') || p.split(['/', '\\']).any(|s| s == "..") { Err(format!("invalid path {p}")) } else { Ok(()) }
}

/// Blames `path` at `rev` (`None`: the working-tree file, uncommitted lines included).
pub fn blame(dir: &Path, path: &str, rev: Option<&str>) -> GitResult<Blame> {
    check_path(path)?;
    let sha = rev.map(|r| compare::resolve(dir, r)).transpose()?;
    if sha.is_none() {
        let abs = crate::fsutil::safe_join(dir, path).map_err(|e| e.to_string())?;
        if std::fs::metadata(&abs).is_ok_and(|m| m.len() > MAX_BLAME_BYTES) {
            return Err(format!("{path} is too large to blame here"));
        }
    }
    let ignore = dir.join(IGNORE_REVS_FILE).is_file();
    let ignore_arg = format!("--ignore-revs-file={IGNORE_REVS_FILE}");
    let mut args = vec!["blame", "--porcelain", "-M", "-C", "-w"];
    if ignore {
        args.push(&ignore_arg);
    }
    if let Some(s) = &sha {
        args.push(s);
    }
    args.extend(["--", path]);
    let out = match git::run(dir, &args) {
        Ok(o) => o,
        // A broken ignore-revs file (or a `blame.ignoreRevsFile` pointing nowhere): blame without.
        Err(e) if e.contains("ignore-revs") || e.contains("could not open object name list") => {
            let mut plain = vec!["-c", "blame.ignoreRevsFile=", "blame", "--porcelain", "-M", "-C", "-w"];
            if let Some(s) = &sha {
                plain.push(s);
            }
            plain.extend(["--", path]);
            git::run(dir, &plain)?
        }
        Err(e) => return Err(e),
    };
    let (commits, lines) = parse_porcelain(&out);
    Ok(Blame { path: path.to_string(), rev: sha, commits, lines, ignored_revs: ignore })
}

/// One version of a file in its history.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    pub hash: String,
    pub short: String,
    /// First parent (`None` for a root commit) — diff against it at `old_path`.
    pub parent: Option<String>,
    pub author: String,
    pub email: String,
    /// ISO 8601.
    pub date: String,
    pub subject: String,
    /// The file's path in this commit.
    pub path: String,
    /// Its path in the parent (differs after a rename).
    pub old_path: String,
    /// A, M, D, R, C…
    pub status: String,
}

/// Parses `git log --follow -z --name-status --format=%x1e…`.
pub fn parse_history(out: &str, path: &str) -> Vec<FileVersion> {
    let mut list = Vec::new();
    for rec in out.split('\x1e').filter(|r| !r.trim().is_empty()) {
        let mut tokens = rec.split('\0');
        let head = tokens.next().unwrap_or_default();
        let f: Vec<&str> = head.splitn(7, '\x1f').collect();
        if f.len() < 7 {
            continue;
        }
        let rest: Vec<&str> = tokens.map(|t| t.trim_start_matches('\n')).filter(|t| !t.is_empty()).collect();
        let (status, old_path, new_path) = match rest.as_slice() {
            [st, old, new, ..] if st.starts_with('R') || st.starts_with('C') => (st[..1].to_string(), old.to_string(), new.to_string()),
            [st, p, ..] => (st[..1].to_string(), p.to_string(), p.to_string()),
            _ => ("M".to_string(), path.to_string(), path.to_string()),
        };
        list.push(FileVersion {
            hash: f[0].to_string(),
            short: f[1].to_string(),
            parent: f[2].split_whitespace().next().map(str::to_string),
            author: f[3].to_string(),
            email: f[4].to_string(),
            date: f[5].to_string(),
            subject: f[6].to_string(),
            path: new_path,
            old_path,
            status,
        });
    }
    list
}

/// Newest first, renames followed, at most `limit` versions (≤ 1000).
pub fn file_history(dir: &Path, path: &str, limit: usize) -> GitResult<Vec<FileVersion>> {
    check_path(path)?;
    let n = format!("-n{}", limit.clamp(1, 1000));
    let out = git::run(dir, &["log", "--follow", "-z", "-M", "--name-status", &n, "--format=%x1e%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%s", "--", path])?;
    Ok(parse_history(&out, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::testgit::repo;

    #[test]
    fn parses_porcelain() {
        let out = "abcdef0123456789abcdef0123456789abcdef01 1 1 2\nauthor Ann\nauthor-mail <ann@x.io>\nauthor-time 1700000000\nsummary First\nprevious 1111111111111111111111111111111111111111 old.txt\nfilename a.txt\n\tline one\nabcdef0123456789abcdef0123456789abcdef01 2 2\n\tline two\n0000000000000000000000000000000000000000 3 3 1\nauthor Not Committed Yet\nsummary Version of a.txt from a.txt\nfilename a.txt\n\tnew line\n";
        let (c, l) = parse_porcelain(out);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].author, "Ann");
        assert_eq!(c[0].email, "ann@x.io");
        assert_eq!(c[0].previous.as_ref().map(|p| p.1.as_str()), Some("old.txt"));
        assert!(c[1].uncommitted);
        assert_eq!(l.iter().map(|x| (x.commit, x.text.as_str())).collect::<Vec<_>>(), [(0, "line one"), (0, "line two"), (1, "new line")]);
    }

    #[test]
    fn blames_follows_renames_and_honours_ignore_revs() {
        let Some(r) = repo() else { return };
        r.write("a.txt", "one\ntwo\n");
        let first = r.commit("first");
        r.write("a.txt", "one\ntwo\nthree\n");
        let second = r.commit("second");
        r.git(&["mv", "a.txt", "b.txt"]);
        r.commit("rename");
        // A formatting commit, ignored via .git-blame-ignore-revs.
        r.write("b.txt", "ONE\ntwo\nthree\n");
        let fmt = r.commit("format");
        r.write("b.txt", "ONE\ntwo\nthree\nfour\n");
        let b = blame(r.root(), "b.txt", None).unwrap();
        assert_eq!(b.lines.len(), 4);
        assert_eq!(b.commits[b.lines[2].commit].hash, second);
        assert_eq!(b.commits[b.lines[1].commit].hash, first);
        assert!(b.commits[b.lines[3].commit].uncommitted);
        assert_eq!(b.commits[b.lines[0].commit].hash, fmt);
        r.write(IGNORE_REVS_FILE, format!("{fmt}\n"));
        let b = blame(r.root(), "b.txt", Some("HEAD")).unwrap();
        assert!(b.ignored_revs);
        assert_ne!(b.commits[b.lines[0].commit].hash, fmt, "the ignored commit is skipped");
        assert_eq!(b.lines.len(), 3);
        let h = file_history(r.root(), "b.txt", 50).unwrap();
        let subjects: Vec<&str> = h.iter().map(|v| v.subject.as_str()).collect();
        assert_eq!(subjects, ["format", "rename", "second", "first"]);
        assert_eq!(h[1].status, "R");
        assert_eq!(h[1].old_path, "a.txt");
        assert_eq!(h[1].path, "b.txt");
        assert_eq!(h[3].path, "a.txt");
        assert_eq!(h[3].parent, None);
        assert!(blame(r.root(), "../x", None).is_err());
    }
}
