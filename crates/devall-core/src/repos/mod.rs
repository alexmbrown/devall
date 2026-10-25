//! The git repositories of a project.
//!
//! A project folder is usually one service's repo — or lives inside one (a monorepo subfolder).
//! A multi-repo project (several frontends and backends sharing requests, environments, notes…)
//! lists its repositories in `devall.yaml`:
//!
//! ```yaml
//! repos:
//!   - name: web
//!     path: frontend/web            # relative to the project folder; may climb out: ../api
//!     remote: git@github.com:acme/web.git
//!     group: Frontend
//! ```
//!
//! Paths in `devall.yaml` are always relative (shared with the team). When a repo lives somewhere
//! else on this machine, its absolute location goes in the project data folder's `repos.json`
//! (`{ "<name>": "<absolute path>" }`, never committed). [`resolve`] combines both with what is on
//! disk: listed repos (found / missing / not a git repo), the repo containing the project folder
//! (the single-repo case), and nested repos found under the project folder that aren't listed
//! yet ("discovered").
//!
//! Files inside a listed repo are referenced portably as `repo:<name>/<path>` (links in notes and
//! to-dos, OpenAPI spec sources, notebook working directories) — see [`parse_link`].

pub mod blame;
pub mod compare;
pub mod conflicts;
pub mod git;
pub mod graph;
pub mod ops;
pub mod patch;
pub mod rebase;
pub mod remotes;
pub mod review;
pub mod stash;
pub mod undo;
pub mod worktree;
#[cfg(test)]
pub(crate) mod testgit;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};
use crate::fsutil::{atomic_write, safe_join};
use crate::model::{ProjectManifest, RepoRef};
use crate::project::{CONTENT_DIRS, Project};

/// Machine-specific repo locations, in the project data folder.
pub const REPOS_FILE: &str = "repos.json";
/// Repositories opened on this machine only (worktrees), in the project data folder.
pub const LOCAL_REPOS_FILE: &str = "local-repos.json";
/// Prefix of repo-qualified paths: `repo:<name>/<path>`.
pub const LINK_PREFIX: &str = "repo:";
/// How deep under the project folder nested repos are looked for (`a/b/c`).
pub const DISCOVERY_DEPTH: usize = 3;
/// Caps that keep discovery cheap in a folder full of code.
const MAX_DISCOVERED: usize = 64;
const MAX_DIRS_VISITED: usize = 4000;
/// Folders never searched for nested repos (build output, dependencies).
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "out", "vendor", "bower_components", "__pycache__", "venv", "Pods"];

/// `{ repo name → absolute path }` from `repos.json`.
pub type Overrides = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepoState {
    /// A git work tree exists at the path.
    Found,
    /// Nothing at the path on this machine (clone or locate it).
    Missing,
    /// The folder exists but isn't a git repository.
    NotARepo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepoOrigin {
    /// Listed in `devall.yaml`.
    Manifest,
    /// The repo the project folder is in (single-repo projects, monorepo subfolders).
    Containing,
    /// A nested repo under the project folder that isn't listed yet.
    Discovered,
    /// Opened on this machine only (a worktree): kept in the project data folder's
    /// `local-repos.json`, never in `devall.yaml`.
    Local,
}

/// A repository as resolved on this machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// Unique in the project (listed names first; others get a free name).
    pub name: String,
    /// Absolute work-tree folder (lexically normalized).
    pub path: PathBuf,
    pub origin: RepoOrigin,
    pub state: RepoState,
    /// The path as written in `devall.yaml` (listed repos) or relative to the project folder
    /// (discovered ones).
    pub rel_path: Option<String>,
    pub remote: Option<String>,
    pub group: Option<String>,
    /// The location comes from this machine's `repos.json`.
    pub overridden: bool,
    /// The project folder relative to this repo when the project lives inside it (`""` = the
    /// repo root itself).
    pub project_prefix: Option<String>,
}

impl Repo {
    pub fn is_found(&self) -> bool {
        self.state == RepoState::Found
    }
}

