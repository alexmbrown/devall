//! Source control for the project's repositories — one (the project's own repo) or many (a
//! multi-repo project: `repos:` in `devall.yaml`, see `devall_core::repos`). Uses the user's own
//! `git` (their hooks, credentials and config apply — including `devall scan --staged` as a
//! pre-commit hook, installed per repo).
//!
//! Every command names the repository it acts on (`repo`, a name from `git_repos`; `None` = the
//! project's default repo). The folder git runs in is always resolved here from the project's
//! repo list — the renderer never passes a path to run git in. Clone, pull and branch switches
//! only run on explicit user actions (the UI confirms).
//!
//! Registration: `pub mod git;`, `.manage(git::GitWatchState::default())`, the `git_*` and
//! `repos_*` commands. Emits `git-changed` (`{ root, repos }`) when a watched repo's `HEAD`,
//! `index` or fetch/merge state changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use devall_core::Project;
use devall_core::repos::git::{self, Branches, Commit, GitFile, Summary};
use devall_core::repos::{self, Repo, RepoState, RepoSuggestion};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> R<T> + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

fn open(root: &str) -> R<Project> {
    Project::open(root).map_err(err)
}

/// The work tree a command acts on, from the project's own repo list. `None` = the default repo
/// (the one containing the project folder, else the first present one).
fn repo_of(root: &str, repo: Option<&str>) -> R<Repo> {
    let p = open(root)?;
    let r = match repo.filter(|r| !r.is_empty()) {
        Some(name) => p.repo(name).ok_or_else(|| format!("{name} isn't a repository of this project"))?,
        None => repos::default_repo(&p.repos()).cloned().ok_or("this project isn't in a git repository")?,
    };
    match r.state {
        RepoState::Found => Ok(r),
        RepoState::Missing => Err(format!("{} is missing on this machine ({})", r.name, r.path.display())),
        RepoState::NotARepo => Err(format!("{} isn't a git repository ({})", r.name, r.path.display())),
    }
}

/// `repo_of` + run `f` with the work tree on a worker thread.
async fn in_repo<T: Send + 'static>(root: String, repo: Option<String>, f: impl FnOnce(&Path) -> R<T> + Send + 'static) -> R<T> {
    blocking(move || {
        let r = repo_of(&root, repo.as_deref())?;
        f(&r.path)
    })
    .await
}

fn with_paths<'a>(head: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    let mut args = head.to_vec();
    args.push("--");
    args.extend(paths.iter().map(String::as_str));
    args
}

// ---------- repositories ----------

