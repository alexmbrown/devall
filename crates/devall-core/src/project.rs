//! A project is any folder with a `devall.yaml`. All paths passed in and out of this module are
//! `/`-separated and relative to the project's `http/` folder (requests and folders) or are
//! environment ids (the file stem in `environments/`).

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;

use crate::error::{Error, IoContext, Result};
use crate::fsutil::{
    ensure_gitignore, normalize_rel, read_yaml, rel_join, rel_name, rel_parent, safe_join, slugify, unique_name, write_yaml,
};
use crate::model::*;

pub const MANIFEST: &str = "devall.yaml";
pub const HTTP_DIR: &str = "http";
pub const ENV_DIR: &str = "environments";
/// Where older versions kept local state inside the project; now only migrated away from
/// (see [`crate::local`]) and still git-ignored for old clones.
pub const LOCAL_DIR: &str = ".devall";
pub const FOLDER_FILE: &str = "_folder.yaml";
pub const REQUEST_EXT: &str = ".req.yaml";
/// Committed response snapshots (`http/__snapshots__/**/*.snap.json`); hidden from the request tree.
pub const SNAPSHOT_DIR: &str = "__snapshots__";

/// Request-like files under `http/`: (extension, protocol). HTTP requests are typed here; the
/// other protocols' models live in their crates and use the generic `*_item` helpers.
pub const ITEM_KINDS: &[(&str, &str)] =
    &[(".req.yaml", "http"), (".ws.yaml", "ws"), (".grpc.yaml", "grpc"), (".mqtt.yaml", "mqtt"), (".sio.yaml", "socketio")];

/// `(extension, protocol)` when `file_name` is a request-like file.
pub fn item_kind(file_name: &str) -> Option<(&'static str, &'static str)> {
    ITEM_KINDS.iter().find(|(ext, _)| file_name.ends_with(ext)).copied()
}
pub const ENV_EXT: &str = ".env.yaml";

/// Lines every project's `.gitignore` must contain.
pub const GITIGNORE_ENTRIES: &[&str] = &[".devall/", ".env", ".env.*"];

#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
    pub manifest: ProjectManifest,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentEntry {
    /// File stem, e.g. `staging` for `environments/staging.env.yaml`.
    pub id: String,
    pub file: EnvironmentFile,
}

/// A request with folder inheritance applied: headers merged, auth resolved, folder variables
/// collected nearest-first.
#[derive(Debug, Clone)]
pub struct EffectiveRequest {
    pub request: HttpRequestFile,
    pub folder_variables: Vec<IndexMap<String, String>>,
}