// ---------- paths ----------

/// Collapses `.` and `..` without touching the disk (the target may not exist).
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn absolute(p: &Path) -> PathBuf {
    normalize(&std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
}

/// Absolute-looking in either platform's syntax (`/x`, `\\server\x`, `C:\x`, `C:/x`).
fn looks_absolute(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/') || s.starts_with('\\') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// A `devall.yaml` path (relative, `/` or `\` separated) resolved against the project folder.
/// Absolute paths (hand-written) are taken as they are.
pub fn resolve_rel(root: &Path, rel: &str) -> PathBuf {
    let rel = rel.trim();
    if looks_absolute(rel) {
        return normalize(Path::new(rel));
    }
    let mut out = root.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".") {
        out.push(part);
    }
    normalize(&out)
}

/// Comparable form of an absolute path (case-insensitive file systems on Windows and macOS).
fn key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(any(windows, target_os = "macos")) { s.to_lowercase() } else { s }
}

fn same(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

/// `inner` relative to `outer` (`/`-separated) when `inner` is `outer` or inside it.
fn rel_inside(outer: &Path, inner: &Path) -> Option<String> {
    let (o, i) = (key(outer), key(inner));
    if o == i {
        return Some(String::new());
    }
    let prefix = format!("{o}/");
    if !i.starts_with(&prefix) {
        return None;
    }
    // Slice the original (not lower-cased) spelling.
    let orig = inner.to_string_lossy().replace('\\', "/");
    Some(orig.get(prefix.len()..).unwrap_or_default().trim_matches('/').to_string())
}

/// A relative path from `from_dir` to `to` (`../api`, `apps/web`, `.`), both absolute; `None`
/// when they don't share a root (another drive).
pub fn relative_path(from_dir: &Path, to: &Path) -> Option<String> {
    let a: Vec<String> = normalize(from_dir).components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    let b: Vec<String> = normalize(to).components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    let eq = |x: &String, y: &String| if cfg!(any(windows, target_os = "macos")) { x.to_lowercase() == y.to_lowercase() } else { x == y };
    if a.is_empty() || b.is_empty() || !eq(&a[0], &b[0]) {
        return None;
    }
    let common = a.iter().zip(&b).take_while(|(x, y)| eq(x, y)).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), a.len() - common).collect();
    parts.extend(b[common..].iter().cloned());
    Some(if parts.is_empty() { ".".into() } else { parts.join("/") })
}

/// A folder is a git work tree when it has `.git` (a folder, or a file for worktrees and
/// submodules).
pub fn is_git_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// The nearest folder at or above `dir` that is a git work tree.
pub fn containing_repo(dir: &Path) -> Option<PathBuf> {
    absolute(dir).ancestors().find(|d| is_git_repo(d)).map(Path::to_path_buf)
}

/// The repository a folder is in, as [`resolve`] lists it for a project there — without needing
/// `devall.yaml`. Git commands fall back to it when the manifest is briefly missing from the work
/// tree (e.g. mid-rebase onto a branch that doesn't have it yet), so the operation can be finished.
pub fn containing(root: &Path) -> Option<Repo> {
    let root = absolute(root);
    let top = containing_repo(&root)?;
    Some(Repo {
        name: dir_name(&top),
        project_prefix: rel_inside(&top, &root),
        remote: origin_url(&top),
        path: top,
        origin: RepoOrigin::Containing,
        state: RepoState::Found,
        rel_path: None,
        group: None,
        overridden: false,
    })
}

/// `remote "origin"` URL from the repo's `.git/config` (no git process; `None` for worktrees).
pub fn origin_url(repo: &Path) -> Option<String> {
    let text = fs::read_to_string(repo.join(".git").join("config")).ok()?;
    let mut in_origin = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line.replace(' ', "") == "[remote\"origin\"]";
        } else if in_origin && let Some((k, v)) = line.split_once('=') && k.trim() == "url" {
            return Some(v.trim().to_string()).filter(|s| !s.is_empty());
        }
    }
    None
}

