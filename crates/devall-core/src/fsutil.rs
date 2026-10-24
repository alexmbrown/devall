//! Filesystem helpers: safe relative paths, atomic writes, YAML I/O, slugs.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, IoContext, Result};

/// Joins a `/`-separated path that came from the UI or CLI onto `base`, rejecting anything that
/// could escape it (absolute paths, drive prefixes, `..`).
pub fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.replace('\\', "/");
    let mut out = base.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty() && *p != ".") {
        let comp = Path::new(part).components().next();
        match comp {
            Some(Component::Normal(c)) if Path::new(part).components().count() == 1 => out.push(c),
            _ => return Err(Error::InvalidPath(rel.clone())),
        }
    }
    Ok(out)
}

/// Normalizes a relative path for display/storage: forward slashes, no leading/trailing `/`.
pub fn normalize_rel(rel: &str) -> String {
    rel.replace('\\', "/").split('/').filter(|p| !p.is_empty() && *p != ".").collect::<Vec<_>>().join("/")
}

/// Parent of a normalized relative path (`a/b/c` → `a/b`, `c` → ``).
pub fn rel_parent(rel: &str) -> String {
    match rel.rfind('/') {
        Some(i) => rel[..i].to_string(),
        None => String::new(),
    }
}

/// Last segment of a normalized relative path.
pub fn rel_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

pub fn rel_join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") }
}

/// Writes via a temp file + rename so a crash never leaves a half-written project file.
/// Skips the write when the content is unchanged (keeps mtimes and file watchers quiet).
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    if let Ok(existing) = fs::read(path)
        && existing == data
    {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{file_name}.{:08x}.tmp", fastrand::u32(..)));
    fs::write(&tmp, data).at(&tmp)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(Error::Io { path: path.to_path_buf(), source: e });
    }
    // Lets local file history label the change "saved in /dev/all".
    crate::local::note_app_write(path, data);
    Ok(())
}

pub fn read_yaml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).at(path)?;
    serde_yaml_ng::from_str(&text).map_err(|e| Error::Parse { path: path.to_path_buf(), message: e.to_string() })
}

pub fn to_yaml<T: Serialize>(value: &T) -> Result<String> {
    serde_yaml_ng::to_string(value).map_err(|e| Error::Invalid(format!("cannot serialize: {e}")))
}

pub fn write_yaml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    atomic_write(path, to_yaml(value)?.as_bytes())
}

/// File-name slug: lowercase ASCII letters/digits separated by single dashes.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(64);
    if out.is_empty() { "untitled".into() } else { out }
}

/// First free `<stem><suffix>` in `dir`, trying `stem`, `stem-2`, `stem-3`, …
pub fn unique_name(dir: &Path, stem: &str, suffix: &str) -> String {
    let mut candidate = format!("{stem}{suffix}");
    let mut n = 2;
    while dir.join(&candidate).exists() {
        candidate = format!("{stem}-{n}{suffix}");
        n += 1;
    }
    candidate
}

/// Makes sure each entry appears as a line in `<root>/.gitignore`, appending only what's missing.
pub fn ensure_gitignore(root: &Path, entries: &[&str]) -> Result<()> {
    let path = root.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let have: Vec<&str> = existing.lines().map(str::trim).collect();
    let missing: Vec<&str> = entries.iter().copied().filter(|e| !have.contains(e)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut text = existing.clone();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !existing.contains("# /dev/all") {
        text.push_str("# /dev/all - local-only state and secrets\n");
    }
    for e in missing {
        text.push_str(e);
        text.push('\n');
    }
    atomic_write(&path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_escapes() {
        let base = Path::new("/p");
        assert!(safe_join(base, "../x").is_err());
        assert!(safe_join(base, "a/../../x").is_err());
        assert!(safe_join(base, "a/b.req.yaml").is_ok());
        assert_eq!(safe_join(base, "").unwrap(), base);
        assert_eq!(safe_join(base, "a\\b").unwrap(), base.join("a").join("b"));
        #[cfg(windows)]
        assert!(safe_join(base, "C:/Windows").is_err());
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("List users (v2)!"), "list-users-v2");
        assert_eq!(slugify("  ---  "), "untitled");
        assert_eq!(slugify("GET /users/{id}"), "get-users-id");
    }

    #[test]
    fn rel_helpers() {
        assert_eq!(normalize_rel("/a//b/./c/"), "a/b/c");
        assert_eq!(rel_parent("a/b/c"), "a/b");
        assert_eq!(rel_parent("c"), "");
        assert_eq!(rel_name("a/b/c"), "c");
        assert_eq!(rel_join("", "x"), "x");
    }

    #[test]
    fn gitignore_appends_once() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "target").unwrap();
        ensure_gitignore(dir.path(), &[".devall/", ".env"]).unwrap();
        ensure_gitignore(dir.path(), &[".devall/", ".env"]).unwrap();
        let text = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(text.matches(".devall/").count(), 1);
        assert!(text.starts_with("target\n"));
    }
}
