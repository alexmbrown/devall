//! The user's own `git`, run in one repository at a time (their hooks, credentials and config
//! apply). Shared by the desktop app's source control, `devall repos` and the MCP server; callers
//! resolve the folder from the project's repo list ([`super::resolve`]) — never from user input.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde::Serialize;

/// How many repositories are queried at once (statuses, fetches).
pub const PARALLEL: usize = 6;

pub type GitResult<T> = std::result::Result<T, String>;

/// `git -C <dir>` with the settings every call shares: no credential prompts, reads that don't
/// rewrite the index (which the app watches), and paths printed as they are (no `"\303\251"`
/// quoting) — callers also use `-z` wherever paths come back.
pub(crate) fn command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(["-c", "core.quotePath=false"]).env("GIT_TERMINAL_PROMPT", "0").env("GIT_OPTIONAL_LOCKS", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash
    }
    cmd
}

fn spawn_error(e: std::io::Error) -> String {
    format!("git is not installed or not on PATH ({e})")
}

fn failure(out: &Output) -> String {
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err }
}

/// Runs `git -C <dir> <args>`; the error is git's own message.
pub fn run(dir: &Path, args: &[&str]) -> GitResult<String> {
    run_bytes(dir, args).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// [`run`] with the raw output (file contents, patches that must be applied byte for byte).
pub fn run_bytes(dir: &Path, args: &[&str]) -> GitResult<Vec<u8>> {
    let out = command(dir).args(args).stdin(Stdio::null()).output().map_err(spawn_error)?;
    if !out.status.success() {
        return Err(failure(&out));
    }
    Ok(out.stdout)
}

/// Runs git with `input` on stdin (`apply -`, `hash-object --stdin-paths`, `check-attr --stdin`).
pub fn run_input(dir: &Path, args: &[&str], input: &[u8]) -> GitResult<Vec<u8>> {
    let out = output_with_input(dir, args, input)?;
    if !out.status.success() {
        return Err(failure(&out));
    }
    Ok(out.stdout)
}

/// The whole result, whatever the exit code (`merge-tree` exits 1 for "conflicts").
pub fn output(dir: &Path, args: &[&str]) -> GitResult<Output> {
    command(dir).args(args).stdin(Stdio::null()).output().map_err(spawn_error)
}

fn output_with_input(dir: &Path, args: &[&str], input: &[u8]) -> GitResult<Output> {
    let mut child = command(dir).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(spawn_error)?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let data = input.to_vec();
    // Written from another thread so a large input can't deadlock against a full stdout pipe.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    Ok(out)
}

/// The installed git's version (`git version 2.48.1.windows.1` → `(2, 48, 1)`), read once.
pub fn version() -> Option<(u32, u32, u32)> {
    static V: OnceLock<Option<(u32, u32, u32)>> = OnceLock::new();
    *V.get_or_init(|| {
        let mut cmd = Command::new("git");
        cmd.arg("--version").stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().ok()?;
        parse_version(&String::from_utf8_lossy(&out.stdout))
    })
}

/// `git version 2.39.3 (Apple Git-146)` → `(2, 39, 3)`.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let v = s.split_whitespace().find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = v.split('.').map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

/// The installed git is at least `major.minor`.
pub fn version_at_least(major: u32, minor: u32) -> bool {
    version().is_some_and(|(a, b, _)| (a, b) >= (major, minor))
}

/// `2.48.1` (or `unknown`) for messages.
pub fn version_text() -> String {
    version().map(|(a, b, c)| format!("{a}.{b}.{c}")).unwrap_or_else(|| "unknown".into())
}

// ---------- one writer per repository ----------

fn repo_lock(dir: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_default().clone()
}

/// `index.lock` of a work tree, if one exists, and how old it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexLock {
    pub path: String,
    pub age_secs: u64,
}

/// A lock older than this is treated as left behind by a crashed git (nothing holds one that long).
pub const STALE_LOCK_SECS: u64 = 120;

pub fn index_lock(dir: &Path) -> Option<IndexLock> {
    let p = run(dir, &["rev-parse", "--git-path", "index.lock"]).ok()?.trim().to_string();
    let path = if Path::new(&p).is_absolute() { PathBuf::from(p) } else { dir.join(p) };
    let meta = std::fs::metadata(&path).ok()?;
    let age_secs = meta.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).map_or(0, |d| d.as_secs());
    Some(IndexLock { path: path.display().to_string(), age_secs })
}