/// The checked-out branch read from the repo's `HEAD` file (no git process); `None` when detached
/// or unreadable.
pub fn head_branch(repo: &Path) -> Option<String> {
    let head = fs::read_to_string(git::git_dir(repo)?.join("HEAD")).ok()?;
    head.trim().strip_prefix("ref: refs/heads/").map(str::to_string)
}

/// One line per repository for AI context: `- web [Frontend] frontend/web — branch main;
/// remote git@…` (no secrets: remotes are shown without credentials).
pub fn outline(repos: &[Repo]) -> String {
    repos
        .iter()
        .map(|r| {
            let mut line = format!("- {}", r.name);
            if let Some(g) = &r.group {
                line.push_str(&format!(" [{g}]"));
            }
            let place = match (&r.rel_path, r.origin) {
                (_, RepoOrigin::Containing) => match r.project_prefix.as_deref() {
                    Some("") | None => "the project folder's own repo".to_string(),
                    Some(p) => format!("contains the project (at {p})"),
                },
                (Some(rel), _) => rel.clone(),
                (None, _) => String::new(),
            };
            line.push_str(&format!(" {place}"));
            let mut meta = Vec::new();
            match r.state {
                RepoState::Found => meta.push(head_branch(&r.path).map(|b| format!("branch {b}")).unwrap_or_else(|| "detached HEAD".into())),
                RepoState::Missing => meta.push("missing on this machine".into()),
                RepoState::NotARepo => meta.push("not a git repository".into()),
            }
            if r.origin == RepoOrigin::Discovered {
                meta.push("not listed in devall.yaml".into());
            }
            if let Some(remote) = &r.remote {
                meta.push(format!("remote {}", strip_credentials(remote)));
            }
            format!("{line} — {}", meta.join("; "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Moves a file or folder to the OS trash — never a permanent delete. (Tests remove it instead, so
/// they don't fill the real trash.)
pub(crate) fn to_trash(path: &Path) -> std::result::Result<(), String> {
    #[cfg(test)]
    {
        let r = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
        r.map_err(|e| format!("{}: {e}", path.display()))
    }
    #[cfg(not(test))]
    {
        trash::delete(path).map_err(|e| format!("couldn't move {} to the trash: {e}", path.display()))
    }
}

/// `https://user:token@host/x.git` → `https://host/x.git`.
pub fn strip_credentials(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((creds, host)) if !creds.contains('/') => format!("{scheme}://{host}"),
            _ => url.to_string(),
        },
        None => url.to_string(),
    }
}

/// Nested git repos under `root` (not `root` itself), `/`-separated and relative to it, at most
/// [`DISCOVERY_DEPTH`] levels down. Skips hidden folders, dependency/build folders and the
/// /dev/all content folders, and never descends into a repo. `skip_linked`: ignore entries whose
/// `.git` is a file (submodules and worktrees of the repo the project lives in).
pub fn discover(root: &Path, skip_linked: bool) -> Vec<String> {
    fn walk(dir: &Path, rel: &str, depth: usize, skip_linked: bool, out: &mut Vec<String>, visited: &mut usize) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<(String, PathBuf)> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
            .collect();
        entries.sort();
        for (name, path) in entries {
            if out.len() >= MAX_DISCOVERED || *visited >= MAX_DIRS_VISITED {
                return;
            }
            *visited += 1;
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let git = path.join(".git");
            if git.exists() {
                if !(skip_linked && git.is_file()) {
                    out.push(child);
                }
                continue;
            }
            if depth == 1 && CONTENT_DIRS.contains(&name.as_str()) {
                continue;
            }
            if depth < DISCOVERY_DEPTH {
                walk(&path, &child, depth + 1, skip_linked, out, visited);
            }
        }
    }
    let mut out = Vec::new();
    let mut visited = 0;
    walk(root, "", 1, skip_linked, &mut out, &mut visited);
    out
}

// ---------- machine-specific locations ----------

pub fn overrides_path(root: &Path) -> PathBuf {
    crate::local::project_dir(root).join(REPOS_FILE)
}

/// This machine's repo locations (empty when there are none or the file is unreadable).
pub fn read_overrides(root: &Path) -> Overrides {
    fs::read(overrides_path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn write_overrides(root: &Path, o: &Overrides) -> Result<()> {
    let path = overrides_path(root);
    if o.is_empty() {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| Error::Io { path: path.clone(), source: e })?;
        }
        return Ok(());
    }
    let mut json = serde_json::to_vec_pretty(o).map_err(|e| Error::Invalid(e.to_string()))?;
    json.push(b'\n');
    atomic_write(&path, &json)
}

// ---------- repositories opened on this machine only ----------

/// A repository (usually a worktree) shown in this project on this machine only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRepo {
    pub name: String,
    /// Absolute.
    pub path: String,
}

pub fn local_repos_path(root: &Path) -> PathBuf {
    crate::local::project_dir(root).join(LOCAL_REPOS_FILE)
}

pub fn read_local_repos(root: &Path) -> Vec<LocalRepo> {
    fs::read(local_repos_path(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_local_repos(root: &Path, list: &[LocalRepo]) -> Result<()> {
    let path = local_repos_path(root);
    if list.is_empty() {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| Error::Io { path: path.clone(), source: e })?;
        }
        return Ok(());
    }
    let mut json = serde_json::to_vec_pretty(list).map_err(|e| Error::Invalid(e.to_string()))?;
    json.push(b'\n');
    atomic_write(&path, &json)
}

/// Shows the git work tree at `folder` in this project on this machine (not in devall.yaml);
/// returns its name (a free one based on `name` or the folder's name).
pub fn add_local_repo(project: &Project, folder: &Path, name: Option<&str>) -> Result<String> {
    let folder = absolute(folder);
    if !is_git_repo(&folder) {
        return Err(Error::Invalid(format!("{} isn't a git work tree", folder.display())));
    }
    let existing = project.repos();
    if let Some(r) = existing.iter().find(|r| same(&r.path, &folder)) {
        return Ok(r.name.clone());
    }
    let used: HashSet<String> = existing.iter().map(|r| r.name.to_lowercase()).collect();
    let wanted = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| dir_name(&folder));
    validate_name(&wanted)?;
    let name = free_name(&wanted, None, &used);
    let mut list = read_local_repos(project.root());
    list.push(LocalRepo { name: name.clone(), path: folder.display().to_string() });
    write_local_repos(project.root(), &list)?;
    Ok(name)
}

/// Stops showing a repository opened on this machine only (its files stay).
pub fn remove_local_repo(project: &Project, name: &str) -> Result<()> {
    let mut list = read_local_repos(project.root());
    let before = list.len();
    list.retain(|r| r.name != name);
    if list.len() == before {
        return Err(Error::NotFound(format!("repository {name} opened on this machine")));
    }
    write_local_repos(project.root(), &list)
}

/// Appends this machine's local-only repositories to a resolved list (names already taken and
/// folders already listed are skipped).
fn with_local(mut repos: Vec<Repo>, local: &[LocalRepo]) -> Vec<Repo> {
    for l in local {
        let path = absolute(Path::new(&l.path));
        if repos.iter().any(|r| r.name.eq_ignore_ascii_case(&l.name) || same(&r.path, &path)) {
            continue;
        }
        repos.push(Repo {
            name: l.name.clone(),
            state: state_of(&path),
            path,
            origin: RepoOrigin::Local,
            rel_path: None,
            remote: None,
            group: Some("Worktrees".into()),
            overridden: false,
            project_prefix: None,
        });
    }
    repos
}

// ---------- resolution ----------

fn free_name(wanted: &str, alt: Option<&str>, used: &HashSet<String>) -> String {
    let taken = |n: &str| used.contains(&n.to_lowercase());
    if !taken(wanted) {
        return wanted.to_string();
    }
    if let Some(a) = alt.filter(|a| !taken(a)) {
        return a.to_string();
    }
    (2..).map(|i| format!("{wanted}-{i}")).find(|n| !taken(n)).unwrap_or_else(|| wanted.to_string())
}

fn dir_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "repo".into())
}