/// The project's repositories on this machine (listed, containing, discovered), without git.
#[tauri::command]
pub async fn git_repos(root: String) -> R<Vec<Repo>> {
    blocking(move || Ok(open(&root)?.repos())).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoOverview {
    repo: Repo,
    /// `None` for repos that aren't on this machine.
    summary: Option<Summary>,
}

/// Every repository (or only `repos`) with its branch, upstream state, change count and last
/// commit — statuses run concurrently (bounded), so ten repos stay quick.
#[tauri::command]
pub async fn git_overview(root: String, repos: Option<Vec<String>>) -> R<Vec<RepoOverview>> {
    blocking(move || {
        let mut list = open(&root)?.repos();
        if let Some(only) = repos {
            list.retain(|r| only.contains(&r.name));
        }
        Ok(git::parallel(list, git::PARALLEL, |repo| {
            let summary = repo.is_found().then(|| git::summary(&repo.path));
            RepoOverview { repo, summary }
        }))
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    is_repo: bool,
    /// Name of the repository (see `git_repos`).
    repo: Option<String>,
    repo_root: Option<String>,
    /// Project folder relative to the repo root ('' when they're the same; `None` when the
    /// project isn't inside this repo).
    project_prefix: Option<String>,
    branch: Option<String>,
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    files: Vec<GitFile>,
    has_scan_hook: bool,
}

#[tauri::command]
pub async fn git_status(root: String, repo: Option<String>) -> R<GitStatus> {
    blocking(move || {
        let p = open(&root)?;
        if repo.as_deref().is_none_or(str::is_empty) && repos::default_repo(&p.repos()).is_none() {
            return Ok(GitStatus {
                is_repo: false,
                repo: None,
                repo_root: None,
                project_prefix: None,
                branch: None,
                upstream: None,
                ahead: 0,
                behind: 0,
                files: vec![],
                has_scan_hook: false,
            });
        }
        let r = repo_of(&root, repo.as_deref())?;
        let s = git::status(&r.path, true)?;
        Ok(GitStatus {
            is_repo: true,
            has_scan_hook: git::has_scan_hook(&r.path),
            repo: Some(r.name),
            repo_root: Some(r.path.display().to_string()),
            project_prefix: r.project_prefix,
            branch: s.branch,
            upstream: s.upstream,
            ahead: s.ahead,
            behind: s.behind,
            files: s.files,
        })
    })
    .await
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
pub async fn git_diff(root: String, repo: Option<String>, path: String, staged: bool) -> R<GitDiff> {
    in_repo(root, repo, move |top| {
        let show = |spec: &str| git::run(top, &["show", spec]).unwrap_or_default();
        let (original, modified) = if staged {
            (show(&format!("HEAD:{path}")), show(&format!(":{path}")))
        } else {
            let index = git::run(top, &["show", &format!(":{path}")]).or_else(|_| git::run(top, &["show", &format!("HEAD:{path}")])).unwrap_or_default();
            let file = devall_core::fsutil::safe_join(top, &path).map_err(err)?;
            let bytes = std::fs::read(file).unwrap_or_default();
            (index, String::from_utf8_lossy(&bytes).into_owned())
        };
        let binary = original.contains('\0') || modified.contains('\0');
        Ok(GitDiff { original, modified, binary })
    })
    .await
}

#[tauri::command]
pub async fn git_stage(root: String, repo: Option<String>, paths: Vec<String>) -> R<()> {
    in_repo(root, repo, move |top| git::run(top, &with_paths(&["add"], &paths)).map(|_| ())).await
}

#[tauri::command]
pub async fn git_unstage(root: String, repo: Option<String>, paths: Vec<String>) -> R<()> {
    in_repo(root, repo, move |top| git::run(top, &with_paths(&["restore", "--staged"], &paths)).map(|_| ())).await
}

/// Throws away working-tree changes (tracked files) — the UI confirms first.
#[tauri::command]
pub async fn git_discard(root: String, repo: Option<String>, paths: Vec<String>) -> R<()> {
    in_repo(root, repo, move |top| git::run(top, &with_paths(&["restore"], &paths)).map(|_| ())).await
}

#[tauri::command]
pub async fn git_commit(root: String, repo: Option<String>, message: String, amend: bool) -> R<String> {
    if message.trim().is_empty() && !amend {
        return Err("commit message is empty".into());
    }
    in_repo(root, repo, move |top| {
        let mut args = vec!["commit", "-m", message.as_str()];
        if amend {
            args.push("--amend");
        }
        git::run(top, &args)
    })
    .await
}

#[tauri::command]
pub async fn git_log(root: String, repo: Option<String>, path: Option<String>, limit: Option<usize>) -> R<Vec<Commit>> {
    in_repo(root, repo, move |top| Ok(git::log(top, path.as_deref(), limit.unwrap_or(50)))).await
}

fn sync_args(action: &str) -> R<&'static [&'static str]> {
    Ok(match action {
        "fetch" => &["fetch", "--prune"],
        "pull" => &["pull", "--ff-only"],
        "push" => &["push"],
        _ => return Err(format!("unknown action {action}")),
    })
}

#[tauri::command]
pub async fn git_sync(root: String, repo: Option<String>, action: String) -> R<String> {
    let args = sync_args(&action)?;
    in_repo(root, repo, move |top| git::run(top, args)).await
}

#[tauri::command]
pub async fn git_branches(root: String, repo: Option<String>) -> R<Vec<String>> {
    in_repo(root, repo, |top| git::branches(top).map(|b| b.local)).await
}

#[tauri::command]
pub async fn git_checkout(root: String, repo: Option<String>, branch: String, create: bool) -> R<()> {
    if !git::valid_branch(&branch) {
        return Err(format!("“{branch}” isn't a valid branch name"));
    }
    in_repo(root, repo, move |top| git::run(top, &if create { vec!["switch", "-c", &branch] } else { vec!["switch", &branch] }).map(|_| ())).await
}

/// `git init` in the project folder (no repo yet) or in a listed folder that isn't a repo.
#[tauri::command]
pub async fn git_init(root: String, repo: Option<String>) -> R<()> {
    blocking(move || {
        let p = open(&root)?;
        let dir = match repo.filter(|r| !r.is_empty()) {
            Some(name) => {
                let r = p.repo(&name).ok_or_else(|| format!("{name} isn't a repository of this project"))?;
                if r.state != RepoState::NotARepo {
                    return Err(format!("{name} is already a repository or is missing"));
                }
                r.path
            }
            None => p.root().to_path_buf(),
        };
        git::run(&dir, &["init"]).map(|_| ())
    })
    .await
}

/// Installs the `devall scan --staged` pre-commit hook in one repo (refuses to overwrite a
/// foreign hook).
#[tauri::command]
pub async fn git_install_scan_hook(root: String, repo: Option<String>) -> R<String> {
    in_repo(root, repo, |top| git::install_scan_hook(top).map(|h| h.display().to_string())).await
}

// ---------- across repositories ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkResult {
    repo: String,
    ok: bool,
    /// git's output (last lines) or error.
    message: String,
}

/// Runs `f` in each named repo (concurrently, bounded); per-repo failures are results, not errors.
async fn each_repo(root: String, names: Vec<String>, f: impl Fn(&Path) -> R<String> + Send + Sync + 'static) -> R<Vec<BulkResult>> {
    blocking(move || {
        let p = open(&root)?;
        let all = p.repos();
        let jobs: Vec<(String, Option<Repo>)> = names.into_iter().map(|n| (n.clone(), repos::find(&all, &n).cloned())).collect();
        Ok(git::parallel(jobs, git::PARALLEL, |(name, repo)| {
            let res = match repo {
                None => Err(format!("{name} isn't a repository of this project")),
                Some(r) if r.state != RepoState::Found => Err(format!("missing on this machine ({})", r.path.display())),
                Some(r) => f(&r.path),
            };
            match res {
                Ok(message) => BulkResult { repo: name, ok: true, message },
                Err(message) => BulkResult { repo: name, ok: false, message },
            }
        }))
    })
    .await
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.trim().lines().collect();
    lines[lines.len().saturating_sub(3)..].join("\n")
}

/// Fetch all / Pull all (`fetch --prune`, `pull --ff-only`) over the named repos.
#[tauri::command]
pub async fn git_bulk(root: String, repos: Vec<String>, action: String) -> R<Vec<BulkResult>> {
    let args = match action.as_str() {
        "fetch" | "pull" => sync_args(&action)?,
        other => return Err(format!("{other} isn't a bulk action")),
    };
    each_repo(root, repos, move |dir| git::run(dir, args).map(|o| tail(&o))).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoBranches {
    repo: String,
    branches: Option<Branches>,
    error: Option<String>,
}

/// Local and remote branches of each named repo (for "Switch branch…" across repos).
#[tauri::command]
pub async fn git_branches_all(root: String, repos: Vec<String>) -> R<Vec<RepoBranches>> {
    blocking(move || {
        let p = open(&root)?;
        let all = p.repos();
        let found: Vec<(String, Option<Repo>)> = repos.into_iter().map(|n| (n.clone(), repos::find(&all, &n).cloned().filter(Repo::is_found))).collect();
        Ok(git::parallel(found, git::PARALLEL, |(repo, r)| match r.map(|r| git::branches(&r.path)) {
            Some(Ok(b)) => RepoBranches { repo, branches: Some(b), error: None },
            Some(Err(e)) => RepoBranches { repo, branches: None, error: Some(e) },
            None => RepoBranches { repo, branches: None, error: Some("not on this machine".into()) },
        }))
    })
    .await
}

/// Switches each named repo to `branch` — an existing local branch, else the remote one
/// (tracked), else (`create`) a new branch from its current HEAD. Git refuses where uncommitted
/// changes would be overwritten; nothing is stashed or discarded.
#[tauri::command]
pub async fn git_switch_all(root: String, repos: Vec<String>, branch: String, create: bool) -> R<Vec<BulkResult>> {
    if !git::valid_branch(&branch) {
        return Err(format!("“{branch}” isn't a valid branch name"));
    }
    each_repo(root, repos, move |dir| {
        git::switch(dir, &branch, create).map(|s| {
            match s {
                git::Switched::Already => "already on it",
                git::Switched::Local => "switched",
                git::Switched::Tracking => "switched (tracking the remote branch)",
                git::Switched::Created => "created",
            }
            .to_string()
        })
    })
    .await
}

// ---------- watching HEAD / index ----------

/// Files in a git dir whose change means the status shown may be stale.
const WATCHED: &[&str] = &["HEAD", "index", "FETCH_HEAD", "ORIG_HEAD", "MERGE_HEAD", "packed-refs"];

/// The running watch: which project, which git dirs (→ repo names), and the watcher itself.
struct Watching {
    root: String,
    targets: Vec<(PathBuf, String)>,
    _watcher: RecommendedWatcher,
}

#[derive(Default)]
pub struct GitWatchState {
    current: Mutex<Option<Watching>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitChanged {
    root: String,
    repos: Vec<String>,
}

/// Watches each present repo's git dir (not the working tree, not recursively) so commits,
/// checkouts, fetches and staging done elsewhere refresh the status. Replaces the previous watch.
#[tauri::command]
pub async fn git_watch(app: AppHandle, state: State<'_, GitWatchState>, root: String) -> R<()> {
    let targets: Vec<(PathBuf, String)> = {
        let root = root.clone();
        blocking(move || Ok(open(&root)?.repos().into_iter().filter(Repo::is_found).filter_map(|r| git::git_dir(&r.path).map(|d| (d, r.name))).collect())).await?
    };
    let mut cur = state.current.lock().unwrap();
    if cur.as_ref().is_some_and(|w| w.root == root && w.targets == targets) {
        return Ok(());
    }
    *cur = None;
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx).map_err(err)?;
    for (dir, _) in &targets {
        let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
    }
    let map = targets.clone();
    let event_root = root.clone();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            let deadline = std::time::Instant::now() + Duration::from_millis(400);
            while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
                match rx.recv_timeout(left) {
                    Ok(ev) => batch.push(ev),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            let mut names: Vec<String> = batch
                .into_iter()
                .flatten()
                .filter(|e| !matches!(e.kind, notify::EventKind::Access(_)))
                .flat_map(|e| e.paths)
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| WATCHED.contains(&n)))
                .filter_map(|p| map.iter().find(|(d, _)| p.parent() == Some(d.as_path())).map(|(_, n)| n.clone()))
                .collect();
            names.sort();
            names.dedup();
            if !names.is_empty() {
                let _ = app.emit("git-changed", GitChanged { root: event_root.clone(), repos: names });
            }
        }
    });
    *cur = Some(Watching { root, targets, _watcher: watcher });
    Ok(())
}