/// Runs a mutating git operation: one at a time per repository (two clicks can't race for
/// `index.lock`), after waiting briefly for another git process (an editor, a terminal) to let go
/// of the index. A lock that stays is reported — "stale" when it's older than [`STALE_LOCK_SECS`].
pub fn mutate<T>(dir: &Path, f: impl FnOnce() -> GitResult<T>) -> GitResult<T> {
    let lock = repo_lock(dir);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut waited = Duration::ZERO;
    while let Some(l) = index_lock(dir) {
        if waited >= Duration::from_millis(1500) {
            return Err(if l.age_secs >= STALE_LOCK_SECS {
                format!("{} has been there for {} min — probably left behind by a git that crashed. If no git is running, remove the stale lock and try again.", l.path, l.age_secs / 60)
            } else {
                format!("another git process is using this repository ({} exists); try again in a moment", l.path)
            });
        }
        std::thread::sleep(Duration::from_millis(150));
        waited += Duration::from_millis(150);
    }
    f()
}

/// Removes `index.lock` when it is stale (older than [`STALE_LOCK_SECS`]); `false` when there's none.
pub fn clear_stale_lock(dir: &Path) -> GitResult<bool> {
    let lock = repo_lock(dir);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    match index_lock(dir) {
        None => Ok(false),
        Some(l) if l.age_secs < STALE_LOCK_SECS => Err(format!("{} is only {} s old — a git process may still be running", l.path, l.age_secs)),
        Some(l) => std::fs::remove_file(&l.path).map(|_| true).map_err(|e| format!("{}: {e}", l.path)),
    }
}

/// `git` can be started.
pub fn available() -> bool {
    let mut cmd = Command::new("git");
    cmd.arg("--version").stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.status().is_ok_and(|s| s.success())
}

/// Runs `f` over `items` on up to `max` threads, keeping the input order.
pub fn parallel<T: Send, U: Send>(items: Vec<T>, max: usize, f: impl Fn(T) -> U + Sync) -> Vec<U> {
    let n = items.len();
    let queue = Mutex::new(items.into_iter().enumerate().collect::<VecDeque<_>>());
    let results: Mutex<Vec<Option<U>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..max.clamp(1, n.max(1)) {
            s.spawn(|| {
                loop {
                    let next = queue.lock().unwrap().pop_front();
                    let Some((i, item)) = next else { break };
                    let out = f(item);
                    results.lock().unwrap()[i] = Some(out);
                }
            });
        }
    });
    results.into_inner().unwrap().into_iter().flatten().collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFile {
    /// Relative to the repository root, `/`-separated.
    pub path: String,
    /// Index (staged) status letter: M, A, D, R, C, U or `.` for none.
    pub staged: String,
    /// Working-tree status letter, or `?` for untracked.
    pub unstaged: String,
    /// Original path for renames.
    pub from: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `None` when detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitFile>,
}

impl Status {
    pub fn staged_count(&self) -> usize {
        self.files.iter().filter(|f| f.staged != "." && f.staged != "?").count()
    }
    pub fn conflicts(&self) -> usize {
        self.files.iter().filter(|f| f.staged == "U").count()
    }
}

