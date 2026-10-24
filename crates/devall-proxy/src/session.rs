//! Saving captures on demand: `<project data>/captures/<name>.har` (on the device, outside the
//! repository — see `devall_core::local::project_dir`) and loading them back into the inspector.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{IoContext, ProxyError, Result};
use crate::export::har::{HarOptions, from_har, to_har_string};
use crate::model::Exchange;

pub const CAPTURES_DIR: &str = "captures";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub file: String,
    pub size: u64,
    /// Unix epoch milliseconds.
    pub modified: i64,
}

fn captures_dir(root: &Path) -> PathBuf {
    devall_core::local::project_dir(root).join(CAPTURES_DIR)
}

fn file_path(root: &Path, file: &str) -> Result<PathBuf> {
    let name = devall_core::fsutil::normalize_rel(file);
    if name.contains('/') || !name.to_ascii_lowercase().ends_with(".har") {
        return Err(ProxyError::Invalid(format!("{file} is not a capture file")));
    }
    Ok(devall_core::fsutil::safe_join(&captures_dir(root), &name)?)
}

/// Writes the exchanges as HAR and returns the file name (inside the project's captures folder).
pub fn save_session(root: &Path, name: Option<&str>, exchanges: &[Exchange], include_sensitive: bool) -> Result<String> {
    if !root.join(devall_core::project::MANIFEST).is_file() {
        return Err(ProxyError::Core(devall_core::Error::NotAProject(root.to_path_buf())));
    }
    let dir = captures_dir(root);
    std::fs::create_dir_all(&dir).at(&dir)?;
    let stem = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => devall_core::fsutil::slugify(n),
        None => chrono::Local::now().format("capture-%Y%m%d-%H%M%S").to_string(),
    };
    let file = devall_core::fsutil::unique_name(&dir, &stem, ".har");
    let text = to_har_string(exchanges, &HarOptions { include_sensitive });
    devall_core::fsutil::atomic_write(&dir.join(&file), text.as_bytes())?;
    Ok(file)
}

/// Saved captures, newest first.
pub fn list_sessions(root: &Path) -> Result<Vec<SessionEntry>> {
    let dir = captures_dir(root);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<SessionEntry> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".har"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64;
            Some(SessionEntry { file: e.file_name().to_string_lossy().into_owned(), size: meta.len(), modified })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.file.cmp(&b.file)));
    Ok(out)
}

pub fn read_session(root: &Path, file: &str) -> Result<Vec<Exchange>> {
    let path = file_path(root, file)?;
    let text = std::fs::read_to_string(&path).at(&path)?;
    from_har(&text)
}

/// Moves a saved capture to the OS trash.
pub fn delete_session(root: &Path, file: &str) -> Result<()> {
    let path = file_path(root, file)?;
    trash::delete(&path).map_err(|e| ProxyError::Invalid(format!("could not move {} to the trash: {e}", path.display())))
}