fn state_of(path: &Path) -> RepoState {
    if !path.is_dir() {
        RepoState::Missing
    } else if is_git_repo(path) {
        RepoState::Found
    } else {
        RepoState::NotARepo
    }
}

/// The project's repositories on this machine: the repo containing the project folder (if any
/// and not listed), the listed ones in `devall.yaml` order, then (with `discover`) unlisted
/// nested repos. Never runs git; only looks at the file system.
pub fn resolve(root: &Path, manifest: &ProjectManifest, overrides: &Overrides, discover_nested: bool) -> Vec<Repo> {
    let root = absolute(root);
    let mut used: HashSet<String> = HashSet::new();
    let mut listed = Vec::new();
    for r in &manifest.repos {
        let name = r.name.trim();
        if name.is_empty() || used.contains(&name.to_lowercase()) {
            continue;
        }
        used.insert(name.to_lowercase());
        let (path, overridden) = match overrides.get(name).filter(|p| !p.trim().is_empty()) {
            Some(abs) => (absolute(Path::new(abs.trim())), true),
            None => (resolve_rel(&root, &r.path), false),
        };
        listed.push(Repo {
            name: name.to_string(),
            state: state_of(&path),
            project_prefix: rel_inside(&path, &root),
            path,
            origin: RepoOrigin::Manifest,
            rel_path: Some(r.path.clone()),
            remote: r.remote.clone(),
            group: r.group.clone().filter(|g| !g.trim().is_empty()),
            overridden,
        });
    }

    let mut out = Vec::new();
    let containing = containing_repo(&root);
    if let Some(top) = &containing
        && !listed.iter().any(|r| same(&r.path, top))
    {
        let name = free_name(&dir_name(top), None, &used);
        used.insert(name.to_lowercase());
        out.push(Repo {
            name,
            project_prefix: rel_inside(top, &root),
            remote: origin_url(top),
            path: top.clone(),
            origin: RepoOrigin::Containing,
            state: RepoState::Found,
            rel_path: None,
            group: None,
            overridden: false,
        });
    }
    out.extend(listed);

    if discover_nested {
        for rel in discover(&root, containing.is_some()) {
            let path = resolve_rel(&root, &rel);
            if out.iter().any(|r| same(&r.path, &path)) {
                continue;
            }
            let last = dir_name(&path);
            let alt = rel.rsplit('/').nth(1).map(|parent| format!("{parent}-{last}"));
            let name = free_name(&last, alt.as_deref(), &used);
            used.insert(name.to_lowercase());
            out.push(Repo {
                name,
                remote: origin_url(&path),
                path,
                origin: RepoOrigin::Discovered,
                state: RepoState::Found,
                rel_path: Some(rel),
                group: None,
                overridden: false,
                project_prefix: None,
            });
        }
    }
    out
}

