//! Watches the open project folder and tells the UI when files change outside the app
//! (git pull/checkout, editors, the CLI), so trees and open documents refresh themselves.
//!
//! Registration: `pub mod watch;`, `.manage(watch::WatchState::default())`, commands
//! `watch_project`, `unwatch_project`. Emits the `project-changed` event with `{ root, paths }`
//! and feeds each batch to local file history (`modules::history::record_changes`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

#[derive(Default)]
pub struct WatchState {
    current: Mutex<Option<(PathBuf, RecommendedWatcher)>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectChanged {
    root: String,
    /// Project-relative, `/`-separated.
    paths: Vec<String>,
}

fn ignored(rel: &str) -> bool {
    rel.starts_with(".devall/") || rel.starts_with(".git/") || rel.contains("/node_modules/") || rel.starts_with("target/") || rel.ends_with(".tmp") || rel.split('/').any(|p| p.starts_with('.') && p.ends_with(".tmp"))
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
    watcher.watch(&root_path, RecursiveMode::Recursive).map_err(|e| e.to_string())?;

    // Debounce: collect events for 300 ms after the first one, then emit a single batch.
    let base = root_path.clone();
    std::thread::spawn(move || {
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
            let mut paths: Vec<String> = batch
                .into_iter()
                .flatten()
                .filter(|e| !matches!(e.kind, notify::EventKind::Access(_)))
                .flat_map(|e| e.paths)
                .filter_map(|p| rel(&base, &p))
                .filter(|r| !ignored(r))
                .collect();
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                let _ = app.emit("project-changed", ProjectChanged { root: base.display().to_string(), paths: paths.clone() });
                // Local file history: a version per changed tracked file (app saves included).
                super::history::record_changes(&app, &base, &paths);
            }
        }
    });
    *cur = Some((root_path, watcher));
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
