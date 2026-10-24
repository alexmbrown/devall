//! Shared project notes: Markdown files under `<project>/notes/`, committed with the repo.
//!
//! A note's title is its first `# heading` (falling back to the file name). Optional YAML front
//! matter (`---\ntags: [a, b]\n---`) supplies tags. Paths are `/`-separated, relative to `notes/`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, IoContext, Result};
use crate::fsutil::{atomic_write, normalize_rel, rel_join, rel_name, rel_parent, safe_join, slugify, unique_name};
use crate::notebook::{NOTEBOOK_EXT, notebook_text};
use crate::project::Project;

pub const NOTES_DIR: &str = "notes";
pub const NOTE_EXT: &str = ".md";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum NoteNode {
    Folder { name: String, path: String, children: Vec<NoteNode> },
    /// `format` is `markdown` (`.md`) or `notebook` (`.ipynb`).
    Note { title: String, path: String, tags: Vec<String>, modified: i64, format: String },
}

fn is_note_file(name: &str) -> bool {
    name.ends_with(NOTE_EXT) || name.ends_with(NOTEBOOK_EXT)
}

fn note_ext(name: &str) -> &'static str {
    if name.ends_with(NOTEBOOK_EXT) { NOTEBOOK_EXT } else { NOTE_EXT }
}