/// The repo commands act on when none is named: the one containing the project folder, else
/// the first one present on this machine.
pub fn default_repo(repos: &[Repo]) -> Option<&Repo> {
    repos.iter().find(|r| r.origin == RepoOrigin::Containing).or_else(|| repos.iter().find(|r| r.is_found()))
}

pub fn find<'a>(repos: &'a [Repo], name: &str) -> Option<&'a Repo> {
    repos.iter().find(|r| r.name == name).or_else(|| repos.iter().find(|r| r.name.eq_ignore_ascii_case(name)))
}

impl Project {
    /// Every repository of the project on this machine (see [`resolve`]).
    pub fn repos(&self) -> Vec<Repo> {
        with_local(resolve(self.root(), &self.manifest, &read_overrides(self.root()), true), &read_local_repos(self.root()))
    }

    /// One repository by name; only searches the folder for unlisted repos when needed.
    pub fn repo(&self, name: &str) -> Option<Repo> {
        let overrides = read_overrides(self.root());
        let quick = resolve(self.root(), &self.manifest, &overrides, false);
        if let Some(r) = find(&quick, name) {
            return Some(r.clone());
        }
        find(&self.repos(), name).cloned()
    }
}

// ---------- repo-qualified paths ----------

/// `repo:web/src/app.ts` → `("web", "src/app.ts")`; `repo:web` → `("web", "")`.
pub fn parse_link(s: &str) -> Option<(&str, &str)> {
    let rest = s.trim().strip_prefix(LINK_PREFIX)?;
    let rest = rest.trim_start_matches(['/', '\\']);
    let (name, path) = match rest.find(['/', '\\']) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    (!name.is_empty()).then_some((name, path))
}

