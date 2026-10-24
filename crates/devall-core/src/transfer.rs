//! Copying (or moving) requests, folders, environments and notes from one project into another —
//! "Move to project" for the scratch pad. Name clashes get ` (2)`, ` (3)`… like the importers;
//! nothing in the destination is ever overwritten. Secret *values* are not touched here: the
//! report lists which secret names travelled so the caller can offer to copy keychain entries.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::fsutil::{normalize_rel, read_yaml, rel_join, rel_name, rel_parent, safe_join, slugify, unique_name, write_yaml};
use crate::model::{Body, FolderFile, HttpRequestFile, new_id};
use crate::notes::NOTES_DIR;
use crate::project::{FOLDER_FILE, Project, REQUEST_EXT, item_kind};
use crate::tree::TreeNode;

/// What to bring over. Paths are `/`-separated: items relative to `http/`, notes relative to
/// `notes/`, environments by id.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TransferSelection {
    /// Request-like files and folders under `http/`.
    pub items: Vec<String>,
    pub environments: Vec<String>,
    /// Note files or note folders.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TransferOptions {
    /// Folder under the destination's `http/` that receives the items (empty = root).
    pub target_folder: String,
    /// Recreate the parent folders of individually selected requests (so inherited headers and
    /// auth keep working); same-named folders that already exist are reused.
    pub keep_folders: bool,
    /// Copy project variables the destination doesn't define yet.
    pub include_variables: bool,
    /// Move the originals to the OS trash afterwards (a move instead of a copy).
    pub remove_source: bool,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self { target_folder: String::new(), keep_folders: true, include_variables: true, remove_source: false }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    pub from: String,
    pub to: String,
}

/// An environment copied over, with the secret names whose values live in the source keychain.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MovedEnvironment {
    pub from: String,
    pub to: String,
    pub name: String,
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReport {
    pub items: Vec<Moved>,
    /// Request-like files written (including those inside copied folders).
    pub requests: usize,
    pub environments: Vec<MovedEnvironment>,
    pub notes: Vec<Moved>,
    pub variables_added: Vec<String>,
    /// `{{secret.NAME}}` names used by the copied requests and folders.
    pub secret_refs: Vec<String>,
    pub warnings: Vec<String>,
    /// First request written, to open it afterwards.
    pub first_request: Option<String>,
    /// Whether the originals were moved to the trash.
    pub removed_source: bool,
}

/// `name`, or `name (2)`, `name (3)`… — the first not in `taken` (lowercased).
pub fn unique_display(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(&name.to_lowercase()) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name} ({n})")).find(|c| !taken.contains(&c.to_lowercase())).unwrap_or_else(|| name.to_string())
}

/// Display names of the folders and request-like items directly inside `folder` (lowercased).
fn names_in(p: &Project, folder: &str) -> Result<(HashSet<String>, HashSet<String>)> {
    let dir = p.item_path(folder)?;
    let (mut folders, mut items) = (HashSet::new(), HashSet::new());
    let Ok(rd) = fs::read_dir(&dir) else { return Ok((folders, items)) };
    for e in rd.flatten() {
        let file = e.file_name().to_string_lossy().to_string();
        if file.starts_with('.') {
            continue;
        }
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let name = p.read_folder(&rel_join(folder, &file)).ok().and_then(|f| f.name).unwrap_or(file);
            folders.insert(name.to_lowercase());
        } else if let Some((ext, _)) = item_kind(&file) {
            let name = read_yaml::<serde_yaml_ng::Value>(&e.path())
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .unwrap_or_else(|| file.trim_end_matches(ext).to_string());
            items.insert(name.to_lowercase());
        }
    }
    Ok((folders, items))
}

fn all_ids(node: &TreeNode, out: &mut HashSet<String>) {
    match node {
        TreeNode::Folder { children, .. } => children.iter().for_each(|c| all_ids(c, out)),
        TreeNode::Request { id, .. } => {
            if !id.is_empty() {
                out.insert(id.clone());
            }
        }
    }
}

fn secret_refs_in(text: &str, out: &mut BTreeSet<String>) {
    for r in crate::vars::references(text) {
        if let Some(name) = r.strip_prefix("secret.") {
            out.insert(name.trim().to_string());
        }
    }
}

struct Ctx<'a> {
    from: &'a Project,
    to: &'a Project,
    /// Ids already used in the destination: copies that would collide get a fresh id.
    ids: HashSet<String>,
    secret_refs: BTreeSet<String>,
    report: TransferReport,
}

