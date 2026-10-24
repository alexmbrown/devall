//! Files attached to tasks. Each is a copy kept on this device in
//! `<app data>/todos/attachments/<task id>/<name>` — never in a project or a repository.
//!
//! - Names are sanitized (no separators, reserved characters or Windows device names) and made
//!   unique within the task's folder; files are capped at [`MAX_BYTES`].
//! - Paths are always derived here from a task id and an [`Attachment`] record — never taken from
//!   the UI — and re-validated, so a hand-edited or agent-written record can't point elsewhere.
//! - Deleting a task or removing an attachment keeps the file, so undo works; [`gc`] removes files
//!   nothing has referenced for [`GRACE_MS`]. Tasks removed for good (cleared completed tasks,
//!   deleted lists and sections) are [`purge`]d right away.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::model::{Attachment, TodoData};
use crate::error::{Error, IoContext, Result};

/// Folder inside `<app data>/todos/`.
pub const DIR: &str = "attachments";
/// Largest file that can be attached (50 MB).
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;
/// How long files nothing references are kept (undoing a delete, restoring an older copy).
pub const GRACE_MS: i64 = 7 * 86_400_000;
/// Per-folder bookkeeping for [`gc`]: unreferenced file name → first seen unreferenced (Unix ms).
const MARKER: &str = ".orphans.json";
const MAX_NAME_CHARS: usize = 120;
const OCTET: &str = "application/octet-stream";

/// `<base_dir>/todos/attachments` (`base_dir` = the app data directory).
pub fn root(base_dir: &Path) -> PathBuf {
    base_dir.join("todos").join(DIR)
}

/// Task ids name folders: ULIDs (or v1 ids) — ASCII letters, digits, `-` and `_` only.
pub fn valid_task_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 16 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// A safe file name: the last path component of `raw` without control or reserved characters
/// (`<>:"/\|?*`), leading/trailing dots and spaces, or Windows device names (`CON`, `COM1`…),
/// at most 120 characters (the extension is kept). Never empty.
pub fn sanitize_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let mut name = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if name.is_empty() {
        name = "file".into();
    }
    if name.chars().count() > MAX_NAME_CHARS {
        let (stem, ext) = split_ext(&name);
        let keep = MAX_NAME_CHARS - ext.chars().count();
        name = format!("{}{ext}", stem.chars().take(keep).collect::<String>().trim_end());
    }
    // Device names are reserved on Windows whatever the extension (`nul.txt`).
    let stem = split_ext(&name).0.trim_end().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0');
    if device { format!("_{name}") } else { name }
}

/// MIME type by extension (`application/octet-stream` when unknown).
pub fn mime_for(name: &str) -> &'static str {
    let ext = split_ext(name).1.trim_start_matches('.').to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "json" | "har" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => OCTET,
    }
}

/// The extension's type, else a well-formed `hint` (what the clipboard or drop said), else binary.
fn pick_mime(name: &str, hint: Option<&str>) -> String {
    let by_ext = mime_for(name);
    if by_ext != OCTET {
        return by_ext.into();
    }
    hint.map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| h.len() <= 100 && h.matches('/').count() == 1 && !h.starts_with('/') && !h.ends_with('/') && h.chars().all(|c| c.is_ascii_alphanumeric() || "/.+-".contains(c)))
        .unwrap_or_else(|| OCTET.into())
}

/// Files that run code when "opened" (executables, scripts, shortcuts). The app reveals them
/// in the file manager instead of launching them.
pub fn opens_as_program(name: &str) -> bool {
    let ext = split_ext(name).1.trim_start_matches('.').to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "exe" | "com" | "bat" | "cmd" | "msi" | "msp" | "ps1" | "psm1" | "vbs" | "vbe" | "js" | "jse" | "wsf" | "wsh" | "scr" | "pif" | "cpl" | "hta" | "jar" | "lnk" | "url" | "reg" | "inf"
            | "app" | "command" | "tool" | "sh" | "bash" | "zsh" | "csh" | "ksh" | "fish" | "pkg" | "workflow" | "scpt" | "applescript" | "terminal" | "run" | "bin" | "appimage" | "deb" | "rpm"
    )
}