impl Project {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let manifest_path = root.join(MANIFEST);
        if !manifest_path.is_file() {
            return Err(Error::NotAProject(root));
        }
        let manifest: ProjectManifest = read_yaml(&manifest_path)?;
        Ok(Self { root, manifest })
    }

    /// Creates a new project in `root` (which may already contain other files, e.g. a git repo).
    pub fn init(root: impl AsRef<Path>, name: &str) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        if root.join(MANIFEST).exists() {
            return Err(Error::AlreadyExists(root.join(MANIFEST).display().to_string()));
        }
        fs::create_dir_all(root.join(HTTP_DIR)).at(&root)?;
        fs::create_dir_all(root.join(ENV_DIR)).at(&root)?;
        let manifest = ProjectManifest {
            version: FORMAT_VERSION,
            id: new_id(),
            name: name.trim().to_string(),
            description: None,
            variables: IndexMap::new(),
            extra: Extra::new(),
        };
        write_yaml(&root.join(MANIFEST), &manifest)?;
        let mut local = EnvironmentFile::new("Local");
        local.variables.insert("baseUrl".into(), "http://localhost:8080".into());
        write_yaml(&root.join(ENV_DIR).join(format!("local{ENV_EXT}")), &local)?;
        ensure_gitignore(&root, GITIGNORE_ENTRIES)?;
        Ok(Self { root, manifest })
    }

    /// An in-memory project rooted at `root` (relative body/certificate paths resolve there)
    /// that writes nothing: for running loose `.http` / `.hurl` files. `id` is derived from the
    /// name so keychain lookups never hit a real project's secrets.
    pub fn detached(root: impl AsRef<Path>, name: &str, variables: IndexMap<String, String>) -> Self {
        let manifest = ProjectManifest { version: FORMAT_VERSION, id: format!("detached:{name}"), name: name.to_string(), description: None, variables, extra: Extra::new() };
        Self { root: root.as_ref().to_path_buf(), manifest }
    }

    pub fn open_or_init(root: impl AsRef<Path>, name: &str) -> Result<Self> {
        match Self::open(&root) {
            Err(Error::NotAProject(_)) => Self::init(root, name),
            other => other,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn http_root(&self) -> PathBuf {
        self.root.join(HTTP_DIR)
    }

    /// The project's on-device data folder (history, caches, run summaries) — outside the repo,
    /// see [`crate::local::project_dir`].
    pub fn local_dir(&self) -> PathBuf {
        crate::local::project_dir(&self.root)
    }

    pub fn save_manifest(&self) -> Result<()> {
        write_yaml(&self.root.join(MANIFEST), &self.manifest)
    }

    fn http_path(&self, rel: &str) -> Result<PathBuf> {
        safe_join(&self.http_root(), rel)
    }

    // ---------- requests ----------

    pub fn read_request(&self, rel: &str) -> Result<HttpRequestFile> {
        let rel = normalize_rel(rel);
        if !rel.ends_with(REQUEST_EXT) {
            return Err(Error::InvalidPath(rel));
        }
        let path = self.http_path(&rel)?;
        if !path.is_file() {
            return Err(Error::NotFound(rel));
        }
        read_yaml(&path)
    }

    pub fn save_request(&self, rel: &str, req: &HttpRequestFile) -> Result<()> {
        let rel = normalize_rel(rel);
        if !rel.ends_with(REQUEST_EXT) {
            return Err(Error::InvalidPath(rel));
        }
        let mut req = req.clone();
        if req.id.is_empty() {
            req.id = new_id();
        }
        req.version = FORMAT_VERSION;
        write_yaml(&self.http_path(&rel)?, &req)
    }

    /// Creates `<folder>/<slug(name)>.req.yaml` and appends it to the folder order.
    pub fn create_request(&self, folder_rel: &str, template: HttpRequestFile) -> Result<String> {
        let folder_rel = normalize_rel(folder_rel);
        let dir = self.http_path(&folder_rel)?;
        if !dir.is_dir() {
            fs::create_dir_all(&dir).at(&dir)?;
        }
        let file = unique_name(&dir, &slugify(&template.name), REQUEST_EXT);
        let rel = rel_join(&folder_rel, &file);
        let mut req = template;
        req.id = new_id();
        self.save_request(&rel, &req)?;
        self.order_insert(&folder_rel, &file, None)?;
        Ok(rel)
    }

    /// Duplicates any request-like file (HTTP, WebSocket, gRPC, …) next to the original.
    pub fn duplicate_request(&self, rel: &str) -> Result<String> {
        let rel = normalize_rel(rel);
        let (ext, _) = item_kind(&rel).ok_or_else(|| Error::InvalidPath(rel.clone()))?;
        let mut v: serde_yaml_ng::Value = read_yaml(&self.http_path(&rel)?)?;
        let name = format!("{} copy", v.get("name").and_then(|n| n.as_str()).unwrap_or("Request"));
        v["name"] = serde_yaml_ng::Value::String(name.clone());
        v["id"] = serde_yaml_ng::Value::String(new_id());
        let parent = rel_parent(&rel);
        let dir = self.http_path(&parent)?;
        let file = unique_name(&dir, &slugify(&name), ext);
        let new_rel = rel_join(&parent, &file);
        write_yaml(&self.http_path(&new_rel)?, &v)?;
        let after = self.read_folder(&parent)?.order.iter().position(|n| n == rel_name(&rel)).map(|i| i + 1);
        self.order_insert(&parent, &file, after)?;
        Ok(new_rel)
    }

    // ---------- generic request-like items (WebSocket, gRPC, MQTT, Socket.IO) ----------

    /// Absolute path of an item under `http/` (path-traversal safe).
    pub fn item_path(&self, rel: &str) -> Result<PathBuf> {
        self.http_path(rel)
    }

    pub fn read_item<T: serde::de::DeserializeOwned>(&self, rel: &str) -> Result<T> {
        let rel = normalize_rel(rel);
        if item_kind(&rel).is_none() {
            return Err(Error::InvalidPath(rel));
        }
        let p = self.http_path(&rel)?;
        if !p.is_file() {
            return Err(Error::NotFound(rel));
        }
        read_yaml(&p)
    }

    pub fn save_item<T: Serialize>(&self, rel: &str, value: &T) -> Result<()> {
        let rel = normalize_rel(rel);
        if item_kind(&rel).is_none() {
            return Err(Error::InvalidPath(rel));
        }
        write_yaml(&self.http_path(&rel)?, value)
    }

    /// Creates `<folder>/<slug(name)><ext>` and appends it to the folder order.
    pub fn create_item<T: Serialize>(&self, folder_rel: &str, name: &str, ext: &str, value: &T) -> Result<String> {
        if !ITEM_KINDS.iter().any(|(e, _)| *e == ext) {
            return Err(Error::Invalid(format!("unknown item type {ext}")));
        }
        let folder_rel = normalize_rel(folder_rel);
        let dir = self.http_path(&folder_rel)?;
        fs::create_dir_all(&dir).at(&dir)?;
        let file = unique_name(&dir, &slugify(name), ext);
        let rel = rel_join(&folder_rel, &file);
        write_yaml(&self.http_path(&rel)?, value)?;
        self.order_insert(&folder_rel, &file, None)?;
        Ok(rel)
    }

    // ---------- folders ----------

    pub fn read_folder(&self, rel: &str) -> Result<FolderFile> {
        let path = self.http_path(rel)?.join(FOLDER_FILE);
        if !path.is_file() {
            return Ok(FolderFile { version: FORMAT_VERSION, ..Default::default() });
        }
        read_yaml(&path)
    }

    pub fn save_folder(&self, rel: &str, folder: &FolderFile) -> Result<()> {
        let dir = self.http_path(rel)?;
        if !dir.is_dir() {
            return Err(Error::NotFound(normalize_rel(rel)));
        }
        let mut folder = folder.clone();
        folder.version = FORMAT_VERSION;
        write_yaml(&dir.join(FOLDER_FILE), &folder)
    }

    pub fn create_folder(&self, parent_rel: &str, name: &str) -> Result<String> {
        let parent_rel = normalize_rel(parent_rel);
        let parent = self.http_path(&parent_rel)?;
        let dir_name = unique_name(&parent, &slugify(name), "");
        let rel = rel_join(&parent_rel, &dir_name);
        let dir = parent.join(&dir_name);
        fs::create_dir_all(&dir).at(&dir)?;
        if slugify(name) != name {
            self.save_folder(&rel, &FolderFile { name: Some(name.trim().to_string()), ..Default::default() })?;
        }
        self.order_insert(&parent_rel, &dir_name, None)?;
        Ok(rel)
    }

    // ---------- tree edits ----------

    /// Renames a request or folder: updates its display name, moves the file to the new slug and
    /// keeps its position in the parent order. Returns the new relative path.
    pub fn rename_item(&self, rel: &str, new_name: &str) -> Result<String> {
        let rel = normalize_rel(rel);
        let new_name = new_name.trim();
        if rel.is_empty() || new_name.is_empty() {
            return Err(Error::Invalid("name cannot be empty".into()));
        }
        let parent = rel_parent(&rel);
        let parent_dir = self.http_path(&parent)?;
        let old_file = rel_name(&rel).to_string();
        let path = self.http_path(&rel)?;
        let new_file = if let Some((ext, _)) = item_kind(&old_file) {
            let mut v: serde_yaml_ng::Value = read_yaml(&path)?;
            v["name"] = serde_yaml_ng::Value::String(new_name.to_string());
            let slug = slugify(new_name);
            let file = if format!("{slug}{ext}") == old_file { old_file.clone() } else { unique_name(&parent_dir, &slug, ext) };
            let new_rel = rel_join(&parent, &file);
            if ext == REQUEST_EXT {
                // Typed round trip keeps HTTP files in canonical key order.
                let req: HttpRequestFile = serde_yaml_ng::from_value(v).map_err(|e| Error::Parse { path: path.clone(), message: e.to_string() })?;
                self.save_request(&new_rel, &req)?;
            } else {
                write_yaml(&self.http_path(&new_rel)?, &v)?;
            }
            if file != old_file {
                fs::remove_file(&path).at(&path)?;
            }
            file
        } else if path.is_dir() {
            let slug = slugify(new_name);
            let file = if slug == old_file { old_file.clone() } else { unique_name(&parent_dir, &slug, "") };
            if file != old_file {
                fs::rename(&path, parent_dir.join(&file)).at(&path)?;
            }
            let new_rel = rel_join(&parent, &file);
            let mut folder = self.read_folder(&new_rel)?;
            folder.name = (slug != new_name).then(|| new_name.to_string());
            if folder.name.is_some() || self.http_path(&new_rel)?.join(FOLDER_FILE).exists() {
                self.save_folder(&new_rel, &folder)?;
            }
            file
        } else {
            return Err(Error::NotFound(rel));
        };
        if new_file != old_file {
            let mut f = self.read_folder(&parent)?;
            if let Some(slot) = f.order.iter_mut().find(|n| **n == old_file) {
                *slot = new_file.clone();
                self.save_folder_if_exists_or_needed(&parent, &f)?;
            }
        }
        Ok(rel_join(&parent, &new_file))
    }

    /// Moves a request/folder to the OS trash (recoverable) and drops it from the parent order.
    pub fn delete_item(&self, rel: &str) -> Result<()> {
        let rel = normalize_rel(rel);
        if rel.is_empty() {
            return Err(Error::Invalid("cannot delete the project root".into()));
        }
        let path = self.http_path(&rel)?;
        if !path.exists() {
            return Err(Error::NotFound(rel));
        }
        trash::delete(&path).map_err(|e| Error::Io { path: path.clone(), source: std::io::Error::other(e.to_string()) })?;
        let parent = rel_parent(&rel);
        let mut f = self.read_folder(&parent)?;
        let before = f.order.len();
        f.order.retain(|n| n != rel_name(&rel));
        if f.order.len() != before {
            self.save_folder_if_exists_or_needed(&parent, &f)?;
        }
        Ok(())
    }

    /// Moves `rel` into `target_folder` at `index` (or the end). Also used to reorder within a folder.
    pub fn move_item(&self, rel: &str, target_folder: &str, index: Option<usize>) -> Result<String> {
        let rel = normalize_rel(rel);
        let target_folder = normalize_rel(target_folder);
        if rel.is_empty() {
            return Err(Error::Invalid("cannot move the project root".into()));
        }
        if target_folder == rel || target_folder.starts_with(&format!("{rel}/")) {
            return Err(Error::Invalid("cannot move a folder into itself".into()));
        }
        let src = self.http_path(&rel)?;
        if !src.exists() {
            return Err(Error::NotFound(rel));
        }
        let parent = rel_parent(&rel);
        let name = rel_name(&rel).to_string();
        let target_dir = self.http_path(&target_folder)?;
        if !target_dir.is_dir() {
            return Err(Error::NotFound(target_folder));
        }
        // Remove from the old order first (also handles reordering within the same folder).
        let mut old = self.read_folder(&parent)?;
        old.order.retain(|n| *n != name);
        self.save_folder_if_exists_or_needed(&parent, &old)?;

        let final_name = if parent == target_folder {
            name
        } else {
            let (stem, suffix) = match item_kind(&name) {
                Some((ext, _)) => (name.trim_end_matches(ext).to_string(), ext),
                None => (name.clone(), ""),
            };
            let n = unique_name(&target_dir, &stem, suffix);
            fs::rename(&src, target_dir.join(&n)).at(&src)?;
            n
        };
        self.order_insert(&target_folder, &final_name, index)?;
        Ok(rel_join(&target_folder, &final_name))
    }

    pub(crate) fn order_insert(&self, folder_rel: &str, child: &str, index: Option<usize>) -> Result<()> {
        let mut f = self.read_folder(folder_rel)?;
        // Materialize the current visual order so inserting at an index is meaningful.
        let current = crate::tree::ordered_children(&self.http_path(folder_rel)?, &f.order)?;
        let mut order: Vec<String> = current.into_iter().filter(|n| n != child).collect();
        let at = index.unwrap_or(order.len()).min(order.len());
        order.insert(at, child.to_string());
        f.order = order;
        self.save_folder(folder_rel, &f)
    }

    fn save_folder_if_exists_or_needed(&self, rel: &str, f: &FolderFile) -> Result<()> {
        let path = self.http_path(rel)?.join(FOLDER_FILE);
        if path.exists() || *f != FolderFile::default() {
            self.save_folder(rel, f)?;
        }
        Ok(())
    }

    /// Applies folder inheritance (nearest folder wins) to a request located at `rel`.
    /// `rel` may be empty for an unsaved request, which then only inherits from the root folder.
    pub fn effective_request(&self, rel: &str, req: &HttpRequestFile) -> Result<EffectiveRequest> {
        let rel = normalize_rel(rel);
        let mut chain = Vec::new(); // nearest first
        let mut folder = rel_parent(&rel);
        loop {
            chain.push(self.read_folder(&folder)?);
            if folder.is_empty() {
                break;
            }
            folder = rel_parent(&folder);
        }
        let mut out = req.clone();
        let mut auth = req.auth.clone();
        for f in &chain {
            for h in f.headers.iter().filter(|h| h.enabled) {
                if !out.headers.iter().any(|x| x.name.eq_ignore_ascii_case(&h.name)) {
                    out.headers.push(h.clone());
                }
            }
            if auth.is_inherit() && !f.auth.is_inherit() {
                auth = f.auth.clone();
            }
        }
        out.auth = if auth.is_inherit() { Auth::None } else { auth };
        Ok(EffectiveRequest { request: out, folder_variables: chain.into_iter().map(|f| f.variables).collect() })
    }

    // ---------- environments ----------

    fn env_path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
            return Err(Error::InvalidPath(id.to_string()));
        }
        Ok(self.root.join(ENV_DIR).join(format!("{id}{ENV_EXT}")))
    }

    pub fn environments(&self) -> Result<Vec<EnvironmentEntry>> {
        let dir = self.root.join(ENV_DIR);
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&dir) else { return Ok(out) };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(id) = name.strip_suffix(ENV_EXT) {
                let file: EnvironmentFile = read_yaml(&entry.path())?;
                out.push(EnvironmentEntry { id: id.to_string(), file });
            }
        }
        out.sort_by_key(|e| e.file.name.to_lowercase());
        Ok(out)
    }

    pub fn read_environment(&self, id: &str) -> Result<EnvironmentFile> {
        let path = self.env_path(id)?;
        if !path.is_file() {
            return Err(Error::NotFound(format!("environment {id}")));
        }
        read_yaml(&path)
    }

    pub fn save_environment(&self, id: &str, env: &EnvironmentFile) -> Result<()> {
        let mut env = env.clone();
        env.version = FORMAT_VERSION;
        write_yaml(&self.env_path(id)?, &env)
    }

    pub fn create_environment(&self, name: &str) -> Result<String> {
        let dir = self.root.join(ENV_DIR);
        fs::create_dir_all(&dir).at(&dir)?;
        let file = unique_name(&dir, &slugify(name), ENV_EXT);
        let id = file.trim_end_matches(ENV_EXT).to_string();
        self.save_environment(&id, &EnvironmentFile::new(name.trim()))?;
        Ok(id)
    }

    pub fn delete_environment(&self, id: &str) -> Result<()> {
        let path = self.env_path(id)?;
        if !path.exists() {
            return Err(Error::NotFound(format!("environment {id}")));
        }
        trash::delete(&path).map_err(|e| Error::Io { path: path.clone(), source: std::io::Error::other(e.to_string()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::TreeNode;

    fn temp_project() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Demo").unwrap();
        (dir, p)
    }

    #[test]
    fn init_writes_manifest_env_and_gitignore() {
        let (dir, p) = temp_project();
        assert!(dir.path().join(MANIFEST).is_file());
        assert_eq!(p.manifest.name, "Demo");
        let envs = p.environments().unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "local");
        let gi = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gi.contains(".devall/") && gi.contains(".env"));
        assert!(Project::init(dir.path(), "again").is_err());
        assert_eq!(Project::open(dir.path()).unwrap().manifest.id, p.manifest.id);
    }

    #[test]
    fn request_round_trip_is_stable() {
        let (_d, p) = temp_project();
        let mut req = HttpRequestFile::new("List users");
        req.url = "{{baseUrl}}/users".into();
        req.params.push(KeyValue::new("page", "1"));
        req.body = Body::Json { content: "{\n  \"a\": 1\n}".into() };
        let rel = p.create_request("", req).unwrap();
        assert_eq!(rel, "list-users.req.yaml");
        let back = p.read_request(&rel).unwrap();
        assert_eq!(back.url, "{{baseUrl}}/users");
        let text = fs::read_to_string(p.http_root().join(&rel)).unwrap();
        // Defaults stay out of the file; multi-line bodies are block scalars.
        assert!(!text.contains("settings"), "{text}");
        assert!(!text.contains("enabled"), "{text}");
        assert!(text.contains("content: |"), "{text}");
        p.save_request(&rel, &back).unwrap();
        assert_eq!(fs::read_to_string(p.http_root().join(&rel)).unwrap(), text);
    }

    #[test]
    fn unknown_keys_survive() {
        let (_d, p) = temp_project();
        fs::write(p.http_root().join("x.req.yaml"), "name: X\nfutureField:\n  a: 1\n").unwrap();
        let r = p.read_request("x.req.yaml").unwrap();
        p.save_request("x.req.yaml", &r).unwrap();
        let text = fs::read_to_string(p.http_root().join("x.req.yaml")).unwrap();
        assert!(text.contains("futureField"), "{text}");
    }

    #[test]
    fn order_rename_move_delete() {
        let (_d, p) = temp_project();
        let a = p.create_request("", HttpRequestFile::new("A")).unwrap();
        let b = p.create_request("", HttpRequestFile::new("B")).unwrap();
        let folder = p.create_folder("", "Auth Flows").unwrap();
        assert_eq!(folder, "auth-flows");
        assert_eq!(p.read_folder("auth-flows").unwrap().name.as_deref(), Some("Auth Flows"));
        assert_eq!(p.read_folder("").unwrap().order, vec!["a.req.yaml", "b.req.yaml", "auth-flows"]);

        // Reorder: B first.
        p.move_item(&b, "", Some(0)).unwrap();
        assert_eq!(p.read_folder("").unwrap().order, vec!["b.req.yaml", "a.req.yaml", "auth-flows"]);

        // Rename keeps position.
        let a2 = p.rename_item(&a, "Alpha").unwrap();
        assert_eq!(a2, "alpha.req.yaml");
        assert_eq!(p.read_folder("").unwrap().order, vec!["b.req.yaml", "alpha.req.yaml", "auth-flows"]);
        assert_eq!(p.read_request(&a2).unwrap().name, "Alpha");

        // Move into folder.
        let moved = p.move_item(&a2, &folder, None).unwrap();
        assert_eq!(moved, "auth-flows/alpha.req.yaml");
        assert_eq!(p.read_folder("").unwrap().order, vec!["b.req.yaml", "auth-flows"]);

        // A folder can't move into itself.
        assert!(p.move_item(&folder, &folder, None).is_err());

        let tree = p.tree().unwrap();
        let TreeNode::Folder { children, .. } = tree else { panic!() };
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn inheritance_merges_headers_and_auth() {
        let (_d, p) = temp_project();
        let folder = p.create_folder("", "api").unwrap();
        p.save_folder(
            &folder,
            &FolderFile {
                headers: vec![KeyValue::new("X-Team", "core"), KeyValue::new("Accept", "text/plain")],
                auth: Auth::Bearer { token: "{{secret.TOKEN}}".into(), prefix: None },
                variables: [("v".to_string(), "folder".to_string())].into_iter().collect(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut req = HttpRequestFile::new("r");
        req.headers.push(KeyValue::new("accept", "application/json"));
        let rel = p.create_request(&folder, req.clone()).unwrap();
        let eff = p.effective_request(&rel, &p.read_request(&rel).unwrap()).unwrap();
        assert_eq!(eff.request.headers.len(), 2);
        assert!(eff.request.headers.iter().any(|h| h.name == "X-Team"));
        assert!(matches!(eff.request.auth, Auth::Bearer { .. }));
        assert_eq!(eff.folder_variables[0].get("v").map(String::as_str), Some("folder"));

        // Unsaved request at root: inherits only the root folder → no auth.
        let eff = p.effective_request("", &req).unwrap();
        assert_eq!(eff.request.auth, Auth::None);
    }

    #[test]
    fn environments_crud() {
        let (_d, p) = temp_project();
        let id = p.create_environment("Staging EU").unwrap();
        assert_eq!(id, "staging-eu");
        let mut env = p.read_environment(&id).unwrap();
        env.secrets.push("API_TOKEN".into());
        env.protected = true;
        p.save_environment(&id, &env).unwrap();
        assert_eq!(p.read_environment(&id).unwrap(), env);
        assert!(p.read_environment("../x").is_err());
        assert_eq!(p.environments().unwrap().len(), 2);
    }
}