impl Ctx<'_> {
    /// Fresh id when `id` is empty or already taken in the destination.
    fn claim_id(&mut self, id: &str) -> String {
        let id = if id.is_empty() || self.ids.contains(id) { new_id() } else { id.to_string() };
        self.ids.insert(id.clone());
        id
    }

    fn warn_about_files(&mut self, req: &HttpRequestFile) {
        let relative = |p: &str| !p.trim().is_empty() && !p.contains("{{") && !Path::new(p).is_absolute();
        let mut files: Vec<&str> = Vec::new();
        match &req.body {
            Body::Binary { path, .. } if relative(path) => files.push(path),
            Body::Multipart { parts } => files.extend(parts.iter().filter_map(|p| p.file.as_deref()).filter(|f| relative(f))),
            _ => {}
        }
        if let Some(c) = &req.settings.client_cert {
            files.extend([Some(c.cert.as_str()), c.key.as_deref()].into_iter().flatten().filter(|f| relative(f)));
        }
        for f in files {
            self.report.warnings.push(format!("“{}” uses the project file {f}; copy it into the destination project too", req.name));
        }
    }

    /// Copies one request-like file into `dest_folder` under a unique display name.
    fn copy_item(&mut self, rel: &str, dest_folder: &str, taken: &mut HashSet<String>) -> Result<String> {
        let (ext, _) = item_kind(rel).ok_or_else(|| Error::InvalidPath(rel.to_string()))?;
        let src = self.from.item_path(rel)?;
        if !src.is_file() {
            return Err(Error::NotFound(rel.to_string()));
        }
        let text = fs::read_to_string(&src).at(&src)?;
        secret_refs_in(&text, &mut self.secret_refs);
        let mut v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).map_err(|e| Error::Parse { path: src.clone(), message: e.to_string() })?;
        let name = v.get("name").and_then(|n| n.as_str()).map(str::to_string).unwrap_or_else(|| rel_name(rel).trim_end_matches(ext).to_string());
        let display = unique_display(&name, taken);
        taken.insert(display.to_lowercase());
        let id = self.claim_id(v.get("id").and_then(|i| i.as_str()).unwrap_or_default());
        v["name"] = serde_yaml_ng::Value::String(display.clone());
        v["id"] = serde_yaml_ng::Value::String(id);

        let dir = self.to.item_path(dest_folder)?;
        fs::create_dir_all(&dir).at(&dir)?;
        let file = unique_name(&dir, &slugify(&display), ext);
        let new_rel = rel_join(dest_folder, &file);
        if ext == REQUEST_EXT {
            // Typed round trip keeps HTTP files in canonical key order.
            let req: HttpRequestFile = serde_yaml_ng::from_value(v).map_err(|e| Error::Parse { path: src.clone(), message: e.to_string() })?;
            self.warn_about_files(&req);
            self.to.save_request(&new_rel, &req)?;
            self.report.first_request.get_or_insert_with(|| new_rel.clone());
        } else {
            write_yaml(&self.to.item_path(&new_rel)?, &v)?;
        }
        self.to.order_insert(dest_folder, &file, None)?;
        self.report.requests += 1;
        Ok(new_rel)
    }

    /// Copies everything inside `src` into the (fresh, empty) directory `dst`.
    fn copy_tree(&mut self, src: &Path, dst: &Path, skip_folder_file: bool) -> Result<()> {
        fs::create_dir_all(dst).at(dst)?;
        for e in fs::read_dir(src).at(src)?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || (skip_folder_file && name == FOLDER_FILE) {
                continue;
            }
            let (from, to) = (e.path(), dst.join(&name));
            if from.is_dir() {
                self.copy_tree(&from, &to, false)?;
            } else if let Some((ext, _)) = item_kind(&name) {
                let text = fs::read_to_string(&from).at(&from)?;
                secret_refs_in(&text, &mut self.secret_refs);
                let mut v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).map_err(|e| Error::Parse { path: from.clone(), message: e.to_string() })?;
                let old = v.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();
                let id = self.claim_id(&old);
                if ext == REQUEST_EXT
                    && let Ok(req) = serde_yaml_ng::from_value::<HttpRequestFile>(v.clone())
                {
                    self.warn_about_files(&req);
                }
                if id == old {
                    fs::write(&to, text.as_bytes()).at(&to)?;
                } else {
                    v["id"] = serde_yaml_ng::Value::String(id);
                    write_yaml(&to, &v)?;
                }
                self.report.requests += 1;
            } else {
                if name == FOLDER_FILE
                    && let Ok(t) = fs::read_to_string(&from)
                {
                    secret_refs_in(&t, &mut self.secret_refs);
                }
                fs::copy(&from, &to).at(&from)?;
            }
        }
        Ok(())
    }

    /// Copies a folder (settings and contents) into `dest_folder` under a unique display name.
    fn copy_folder(&mut self, rel: &str, dest_folder: &str, taken: &mut HashSet<String>) -> Result<String> {
        let src = self.from.item_path(rel)?;
        if !src.is_dir() {
            return Err(Error::NotFound(rel.to_string()));
        }
        let meta = self.from.read_folder(rel)?;
        let name = meta.name.clone().unwrap_or_else(|| rel_name(rel).to_string());
        let display = unique_display(&name, taken);
        taken.insert(display.to_lowercase());
        let new_rel = self.to.create_folder(dest_folder, &display)?;
        let dst = self.to.item_path(&new_rel)?;
        self.copy_tree(&src, &dst, true)?;
        if src.join(FOLDER_FILE).is_file() {
            secret_refs_in(&fs::read_to_string(src.join(FOLDER_FILE)).unwrap_or_default(), &mut self.secret_refs);
            let folder = FolderFile { name: (slugify(&display) != display || meta.name.is_some()).then(|| display.clone()), ..meta };
            self.to.save_folder(&new_rel, &folder)?;
        }
        Ok(new_rel)
    }

    /// Makes sure the destination has the chain of folders `rel` lives in (below `base`), reusing
    /// same-named folders and copying the settings of ones it creates. Returns the folder path.
    fn ensure_parents(&mut self, rel: &str, base: &str) -> Result<String> {
        let mut src_path = String::new();
        let mut dest = base.to_string();
        let parent = rel_parent(rel);
        for seg in parent.split('/').filter(|s| !s.is_empty()) {
            src_path = rel_join(&src_path, seg);
            let meta = self.from.read_folder(&src_path)?;
            let name = meta.name.clone().unwrap_or_else(|| seg.to_string());
            let existing = existing_folder(self.to, &dest, &name)?;
            dest = match existing {
                Some(found) => found,
                None => {
                    let created = self.to.create_folder(&dest, &name)?;
                    let src_file = self.from.item_path(&src_path)?.join(FOLDER_FILE);
                    if src_file.is_file() {
                        secret_refs_in(&fs::read_to_string(&src_file).unwrap_or_default(), &mut self.secret_refs);
                        // Settings travel; the order list is rebuilt as items arrive.
                        self.to.save_folder(&created, &FolderFile { order: Vec::new(), ..meta })?;
                    }
                    created
                }
            };
        }
        Ok(dest)
    }

    fn copy_note(&mut self, rel: &str) -> Result<String> {
        let from_root = self.from.notes_root();
        let src = safe_join(&from_root, rel)?;
        if !src.exists() {
            return Err(Error::NotFound(format!("notes/{rel}")));
        }
        let parent = rel_parent(rel);
        let dest_dir = safe_join(&self.to.notes_root(), &parent)?;
        fs::create_dir_all(&dest_dir).at(&dest_dir)?;
        let name = rel_name(rel);
        let new_name = if src.is_dir() {
            unique_name(&dest_dir, name, "")
        } else {
            let ext = [".ipynb", ".md"].into_iter().find(|e| name.ends_with(e)).unwrap_or("");
            unique_name(&dest_dir, name.trim_end_matches(ext), ext)
        };
        let dest = dest_dir.join(&new_name);
        if src.is_dir() {
            copy_dir_all(&src, &dest)?;
        } else {
            fs::copy(&src, &dest).at(&src)?;
        }
        Ok(rel_join(&parent, &new_name))
    }
}