/// Parses `git status --porcelain=v2 --branch -z`.
pub fn parse_status(out: &str) -> Status {
    let mut s = Status::default();
    let mut entries = out.split('\0');
    while let Some(e) = entries.next() {
        if e.is_empty() {
            continue;
        }
        if let Some(h) = e.strip_prefix("# ") {
            if let Some(b) = h.strip_prefix("branch.head ") {
                s.branch = (b != "(detached)").then(|| b.to_string());
            } else if let Some(u) = h.strip_prefix("branch.upstream ") {
                s.upstream = Some(u.to_string());
            } else if let Some(ab) = h.strip_prefix("branch.ab ") {
                for part in ab.split_whitespace() {
                    if let Some(a) = part.strip_prefix('+') {
                        s.ahead = a.parse().unwrap_or(0);
                    } else if let Some(b) = part.strip_prefix('-') {
                        s.behind = b.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        match &e[..1] {
            "1" => {
                // 1 XY sub mH mI mW hH hI path
                let parts: Vec<&str> = e.splitn(9, ' ').collect();
                if parts.len() == 9 && parts[1].len() == 2 {
                    let xy = parts[1];
                    s.files.push(GitFile { path: parts[8].to_string(), staged: xy[..1].to_string(), unstaged: xy[1..2].to_string(), from: None });
                }
            }
            "2" => {
                // 2 XY sub mH mI mW hH hI Xscore path\0origPath
                let parts: Vec<&str> = e.splitn(10, ' ').collect();
                if parts.len() == 10 && parts[1].len() == 2 {
                    let xy = parts[1];
                    let from = entries.next().map(str::to_string);
                    s.files.push(GitFile { path: parts[9].to_string(), staged: xy[..1].to_string(), unstaged: xy[1..2].to_string(), from });
                }
            }
            "u" => {
                if let Some(p) = e.splitn(11, ' ').last() {
                    s.files.push(GitFile { path: p.to_string(), staged: "U".into(), unstaged: "U".into(), from: None });
                }
            }
            "?" if e.len() > 2 => s.files.push(GitFile { path: e[2..].to_string(), staged: ".".into(), unstaged: "?".into(), from: None }),
            _ => {}
        }
    }
    s
}

/// Branch, upstream, ahead/behind and changed files. `all_untracked` lists every untracked file
/// (the changes view); otherwise untracked folders count once (cheaper, for overviews).
pub fn status(dir: &Path, all_untracked: bool) -> GitResult<Status> {
    let untracked = if all_untracked { "--untracked-files=all" } else { "--untracked-files=normal" };
    run(dir, &["status", "--porcelain=v2", "--branch", "-z", untracked]).map(|o| parse_status(&o))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    /// ISO 8601.
    pub date: String,
    pub subject: String,
}

/// Newest first; `path` limits it to one file or folder.
pub fn log(dir: &Path, path: Option<&str>, limit: usize) -> Vec<Commit> {
    let n = format!("-{}", limit.clamp(1, 500));
    let mut args = vec!["log", n.as_str(), "--pretty=format:%H%x1f%h%x1f%an%x1f%aI%x1f%s%x1e"];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    run(dir, &args)
        .unwrap_or_default()
        .split('\x1e')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim().split('\x1f').collect();
            (f.len() == 5).then(|| Commit { hash: f[0].into(), short: f[1].into(), author: f[2].into(), date: f[3].into(), subject: f[4].into() })
        })
        .collect()
}

/// One line per repository for overviews and `devall repos status`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Changed paths (staged, unstaged and untracked).
    pub changes: usize,
    pub staged: usize,
    pub conflicts: usize,
    pub last_commit: Option<Commit>,
    pub has_scan_hook: bool,
    /// Why the status couldn't be read.
    pub error: Option<String>,
}

pub fn summary(dir: &Path) -> Summary {
    match status(dir, false) {
        Ok(s) => Summary {
            changes: s.files.len(),
            staged: s.staged_count(),
            conflicts: s.conflicts(),
            last_commit: log(dir, None, 1).into_iter().next(),
            has_scan_hook: has_scan_hook(dir),
            branch: s.branch,
            upstream: s.upstream,
            ahead: s.ahead,
            behind: s.behind,
            error: None,
        },
        Err(e) => Summary { error: Some(e), ..Default::default() },
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branches {
    pub current: Option<String>,
    pub local: Vec<String>,
    /// Remote branches without the remote name (`origin/feature/x` → `feature/x`), no `HEAD`.
    pub remote: Vec<String>,
}

pub fn branches(dir: &Path) -> GitResult<Branches> {
    let local: Vec<String> = run(dir, &["branch", "--format=%(refname:short)"])?.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('(')).map(str::to_string).collect();
    let mut remote: Vec<String> = run(dir, &["branch", "-r", "--format=%(refname:short)"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().split_once('/').map(|(_, b)| b.to_string()))
        .filter(|b| b != "HEAD" && !b.is_empty())
        .collect();
    remote.sort();
    remote.dedup();
    let current = run(dir, &["branch", "--show-current"]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Ok(Branches { current, local, remote })
}

/// Valid branch name for `switch`/`switch -c` (git's own check plus no leading dash).
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("@{")
        && !name.chars().any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
}

/// What [`switch`] did in one repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Switched {
    /// Already on the branch.
    Already,
    /// Switched to an existing local branch.
    Local,
    /// Created a local branch tracking the remote one.
    Tracking,
    /// Created a new branch from the current HEAD.
    Created,
}

/// Switches to `branch`: an existing local branch, else the remote branch of that name (tracked),
/// else — with `create` — a new branch from HEAD. Uncommitted changes that conflict make git
/// refuse; nothing is stashed or thrown away.
pub fn switch(dir: &Path, branch: &str, create: bool) -> GitResult<Switched> {
    if !valid_branch(branch) {
        return Err(format!("“{branch}” isn't a valid branch name"));
    }
    let b = branches(dir)?;
    if b.current.as_deref() == Some(branch) {
        return Ok(Switched::Already);
    }
    if b.local.iter().any(|l| l == branch) {
        return run(dir, &["switch", branch]).map(|_| Switched::Local);
    }
    if b.remote.iter().any(|r| r == branch) {
        return run(dir, &["switch", "--track", &format!("origin/{branch}")]).or_else(|_| run(dir, &["switch", branch])).map(|_| Switched::Tracking);
    }
    if create {
        return run(dir, &["switch", "-c", branch]).map(|_| Switched::Created);
    }
    Err(format!("no branch “{branch}” here"))
}

/// The hooks folder git uses for this work tree (respects `core.hooksPath` and worktrees).
pub fn hooks_dir(dir: &Path) -> GitResult<PathBuf> {
    let hooks = run(dir, &["rev-parse", "--git-path", "hooks"])?.trim().to_string();
    Ok(if Path::new(&hooks).is_absolute() { PathBuf::from(hooks) } else { dir.join(hooks) })
}

pub fn has_scan_hook(dir: &Path) -> bool {
    hooks_dir(dir).ok().and_then(|h| std::fs::read_to_string(h.join("pre-commit")).ok()).is_some_and(|h| h.contains("devall scan"))
}

/// Installs the `devall scan --staged` pre-commit hook (refuses to overwrite a foreign hook).
pub fn install_scan_hook(dir: &Path) -> GitResult<PathBuf> {
    let hooks = hooks_dir(dir)?;
    std::fs::create_dir_all(&hooks).map_err(|e| e.to_string())?;
    let hook = hooks.join("pre-commit");
    if hook.exists() && !std::fs::read_to_string(&hook).unwrap_or_default().contains("devall scan") {
        return Err(format!("{} already exists — add `devall scan --staged` to it yourself", hook.display()));
    }
    std::fs::write(&hook, crate::scan::PRE_COMMIT_HOOK).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755));
    }
    Ok(hook)
}