fn too_big(name: &str, size: u64) -> Error {
    Error::Invalid(format!("{name} is {:.1} MB — attachments are limited to {} MB", size as f64 / 1_048_576.0, MAX_BYTES / 1_048_576))
}

/// A name not used on disk nor by `existing` records: `shot.png`, `shot (2).png`…
fn unique_name(dir: &Path, name: &str, existing: &[Attachment]) -> String {
    let (stem, ext) = split_ext(name);
    // `symlink_metadata`: a dangling link counts as taken too.
    let taken = |n: &str| dir.join(n).symlink_metadata().is_ok() || existing.iter().any(|a| a.name.eq_ignore_ascii_case(n));
    let mut candidate = name.to_string();
    let mut i = 2;
    while taken(&candidate) {
        candidate = format!("{stem} ({i}){ext}");
        i += 1;
    }
    candidate
}

/// The task's folder (created) and a free, sanitized name for `raw`, reserved by creating the
/// (empty) file so two attaches at once never pick the same name.
fn prepare(base_dir: &Path, task_id: &str, raw: &str, existing: &[Attachment]) -> Result<(PathBuf, String)> {
    if !valid_task_id(task_id) {
        return Err(Error::InvalidPath(format!("task id {task_id}")));
    }
    let dir = root(base_dir).join(task_id);
    std::fs::create_dir_all(&dir).at(&dir)?;
    let base = sanitize_name(raw);
    for _ in 0..1000 {
        let name = unique_name(&dir, &base, existing);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(&name)) {
            Ok(_) => return Ok((dir, name)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).at(&dir.join(&name)),
        }
    }
    Err(Error::Invalid(format!("no free file name for {base}")))
}

fn record(name: String, mime_hint: Option<&str>, size: u64, now_ms: i64) -> Attachment {
    Attachment { id: ulid::Ulid::generate().to_string(), mime: pick_mime(&name, mime_hint), name, size, added_at: now_ms }
}

/// Copies `src` — an absolute path to a regular file the user picked — into the task's folder.
/// `existing`: the task's current attachments (for a unique name).
pub fn store_file(base_dir: &Path, task_id: &str, src: &Path, existing: &[Attachment], now_ms: i64) -> Result<Attachment> {
    if !src.is_absolute() {
        return Err(Error::InvalidPath(src.display().to_string()));
    }
    let meta = std::fs::metadata(src).at(src)?;
    let raw = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !meta.is_file() {
        return Err(Error::Invalid(format!("{} is not a file", src.display())));
    }
    if meta.len() > MAX_BYTES {
        return Err(too_big(&raw, meta.len()));
    }
    let (dir, name) = prepare(base_dir, task_id, &raw, existing)?;
    let dest = dir.join(&name);
    let size = match std::fs::copy(src, &dest).at(&dest) {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&dest);
            return Err(e);
        }
    };
    // The file may have grown since it was checked.
    if size > MAX_BYTES {
        let _ = std::fs::remove_file(&dest);
        return Err(too_big(&raw, size));
    }
    Ok(record(name, None, size, now_ms))
}

/// Writes dropped or pasted bytes into the task's folder as `raw_name` (sanitized, made unique).
pub fn store_bytes(base_dir: &Path, task_id: &str, raw_name: &str, mime_hint: Option<&str>, bytes: &[u8], existing: &[Attachment], now_ms: i64) -> Result<Attachment> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(too_big(&sanitize_name(raw_name), bytes.len() as u64));
    }
    let (dir, name) = prepare(base_dir, task_id, raw_name, existing)?;
    let dest = dir.join(&name);
    if let Err(e) = std::fs::write(&dest, bytes).at(&dest) {
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }
    Ok(record(name, mime_hint, bytes.len() as u64, now_ms))
}

