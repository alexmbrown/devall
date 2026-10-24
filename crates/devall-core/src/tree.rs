//! The request tree shown in the sidebar, built from the `http/` folder.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::error::{IoContext, Result};
use crate::fsutil::{read_yaml, rel_join};
use crate::model::FolderFile;
use crate::project::{FOLDER_FILE, Project, item_kind};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TreeNode {
    Folder {
        name: String,
        /// Relative to `http/`; empty for the root.
        path: String,
        children: Vec<TreeNode>,
    },
    Request {
        name: String,
        path: String,
        method: String,
        id: String,
        /// `http`, `ws`, `grpc`, `mqtt` or `socketio`.
        protocol: String,
        /// Set when the file exists but can't be parsed; the UI shows it so the user can fix it.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

impl TreeNode {
    pub fn path(&self) -> &str {
        match self {
            TreeNode::Folder { path, .. } | TreeNode::Request { path, .. } => path,
        }
    }

    /// Every HTTP request path below this node, in display order (what runners execute).
    pub fn request_paths(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out.retain(|p| p.ends_with(crate::project::REQUEST_EXT));
        out
    }

    fn collect(&self, out: &mut Vec<String>) {
        match self {
            TreeNode::Folder { children, .. } => children.iter().for_each(|c| c.collect(out)),
            TreeNode::Request { path, .. } => out.push(path.clone()),
        }
    }
}

fn is_child(name: &str, is_dir: bool) -> bool {
    !name.starts_with('.') && (if is_dir { name != crate::project::SNAPSHOT_DIR } else { item_kind(name).is_some() })
}

/// Child file/dir names of `dir` in display order: entries named in `order` first (if they
/// exist), then the rest alphabetically.
pub(crate) fn ordered_children(dir: &Path, order: &[String]) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(dir).at(dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_child(&name, is_dir) && name != FOLDER_FILE {
                names.push(name);
            }
        }
    }
    names.sort_by_key(|n| n.to_lowercase());
    let mut out: Vec<String> = order.iter().filter(|n| names.contains(n)).cloned().collect();
    out.dedup();
    for n in names {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    Ok(out)
}

impl Project {
    pub fn tree(&self) -> Result<TreeNode> {
        let root = self.http_root();
        let children = if root.is_dir() { scan(&root, "")? } else { Vec::new() };
        Ok(TreeNode::Folder { name: self.manifest.name.clone(), path: String::new(), children })
    }
}

fn scan(dir: &Path, rel: &str) -> Result<Vec<TreeNode>> {
    let folder: FolderFile = match dir.join(FOLDER_FILE) {
        p if p.is_file() => read_yaml(&p).unwrap_or_default(),
        _ => FolderFile::default(),
    };
    let mut out = Vec::new();
    for name in ordered_children(dir, &folder.order)? {
        let path = dir.join(&name);
        let child_rel = rel_join(rel, &name);
        if path.is_dir() {
            let meta: FolderFile = match path.join(FOLDER_FILE) {
                p if p.is_file() => read_yaml(&p).unwrap_or_default(),
                _ => FolderFile::default(),
            };
            out.push(TreeNode::Folder {
                name: meta.name.unwrap_or_else(|| name.clone()),
                children: scan(&path, &child_rel)?,
                path: child_rel,
            });
        } else {
            let (ext, protocol) = item_kind(&name).unwrap_or((".req.yaml", "http"));
            out.push(match read_yaml::<Named>(&path) {
                Ok(r) => TreeNode::Request {
                    name: r.name,
                    method: if protocol == "http" { r.method.unwrap_or_else(|| "GET".into()).to_uppercase() } else { protocol_label(protocol).into() },
                    id: r.id,
                    protocol: protocol.into(),
                    path: child_rel,
                    error: None,
                },
                Err(e) => TreeNode::Request {
                    name: name.trim_end_matches(ext).to_string(),
                    method: "?".into(),
                    id: String::new(),
                    protocol: protocol.into(),
                    path: child_rel,
                    error: Some(e.to_string()),
                },
            });
        }
    }
    Ok(out)
}

/// Minimal shape shared by every request-like file (for the tree).
#[derive(serde::Deserialize)]
struct Named {
    name: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    method: Option<String>,
}

/// Badge text for non-HTTP request types.
pub fn protocol_label(protocol: &str) -> &'static str {
    match protocol {
        "ws" => "WS",
        "grpc" => "GRPC",
        "mqtt" => "MQTT",
        "socketio" => "SIO",
        _ => "HTTP",
    }
}
