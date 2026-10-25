//! Comparing two refs the way a merge request will show them — the review before opening one.
//!
//! - **Since they diverged** ([`Mode::MergeBase`], the default): `git diff $(git merge-base B H) H`,
//!   what GitHub/GitLab show for an MR (`B...H`). Newer commits on the base don't show up reversed.
//! - **Direct** ([`Mode::Direct`]): tip to tip, `git diff B H`.
//! - **Uncommitted**: when the head is the checked-out branch, the diff can run against the working
//!   tree instead (staged + unstaged + untracked files).
//!
//! Everything here reads: the conflict prediction (`git merge-tree --write-tree`, Git ≥ 2.38) writes
//! no ref, index or working-tree file. Revisions from the UI pass [`valid_rev`] and are resolved to
//! SHAs before they reach any other git command.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::git::{self, GitResult};

/// Commits listed per comparison (the rest are counted, not listed).
pub const MAX_COMMITS: usize = 2000;
/// Largest file side read for a text diff (bigger files show "too large").
pub const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;
/// Largest image side shown in an image diff.
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Changes on the head since it branched off the base (`B...H`, what an MR shows).
    #[default]
    MergeBase,
    /// Tip to tip (`B H`).
    Direct,
}

/// A branch, tag, commit or simple revision expression (`HEAD~2`) the UI may pass. Never an
/// option (`-x`), a range (`a..b`, built here) or a `rev:path` / search expression.
pub fn valid_rev(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.starts_with('-') && !s.contains("..") && !s.chars().any(|c| c.is_whitespace() || c.is_control() || ":?*[\\".contains(c))
}