#[tauri::command]
pub fn git_unwatch(state: State<'_, GitWatchState>) {
    *state.current.lock().unwrap() = None;
}

// ---------- managing the list (devall.yaml + this machine's locations) ----------

/// What adding `folder` (from a folder picker) would store: name, relative path, far away?
#[tauri::command]
pub async fn repos_suggest(root: String, folder: String) -> R<RepoSuggestion> {
    blocking(move || Ok(repos::suggest(&open(&root)?, Path::new(&folder)))).await
}

/// Adds a folder to `devall.yaml` (relative path), or — `localOnly` / another drive — keeps its
/// location on this machine and shares only a sibling-path guess.
#[tauri::command]
pub async fn repos_add(root: String, folder: String, name: Option<String>, group: Option<String>, local_only: bool) -> R<devall_core::RepoRef> {
    blocking(move || repos::add(&mut open(&root)?, Path::new(&folder), name.as_deref(), group.as_deref(), local_only).map_err(err)).await
}

/// Renames a listed repo and/or sets its group (`""` clears it).
#[tauri::command]
pub async fn repos_update(root: String, name: String, new_name: Option<String>, group: Option<String>) -> R<()> {
    blocking(move || repos::update(&mut open(&root)?, &name, new_name.as_deref(), group.as_deref()).map_err(err)).await
}