/// The file or folder a `repo:<name>/<path>` link points at (the path can't climb out of the
/// repo). Errors when the repo is unknown or missing on this machine.
pub fn resolve_link(project: &Project, link: &str) -> Result<PathBuf> {
    let (name, rel) = parse_link(link).ok_or_else(|| Error::InvalidPath(link.to_string()))?;
    let repo = project.repo(name).ok_or_else(|| Error::NotFound(format!("repository {name}")))?;
    if repo.state == RepoState::Missing {
        return Err(Error::NotFound(format!("repository {name} on this machine ({})", repo.path.display())));
    }
    safe_join(&repo.path, rel)
}

/// [`resolve_link`] for callers that only have the project folder.
pub fn resolve_link_at(root: &Path, link: &str) -> Result<PathBuf> {
    resolve_link(&Project::open(root)?, link)
}

/// `repo:<name>/<path>` for a file inside a listed repo (the innermost one), so references stay
/// valid when teammates keep the repo elsewhere. `None` for anything else.
pub fn link_for(repos: &[Repo], abs: &Path) -> Option<String> {
    let abs = absolute(abs);
    repos
        .iter()
        .filter(|r| r.origin == RepoOrigin::Manifest)
        .filter_map(|r| rel_inside(&r.path, &abs).map(|rel| (r, rel)))
        .max_by_key(|(r, _)| key(&r.path).len())
        .map(|(r, rel)| if rel.is_empty() { format!("{LINK_PREFIX}{}", r.name) } else { format!("{LINK_PREFIX}{}/{rel}", r.name) })
}

/// How to store a reference to `abs` in a project file: `repo:<name>/…` inside a listed repo,
/// else project-relative inside the project folder, else the absolute path.
pub fn portable_path(project: &Project, abs: &Path) -> String {
    if let Some(l) = link_for(&project.repos(), abs) {
        return l;
    }
    let root = absolute(project.root());
    match rel_inside(&root, &absolute(abs)) {
        Some(rel) if !rel.is_empty() => rel,
        _ => abs.display().to_string(),
    }
}

// ---------- editing the list ----------

/// Names appear in links (`repo:<name>/…`), so no separators, colons or leading dots.
pub fn validate_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() || n.len() > 64 || n.starts_with('.') || n.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) || n.chars().any(char::is_control) {
        return Err(Error::Invalid(format!("“{name}” can't be a repository name (no slashes, colons or leading dots)")));
    }
    Ok(())
}

fn ensure_unique(manifest: &ProjectManifest, name: &str, except: Option<&str>) -> Result<()> {
    if manifest.repos.iter().any(|r| r.name.eq_ignore_ascii_case(name) && Some(r.name.as_str()) != except) {
        return Err(Error::AlreadyExists(format!("repository {name}")));
    }
    Ok(())
}

/// What adding `folder` would store.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepoSuggestion {
    pub name: String,
    /// Relative to the project folder; `None` on another drive.
    pub rel_path: Option<String>,
    /// Further than a sibling of the project folder (or on another drive): ask whether to share the relative path
    /// or keep the location on this machine only.
    pub far_away: bool,
    pub is_git: bool,
    pub remote: Option<String>,
    /// A listed repo already points there.
    pub existing: Option<String>,
}