/// A full or abbreviated object id.
pub fn valid_oid(s: &str) -> bool {
    (4..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn check_rev(s: &str) -> GitResult<()> {
    if valid_rev(s) { Ok(()) } else { Err(format!("“{s}” isn't a branch, tag or commit")) }
}

/// Full SHA of the commit `rev` names.
pub fn resolve(dir: &Path, rev: &str) -> GitResult<String> {
    check_rev(rev)?;
    git::run(dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("“{rev}” isn't a branch, tag or commit in this repository"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    Branch,
    Remote,
    Tag,
    Commit,
}

/// One side of a comparison: what the user picked and what it is right now.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefInfo {
    /// As given (`main`, `origin/main`, `v1.2`, a SHA).
    pub name: String,
    pub sha: String,
    pub short: String,
    pub kind: RefKind,
    /// `refs/heads/main` when it names a ref.
    pub refname: Option<String>,
}

pub fn ref_info(dir: &Path, rev: &str) -> GitResult<RefInfo> {
    let sha = resolve(dir, rev)?;
    let refname = git::run(dir, &["rev-parse", "--symbolic-full-name", rev]).ok().map(|s| s.trim().to_string()).filter(|s| s.starts_with("refs/"));
    let kind = match refname.as_deref() {
        Some(r) if r.starts_with("refs/heads/") => RefKind::Branch,
        Some(r) if r.starts_with("refs/remotes/") => RefKind::Remote,
        Some(r) if r.starts_with("refs/tags/") => RefKind::Tag,
        _ => RefKind::Commit,
    };
    Ok(RefInfo { name: rev.to_string(), short: sha[..sha.len().min(7)].to_string(), sha, kind, refname })
}

pub fn remotes(dir: &Path) -> Vec<String> {
    git::run(dir, &["remote"]).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

pub fn current_branch(dir: &Path) -> Option<String> {
    git::run(dir, &["branch", "--show-current"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn ref_exists(dir: &Path, refname: &str) -> bool {
    git::run(dir, &["show-ref", "--verify", "--quiet", refname]).is_ok()
}

fn config(dir: &Path, key: &str) -> Option<String> {
    git::run(dir, &["config", "--get", key]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The remote the current branch tracks, else `origin`, else the first remote.
pub fn main_remote(dir: &Path) -> Option<String> {
    let all = remotes(dir);
    current_branch(dir)
        .and_then(|b| config(dir, &format!("branch.{b}.remote")))
        .filter(|r| all.contains(r))
        .or_else(|| all.iter().find(|r| *r == "origin").cloned())
        .or_else(|| all.first().cloned())
}

const DEFAULT_BRANCHES: &[&str] = &["main", "master", "develop", "trunk"];

/// What a branch is usually compared against: the remote's default branch
/// (`refs/remotes/<remote>/HEAD`, e.g. `origin/main`), else `<remote>/main|master|develop|trunk`,
/// else a local branch of those names.
pub fn default_base(dir: &Path) -> Option<String> {
    if let Some(r) = main_remote(dir) {
        if let Ok(s) = git::run(dir, &["symbolic-ref", "--quiet", "--short", &format!("refs/remotes/{r}/HEAD")]) {
            let s = s.trim();
            if !s.is_empty() && resolve(dir, s).is_ok() {
                return Some(s.to_string());
            }
        }
        if let Some(b) = DEFAULT_BRANCHES.iter().find(|b| ref_exists(dir, &format!("refs/remotes/{r}/{b}"))) {
            return Some(format!("{r}/{b}"));
        }
    }
    DEFAULT_BRANCHES.iter().find(|b| ref_exists(dir, &format!("refs/heads/{b}"))).map(|b| b.to_string())
}

/// A file that differs between the two sides.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    /// New path (the old one for deletions), `/`-separated.
    pub path: String,
    /// Where a renamed/copied file came from.
    pub old_path: Option<String>,
    /// A, M, D, R, C, T (type change) or U (unmerged).
    pub status: String,
    /// Rename/copy similarity (0–100).
    pub similarity: Option<u8>,
    /// Lines added/deleted; `None` for binary files.
    pub added: Option<u32>,
    pub deleted: Option<u32>,
    pub binary: bool,
    /// Blob ids of both sides (`None` where the side doesn't exist). Reviewed marks are keyed to
    /// `new_blob`, so a file that changes after it was reviewed shows up as changed.
    pub old_blob: Option<String>,
    pub new_blob: Option<String>,
    /// Lockfiles, minified bundles, `linguist-generated` paths: collapsed by default.
    pub generated: bool,
    /// Untracked file (only when comparing against the working tree).
    pub untracked: bool,
}

fn null_oid(s: &str) -> bool {
    s.chars().all(|c| c == '0')
}

/// Parses `git diff --raw -z --no-abbrev`.
pub fn parse_raw(out: &str) -> Vec<ChangedFile> {
    let mut files = Vec::new();
    let mut tokens = out.split('\0');
    while let Some(meta) = tokens.next() {
        let Some(meta) = meta.strip_prefix(':') else { continue };
        // :<old mode> <new mode> <old sha> <new sha> <status><score>
        let f: Vec<&str> = meta.split(' ').collect();
        if f.len() < 5 {
            continue;
        }
        let status = &f[4][..1];
        let similarity = f[4][1..].parse::<u8>().ok();
        let first = tokens.next().unwrap_or_default().to_string();
        let (path, old_path) = if matches!(status, "R" | "C") { (tokens.next().unwrap_or_default().to_string(), Some(first)) } else { (first, None) };
        let blob = |s: &str| (!null_oid(s)).then(|| s.to_string());
        files.push(ChangedFile {
            path,
            old_path,
            status: status.to_string(),
            similarity: similarity.filter(|_| matches!(status, "R" | "C")),
            old_blob: blob(f[2]),
            new_blob: if status == "D" { None } else { blob(f[3]) },
            ..Default::default()
        });
    }
    files
}

/// Parses `git diff --numstat -z` into `new path → (added, deleted)` (`None` = binary).
pub fn parse_numstat(out: &str) -> HashMap<String, Option<(u32, u32)>> {
    let mut map = HashMap::new();
    let mut tokens = out.split('\0');
    while let Some(t) = tokens.next() {
        let mut parts = t.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let counts = a.parse::<u32>().ok().zip(d.parse::<u32>().ok());
        // A rename: the path field is empty and old/new paths follow as their own tokens.
        let path = if p.is_empty() {
            let _old = tokens.next();
            tokens.next().unwrap_or_default().to_string()
        } else {
            p.to_string()
        };
        map.insert(path, counts);
    }
    map
}

const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "bun.lock",
    "Cargo.lock",
    "Gemfile.lock",
    "composer.lock",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "go.sum",
    "mix.lock",
    "pubspec.lock",
    "Podfile.lock",
    "packages.lock.json",
    "flake.lock",
    "gradle.lockfile",
    "deno.lock",
];

/// Lockfiles and minified bundles — collapsed by default even without `linguist-generated`.
pub fn looks_generated(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    LOCKFILES.contains(&name) || name.ends_with(".min.js") || name.ends_with(".min.css") || name.ends_with(".js.map") || name.ends_with(".css.map")
}

/// `linguist-generated` per path from `.gitattributes`: `Some(true|false)` when set either way.
fn generated_attrs(dir: &Path, paths: &[&str]) -> HashMap<String, bool> {
    if paths.is_empty() {
        return HashMap::new();
    }
    let input: Vec<u8> = paths.iter().flat_map(|p| p.bytes().chain(std::iter::once(0))).collect();
    let Ok(out) = git::run_input(dir, &["check-attr", "-z", "--stdin", "linguist-generated"], &input) else { return HashMap::new() };
    let text = String::from_utf8_lossy(&out);
    let t: Vec<&str> = text.split('\0').collect();
    t.chunks(3)
        .filter(|c| c.len() == 3)
        .filter_map(|c| match c[2] {
            "set" | "true" => Some((c[0].to_string(), true)),
            "unset" | "false" => Some((c[0].to_string(), false)),
            _ => None,
        })
        .collect()
}

/// Blob ids git would store for working-tree files (filters such as autocrlf applied).
fn hash_worktree(dir: &Path, paths: &[String]) -> HashMap<String, String> {
    let usable: Vec<&String> = paths.iter().filter(|p| !p.contains('\n')).collect();
    if usable.is_empty() {
        return HashMap::new();
    }
    let input = usable.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("\n") + "\n";
    let Ok(out) = git::run_input(dir, &["hash-object", "--stdin-paths"], input.as_bytes()) else { return HashMap::new() };
    usable.into_iter().cloned().zip(String::from_utf8_lossy(&out).lines().map(|l| l.trim().to_string())).collect()
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

fn count_lines(bytes: &[u8]) -> u32 {
    let n = bytes.iter().filter(|&&b| b == b'\n').count();
    (n + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"))) as u32
}

/// The files that differ from `from` to `to` (`None` = the working tree, tracked files — plus
/// untracked ones when `untracked`), with line counts, blob ids and the generated flag.
pub fn range_files(dir: &Path, from: &str, to: Option<&str>, untracked: bool) -> GitResult<Vec<ChangedFile>> {
    if !valid_oid(from) {
        check_rev(from)?;
    }
    if let Some(t) = to {
        check_rev(t)?;
    }
    let mut raw = vec!["diff", "--raw", "-z", "--no-abbrev", "-M", from];
    let mut num = vec!["diff", "--numstat", "-z", "-M", from];
    if let Some(t) = to {
        raw.push(t);
        num.push(t);
    }
    let mut files = parse_raw(&git::run(dir, &raw)?);
    let counts = parse_numstat(&git::run(dir, &num)?);
    for f in &mut files {
        match counts.get(&f.path) {
            Some(Some((a, d))) => (f.added, f.deleted) = (Some(*a), Some(*d)),
            Some(None) => f.binary = true,
            None => {}
        }
    }
    if to.is_none() {
        // The working-tree side has no blob yet: hash what git would store.
        let pending: Vec<String> = files.iter().filter(|f| f.status != "D" && f.new_blob.is_none()).map(|f| f.path.clone()).collect();
        let hashed = hash_worktree(dir, &pending);
        for f in &mut files {
            if f.new_blob.is_none() && f.status != "D" {
                f.new_blob = hashed.get(&f.path).cloned();
            }
        }
        if untracked {
            let list = git::run(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
            let paths: Vec<String> = list.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect();
            let hashed = hash_worktree(dir, &paths);
            for p in paths {
                let abs = crate::fsutil::safe_join(dir, &p).ok();
                let meta = abs.as_ref().and_then(|a| std::fs::metadata(a).ok());
                let bytes = match (&abs, &meta) {
                    (Some(a), Some(m)) if m.is_file() && m.len() <= MAX_TEXT_BYTES => std::fs::read(a).unwrap_or_default(),
                    _ => Vec::new(),
                };
                let binary = is_binary(&bytes) || meta.as_ref().is_some_and(|m| m.len() > MAX_TEXT_BYTES);
                files.push(ChangedFile {
                    new_blob: hashed.get(&p).cloned(),
                    added: (!binary).then(|| count_lines(&bytes)),
                    deleted: (!binary).then_some(0),
                    binary,
                    status: "A".into(),
                    untracked: true,
                    path: p,
                    ..Default::default()
                });
            }
        }
    }
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let attrs = generated_attrs(dir, &paths);
    for f in &mut files {
        f.generated = attrs.get(&f.path).copied().unwrap_or_else(|| looks_generated(&f.path));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// A commit of the head that isn't on the base.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeCommit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// ISO 8601.
    pub date: String,
    pub subject: String,
    /// The same change is already on the base (cherry-picked, rebased): `--cherry-mark`.
    pub equivalent: bool,
}

/// Above this many commits on either side, patch-equivalence (`--cherry-mark`, a patch id per
/// commit on both sides) is skipped to keep huge comparisons quick.
pub const MAX_CHERRY_MARK: u32 = 1500;

/// Commits reachable from `head` but not from `base` (`git log base..head`), newest first; with
/// `cherry`, `equivalent` marks patches the base already has. The bool says the list was cut at
/// `limit`.
pub fn range_commits(dir: &Path, base: &str, head: &str, limit: usize, cherry: bool) -> GitResult<(Vec<RangeCommit>, bool)> {
    let n = format!("-n{}", limit + 1);
    let range = format!("{base}...{head}");
    let mut args = vec!["log", "-z", "--right-only"];
    if cherry {
        args.push("--cherry-mark");
    }
    args.extend([n.as_str(), "--format=%m%x1f%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%s", &range, "--"]);
    let out = git::run(dir, &args)?;
    let mut list: Vec<RangeCommit> = out
        .split('\0')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_start_matches('\n').split('\x1f').collect();
            (f.len() == 8).then(|| RangeCommit {
                equivalent: f[0] == "=",
                hash: f[1].into(),
                short: f[2].into(),
                parents: f[3].split_whitespace().map(str::to_string).collect(),
                author: f[4].into(),
                email: f[5].into(),
                date: f[6].into(),
                subject: f[7].into(),
            })
        })
        .collect();
    let truncated = list.len() > limit;
    list.truncate(limit);
    Ok((list, truncated))
}

/// `git rev-list --left-right --count base...head` → (behind, ahead): commits only the base has,
/// commits only the head has.
pub fn ahead_behind(dir: &Path, base: &str, head: &str) -> GitResult<(u32, u32)> {
    let out = git::run(dir, &["rev-list", "--left-right", "--count", &format!("{base}...{head}")])?;
    let mut it = out.split_whitespace().map(|n| n.parse::<u32>().unwrap_or(0));
    Ok((it.next().unwrap_or(0), it.next().unwrap_or(0)))
}

/// All merge bases (more than one = criss-cross merges; empty = unrelated histories).
pub fn merge_bases(dir: &Path, a: &str, b: &str) -> Vec<String> {
    git::run(dir, &["merge-base", "--all", a, b]).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// The empty tree's id in this repository (SHA-1 or SHA-256).
pub fn empty_tree(dir: &Path) -> String {
    git::run_input(dir, &["hash-object", "-t", "tree", "--stdin"], b"").map(|o| String::from_utf8_lossy(&o).trim().to_string()).unwrap_or_else(|_| "4b825dc642cb6eb9a060e54bf8d69288fbee4904".into())
}

/// What a commit's own changes are diffed against: its first parent, or the empty tree for a root.
pub fn commit_base(dir: &Path, rev: &str) -> GitResult<String> {
    let sha = resolve(dir, rev)?;
    match git::run(dir, &["rev-parse", "--verify", "--quiet", &format!("{sha}^1")]) {
        Ok(p) if !p.trim().is_empty() => Ok(p.trim().to_string()),
        _ => Ok(empty_tree(dir)),
    }
}

/// How old the last fetch is (`FETCH_HEAD`'s age), `None` if this clone never fetched.
pub fn last_fetch_age(dir: &Path) -> Option<u64> {
    let p = git::run(dir, &["rev-parse", "--git-path", "FETCH_HEAD"]).ok()?.trim().to_string();
    let path = if Path::new(&p).is_absolute() { std::path::PathBuf::from(p) } else { dir.join(p) };
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(m).map(|d| d.as_secs()).unwrap_or(0))
}

/// `refs/remotes/origin/feature/x` → (`origin`, `feature/x`), matching the longest remote name.
fn split_remote_ref(refname: &str, remotes: &[String]) -> Option<(String, String)> {
    let rest = refname.strip_prefix("refs/remotes/")?;
    remotes.iter().filter(|r| rest.starts_with(&format!("{r}/"))).max_by_key(|r| r.len()).map(|r| (r.clone(), rest[r.len() + 1..].to_string()))
}

/// Where a side lives on its remote: (`remote`, branch name there).
fn remote_branch(dir: &Path, info: &RefInfo, remotes: &[String]) -> Option<(String, String)> {
    let refname = info.refname.as_deref()?;
    if let Some(x) = split_remote_ref(refname, remotes) {
        return Some(x);
    }
    let branch = refname.strip_prefix("refs/heads/")?;
    let remote = config(dir, &format!("branch.{branch}.remote")).filter(|r| remotes.contains(r));
    let merge = config(dir, &format!("branch.{branch}.merge")).and_then(|m| m.strip_prefix("refs/heads/").map(str::to_string));
    remote.map(|r| (r, merge.unwrap_or_else(|| branch.to_string())))
}

/// Everything the compare view shows at the top, plus the file and commit lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub base: RefInfo,
    pub head: RefInfo,
    /// The mode in effect ([`Mode::Direct`] when the histories are unrelated).
    pub mode: Mode,
    pub merge_base: Option<String>,
    /// More than one = criss-cross merges (the first is used, as git does).
    pub merge_bases: usize,
    pub unrelated: bool,
    /// The diff runs from this commit (or tree)…
    pub from: String,
    /// …to this commit, or the working tree when `None`.
    pub to: Option<String>,
    /// The head is the checked-out branch (uncommitted changes can be included).
    pub head_is_current: bool,
    /// Uncommitted changes are included.
    pub uncommitted: bool,
    /// Commits on the head the base doesn't have (the MR's commits).
    pub ahead: u32,
    /// Commits on the base since the head branched off.
    pub behind: u32,
    pub files: Vec<ChangedFile>,
    pub commits: Vec<RangeCommit>,
    pub commits_truncated: bool,
    /// The base's remote, when it is (or tracks) a remote branch.
    pub base_remote: Option<String>,
    /// Seconds since the last fetch (`None`: never fetched).
    pub fetched_secs_ago: Option<u64>,
    /// URL of the remote (credentials stripped) — to open the host's compare page.
    pub remote_url: Option<String>,
    /// Branch names on the remote, for that page.
    pub base_remote_branch: Option<String>,
    pub head_remote_branch: Option<String>,
    /// The head is (or tracks) a branch on the remote — the host's compare page can show it.
    pub head_pushed: bool,
}

/// Compares `head` against `base` (see the module docs).
pub fn compare(dir: &Path, base: &str, head: &str, mode: Mode, uncommitted: bool) -> GitResult<Comparison> {
    let base_info = ref_info(dir, base)?;
    let head_info = ref_info(dir, head)?;
    let head_sha = git::run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
    let head_ref = git::run(dir, &["symbolic-ref", "--quiet", "HEAD"]).map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty());
    let head_is_current = head_info.sha == head_sha
        && (head_info.name == "HEAD" || (head_info.refname.is_some() && head_info.refname == head_ref) || (head_ref.is_none() && head_info.kind == RefKind::Commit));
    let bases = merge_bases(dir, &base_info.sha, &head_info.sha);
    let unrelated = bases.is_empty();
    let mode = if unrelated { Mode::Direct } else { mode };
    let from = match mode {
        Mode::MergeBase => bases[0].clone(),
        Mode::Direct => base_info.sha.clone(),
    };
    let uncommitted = uncommitted && head_is_current;
    let to = (!uncommitted).then(|| head_info.sha.clone());
    let files = range_files(dir, &from, to.as_deref(), uncommitted)?;
    let (behind, ahead) = if unrelated { (0, 0) } else { ahead_behind(dir, &base_info.sha, &head_info.sha)? };
    let cherry = behind <= MAX_CHERRY_MARK && ahead <= MAX_CHERRY_MARK;
    let (commits, commits_truncated) = range_commits(dir, &base_info.sha, &head_info.sha, MAX_COMMITS, cherry)?;
    let all_remotes = remotes(dir);
    let base_rb = remote_branch(dir, &base_info, &all_remotes);
    let head_rb = remote_branch(dir, &head_info, &all_remotes);
    let url_remote = base_rb.as_ref().map(|(r, _)| r.clone()).or_else(|| main_remote(dir));
    let remote_url = url_remote.as_deref().and_then(|r| git::run(dir, &["remote", "get-url", r]).ok()).map(|u| super::strip_credentials(u.trim()));
    Ok(Comparison {
        merge_base: bases.first().cloned(),
        merge_bases: bases.len(),
        unrelated,
        mode,
        from,
        to,
        head_is_current,
        uncommitted,
        ahead,
        behind,
        files,
        commits,
        commits_truncated,
        fetched_secs_ago: base_rb.as_ref().and_then(|_| last_fetch_age(dir)),
        base_remote: base_rb.as_ref().map(|(r, _)| r.clone()),
        head_pushed: head_rb.is_some(),
        base_remote_branch: base_rb.map(|(_, b)| b).or_else(|| (base_info.kind == RefKind::Branch).then(|| base_info.name.clone())),
        head_remote_branch: head_rb.map(|(_, b)| b).or_else(|| (head_info.kind == RefKind::Branch).then(|| head_info.name.clone())),
        remote_url,
        base: base_info,
        head: head_info,
    })
}

/// Would merging `head` into `base` conflict? (`git merge-tree --write-tree`, Git ≥ 2.38.)
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictPrediction {
    /// This git can tell (≥ 2.38).
    pub supported: bool,
    pub clean: bool,
    /// Paths that would conflict.
    pub files: Vec<String>,
    /// Why it couldn't tell (old git, unrelated histories…).
    pub message: Option<String>,
    pub git_version: String,
}

/// Parses `git merge-tree --write-tree --name-only --no-messages -z` output: the tree id, then
/// conflicted paths.
pub fn parse_merge_tree(out: &str) -> Vec<String> {
    let mut files: Vec<String> = out.split('\0').skip(1).map(|s| s.trim_end_matches('\n')).filter(|s| !s.is_empty()).map(str::to_string).collect();
    files.dedup();
    files
}

pub fn predict_conflicts(dir: &Path, base: &str, head: &str) -> GitResult<ConflictPrediction> {
    let git_version = git::version_text();
    if !git::version_at_least(2, 38) {
        return Ok(ConflictPrediction { supported: false, clean: false, files: vec![], message: Some(format!("Conflict prediction needs Git 2.38 or newer (this is {git_version}).")), git_version });
    }
    let b = resolve(dir, base)?;
    let h = resolve(dir, head)?;
    let out = git::output(dir, &["merge-tree", "--write-tree", "--name-only", "--no-messages", "-z", &b, &h])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    match out.status.code() {
        Some(0) => Ok(ConflictPrediction { supported: true, clean: true, files: vec![], message: None, git_version }),
        Some(1) => Ok(ConflictPrediction { supported: true, clean: false, files: parse_merge_tree(&stdout), message: None, git_version }),
        _ => Ok(ConflictPrediction { supported: true, clean: false, files: vec![], message: Some(String::from_utf8_lossy(&out.stderr).trim().to_string()), git_version }),
    }
}

// ---------- file contents for a diff ----------

/// One side of a file diff. Never a path to run git in: paths are inside the repository.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Side {
    /// `<rev>:<path>`.
    Rev { rev: String, path: String },
    /// A blob by id (e.g. the version that was reviewed).
    Blob { id: String },
    /// The file in the working tree.
    Worktree { path: String },
    /// The staged version.
    Index { path: String },
    /// Nothing (added/deleted files).
    Empty,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Content {
    pub bytes: Vec<u8>,
    pub size: u64,
    /// The side doesn't exist (no such path at that revision).
    pub missing: bool,
    /// Bigger than the cap: `bytes` is empty.
    pub too_large: bool,
}

fn check_path(p: &str) -> GitResult<()> {
    if p.is_empty() || p.starts_with('-') || p.contains('\0') || p.split(['/', '\\']).any(|s| s == "..") { Err(format!("invalid path {p}")) } else { Ok(()) }
}

fn object(dir: &Path, spec: &str, max: u64) -> GitResult<Content> {
    let size = match git::run(dir, &["cat-file", "-s", spec]) {
        Ok(s) => s.trim().parse::<u64>().unwrap_or(0),
        Err(_) => return Ok(Content { missing: true, ..Default::default() }),
    };
    if size > max {
        return Ok(Content { size, too_large: true, ..Default::default() });
    }
    let bytes = git::run_bytes(dir, &["cat-file", "blob", spec])?;
    Ok(Content { size, bytes, ..Default::default() })
}

pub fn read_side(dir: &Path, side: &Side, max: u64) -> GitResult<Content> {
    match side {
        Side::Empty => Ok(Content { missing: true, ..Default::default() }),
        Side::Rev { rev, path } => {
            if !valid_oid(rev) {
                check_rev(rev)?;
            }
            check_path(path)?;
            object(dir, &format!("{rev}:{path}"), max)
        }
        Side::Blob { id } => {
            if !valid_oid(id) {
                return Err(format!("{id} isn't an object id"));
            }
            object(dir, id, max)
        }
        Side::Index { path } => {
            check_path(path)?;
            object(dir, &format!(":{path}"), max)
        }
        Side::Worktree { path } => {
            check_path(path)?;
            let abs = crate::fsutil::safe_join(dir, path).map_err(|e| e.to_string())?;
            let Ok(meta) = std::fs::metadata(&abs) else { return Ok(Content { missing: true, ..Default::default() }) };
            if meta.len() > max {
                return Ok(Content { size: meta.len(), too_large: true, ..Default::default() });
            }
            Ok(Content { size: meta.len(), bytes: std::fs::read(&abs).map_err(|e| e.to_string())?, ..Default::default() })
        }
    }
}

/// `image/png` for image paths a webview can show.
pub fn image_mime(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// Contents for a side-by-side view of one file.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub original: String,
    pub modified: String,
    pub binary: bool,
    /// Data URLs for image files (side by side).
    pub image: Option<ImagePair>,
    /// A side is over the size cap (nothing loaded).
    pub too_large: bool,
    pub old_size: u64,
    pub new_size: u64,
    pub old_missing: bool,
    pub new_missing: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePair {
    pub old: Option<String>,
    pub new: Option<String>,
}

/// Standard base64 (for image data URLs).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Reads both sides of a file diff. `path` (the new path) decides whether it is shown as an
/// image; `force` lifts the text cap to [`MAX_TEXT_BYTES`] × 4 ("load anyway").
pub fn file_diff(dir: &Path, old: &Side, new: &Side, path: &str, force: bool) -> GitResult<FileDiff> {
    if let Some(mime) = image_mime(path) {
        let o = read_side(dir, old, MAX_IMAGE_BYTES)?;
        let n = read_side(dir, new, MAX_IMAGE_BYTES)?;
        let url = |c: &Content| (!c.missing && !c.too_large).then(|| format!("data:{mime};base64,{}", base64(&c.bytes)));
        return Ok(FileDiff {
            binary: true,
            image: Some(ImagePair { old: url(&o), new: url(&n) }),
            too_large: o.too_large || n.too_large,
            old_size: o.size,
            new_size: n.size,
            old_missing: o.missing,
            new_missing: n.missing,
            ..Default::default()
        });
    }
    let max = if force { MAX_TEXT_BYTES * 4 } else { MAX_TEXT_BYTES };
    let o = read_side(dir, old, max)?;
    let n = read_side(dir, new, max)?;
    let binary = is_binary(&o.bytes) || is_binary(&n.bytes);
    let text = |c: &Content| if binary { String::new() } else { String::from_utf8_lossy(&c.bytes).into_owned() };
    Ok(FileDiff {
        original: text(&o),
        modified: text(&n),
        binary,
        image: None,
        too_large: o.too_large || n.too_large,
        old_size: o.size,
        new_size: n.size,
        old_missing: o.missing,
        new_missing: n.missing,
    })
}

/// A commit's header for the details panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub date: String,
    pub committer: String,
    pub committer_email: String,
    pub committer_date: String,
    pub subject: String,
    pub body: String,
    /// Decorations (`HEAD -> main`, `origin/main`, `tag: v1`).
    pub refs: Vec<String>,
}

pub fn commit_info(dir: &Path, rev: &str) -> GitResult<CommitInfo> {
    let sha = resolve(dir, rev)?;
    let out = git::run(dir, &["show", "-s", "--no-show-signature", "--format=%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%ce%x1f%cI%x1f%s%x1f%b%x1f%D", &sha, "--"])?;
    let f: Vec<&str> = out.splitn(12, '\x1f').collect();
    if f.len() < 12 {
        return Err(format!("couldn't read commit {sha}"));
    }
    Ok(CommitInfo {
        hash: f[0].into(),
        short: f[1].into(),
        parents: f[2].split_whitespace().map(str::to_string).collect(),
        author: f[3].into(),
        email: f[4].into(),
        date: f[5].into(),
        committer: f[6].into(),
        committer_email: f[7].into(),
        committer_date: f[8].into(),
        subject: f[9].into(),
        body: f[10].trim_end().into(),
        refs: f[11].trim().split(", ").filter(|s| !s.is_empty()).map(str::to_string).collect(),
    })
}

/// Files of a commit range as the Commits tab narrows it: from the oldest commit's parent (or
/// the empty tree) to the newest commit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeFiles {
    pub from: String,
    pub to: String,
    pub files: Vec<ChangedFile>,
}

pub fn commit_range(dir: &Path, oldest: &str, newest: &str) -> GitResult<RangeFiles> {
    let from = commit_base(dir, oldest)?;
    let to = resolve(dir, newest)?;
    Ok(RangeFiles { files: range_files(dir, &from, Some(&to), false)?, from, to })
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