/// Title of a Markdown note or notebook, from its raw file content.
pub fn note_title(raw: &str, path: &str) -> String {
    if path.ends_with(NOTEBOOK_EXT) {
        notebook_text(raw).0.unwrap_or_else(|| rel_name(path).trim_end_matches(NOTEBOOK_EXT).replace('-', " "))
    } else {
        title_of(raw, path)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NoteHit {
    pub path: String,
    pub title: String,
    /// A short excerpt around the first match.
    pub snippet: String,
    pub matches: usize,
}

/// Title from the first ATX heading, else the file stem.
pub fn title_of(content: &str, path: &str) -> String {
    let body = strip_front_matter(content).1;
    body.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| rel_name(path).trim_end_matches(NOTE_EXT).replace('-', " "))
}

/// (`tags` from front matter, body without the front matter).
pub fn strip_front_matter(content: &str) -> (Vec<String>, &str) {
    let Some(rest) = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n")) else { return (Vec::new(), content) };
    let Some(end) = rest.find("\n---") else { return (Vec::new(), content) };
    let fm = &rest[..end];
    let after = rest[end + 4..].trim_start_matches(['\r', '\n']);
    let tags = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(fm)
        .ok()
        .and_then(|v| v.get("tags").cloned())
        .map(|t| match t {
            serde_yaml_ng::Value::Sequence(s) => s.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
            serde_yaml_ng::Value::String(s) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    (tags, after)
}

fn mtime(p: &Path) -> i64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Project {
    pub fn notes_root(&self) -> PathBuf {
        self.root().join(NOTES_DIR)
    }

    fn note_path(&self, rel: &str) -> Result<PathBuf> {
        safe_join(&self.notes_root(), rel)
    }

    /// Folders first, then notes, each alphabetical.
    pub fn notes_tree(&self) -> Result<NoteNode> {
        fn scan(dir: &Path, rel: &str) -> Result<Vec<NoteNode>> {
            let mut folders = Vec::new();
            let mut notes = Vec::new();
            if !dir.is_dir() {
                return Ok(Vec::new());
            }
            for entry in fs::read_dir(dir).at(dir)?.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let path = entry.path();
                let child = rel_join(rel, &name);
                if path.is_dir() {
                    folders.push(NoteNode::Folder { name: name.clone(), children: scan(&path, &child)?, path: child });
                } else if is_note_file(&name) {
                    let text = fs::read_to_string(&path).unwrap_or_default();
                    let notebook = name.ends_with(NOTEBOOK_EXT);
                    let tags = if notebook { Vec::new() } else { strip_front_matter(&text).0 };
                    notes.push(NoteNode::Note {
                        title: note_title(&text, &child),
                        tags,
                        modified: mtime(&path),
                        format: if notebook { "notebook".into() } else { "markdown".into() },
                        path: child,
                    });
                }
            }
            let key = |n: &NoteNode| match n {
                NoteNode::Folder { name, .. } => name.to_lowercase(),
                NoteNode::Note { title, .. } => title.to_lowercase(),
            };
            folders.sort_by_key(key);
            notes.sort_by_key(key);
            folders.extend(notes);
            Ok(folders)
        }
        Ok(NoteNode::Folder { name: "Notes".into(), path: String::new(), children: scan(&self.notes_root(), "")? })
    }

    /// Raw content of a Markdown note or notebook (JSON for `.ipynb`).
    pub fn read_note(&self, rel: &str) -> Result<String> {
        let rel = normalize_rel(rel);
        if !is_note_file(&rel) {
            return Err(Error::InvalidPath(rel));
        }
        let p = self.note_path(&rel)?;
        fs::read_to_string(&p).at(&p)
    }

    /// Saves a note; notebooks are validated as nbformat 4 first.
    pub fn save_note(&self, rel: &str, content: &str) -> Result<()> {
        let rel = normalize_rel(rel);
        if !is_note_file(&rel) {
            return Err(Error::InvalidPath(rel));
        }
        if rel.ends_with(NOTEBOOK_EXT) {
            crate::notebook::validate(content).map_err(Error::Invalid)?;
        }
        atomic_write(&self.note_path(&rel)?, content.as_bytes())
    }

    /// Creates `<folder>/<slug(title)>.ipynb` (a runnable runbook); returns its path.
    pub fn create_notebook(&self, folder: &str, title: &str) -> Result<String> {
        let folder = normalize_rel(folder);
        let dir = self.note_path(&folder)?;
        fs::create_dir_all(&dir).at(&dir)?;
        let file = unique_name(&dir, &slugify(title), NOTEBOOK_EXT);
        let rel = rel_join(&folder, &file);
        self.save_note(&rel, &crate::notebook::new_notebook(title))?;
        Ok(rel)
    }

    /// Creates `<folder>/<slug(title)>.md` starting with `# title`; returns its path.
    pub fn create_note(&self, folder: &str, title: &str) -> Result<String> {
        let folder = normalize_rel(folder);
        let dir = self.note_path(&folder)?;
        fs::create_dir_all(&dir).at(&dir)?;
        let file = unique_name(&dir, &slugify(title), NOTE_EXT);
        let rel = rel_join(&folder, &file);
        self.save_note(&rel, &format!("# {}\n\n", title.trim()))?;
        Ok(rel)
    }

    pub fn create_note_folder(&self, parent: &str, name: &str) -> Result<String> {
        let parent = normalize_rel(parent);
        let dir = self.note_path(&parent)?;
        let n = unique_name(&dir, &slugify(name), "");
        fs::create_dir_all(dir.join(&n)).at(&dir)?;
        Ok(rel_join(&parent, &n))
    }

    /// Renames a note file (keeping the heading in sync) or a folder; returns the new path.
    pub fn rename_note(&self, rel: &str, new_name: &str) -> Result<String> {
        let rel = normalize_rel(rel);
        let src = self.note_path(&rel)?;
        if !src.exists() {
            return Err(Error::NotFound(rel));
        }
        let parent = rel_parent(&rel);
        let dir = self.note_path(&parent)?;
        let is_note = is_note_file(&rel);
        let ext = note_ext(&rel);
        let slug = slugify(new_name);
        let file = if is_note {
            if format!("{slug}{ext}") == rel_name(&rel) { rel_name(&rel).to_string() } else { unique_name(&dir, &slug, ext) }
        } else if slug == rel_name(&rel) {
            slug
        } else {
            unique_name(&dir, &slug, "")
        };
        let dest_rel = rel_join(&parent, &file);
        if dest_rel != rel {
            fs::rename(&src, dir.join(&file)).at(&src)?;
        }
        if is_note && ext == NOTEBOOK_EXT {
            let text = self.read_note(&dest_rel)?;
            let retitled = crate::notebook::retitle(&text, new_name).map_err(Error::Invalid)?;
            self.save_note(&dest_rel, &retitled)?;
        } else if is_note {
            // Keep the first heading equal to the new name.
            let text = self.read_note(&dest_rel)?;
            let (_, body) = strip_front_matter(&text);
            let offset = text.len() - body.len();
            let mut replaced = false;
            let new_body: Vec<String> = body
                .lines()
                .map(|l| {
                    if !replaced && l.trim_start().starts_with("# ") {
                        replaced = true;
                        format!("# {}", new_name.trim())
                    } else {
                        l.to_string()
                    }
                })
                .collect();
            let mut out = text[..offset].to_string();
            if !replaced {
                out.push_str(&format!("# {}\n\n", new_name.trim()));
            }
            out.push_str(&new_body.join("\n"));
            if text.ends_with('\n') && !out.ends_with('\n') {
                out.push('\n');
            }
            self.save_note(&dest_rel, &out)?;
        }
        Ok(dest_rel)
    }

    pub fn delete_note(&self, rel: &str) -> Result<()> {
        let rel = normalize_rel(rel);
        if rel.is_empty() {
            return Err(Error::Invalid("cannot delete the notes folder".into()));
        }
        let p = self.note_path(&rel)?;
        if !p.exists() {
            return Err(Error::NotFound(rel));
        }
        trash::delete(&p).map_err(|e| Error::Io { path: p.clone(), source: std::io::Error::other(e.to_string()) })
    }

    /// Case-insensitive full-text search over titles and bodies.
    pub fn search_notes(&self, query: &str, limit: usize) -> Result<Vec<NoteHit>> {
        let q = query.trim().to_lowercase();
        let mut hits = Vec::new();
        if q.is_empty() {
            return Ok(hits);
        }
        for (path, text) in self.all_notes()? {
            let title = title_of(&text, &path);
            let lower = text.to_lowercase();
            let matches = lower.matches(&q).count() + title.to_lowercase().matches(&q).count();
            if matches == 0 {
                continue;
            }
            let snippet = match lower.find(&q) {
                Some(i) => {
                    let start = text[..i].char_indices().rev().nth(60).map(|(j, _)| j).unwrap_or(0);
                    let end = text[i..].char_indices().nth(q.chars().count() + 80).map(|(j, _)| i + j).unwrap_or(text.len());
                    text[start..end].replace('\n', " ")
                }
                None => String::new(),
            };
            hits.push(NoteHit { path, title, snippet, matches });
        }
        hits.sort_by(|a, b| b.matches.cmp(&a.matches).then(a.title.cmp(&b.title)));
        hits.truncate(limit);
        Ok(hits)
    }

    /// Every note as (path, content).
    pub fn all_notes(&self) -> Result<Vec<(String, String)>> {
        fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, String)>) {
            let Ok(rd) = fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let child = rel_join(rel, &name);
                let p = e.path();
                if p.is_dir() {
                    walk(&p, &child, out);
                } else if is_note_file(&name)
                    && let Ok(t) = fs::read_to_string(&p)
                {
                    // Notebooks are rendered to Markdown-with-code-fences for search and AI context.
                    let text = if name.ends_with(NOTEBOOK_EXT) { notebook_text(&t).1 } else { t };
                    out.push((child, text));
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.notes_root(), "", &mut out);
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_crud_search_and_titles() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "N").unwrap();
        let folder = p.create_note_folder("", "Design").unwrap();
        let a = p.create_note(&folder, "Auth flow").unwrap();
        assert_eq!(a, "design/auth-flow.md");
        p.save_note(&a, "---\ntags: [auth, oauth]\n---\n# Auth flow\n\nWe use PKCE for the SPA.\n").unwrap();
        let b = p.create_note("", "Runbook").unwrap();
        p.save_note(&b, "No heading here, mentions pkce once.").unwrap();

        let tree = p.notes_tree().unwrap();
        let NoteNode::Folder { children, .. } = tree else { panic!() };
        assert!(matches!(&children[0], NoteNode::Folder { name, .. } if name == "design"));
        let NoteNode::Folder { children: inner, .. } = &children[0] else { panic!() };
        assert!(matches!(&inner[0], NoteNode::Note { title, tags, .. } if title == "Auth flow" && tags == &vec!["auth".to_string(), "oauth".to_string()]));
        assert!(matches!(&children[1], NoteNode::Note { title, .. } if title == "runbook"));

        let hits = p.search_notes("PKCE", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits[0].snippet.to_lowercase().contains("pkce"));

        let renamed = p.rename_note(&a, "Login flow").unwrap();
        assert_eq!(renamed, "design/login-flow.md");
        let text = p.read_note(&renamed).unwrap();
        assert!(text.starts_with("---\ntags: [auth, oauth]\n---\n# Login flow\n"), "{text}");
        assert!(p.read_note("../devall.yaml").is_err());
        assert_eq!(p.all_notes().unwrap().len(), 2);
    }
}

#[cfg(test)]
mod notebook_tests {
    use super::*;

    #[test]
    fn notebooks_are_notes() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "NB").unwrap();
        let nb = p.create_notebook("", "Run the app").unwrap();
        assert_eq!(nb, "run-the-app.ipynb");
        let tree = p.notes_tree().unwrap();
        let NoteNode::Folder { children, .. } = tree else { panic!() };
        assert!(matches!(&children[0], NoteNode::Note { title, format, .. } if title == "Run the app" && format == "notebook"));
        assert!(p.save_note(&nb, "{\"not\":\"a notebook\"}").is_err());
        assert_eq!(p.search_notes("pwd", 5).unwrap().len(), 1, "code cells are searchable");
        let renamed = p.rename_note(&nb, "Start the stack").unwrap();
        assert_eq!(renamed, "start-the-stack.ipynb");
        assert!(p.read_note(&renamed).unwrap().contains("# Start the stack"));
    }
}