pub fn suggest(project: &Project, folder: &Path) -> RepoSuggestion {
    let folder = absolute(folder);
    let root = absolute(project.root());
    let rel = relative_path(&root, &folder);
    // Inside the project folder or next to it (`../api`) is a layout teammates share; further up
    // is probably this machine's own.
    let far_away = rel.as_deref().is_none_or(|r| r.split('/').filter(|p| *p == "..").count() >= 2);
    let repos = resolve(&root, &project.manifest, &read_overrides(&root), false);
    let existing = repos.iter().find(|r| r.origin == RepoOrigin::Manifest && same(&r.path, &folder)).map(|r| r.name.clone());
    let used: HashSet<String> = project.manifest.repos.iter().map(|r| r.name.to_lowercase()).collect();
    RepoSuggestion { name: free_name(&dir_name(&folder), None, &used), rel_path: rel, far_away, is_git: is_git_repo(&folder), remote: origin_url(&folder), existing }
}

/// Adds `folder` (absolute) to `devall.yaml`. `local_only` keeps the location on this machine
/// (`repos.json`) and shares only a conventional sibling path (`../<folder name>`); so does a
/// folder on another drive.
pub fn add(project: &mut Project, folder: &Path, name: Option<&str>, group: Option<&str>, local_only: bool) -> Result<RepoRef> {
    let folder = absolute(folder);
    if !folder.is_dir() {
        return Err(Error::NotFound(folder.display().to_string()));
    }
    let s = suggest(project, &folder);
    if let Some(existing) = &s.existing {
        return Err(Error::AlreadyExists(format!("repository {existing} ({})", folder.display())));
    }
    let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or(s.name);
    validate_name(&name)?;
    ensure_unique(&project.manifest, &name, None)?;
    let local = local_only || s.rel_path.is_none();
    let path = if local { format!("../{}", dir_name(&folder)) } else { s.rel_path.clone().unwrap_or_default() };
    let r = RepoRef { name: name.clone(), path, remote: s.remote, group: group.map(str::trim).filter(|g| !g.is_empty()).map(str::to_string) };
    project.manifest.repos.push(r.clone());
    project.save_manifest()?;
    let mut o = read_overrides(project.root());
    if local && !same(&resolve_rel(&absolute(project.root()), &r.path), &folder) {
        o.insert(name, folder.display().to_string());
    } else {
        o.remove(&name);
    }
    write_overrides(project.root(), &o)?;
    Ok(r)
}

/// Lists discovered repos (by name, from [`resolve`]) in `devall.yaml` with their relative path
/// and origin URL. `groups` optionally assigns each a group.
pub fn save_discovered(project: &mut Project, names: &[String], groups: &BTreeMap<String, String>) -> Result<Vec<RepoRef>> {
    let repos = project.repos();
    let mut added = Vec::new();
    for n in names {
        let Some(r) = find(&repos, n).filter(|r| r.origin == RepoOrigin::Discovered) else {
            return Err(Error::NotFound(format!("discovered repository {n}")));
        };
        validate_name(&r.name)?;
        ensure_unique(&project.manifest, &r.name, None)?;
        let rr = RepoRef {
            name: r.name.clone(),
            path: r.rel_path.clone().unwrap_or_default(),
            remote: r.remote.clone(),
            group: groups.get(&r.name).map(|g| g.trim().to_string()).filter(|g| !g.is_empty()),
        };
        project.manifest.repos.push(rr.clone());
        added.push(rr);
    }
    if !added.is_empty() {
        project.save_manifest()?;
    }
    Ok(added)
}

/// Renames a listed repo and/or sets its group (`Some("")` clears it). The machine-specific
/// location follows a rename.
pub fn update(project: &mut Project, name: &str, new_name: Option<&str>, group: Option<&str>) -> Result<()> {
    let idx = project.manifest.repos.iter().position(|r| r.name == name).ok_or_else(|| Error::NotFound(format!("repository {name}")))?;
    let renamed = new_name.map(str::trim).filter(|n| !n.is_empty() && *n != name).map(str::to_string);
    if let Some(n) = &renamed {
        validate_name(n)?;
        ensure_unique(&project.manifest, n, Some(name))?;
        project.manifest.repos[idx].name = n.clone();
    }
    if let Some(g) = group {
        project.manifest.repos[idx].group = Some(g.trim().to_string()).filter(|g| !g.is_empty());
    }
    project.save_manifest()?;
    if let Some(n) = renamed {
        let mut o = read_overrides(project.root());
        if let Some(p) = o.remove(name) {
            o.insert(n, p);
            write_overrides(project.root(), &o)?;
        }
    }
    Ok(())
}

