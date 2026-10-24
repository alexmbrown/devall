//! Linked OpenAPI specs: a folder created from a spec remembers where the spec lives (in its
//! `_folder.yaml`) and can be re-imported in place when the spec changes — requests are updated
//! by name (ids, tests and scripts kept), new operations are added, and requests whose operation
//! disappeared are reported (never deleted).
//!
//! ```yaml
//! # http/petstore/_folder.yaml
//! version: 1
//! openapi:
//!   source: specs/petstore.yaml   # project-relative path, absolute path or http(s) URL
//!   autoSync: true                # re-sync when the file changes (the app watches the project)
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use devall_core::fsutil::{normalize_rel, rel_join, rel_parent};
use devall_core::{Project, TreeNode};

use crate::apply::{ApplyOptions, ApplyReport, apply};
use crate::detect::Format;
use crate::error::{Error, Result};

/// Key in `_folder.yaml`.
pub const LINK_KEY: &str = "openapi";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiLink {
    /// Project-relative path (`/`-separated), absolute path, or `http(s)://` URL.
    pub source: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_sync: bool,
}

impl OpenApiLink {
    pub fn is_url(&self) -> bool {
        let s = self.source.to_ascii_lowercase();
        s.starts_with("http://") || s.starts_with("https://")
    }

    /// The spec file on disk (`None` for URLs).
    pub fn path(&self, project_root: &Path) -> Option<PathBuf> {
        if self.is_url() {
            return None;
        }
        let p = Path::new(&self.source);
        Some(if p.is_absolute() { p.to_path_buf() } else { project_root.join(normalize_rel(&self.source)) })
    }