/// The folder holding a work tree's `HEAD` and `index` (`.git/`, or where a `.git` file points).
pub fn git_dir(work_tree: &Path) -> Option<PathBuf> {
    let dot = work_tree.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let p = Path::new(target);
    Some(if p.is_absolute() { p.to_path_buf() } else { super::normalize(&work_tree.join(p)) })
}

/// `git clone --progress <remote> <dest>`; `on_line` gets git's progress lines as they come.
/// `dest` must not exist yet (or be an empty folder).
pub fn clone(remote: &str, dest: &Path, mut on_line: impl FnMut(&str)) -> GitResult<()> {
    if remote.trim().is_empty() || remote.trim_start().starts_with('-') {
        return Err("no clone URL".into());
    }
    if dest.exists() && std::fs::read_dir(dest).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!("{} already exists and isn't empty", dest.display()));
    }
    let parent = dest.parent().ok_or("no parent folder")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let mut child = command(parent)
        .args(["clone", "--progress", "--", remote.trim()])
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git is not installed or not on PATH ({e})"))?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let mut buf = [0u8; 4096];
    let mut line = Vec::new();
    let mut tail: VecDeque<String> = VecDeque::new();
    loop {
        let n = stderr.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            // Progress redraws with `\r`; treat it as a line end.
            if b == b'\r' || b == b'\n' {
                if !line.is_empty() {
                    let s = String::from_utf8_lossy(&line).trim().to_string();
                    on_line(&s);
                    tail.push_back(s);
                    if tail.len() > 6 {
                        tail.pop_front();
                    }
                    line.clear();
                }
            } else {
                line.push(b);
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(tail.into_iter().filter(|l| !l.contains('%')).collect::<Vec<_>>().join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let out = "# branch.oid abc\0# branch.head feature/x\0# branch.upstream origin/feature/x\0# branch.ab +2 -1\0\
            1 .M N... 100644 100644 100644 a b src/app.ts\0\
            1 A. N... 000000 100644 100644 a b new file.txt\0\
            2 R. N... 100644 100644 100644 a b R100 docs/new.md\0docs/old.md\0\
            u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
            ? notes/todo.md\0";
        let s = parse_status(out);
        assert_eq!(s.branch.as_deref(), Some("feature/x"));
        assert_eq!(s.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        let paths: Vec<&str> = s.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/app.ts", "new file.txt", "docs/new.md", "conflict.rs", "notes/todo.md"]);
        assert_eq!(s.files[2].from.as_deref(), Some("docs/old.md"));
        assert_eq!((s.staged_count(), s.conflicts()), (3, 1));
        assert_eq!(parse_status("# branch.head (detached)\0").branch, None);
    }

    #[test]
    fn branch_names() {
        for ok in ["main", "feature/login-v2", "release_1.2", "user/a.b"] {
            assert!(valid_branch(ok), "{ok}");
        }
        for bad in ["", "-x", "a b", "a..b", "x.lock", "a:b", "a~1", "/x", "x/", "a\\b", "@{u}x"] {
            assert!(!valid_branch(bad), "{bad}");
        }
    }

    #[test]
    fn parallel_keeps_order() {
        let out = parallel((0..20).collect(), 4, |i: i32| i * 2);
        assert_eq!(out, (0..20).map(|i| i * 2).collect::<Vec<_>>());
        assert!(parallel(Vec::<i32>::new(), 4, |i| i).is_empty());
    }
}
