//! Minimal source control for the project folder, using the user's own `git` (their hooks,
//! credentials and config apply — including `devall scan --staged` as a pre-commit hook).
//!
//! Registration: `pub mod git;` + the `git_*` commands. No managed state.

use std::path::Path;
use std::process::Command;

use serde::Serialize;

type R<T> = Result<T, String>;

fn git(root: &str, args: &[&str]) -> R<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(args).env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash
    }
    let out = cmd.output().map_err(|e| format!("git is not installed or not on PATH ({e})"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
        return Err(msg);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFile {
    /// Relative to the repository root, `/`-separated.
    path: String,
    /// Index (staged) status letter: M, A, D, R, C, U or `.` for none.
    staged: String,
    /// Working-tree status letter, or `?` for untracked.
    unstaged: String,
    /// Original path for renames.
    from: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    is_repo: bool,
    repo_root: Option<String>,
    /// Project folder relative to the repo root ('' when they're the same).
    project_prefix: String,
    branch: Option<String>,
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    files: Vec<GitFile>,
    has_scan_hook: bool,
}

#[tauri::command]
pub fn git_status(root: String) -> R<GitStatus> {
    let top = match git(&root, &["rev-parse", "--show-toplevel"]) {
        Ok(t) => t.trim().to_string(),
        Err(_) => {
            return Ok(GitStatus {
                is_repo: false,
                repo_root: None,
                project_prefix: String::new(),
                branch: None,
                upstream: None,
                ahead: 0,
                behind: 0,
                files: vec![],
                has_scan_hook: false,
            });
        }
    };
    let prefix = git(&root, &["rev-parse", "--show-prefix"]).unwrap_or_default().trim().trim_end_matches('/').to_string();
    let out = git(&root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
    let (mut branch, mut upstream, mut ahead, mut behind) = (None, None, 0, 0);
    let mut files = Vec::new();
    let mut entries = out.split('\0').peekable();
    while let Some(e) = entries.next() {
        if e.is_empty() {
            continue;
        }
        if let Some(h) = e.strip_prefix("# ") {
            if let Some(b) = h.strip_prefix("branch.head ") {
                branch = (b != "(detached)").then(|| b.to_string());
            } else if let Some(u) = h.strip_prefix("branch.upstream ") {
                upstream = Some(u.to_string());
            } else if let Some(ab) = h.strip_prefix("branch.ab ") {
                for part in ab.split_whitespace() {
                    if let Some(a) = part.strip_prefix('+') {
                        ahead = a.parse().unwrap_or(0);
                    } else if let Some(b) = part.strip_prefix('-') {
                        behind = b.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        let kind = &e[..1];
        match kind {
            "1" => {
                // 1 XY sub mH mI mW hH hI path
                let parts: Vec<&str> = e.splitn(9, ' ').collect();
                if parts.len() == 9 {
                    let xy = parts[1];
                    files.push(GitFile { path: parts[8].to_string(), staged: xy[..1].to_string(), unstaged: xy[1..2].to_string(), from: None });
                }
            }
            "2" => {
                // 2 XY sub mH mI mW hH hI Xscore path\0origPath
                let parts: Vec<&str> = e.splitn(10, ' ').collect();
                if parts.len() == 10 {
                    let xy = parts[1];
                    let from = entries.next().map(str::to_string);
                    files.push(GitFile { path: parts[9].to_string(), staged: xy[..1].to_string(), unstaged: xy[1..2].to_string(), from });
                }
            }
            "u" => {
                let parts: Vec<&str> = e.splitn(11, ' ').collect();
                if let Some(p) = parts.last() {
                    files.push(GitFile { path: p.to_string(), staged: "U".into(), unstaged: "U".into(), from: None });
                }
            }
            "?" => files.push(GitFile { path: e[2..].to_string(), staged: ".".into(), unstaged: "?".into(), from: None }),
            _ => {}
        }
    }
    let hooks = git(&root, &["rev-parse", "--git-path", "hooks"]).unwrap_or_default();
    let hook_path = Path::new(&root).join(hooks.trim()).join("pre-commit");
    let hook_path = if Path::new(hooks.trim()).is_absolute() { Path::new(hooks.trim()).join("pre-commit") } else { hook_path };
    let has_scan_hook = std::fs::read_to_string(&hook_path).is_ok_and(|h| h.contains("devall scan"));
    Ok(GitStatus { is_repo: true, repo_root: Some(top), project_prefix: prefix, branch, upstream, ahead, behind, files, has_scan_hook })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiff {
    original: String,
    modified: String,
    binary: bool,
}

/// Contents for a side-by-side diff of one file: staged = HEAD ↔ index, else index ↔ working tree.
#[tauri::command]
pub fn git_diff(root: String, path: String, staged: bool) -> R<GitDiff> {
    let top = git(&root, &["rev-parse", "--show-toplevel"])?.trim().to_string();
    let show = |spec: &str| git(&top, &["show", spec]).unwrap_or_default();
    let (original, modified) = if staged {
        (show(&format!("HEAD:{path}")), show(&format!(":{path}")))
    } else {
        let index = git(&top, &["show", &format!(":{path}")]).or_else(|_| git(&top, &["show", &format!("HEAD:{path}")])).unwrap_or_default();
        let bytes = std::fs::read(Path::new(&top).join(&path)).unwrap_or_default();
        (index, String::from_utf8_lossy(&bytes).into_owned())
    };
    let binary = original.contains('\0') || modified.contains('\0');
    Ok(GitDiff { original, modified, binary })
}

#[tauri::command]
pub fn git_stage(root: String, paths: Vec<String>) -> R<()> {
    let top = git(&root, &["rev-parse", "--show-toplevel"])?.trim().to_string();
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(&top, &args).map(|_| ())
}

#[tauri::command]
pub fn git_unstage(root: String, paths: Vec<String>) -> R<()> {
    let top = git(&root, &["rev-parse", "--show-toplevel"])?.trim().to_string();
    let mut args = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(&top, &args).map(|_| ())
}

/// Throws away working-tree changes (tracked files) — the UI confirms first.
#[tauri::command]
pub fn git_discard(root: String, paths: Vec<String>) -> R<()> {
    let top = git(&root, &["rev-parse", "--show-toplevel"])?.trim().to_string();
    let mut args = vec!["restore", "--"];
    args.extend(paths.iter().map(String::as_str));
    git(&top, &args).map(|_| ())
}

#[tauri::command]
pub fn git_commit(root: String, message: String, amend: bool) -> R<String> {
    if message.trim().is_empty() && !amend {
        return Err("commit message is empty".into());
    }
    let mut args = vec!["commit", "-m", message.as_str()];
    if amend {
        args.push("--amend");
    }
    git(&root, &args)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    hash: String,
    short: String,
    author: String,
    date: String,
    subject: String,
}

#[tauri::command]
pub fn git_log(root: String, path: Option<String>, limit: Option<usize>) -> R<Vec<GitCommit>> {
    let n = format!("-{}", limit.unwrap_or(50).min(500));
    let mut args = vec!["log", n.as_str(), "--pretty=format:%H%x1f%h%x1f%an%x1f%aI%x1f%s%x1e"];
    if let Some(p) = path.as_deref() {
        args.push("--");
        args.push(p);
    }
    let out = git(&root, &args).unwrap_or_default();
    Ok(out
        .split('\x1e')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim().split('\x1f').collect();
            (f.len() == 5).then(|| GitCommit { hash: f[0].into(), short: f[1].into(), author: f[2].into(), date: f[3].into(), subject: f[4].into() })
        })
        .collect())
}

#[tauri::command]
pub async fn git_sync(root: String, action: String) -> R<String> {
    let args: Vec<&str> = match action.as_str() {
        "fetch" => vec!["fetch", "--prune"],
        "pull" => vec!["pull", "--ff-only"],
        "push" => vec!["push"],
        _ => return Err(format!("unknown action {action}")),
    };
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        git(&root, &refs)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn git_branches(root: String) -> R<Vec<String>> {
    Ok(git(&root, &["branch", "--format=%(refname:short)"])?.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
}

#[tauri::command]
pub fn git_checkout(root: String, branch: String, create: bool) -> R<()> {
    if create {
        git(&root, &["switch", "-c", &branch]).map(|_| ())
    } else {
        git(&root, &["switch", &branch]).map(|_| ())
    }
}

#[tauri::command]
pub fn git_init(root: String) -> R<()> {
    git(&root, &["init"]).map(|_| ())
}

/// Installs the `devall scan --staged` pre-commit hook (refuses to overwrite a foreign hook).
#[tauri::command]
pub fn git_install_scan_hook(root: String) -> R<String> {
    let hooks = git(&root, &["rev-parse", "--git-path", "hooks"])?.trim().to_string();
    let dir = if Path::new(&hooks).is_absolute() { Path::new(&hooks).to_path_buf() } else { Path::new(&root).join(&hooks) };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let hook = dir.join("pre-commit");
    if hook.exists() && !std::fs::read_to_string(&hook).unwrap_or_default().contains("devall scan") {
        return Err(format!("{} already exists — add `devall scan --staged` to it yourself", hook.display()));
    }
    std::fs::write(&hook, devall_core::scan::PRE_COMMIT_HOOK).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755));
    }
    Ok(hook.display().to_string())
}
