//! Exporters: a project (or one folder of it) → OpenAPI 3.1 YAML, Postman Collection v2.1 JSON
//! (+ environments), a JetBrains-compatible `.http` file (+ `http-client.env.json`) or a Hurl
//! file (+ one `--variables-file` per environment).
//!
//! Secret *values* are never exported: `{{secret.NAME}}` references become plain `{{NAME}}`
//! variables declared (empty) in the target format's private/secret slot.

use serde::Serialize;

use devall_core::fsutil::{normalize_rel, rel_join};
use devall_core::project::REQUEST_EXT;
use devall_core::{HttpRequestFile, Project, TreeNode};

use crate::error::{Error, Result};
use crate::util::map_templates;

mod http_file;
mod hurl;
mod openapi;
mod postman;

pub use http_file::to_http_file;
pub use hurl::to_hurl;
pub use openapi::to_openapi;
pub use postman::to_postman;

/// The main document plus companion files (environments).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutput {
    pub content: String,
    /// Suggested file name, e.g. `my-api.openapi.yaml`.
    pub file_name: String,
    /// Editor language for previews: `yaml`, `json`, `http` or `hurl`.
    pub language: String,
    pub warnings: Vec<String>,
    /// Extra files meant to sit next to the main one (Postman environments, JetBrains env files).
    pub attachments: Vec<ExportFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    pub file_name: String,
    pub content: String,
}

/// The exported subtree.
pub(crate) struct Scope {
    /// Display name (folder name, or project name for the whole project).
    pub name: String,
    /// Folder path relative to `http/` (empty = project root).
    pub root: String,
    pub children: Vec<TreeNode>,
}

pub(crate) fn scope(project: &Project, folder: Option<&str>) -> Result<Scope> {
    let root = normalize_rel(folder.unwrap_or(""));
    let tree = project.tree()?;
    let node = if root.is_empty() { Some(&tree) } else { find(&tree, &root) };
    match node {
        Some(TreeNode::Folder { name, children, .. }) => Ok(Scope { name: name.clone(), root, children: children.clone() }),
        _ => Err(Error::Core(devall_core::Error::NotFound(root))),
    }
}

fn find<'a>(node: &'a TreeNode, path: &str) -> Option<&'a TreeNode> {
    if node.path() == path {
        return Some(node);
    }
    match node {
        TreeNode::Folder { children, .. } => children.iter().find_map(|c| find(c, path)),
        _ => None,
    }
}

/// A request to export, with its folder chain (display names below the export root).
pub(crate) struct Entry {
    pub folders: Vec<String>,
    /// Folder headers merged, auth resolved (never `Inherit`).
    pub effective: HttpRequestFile,
}

pub(crate) fn entries(project: &Project, nodes: &[TreeNode], warnings: &mut Vec<String>) -> Result<Vec<Entry>> {
    fn walk(project: &Project, nodes: &[TreeNode], folders: &mut Vec<String>, out: &mut Vec<Entry>, skipped: &mut Vec<String>) -> Result<()> {
        for n in nodes {
            match n {
                TreeNode::Folder { name, children, .. } => {
                    folders.push(name.clone());
                    walk(project, children, folders, out, skipped)?;
                    folders.pop();
                }
                TreeNode::Request { name, path, error, .. } => {
                    if error.is_some() || !path.ends_with(REQUEST_EXT) {
                        skipped.push(name.clone());
                        continue;
                    }
                    let Ok(raw) = project.read_request(path) else {
                        skipped.push(name.clone());
                        continue;
                    };
                    let effective = project.effective_request(path, &raw)?.request;
                    out.push(Entry { folders: folders.clone(), effective });
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    walk(project, nodes, &mut Vec::new(), &mut out, &mut skipped)?;
    if !skipped.is_empty() {
        warnings.push(format!("Skipped {} non-HTTP or unreadable items: {}", skipped.len(), skipped.iter().take(5).cloned().collect::<Vec<_>>().join(", ")));
    }
    Ok(out)
}

/// Effective folder state at `folder` (auth and variables inherited from above).
pub(crate) fn folder_context(project: &Project, folder: &str) -> Result<devall_core::project::EffectiveRequest> {
    Ok(project.effective_request(&rel_join(folder, &format!("_{REQUEST_EXT}")), &HttpRequestFile::new(""))?)
}

/// `{{secret.NAME}}` → `{{NAME}}`; collects the names.
pub(crate) fn unsecret(s: &str, names: &mut Vec<String>) -> String {
    map_templates(s, |k| {
        let n = k.strip_prefix("secret.")?.trim();
        if !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
        Some(format!("{{{{{n}}}}}"))
    })
}

/// A file-name-safe slug.
pub(crate) fn file_slug(name: &str) -> String {
    devall_core::fsutil::slugify(name)
}
