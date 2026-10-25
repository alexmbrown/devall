//! Watches the open project's /dev/all content and tells the UI when files change outside the app
//! (git pull/checkout, editors, the CLI), so trees and open documents refresh themselves.
//!
//! Only `devall.yaml` and the content folders (`devall_core::project::CONTENT_DIRS`: `http/`,
//! `environments/`, `notes/`, `mocks/`…) are watched recursively — never the whole folder, which
//! in a multi-repo project holds several repos' code, `node_modules` and build output. Spec files
//! that request folders are linked to (OpenAPI auto-sync) are watched one by one wherever they
//! live (`repo:<name>/…`, elsewhere in the project, absolute) and reported as the link's source.
//! Source control watches each repo's `HEAD`/`index` itself (`modules::git`).
//!
//! Registration: `pub mod watch;`, `.manage(watch::WatchState::default())`, commands
//! `watch_project`, `unwatch_project`. Emits the `project-changed` event with `{ root, paths }`
//! and feeds each batch to local file history (`modules::history::record_changes`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use devall_core::project::{CONTENT_DIRS, MANIFEST};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

type Shared = Arc<Mutex<RecommendedWatcher>>;
/// Linked spec files: comparable path → (the file, the link sources pointing at it).
type SpecTargets = BTreeMap<String, (PathBuf, Vec<String>)>;

#[derive(Default)]
pub struct WatchState {
    current: Mutex<Option<(PathBuf, Shared)>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectChanged {
    root: String,
    /// Project-relative, `/`-separated — or, for linked spec files outside the content folders,
    /// the link's source (`repo:api/openapi.yaml`, `specs/api.yaml`, an absolute path).
    paths: Vec<String>,
}

fn ignored(rel: &str) -> bool {
    rel.contains("/node_modules/") || rel.ends_with(".tmp") || rel.split('/').any(|p| p.starts_with('.') && p.ends_with(".tmp"))
}

fn in_content(rel: &str) -> bool {
    rel == MANIFEST || CONTENT_DIRS.contains(&rel.split('/').next().unwrap_or_default())
}

/// Comparable form of a path (case-insensitive file systems on Windows and macOS).
fn key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(any(windows, target_os = "macos")) { s.to_lowercase() } else { s }
}

/// Spec files linked from `http/**/_folder.yaml` outside the content folders: file key → the
/// link sources that point at it (what the auto-sync matches on).
fn spec_targets(root: &Path) -> SpecTargets {
    let mut out = SpecTargets::new();
    let Ok(p) = devall_core::Project::open(root) else { return out };
    for l in devall_import::sync::links(&p).unwrap_or_default() {
        if !l.link.auto_sync || l.project_path.as_deref().is_some_and(in_content) {
            continue;
        }
        let Some(file) = l.link.path(root) else { continue };
        out.entry(key(&file)).or_insert_with(|| (file, Vec::new())).1.push(l.link.source.clone());
    }
    out
}

/// Keeps one non-recursive watch per linked spec's folder (outside the content folders).
struct Specs {
    targets: SpecTargets,
    dirs: BTreeSet<PathBuf>,
}

impl Specs {
    fn refresh(&mut self, root: &Path, watcher: &Weak<Mutex<RecommendedWatcher>>) {
        self.targets = spec_targets(root);
        let want: BTreeSet<PathBuf> = self.targets.values().filter_map(|(f, _)| f.parent().map(Path::to_path_buf)).filter(|d| key(d) != key(root)).collect();
        let Some(w) = watcher.upgrade() else { return };
        let mut w = w.lock().unwrap();
        for gone in self.dirs.difference(&want) {
            let _ = w.unwatch(gone);
        }
        for new in want.difference(&self.dirs) {
            let _ = w.watch(new, RecursiveMode::NonRecursive);
        }
        self.dirs = want;
    }
}

#[tauri::command]
pub fn watch_project(app: AppHandle, state: State<'_, WatchState>, root: String) -> Result<(), String> {
    let root_path = PathBuf::from(&root);
    let mut cur = state.current.lock().unwrap();
    if cur.as_ref().is_some_and(|(p, _)| *p == root_path) {
        return Ok(());
    }
    *cur = None; // drops (stops) the previous watcher

    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx).map_err(|e| e.to_string())?;
    // The folder itself non-recursively (devall.yaml, content folders appearing), each content
    // folder recursively.
    watcher.watch(&root_path, RecursiveMode::NonRecursive).map_err(|e| e.to_string())?;
    for d in CONTENT_DIRS {
        let dir = root_path.join(d);
        if dir.is_dir() {
            let _ = watcher.watch(&dir, RecursiveMode::Recursive);
        }
    }
    let shared: Shared = Arc::new(Mutex::new(watcher));
    // The thread only holds a weak handle: dropping the state's watcher ends it (the channel closes).
    let weak = Arc::downgrade(&shared);
    let base = root_path.clone();
    std::thread::spawn(move || {
        let mut specs = Specs { targets: BTreeMap::new(), dirs: BTreeSet::new() };
        specs.refresh(&base, &weak);
        // Debounce: collect events for 300 ms after the first one, then emit a single batch.
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            let deadline = std::time::Instant::now() + Duration::from_millis(300);
            while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
                match rx.recv_timeout(left) {
                    Ok(ev) => batch.push(ev),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            let mut paths: Vec<String> = Vec::new();
            let mut relink = false;
            for ev in batch.into_iter().flatten().filter(|e| !matches!(e.kind, EventKind::Access(_))) {
                let created = matches!(ev.kind, EventKind::Create(_));
                for p in ev.paths {
                    if let Some((_, sources)) = specs.targets.get(&key(&p)) {
                        paths.extend(sources.iter().cloned());
                    }
                    let Some(r) = rel(&base, &p) else { continue };
                    if !in_content(&r) || ignored(&r) {
                        continue;
                    }
                    // A content folder that appeared after the watch started (first note, first mock…).
                    if created && CONTENT_DIRS.contains(&r.as_str()) && p.is_dir()
                        && let Some(w) = weak.upgrade()
                    {
                        let _ = w.lock().unwrap().watch(&p, RecursiveMode::Recursive);
                    }
                    relink |= r.starts_with("http/") && r.ends_with("_folder.yaml");
                    paths.push(r);
                }
            }
            if relink {
                specs.refresh(&base, &weak);
            }
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                let _ = app.emit("project-changed", ProjectChanged { root: base.display().to_string(), paths: paths.clone() });
                // Local file history: a version per changed tracked file (app saves included).
                super::history::record_changes(&app, &base, &paths);
            }
        }
    });
    *cur = Some((root_path, shared));
    Ok(())
}

#[tauri::command]
pub fn unwatch_project(state: State<'_, WatchState>) {
    *state.current.lock().unwrap() = None;
}

fn rel(base: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(base).ok()?;
    Some(r.to_string_lossy().replace('\\', "/"))
}