/// Drops a repo from `devall.yaml` (and its location on this machine). Never touches its files.
pub fn remove(project: &mut Project, name: &str) -> Result<()> {
    let before = project.manifest.repos.len();
    project.manifest.repos.retain(|r| r.name != name);
    if project.manifest.repos.len() == before {
        return Err(Error::NotFound(format!("repository {name}")));
    }
    project.save_manifest()?;
    let mut o = read_overrides(project.root());
    if o.remove(name).is_some() {
        write_overrides(project.root(), &o)?;
    }
    Ok(())
}

/// Points a listed repo at `folder` on this machine only (`repos.json`); pointing it back at its
/// shared path clears the override.
pub fn locate(project: &Project, name: &str, folder: &Path) -> Result<()> {
    let r = project.manifest.repos.iter().find(|r| r.name == name).ok_or_else(|| Error::NotFound(format!("repository {name}")))?;
    let folder = absolute(folder);
    if !folder.is_dir() {
        return Err(Error::NotFound(folder.display().to_string()));
    }
    let mut o = read_overrides(project.root());
    if same(&resolve_rel(&absolute(project.root()), &r.path), &folder) {
        o.remove(name);
    } else {
        o.insert(name.to_string(), folder.display().to_string());
    }
    write_overrides(project.root(), &o)
}

/// Docker Compose file names looked for in each repository.
const COMPOSE_FILES: &[&str] = &["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"];

/// A Compose file in one of the project's repositories.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeFile {
    pub repo: String,
    pub group: Option<String>,
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Relative to the repository, `/`-separated.
    pub rel_path: String,
    /// Default Compose project name (the folder's name, lower-cased, safe characters only).
    pub project: String,
}

/// Compose files at the root of each present repository and one folder down (`deploy/`,
/// `docker/`…), so a multi-repo project's services can be started from the Docker page.
pub fn compose_files(repos: &[Repo]) -> Vec<ComposeFile> {
    let mut out = Vec::new();
    for r in repos.iter().filter(|r| r.is_found()) {
        let mut dirs = vec![(r.path.clone(), String::new())];
        if let Ok(rd) = fs::read_dir(&r.path) {
            let mut subs: Vec<(PathBuf, String)> = rd
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| (e.path(), e.file_name().to_string_lossy().to_string()))
                .filter(|(_, n)| !n.starts_with('.') && !SKIP_DIRS.contains(&n.as_str()))
                .collect();
            subs.sort();
            dirs.extend(subs);
        }
        for (dir, rel) in dirs {
            for name in COMPOSE_FILES {
                let file = dir.join(name);
                if !file.is_file() {
                    continue;
                }
                let folder = dir_name(&dir);
                let project: String = folder.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
                out.push(ComposeFile {
                    repo: r.name.clone(),
                    group: r.group.clone(),
                    rel_path: if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") },
                    path: file,
                    project: project.trim_matches('-').to_string(),
                });
                break; // one per folder, in Compose's own order of preference
            }
        }
    }
    out
}

/// Repositories found under `folder` (not yet a project) — offered when opening or creating a
/// project there. Includes the folder itself first when it is a repo.
pub fn detect(folder: &Path) -> Vec<Repo> {
    let empty = ProjectManifest {
        version: crate::model::FORMAT_VERSION,
        id: String::new(),
        name: String::new(),
        description: None,
        variables: Default::default(),
        repos: Vec::new(),
        extra: Default::default(),
    };
    resolve(folder, &empty, &Overrides::new(), true)
}

#[cfg(test)]
mod tests;