fn existing_folder(p: &Project, parent: &str, name: &str) -> Result<Option<String>> {
    let dir = p.item_path(parent)?;
    let Ok(rd) = fs::read_dir(&dir) else { return Ok(None) };
    for e in rd.flatten() {
        let file = e.file_name().to_string_lossy().to_string();
        if file.starts_with('.') || !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let rel = rel_join(parent, &file);
        let display = p.read_folder(&rel).ok().and_then(|f| f.name).unwrap_or_else(|| file.clone());
        if display.eq_ignore_ascii_case(name) {
            return Ok(Some(rel));
        }
    }
    Ok(None)
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).at(dst)?;
    for e in fs::read_dir(src).at(src)?.flatten() {
        let (from, to) = (e.path(), dst.join(e.file_name()));
        if from.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to).at(&from)?;
        }
    }
    Ok(())
}

/// Drops entries already covered by a selected ancestor folder (and duplicates).
fn outermost(paths: &[String]) -> Vec<String> {
    let mut list: Vec<String> = paths.iter().map(|p| normalize_rel(p)).filter(|p| !p.is_empty()).collect();
    list.sort();
    list.dedup();
    let all = list.clone();
    list.retain(|p| !all.iter().any(|a| a != p && p.starts_with(&format!("{a}/"))));
    list
}