/// Where an attachment's bytes live. The record is re-validated: its name must already be a
/// sanitized single file name.
pub fn path_of(base_dir: &Path, task_id: &str, a: &Attachment) -> Result<PathBuf> {
    if !valid_task_id(task_id) || sanitize_name(&a.name) != a.name {
        return Err(Error::InvalidPath(format!("{task_id}/{}", a.name)));
    }
    Ok(root(base_dir).join(task_id).join(&a.name))
}

/// Removes the folders of tasks that are gone for good (no grace period). Ids of tasks that still
/// exist in `data` are skipped.
pub fn purge(base_dir: &Path, data: &TodoData, task_ids: &[String]) -> usize {
    let mut n = 0;
    for id in task_ids.iter().filter(|id| valid_task_id(id) && data.task(id).is_none()) {
        let dir = root(base_dir).join(id);
        if dir.is_dir() && std::fs::remove_dir_all(&dir).is_ok() {
            n += 1;
        }
    }
    n
}

/// Garbage-collects attachment files nothing references: a task's folder once the task is gone,
/// single files removed from a task. A file is deleted once it has been unreferenced for
/// `grace_ms` (first sighting tracked in the folder's `.orphans.json`); files referenced again in
/// the meantime (undo, restore) are kept. Empty folders are removed. Returns the files removed.
pub fn gc(base_dir: &Path, data: &TodoData, now_ms: i64, grace_ms: i64) -> usize {
    let Ok(rd) = std::fs::read_dir(root(base_dir)) else { return 0 };
    let mut removed = 0;
    for entry in rd.flatten() {
        // Folders only (a symlink is not followed).
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = entry.path();
        let Some(task_id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
        let referenced: HashSet<&str> = data.task(&task_id).map(|t| t.attachments.iter().map(|a| a.name.as_str()).collect()).unwrap_or_default();
        let marker = dir.join(MARKER);
        let seen: HashMap<String, i64> = std::fs::read(&marker).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut pending: HashMap<String, i64> = HashMap::new();
        let mut left = 0;
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            if name == MARKER {
                continue;
            }
            if referenced.contains(name.as_str()) {
                left += 1;
                continue;
            }
            let first = seen.get(&name).copied().unwrap_or(now_ms);
            if now_ms - first >= grace_ms {
                let p = f.path();
                let ok = if f.file_type().is_ok_and(|t| t.is_dir()) { std::fs::remove_dir_all(&p).is_ok() } else { std::fs::remove_file(&p).is_ok() };
                if ok {
                    removed += 1;
                    continue;
                }
            }
            pending.insert(name, first);
            left += 1;
        }
        if left == 0 {
            let _ = std::fs::remove_dir_all(&dir);
        } else if pending.is_empty() {
            let _ = std::fs::remove_file(&marker);
        } else if pending != seen
            && let Ok(json) = serde_json::to_vec(&pending)
        {
            let _ = std::fs::write(&marker, json);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todos::model::Task;

    fn task(id: &str, attachments: Vec<Attachment>) -> Task {
        let mut t: Task = serde_json::from_value(serde_json::json!({ "id": id, "content": "x", "listId": "inbox", "createdAt": 0 })).unwrap();
        t.attachments = attachments;
        t
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name(r"C:\Users\me\a<b>|c?.png"), "a_b__c_.png");
        assert_eq!(sanitize_name("  ..hidden.txt. "), "hidden.txt");
        assert_eq!(sanitize_name(""), "file");
        assert_eq!(sanitize_name("..."), "file");
        assert_eq!(sanitize_name("a\u{0}b\nc.md"), "a_b_c.md");
        assert_eq!(sanitize_name("con.txt"), "_con.txt");
        assert_eq!(sanitize_name("COM1"), "_COM1");
        assert_eq!(sanitize_name("lpt9.log"), "_lpt9.log");
        assert_eq!(sanitize_name("console.log"), "console.log");
        assert_eq!(sanitize_name("Résumé 2026.pdf"), "Résumé 2026.pdf");
        let long = format!("{}.png", "x".repeat(300));
        let s = sanitize_name(&long);
        assert_eq!(s.chars().count(), MAX_NAME_CHARS);
        assert!(s.ends_with(".png"));
        // Sanitizing is idempotent: stored names validate.
        for raw in ["a<b>.png", "con.txt", &long, "  x  "] {
            let once = sanitize_name(raw);
            assert_eq!(sanitize_name(&once), once, "{raw}");
        }
    }

    #[test]
    fn mime_types_and_programs() {
        assert_eq!(mime_for("Shot.PNG"), "image/png");
        assert_eq!(mime_for("doc.pdf"), "application/pdf");
        assert_eq!(mime_for("noext"), OCTET);
        assert_eq!(pick_mime("blob", Some("Image/PNG")), "image/png");
        assert_eq!(pick_mime("blob", Some("text/html; charset=utf-8")), OCTET);
        assert_eq!(pick_mime("a.jpg", Some("text/plain")), "image/jpeg");
        assert!(opens_as_program("setup.EXE") && opens_as_program("run.sh") && opens_as_program("x.lnk"));
        assert!(!opens_as_program("notes.md") && !opens_as_program("shot.png"));
        assert!(valid_task_id("01JABCDEF") && valid_task_id("t-1_a"));
        assert!(!valid_task_id("") && !valid_task_id("..") && !valid_task_id("a/b") && !valid_task_id(r"a\b"));
    }

    #[test]
    fn stores_bytes_and_files_with_unique_names() {
        let base = tempfile::tempdir().unwrap();
        let a = store_bytes(base.path(), "T1", "shot.png", Some("image/png"), b"png-bytes", &[], 5).unwrap();
        assert_eq!((a.name.as_str(), a.mime.as_str(), a.size, a.added_at), ("shot.png", "image/png", 9, 5));
        let p = path_of(base.path(), "T1", &a).unwrap();
        assert_eq!(p, base.path().join("todos").join(DIR).join("T1").join("shot.png"));
        assert_eq!(std::fs::read(&p).unwrap(), b"png-bytes");
        // Same name again (on disk and in the records): numbered.
        let b = store_bytes(base.path(), "T1", "../shot.png", None, b"2", std::slice::from_ref(&a), 6).unwrap();
        assert_eq!(b.name, "shot (2).png");
        let c = store_bytes(base.path(), "T1", "SHOT.png", None, b"3", &[a.clone(), b.clone()], 7).unwrap();
        assert_eq!(c.name, "SHOT (3).png");
        // Two attaches that don't know of each other's records (copied before recording): the
        // file on disk reserves the name.
        let d1 = store_bytes(base.path(), "T1", "dup.txt", None, b"1", &[], 7).unwrap();
        let d2 = store_bytes(base.path(), "T1", "dup.txt", None, b"2", &[], 7).unwrap();
        assert_eq!((d1.name.as_str(), d2.name.as_str()), ("dup.txt", "dup (2).txt"));
        assert_eq!(std::fs::read(path_of(base.path(), "T1", &d1).unwrap()).unwrap(), b"1");

        // Files: absolute paths to regular files only, copied.
        let src = base.path().join("report.pdf");
        std::fs::write(&src, b"%PDF").unwrap();
        let f = store_file(base.path(), "T2", &src, &[], 8).unwrap();
        assert_eq!((f.name.as_str(), f.mime.as_str(), f.size), ("report.pdf", "application/pdf", 4));
        assert!(src.exists(), "the original stays where it was");
        assert!(store_file(base.path(), "T2", Path::new("report.pdf"), &[], 8).is_err(), "relative path");
        assert!(store_file(base.path(), "T2", base.path(), &[], 8).is_err(), "a folder");
        assert!(store_file(base.path(), "T2", &base.path().join("missing.txt"), &[], 8).is_err());
        assert!(store_bytes(base.path(), "../evil", "x.txt", None, b"x", &[], 8).is_err(), "task ids can't escape");
    }

    #[test]
    fn size_limit_and_tampered_records() {
        let base = tempfile::tempdir().unwrap();
        let big = vec![0u8; MAX_BYTES as usize + 1];
        let e = store_bytes(base.path(), "T1", "big.bin", None, &big, &[], 0).unwrap_err().to_string();
        assert!(e.contains("limited to 50 MB"), "{e}");
        assert!(!root(base.path()).join("T1").join("big.bin").exists());
        // A record pointing outside its folder (hand-edited file, agent) is refused.
        for name in ["../../todos.json", r"..\x", "a/b.png", ".orphans.json", "con"] {
            let a = Attachment { id: "A".into(), name: name.into(), mime: OCTET.into(), size: 1, added_at: 0 };
            assert!(path_of(base.path(), "T1", &a).is_err(), "{name}");
        }
        let ok = Attachment { id: "A".into(), name: "ok.txt".into(), mime: OCTET.into(), size: 1, added_at: 0 };
        assert!(path_of(base.path(), "../T1", &ok).is_err());
    }

    #[test]
    fn gc_keeps_referenced_files_and_waits_out_the_grace_period() {
        let base = tempfile::tempdir().unwrap();
        let day = 86_400_000;
        let keep = store_bytes(base.path(), "LIVE", "keep.txt", None, b"k", &[], 0).unwrap();
        let dropped = store_bytes(base.path(), "LIVE", "dropped.txt", None, b"d", &[], 0).unwrap();
        let gone = store_bytes(base.path(), "GONE", "a.png", None, b"a", &[], 0).unwrap();
        let back = store_bytes(base.path(), "BACK", "b.png", None, b"b", &[], 0).unwrap();
        let mut data = TodoData { tasks: vec![task("LIVE", vec![keep.clone()])], ..Default::default() };
        let dir = |id: &str| root(base.path()).join(id);

        // First sighting: nothing removed yet, the orphans are remembered.
        assert_eq!(gc(base.path(), &data, 10 * day, GRACE_MS), 0);
        assert!(dir("LIVE").join(&dropped.name).exists() && dir("GONE").join(&gone.name).exists());
        assert!(dir("GONE").join(MARKER).exists());
        // "BACK" is restored (undo) before the grace period ends: its file stays, marker cleared.
        data.tasks.push(task("BACK", vec![back.clone()]));
        assert_eq!(gc(base.path(), &data, 13 * day, GRACE_MS), 0);
        assert!(!dir("BACK").join(MARKER).exists());
        // A week after the first sighting the orphans go, and the empty folder with them.
        assert_eq!(gc(base.path(), &data, 17 * day, GRACE_MS), 2);
        assert!(!dir("LIVE").join(&dropped.name).exists() && dir("LIVE").join(&keep.name).exists());
        assert!(!dir("GONE").exists());
        assert!(dir("BACK").join(&back.name).exists());
        assert!(!dir("LIVE").join(MARKER).exists());

        // Purge: tasks gone for good lose their folder at once; live tasks are never purged.
        assert_eq!(purge(base.path(), &data, &["BACK".into(), "LIVE".into()]), 0);
        data.tasks.retain(|t| t.id != "BACK");
        assert_eq!(purge(base.path(), &data, &["BACK".into(), "../x".into()]), 1);
        assert!(!dir("BACK").exists());
        // No attachments folder at all: nothing to do.
        assert_eq!(gc(tempfile::tempdir().unwrap().path(), &data, 0, 0), 0);
    }
}