/// Removes a repo from the project (its files stay where they are).
#[tauri::command]
pub async fn repos_remove(root: String, name: String) -> R<()> {
    blocking(move || repos::remove(&mut open(&root)?, &name).map_err(err)).await
}

/// "Locate…": where a listed repo is on this machine (written to the local `repos.json`).
#[tauri::command]
pub async fn repos_locate(root: String, name: String, folder: String) -> R<()> {
    blocking(move || repos::locate(&open(&root)?, &name, Path::new(&folder)).map_err(err)).await
}

/// "Save to devall.yaml" for discovered repos; `groups` maps names to groups.
#[tauri::command]
pub async fn repos_save_discovered(root: String, names: Vec<String>, groups: Option<BTreeMap<String, String>>) -> R<Vec<devall_core::RepoRef>> {
    blocking(move || repos::save_discovered(&mut open(&root)?, &names, &groups.unwrap_or_default()).map_err(err)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    /// The folder already has a `devall.yaml`.
    is_project: bool,
    repos: Vec<Repo>,
}

/// What's in a folder picked to open or create a project: is it one already, and which git
/// repositories are in it (the containing one first, then nested ones) — so a parent folder of
/// several repos can become a multi-repo project.
#[tauri::command]
pub async fn repos_detect(folder: String) -> R<Detected> {
    blocking(move || {
        let dir = Path::new(&folder);
        Ok(Detected { is_project: dir.join(devall_core::project::MANIFEST).is_file(), repos: repos::detect(dir) })
    })
    .await
}

/// Clones a missing listed repo from its `remote` into its path on this machine. Explicit user
/// action (the UI confirms); progress lines stream to `on_progress`.
#[tauri::command]
pub async fn repos_clone(root: String, name: String, on_progress: Channel<String>) -> R<Repo> {
    blocking(move || {
        let p = open(&root)?;
        let r = p.repo(&name).ok_or_else(|| format!("{name} isn't a repository of this project"))?;
        if r.state != RepoState::Missing {
            return Err(format!("{name} is already on this machine ({})", r.path.display()));
        }
        let remote = r.remote.clone().ok_or_else(|| format!("{name} has no remote in devall.yaml"))?;
        git::clone(&remote, &r.path, |line| {
            let _ = on_progress.send(line.to_string());
        })?;
        p.repo(&name).ok_or_else(|| "cloned, but the repository can't be found".into())
    })
    .await
}

/// Absolute path of a `repo:<name>/<path>` link (to reveal it in Explorer/Finder).
#[tauri::command]
pub async fn repos_resolve(root: String, link: String) -> R<String> {
    blocking(move || repos::resolve_link_at(Path::new(&root), &link).map(|p| p.display().to_string()).map_err(err)).await
}

/// Docker Compose files in the project's repositories (the Docker page offers `compose up`).
#[tauri::command]
pub async fn repos_compose_files(root: String) -> R<Vec<repos::ComposeFile>> {
    blocking(move || Ok(repos::compose_files(&open(&root)?.repos()))).await
}

/// How a picked file should be referenced in project files: `repo:<name>/…`, project-relative or
/// absolute.
#[tauri::command]
pub async fn repos_portable(root: String, path: String) -> R<String> {
    blocking(move || Ok(repos::portable_path(&open(&root)?, Path::new(&path)))).await
}