/// Copies `selection` from `from` into `to`. With `remove_source`, the originals then go to the
/// OS trash. Fails before writing anything if a selected path doesn't exist.
pub fn transfer(from: &Project, to: &mut Project, selection: &TransferSelection, options: &TransferOptions) -> Result<TransferReport> {
    if from.root() == to.root() {
        return Err(Error::Invalid("pick a different project to move to".into()));
    }
    let items = outermost(&selection.items);
    let notes = outermost(&selection.notes);
    for rel in &items {
        if !from.item_path(rel)?.exists() {
            return Err(Error::NotFound(format!("http/{rel}")));
        }
    }
    for rel in &notes {
        if !safe_join(&from.notes_root(), rel)?.exists() {
            return Err(Error::NotFound(format!("{NOTES_DIR}/{rel}")));
        }
    }
    let envs: Vec<_> = selection.environments.iter().map(|id| from.read_environment(id).map(|f| (id.clone(), f))).collect::<Result<_>>()?;
    let target = normalize_rel(&options.target_folder);
    let target_dir = to.item_path(&target)?;
    if !target_dir.is_dir() {
        if target.is_empty() {
            fs::create_dir_all(&target_dir).at(&target_dir)?;
        } else {
            return Err(Error::NotFound(format!("http/{target}")));
        }
    }

    let mut ids = HashSet::new();
    all_ids(&to.tree()?, &mut ids);
    let mut cx = Ctx { from, to: &*to, ids, secret_refs: BTreeSet::new(), report: TransferReport::default() };

    // Requests and folders, in the source's display order.
    let order: Vec<String> = from.tree()?.request_paths_and_folders();
    let mut sorted = items.clone();
    sorted.sort_by_key(|p| order.iter().position(|o| o == p).unwrap_or(usize::MAX));
    for rel in &sorted {
        let dest_folder = if options.keep_folders { cx.ensure_parents(rel, &target)? } else { target.clone() };
        let (mut folders, mut names) = names_in(cx.to, &dest_folder)?;
        let new_rel = if cx.from.item_path(rel)?.is_dir() { cx.copy_folder(rel, &dest_folder, &mut folders)? } else { cx.copy_item(rel, &dest_folder, &mut names)? };
        cx.report.items.push(Moved { from: rel.clone(), to: new_rel });
    }

    // Environments: non-secret parts only; secret names come along, values stay in the keychain.
    if !envs.is_empty() {
        let mut taken: HashSet<String> = cx.to.environments()?.iter().map(|e| e.file.name.to_lowercase()).collect();
        for (id, file) in envs {
            let name = unique_display(file.name.trim(), &taken);
            taken.insert(name.to_lowercase());
            let new_id = cx.to.create_environment(&name)?;
            let secrets = file.secrets.clone();
            cx.to.save_environment(&new_id, &crate::model::EnvironmentFile { name: name.clone(), ..file })?;
            cx.report.environments.push(MovedEnvironment { from: id, to: new_id, name, secrets });
        }
    }

    for rel in &notes {
        let new_rel = cx.copy_note(rel)?;
        cx.report.notes.push(Moved { from: rel.clone(), to: new_rel });
    }
    let mut report = std::mem::take(&mut cx.report);
    report.secret_refs = cx.secret_refs.into_iter().collect();

    if options.include_variables {
        let mut changed = false;
        for (k, v) in &from.manifest.variables {
            if !to.manifest.variables.contains_key(k) {
                to.manifest.variables.insert(k.clone(), v.clone());
                report.variables_added.push(k.clone());
                changed = true;
            }
        }
        if changed {
            to.save_manifest()?;
        }
    }

    if options.remove_source {
        for rel in &items {
            from.delete_item(rel)?;
        }
        for e in &report.environments {
            from.delete_environment(&e.from)?;
        }
        for rel in &notes {
            from.delete_note(rel)?;
        }
        report.removed_source = true;
    }
    Ok(report)
}