    /// Project-relative path when the spec is inside the project (what file watching matches).
    pub fn project_rel(&self, project_root: &Path) -> Option<String> {
        let p = self.path(project_root)?;
        let rel = p.strip_prefix(project_root).ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedFolder {
    /// Folder relative to `http/`.
    pub folder: String,
    pub name: String,
    pub link: OpenApiLink,
    /// Project-relative spec path when it is inside the project.
    pub project_path: Option<String>,
    /// The spec file exists (always `true` for URLs).
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub folder: String,
    pub source: String,
    pub report: ApplyReport,
    /// Requests (paths under `http/`) whose operation is no longer in the spec. Not deleted.
    pub stale: Vec<String>,
}

/// Stores `source` as the folder's spec: absolute paths inside the project become
/// project-relative so the link works for everyone who clones the repo.
pub fn link(project: &Project, folder: &str, source: &str, auto_sync: bool) -> Result<OpenApiLink> {
    let folder = normalize_rel(folder);
    let source = source.trim();
    if source.is_empty() {
        return Err(Error::Parse("the spec source is empty".into()));
    }
    let mut link = OpenApiLink { source: source.to_string(), auto_sync };
    if !link.is_url() {
        let p = Path::new(source);
        let abs = if p.is_absolute() { p.to_path_buf() } else { project.root().join(normalize_rel(source)) };
        let root = project.root().canonicalize().unwrap_or_else(|_| project.root().to_path_buf());
        let canon = abs.canonicalize().unwrap_or(abs.clone());
        link.source = match canon.strip_prefix(&root) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => canon.to_string_lossy().to_string(),
        };
    }
    let mut f = project.read_folder(&folder)?;
    f.extra.insert(LINK_KEY.into(), serde_json::to_value(&link).map_err(|e| Error::Parse(e.to_string()))?);
    project.save_folder(&folder, &f)?;
    Ok(link)
}

pub fn unlink(project: &Project, folder: &str) -> Result<()> {
    let folder = normalize_rel(folder);
    let mut f = project.read_folder(&folder)?;
    if f.extra.shift_remove(LINK_KEY).is_some() {
        project.save_folder(&folder, &f)?;
    }
    Ok(())
}

pub fn read_link(project: &Project, folder: &str) -> Result<Option<OpenApiLink>> {
    let f = project.read_folder(&normalize_rel(folder))?;
    Ok(f.extra.get(LINK_KEY).and_then(|v| serde_json::from_value(v.clone()).ok()))
}

/// Every folder with a linked spec.
pub fn links(project: &Project) -> Result<Vec<LinkedFolder>> {
    fn walk(project: &Project, n: &TreeNode, out: &mut Vec<LinkedFolder>) {
        if let TreeNode::Folder { path, name, children, .. } = n {
            if !path.is_empty()
                && let Ok(Some(link)) = read_link(project, path)
            {
                let exists = link.is_url() || link.path(project.root()).is_some_and(|p| p.is_file());
                out.push(LinkedFolder { folder: path.clone(), name: name.clone(), project_path: link.project_rel(project.root()), exists, link });
            }
            for c in children {
                walk(project, c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(project, &project.tree()?, &mut out);
    Ok(out)
}

/// Linked folders whose spec is one of `changed` (project-relative paths, e.g. from a file
/// watcher) and that opted into `autoSync`.
pub fn affected_by(project: &Project, changed: &[String]) -> Result<Vec<LinkedFolder>> {
    let changed: Vec<String> = changed.iter().map(|c| normalize_rel(c)).collect();
    Ok(links(project)?.into_iter().filter(|l| l.link.auto_sync && l.project_path.as_ref().is_some_and(|p| changed.contains(p))).collect())
}

/// Re-imports the folder's linked spec from disk (URL sources: fetch the text and call
/// [`resync_text`]).
pub fn resync(project: &mut Project, folder: &str) -> Result<SyncReport> {
    let link = read_link(project, folder)?.ok_or_else(|| Error::Parse(format!("http/{folder} has no linked OpenAPI spec")))?;
    let path = link.path(project.root()).ok_or_else(|| Error::Parse("this spec is a URL; fetch it and use resync_text".into()))?;
    let text = std::fs::read_to_string(&path).map_err(crate::error::io_err(&path))?;
    resync_text(project, folder, &text)
}

/// Re-imports `text` (the spec) into the linked folder in place.
pub fn resync_text(project: &mut Project, folder: &str, text: &str) -> Result<SyncReport> {
    let folder = normalize_rel(folder);
    if folder.is_empty() {
        return Err(Error::Parse("link a folder, not the project root".into()));
    }
    let link = read_link(project, &folder)?.ok_or_else(|| Error::Parse(format!("http/{folder} has no linked OpenAPI spec")))?;
    let format = crate::detect(text).ok_or(Error::UnknownFormat)?;
    if !matches!(format, Format::OpenApi | Format::Swagger) {
        return Err(Error::Parse(format!("{} is not an OpenAPI document (detected {})", link.source, format.label())));
    }
    let plan = crate::parse_text_as(text, format, &Default::default())?;
    // (folder chain below the linked folder, lowercased) + request name, for stale detection.
    let wanted: Vec<(Vec<String>, String)> = plan.requests().iter().map(|(chain, r)| (chain.iter().map(|c| c.to_lowercase()).collect(), r.name.to_lowercase())).collect();
    let display = project.read_folder(&folder)?.name.unwrap_or_else(|| folder.rsplit('/').next().unwrap_or(&folder).to_string());
    let parent = rel_parent(&folder);
    let opts = ApplyOptions {
        wrap_in_folder: true,
        folder_name: Some(display),
        overwrite: true,
        // Environments and project variables are left alone: a re-sync must not reset values the
        // team edited (base URLs, keys) after the first import.
        import_environments: false,
        import_variables: false,
        extract_secrets: true,
    };
    let mut report = apply(plan, project, &parent, &opts)?;
    report.secrets.clear(); // only offered on the first import
    if report.folder != folder {
        report.warnings.push(format!("The spec was imported into http/{} instead of http/{folder} (folder renamed?)", report.folder));
    }

    let mut stale = Vec::new();
    let tree = project.tree()?;
    if let Some(TreeNode::Folder { children, .. }) = find(&tree, &folder) {
        collect_stale(children, &mut Vec::new(), &wanted, &mut stale);
    }
    Ok(SyncReport { folder, source: link.source, report, stale })
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

fn collect_stale(nodes: &[TreeNode], chain: &mut Vec<String>, wanted: &[(Vec<String>, String)], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            TreeNode::Folder { name, children, .. } => {
                chain.push(name.to_lowercase());
                collect_stale(children, chain, wanted, out);
                chain.pop();
            }
            TreeNode::Request { name, path, .. } => {
                let key = (chain.clone(), name.to_lowercase());
                if path.ends_with(devall_core::project::REQUEST_EXT) && !wanted.contains(&key) {
                    out.push(rel_join("", path));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = r#"openapi: 3.0.0
info: { title: Pets, version: "1" }
servers: [{ url: "https://pets.example.com" }]
paths:
  /pets:
    get: { operationId: listPets, summary: List pets, responses: { "200": { description: ok } } }
  /pets/{id}:
    delete: { operationId: deletePet, summary: Delete pet, parameters: [{ name: id, in: path, required: true }], responses: { "204": { description: gone } } }
"#;

    #[test]
    fn link_resync_and_stale() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::init(dir.path(), "t").unwrap();
        std::fs::create_dir_all(dir.path().join("specs")).unwrap();
        let spec = dir.path().join("specs").join("pets.yaml");
        std::fs::write(&spec, V1).unwrap();
        let plan = crate::parse_path(&spec, &Default::default()).unwrap();
        let first = apply(plan, &mut p, "", &ApplyOptions::default()).unwrap();
        let folder = first.folder.clone();

        let l = link(&p, &folder, &spec.display().to_string(), true).unwrap();
        assert_eq!(l.source, "specs/pets.yaml", "absolute path inside the project becomes relative");
        let listed = links(&p).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].folder.as_str(), listed[0].exists, listed[0].project_path.as_deref()), (folder.as_str(), true, Some("specs/pets.yaml")));
        assert_eq!(affected_by(&p, &["specs/pets.yaml".into()]).unwrap().len(), 1);
        assert!(affected_by(&p, &["other.yaml".into()]).unwrap().is_empty());

        // Edit a request so we can see it is updated in place, then change the spec.
        let tree = p.tree().unwrap();
        let mut paths = Vec::new();
        fn reqs(n: &TreeNode, out: &mut Vec<(String, String)>) {
            match n {
                TreeNode::Folder { children, .. } => children.iter().for_each(|c| reqs(c, out)),
                TreeNode::Request { name, path, .. } => out.push((name.clone(), path.clone())),
            }
        }
        reqs(&tree, &mut paths);
        assert_eq!(paths.len(), 2, "{paths:?}");
        let v2 = V1.replace(
            "  /pets/{id}:\n    delete: { operationId: deletePet, summary: Delete pet, parameters: [{ name: id, in: path, required: true }], responses: { \"204\": { description: gone } } }\n",
            "  /pets/{id}:\n    get: { operationId: getPet, summary: Get pet, parameters: [{ name: id, in: path, required: true }], responses: { \"200\": { description: ok } } }\n",
        );
        std::fs::write(&spec, v2).unwrap();
        let r = resync(&mut p, &folder).unwrap();
        assert_eq!(r.report.folder, folder);
        assert_eq!((r.report.requests_updated, r.report.requests_created), (1, 1), "{:#?}", r.report);
        assert_eq!(r.stale.len(), 1, "{:?}", r.stale);
        assert!(r.stale[0].ends_with(".req.yaml"));
        // The link survives the re-import.
        assert_eq!(read_link(&p, &folder).unwrap().unwrap().source, "specs/pets.yaml");
        unlink(&p, &folder).unwrap();
        assert!(links(&p).unwrap().is_empty());
        assert!(resync(&mut p, &folder).is_err());
    }
}