impl TreeNode {
    /// Every folder and item path below this node, in display order (folders before their contents).
    fn request_paths_and_folders(&self) -> Vec<String> {
        fn walk(n: &TreeNode, out: &mut Vec<String>) {
            match n {
                TreeNode::Folder { path, children, .. } => {
                    if !path.is_empty() {
                        out.push(path.clone());
                    }
                    children.iter().for_each(|c| walk(c, out));
                }
                TreeNode::Request { path, .. } => out.push(path.clone()),
            }
        }
        let mut out = Vec::new();
        walk(self, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Auth, EnvironmentFile, KeyValue};

    fn two_projects() -> (tempfile::TempDir, tempfile::TempDir, Project, Project) {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let scratch = Project::init(a.path(), "Scratch pad").unwrap();
        let dest = Project::init(b.path(), "Shop").unwrap();
        (a, b, scratch, dest)
    }

    fn req(name: &str, url: &str) -> HttpRequestFile {
        let mut r = HttpRequestFile::new(name);
        r.url = url.into();
        r
    }

    #[test]
    fn copies_requests_folders_envs_notes_with_unique_names() {
        let (_a, _b, scratch, mut dest) = two_projects();
        // Source: a loose request, a folder with settings + a nested request, an env, a note.
        let mut login = req("Login", "{{baseUrl}}/login");
        login.headers.push(KeyValue::new("Authorization", "Bearer {{secret.TOKEN}}"));
        let login_rel = scratch.create_request("", login).unwrap();
        let folder = scratch.create_folder("", "Orders API").unwrap();
        scratch
            .save_folder(&folder, &FolderFile { name: Some("Orders API".into()), auth: Auth::Bearer { token: "{{secret.ORDERS}}".into(), prefix: None }, ..Default::default() })
            .unwrap();
        let list = scratch.create_request(&folder, req("List orders", "{{baseUrl}}/orders")).unwrap();
        let mut env = EnvironmentFile::new("Local");
        env.variables.insert("baseUrl".into(), "http://localhost:3000".into());
        env.secrets.push("TOKEN".into());
        scratch.save_environment("local", &env).unwrap();
        let note = scratch.create_note("", "Ideas").unwrap();
        let mut scratch_m = scratch.clone();
        scratch_m.manifest.variables.insert("apiVersion".into(), "v2".into());
        scratch_m.save_manifest().unwrap();

        // Destination already has a "Login" request, a "Local" environment and an ideas note.
        let existing_login = dest.create_request("", req("Login", "https://x")).unwrap();
        dest.create_note("", "Ideas").unwrap();
        let dest_login_id = dest.read_request(&existing_login).unwrap().id;
        // Same id as the source request → the copy must get a fresh one.
        let mut clash = scratch.read_request(&login_rel).unwrap();
        clash.id = dest_login_id.clone();
        scratch.save_request(&login_rel, &clash).unwrap();

        let sel = TransferSelection { items: vec![login_rel.clone(), folder.clone(), list.clone()], environments: vec!["local".into()], notes: vec![note.clone()] };
        let report = transfer(&scratch_m, &mut dest, &sel, &TransferOptions::default()).unwrap();

        assert_eq!(report.items.len(), 2, "the nested request is covered by its folder");
        assert_eq!(report.requests, 2);
        let login_to = &report.items.iter().find(|m| m.from == login_rel).unwrap().to;
        assert_eq!(login_to, "login-2.req.yaml");
        let copied = dest.read_request(login_to).unwrap();
        assert_eq!(copied.name, "Login (2)");
        assert_ne!(copied.id, dest_login_id);
        let folder_to = &report.items.iter().find(|m| m.from == folder).unwrap().to;
        assert_eq!(folder_to, "orders-api");
        let f = dest.read_folder(folder_to).unwrap();
        assert_eq!(f.name.as_deref(), Some("Orders API"));
        assert!(matches!(f.auth, Auth::Bearer { .. }));
        assert_eq!(dest.read_request("orders-api/list-orders.req.yaml").unwrap().url, "{{baseUrl}}/orders");
        assert_eq!(report.secret_refs, vec!["ORDERS".to_string(), "TOKEN".to_string()]);

        assert_eq!(report.environments.len(), 1);
        let e = &report.environments[0];
        assert_eq!((e.name.as_str(), e.to.as_str()), ("Local (2)", "local-2"));
        assert_eq!(e.secrets, vec!["TOKEN".to_string()]);
        let copied_env = dest.read_environment("local-2").unwrap();
        assert_eq!(copied_env.variables.get("baseUrl").map(String::as_str), Some("http://localhost:3000"));

        assert_eq!(report.notes, vec![Moved { from: "ideas.md".into(), to: "ideas-2.md".into() }]);
        assert_eq!(report.variables_added, vec!["apiVersion".to_string()]);
        assert_eq!(Project::open(dest.root()).unwrap().manifest.variables.get("apiVersion").map(String::as_str), Some("v2"));
        assert_eq!(report.first_request.as_deref(), Some("login-2.req.yaml"));
        // A copy leaves the source alone.
        assert!(scratch.read_request(&login_rel).is_ok());
    }

    #[test]
    fn keeps_parent_folders() {
        let (_a, _b, scratch, mut dest) = two_projects();
        let folder = scratch.create_folder("", "Auth").unwrap();
        scratch.save_folder(&folder, &FolderFile { headers: vec![KeyValue::new("X-Team", "core")], ..Default::default() }).unwrap();
        let a = scratch.create_request(&folder, req("Refresh", "/refresh")).unwrap();
        let b = scratch.create_request(&folder, req("Logout", "/logout")).unwrap();
        // The destination already has an "auth" folder: it is reused for parents.
        dest.create_folder("", "auth").unwrap();

        // (`remove_source` only adds Project::delete_* calls, which send files to the OS trash —
        // not exercised here so test runs don't fill the trash.)
        let sel = TransferSelection { items: vec![a.clone()], ..Default::default() };
        let report = transfer(&scratch, &mut dest, &sel, &TransferOptions::default()).unwrap();
        assert_eq!(report.items[0].to, "auth/refresh.req.yaml");
        assert!(!report.removed_source);
        assert!(scratch.read_request(&a).is_ok() && scratch.read_request(&b).is_ok());
        assert_eq!(dest.read_folder("auth").unwrap().headers.len(), 0, "an existing folder keeps its own settings");

        // Without a matching folder, the chain is created with the source's settings.
        let (_c, _d, scratch2, mut dest2) = two_projects();
        let f2 = scratch2.create_folder("", "Billing").unwrap();
        scratch2.save_folder(&f2, &FolderFile { headers: vec![KeyValue::new("X-Team", "pay")], ..Default::default() }).unwrap();
        let r2 = scratch2.create_request(&f2, req("Charge", "/charge")).unwrap();
        let report = transfer(&scratch2, &mut dest2, &TransferSelection { items: vec![r2], ..Default::default() }, &TransferOptions::default()).unwrap();
        assert_eq!(report.items[0].to, "billing/charge.req.yaml");
        assert_eq!(dest2.read_folder("billing").unwrap().headers[0].value, "pay");
        assert_eq!(dest2.read_folder("billing").unwrap().order, vec!["charge.req.yaml"]);

        // Flat copy into a target folder.
        let target = dest2.create_folder("", "Inbox").unwrap();
        let r3 = scratch2.create_request("", req("Ping", "/ping")).unwrap();
        let opts = TransferOptions { target_folder: target.clone(), keep_folders: false, ..Default::default() };
        let report = transfer(&scratch2, &mut dest2, &TransferSelection { items: vec![r3], ..Default::default() }, &opts).unwrap();
        assert_eq!(report.items[0].to, "inbox/ping.req.yaml");
    }

    #[test]
    fn rejects_missing_paths_and_same_project() {
        let (_a, _b, scratch, mut dest) = two_projects();
        let sel = TransferSelection { items: vec!["nope.req.yaml".into()], ..Default::default() };
        assert!(transfer(&scratch, &mut dest, &sel, &TransferOptions::default()).is_err());
        assert!(dest.tree().unwrap().request_paths().is_empty(), "nothing written");
        let mut same = scratch.clone();
        assert!(transfer(&scratch, &mut same, &TransferSelection::default(), &TransferOptions::default()).is_err());
        assert_eq!(unique_display("A", &["a".to_string(), "a (2)".to_string()].into_iter().collect()), "A (3)");
    }
}
