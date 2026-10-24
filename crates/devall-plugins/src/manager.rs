//! Installed (user) and project plugins, their approvals, and calls into their contributions.
//!
//! - User plugins live in `<app data>/plugins/<id>/` and are installed from a folder or a `.zip`
//!   after the user reviewed the manifest's capabilities.
//! - Project plugins live in `<project>/plugins/<id>/` (committed with the repo). They never run
//!   until the user trusts them on this machine; the trust is pinned to the module's SHA-256 and
//!   the capabilities as approved, so a `git pull` that changes either asks again.
//!
//! Approvals live in `<app data>/plugins.json` — never in a project, so a repository cannot
//! pre-approve itself. When a project plugin has the same id as an installed one, the installed
//! one wins and the project copy is reported as shadowed.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::*;
use crate::error::{PluginError, Result};
use crate::manifest::{ArgSpec, Capabilities, HookEvent, MANIFEST_FILE, Manifest, host_allowed};
use crate::runtime::{CallContext, CallOutcome, CompiledPlugin, HostServices, Limits, LogLine, Permissions, sha256_hex};

pub const PLUGINS_DIR: &str = "plugins";
pub const STATE_FILE: &str = "plugins.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// Installed for this user (`<app data>/plugins`).
    User,
    /// Shipped in the open project (`<project>/plugins`).
    Project,
}

/// What the user approved for one plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub wasm_sha256: String,
    pub capabilities: Capabilities,
    #[serde(default)]
    pub granted_secrets: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub disabled_hooks: Vec<String>,
    #[serde(default)]
    pub approved_at: i64,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StateFile {
    #[serde(default)]
    version: u32,
    /// Installed plugins by id.
    #[serde(default)]
    user: BTreeMap<String, Approval>,
    /// `<project id>:<plugin id>` → trusted project plugin.
    #[serde(default)]
    project: BTreeMap<String, Approval>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ready,
    Disabled,
    /// Never approved on this machine.
    Untrusted,
    /// Approved, but the module or its capabilities changed since.
    Changed,
    /// Manifest or module invalid.
    Broken,
    /// A project plugin with the id of an installed plugin (the installed one is used).
    Shadowed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub name: String,
    pub granted: bool,
    pub set: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub scope: Scope,
    /// Absolute folder.
    pub dir: String,
    pub status: Status,
    pub error: Option<String>,
    pub manifest: Option<Manifest>,
    pub wasm_sha256: Option<String>,
    pub approved_sha256: Option<String>,
    pub enabled: bool,
    /// Capabilities as sentences for prompts.
    pub capabilities: Vec<String>,
    pub secrets: Vec<SecretStatus>,
    pub disabled_hooks: Vec<String>,
}

/// The open project, for project plugins and `fs.project.read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    pub root: PathBuf,
    /// Stable project id (`devall.yaml` id) — the key for trust decisions.
    pub id: String,
}

impl ProjectRef {
    pub fn new(root: impl Into<PathBuf>, id: impl Into<String>) -> Self {
        Self { root: root.into(), id: id.into() }
    }
}

/// What a plugin would get, shown before installing it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    pub manifest: Manifest,
    pub wasm_sha256: String,
    pub wasm_bytes: u64,
    pub capabilities: Vec<String>,
    /// Version of the already installed plugin with this id.
    pub replaces: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UtilityInfo {
    /// `<plugin>:<op>` — the id used by Utilities, the palette and `devall util`.
    pub id: String,
    pub plugin: String,
    pub plugin_name: String,
    pub name: String,
    pub description: String,
    pub category: String,
    /// `text` or `json`.
    pub output: String,
    pub no_input: bool,
    pub args: Vec<ArgSpec>,
    pub example: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImporterInfo {
    /// `<plugin>:<importer>`.
    pub id: String,
    pub plugin: String,
    pub name: String,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodegenInfo {
    /// `<plugin>:<target>`.
    pub id: String,
    pub plugin: String,
    pub name: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProviderInfo {
    pub plugin: String,
    pub plugin_name: String,
    /// Provider id inside the plugin.
    pub id: String,
    pub name: String,
    pub description: String,
    pub fields: Vec<ArgSpec>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInfo {
    pub plugin: String,
    pub id: String,
    pub name: String,
    pub events: Vec<HookEvent>,
    pub hosts: Vec<String>,
    pub enabled: bool,
}

/// Every contribution of the plugins that are ready to run.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub utilities: Vec<UtilityInfo>,
    pub importers: Vec<ImporterInfo>,
    pub codegen: Vec<CodegenInfo>,
    pub auth: Vec<AuthProviderInfo>,
    pub hooks: Vec<HookInfo>,
}

/// Result of running every pre-request hook.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreRequestRun {
    pub request: PluginRequest,
    pub changed: bool,
    /// `[plugin/hook] message` lines.
    pub logs: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostResponseRun {
    /// Test names are prefixed with the hook name.
    pub tests: Vec<TestResult>,
    pub variables: BTreeMap<String, String>,
    pub logs: Vec<String>,
    pub errors: Vec<String>,
}

/// An importer that recognised the text.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportMatch {
    pub id: String,
    pub plugin: String,
    pub name: String,
    pub confidence: f64,
}

struct CacheEntry {
    manifest_text: String,
    modified: Option<SystemTime>,
    len: u64,
    plugin: Arc<CompiledPlugin>,
}

struct Entry {
    scope: Scope,
    dir: PathBuf,
    loaded: Result<Arc<CompiledPlugin>>,
}

pub struct PluginManager {
    app_dir: PathBuf,
    state_path: Option<PathBuf>,
    state: Mutex<StateFile>,
    /// Modification time of `plugins.json` when it was last read or written (the app and the
    /// CLI share the file: changes made by the other one are picked up).
    state_mtime: Mutex<Option<SystemTime>>,
    services: Arc<dyn HostServices>,
    cache: Mutex<HashMap<PathBuf, CacheEntry>>,
    /// Limits for utilities, importers and codegen.
    pub limits: Limits,
    /// Limits for hooks and auth (on the request path).
    pub request_limits: Limits,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn split_global(id: &str) -> Result<(&str, &str)> {
    id.split_once(':').filter(|(p, i)| !p.is_empty() && !i.is_empty()).ok_or_else(|| PluginError::Invalid(format!("`{id}` is not a plugin contribution id (<plugin>:<id>)")))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl PluginManager {
    /// Plugins installed under `<app_dir>/plugins`, approvals in `<app_dir>/plugins.json`.
    pub fn new(app_dir: impl Into<PathBuf>, services: Arc<dyn HostServices>) -> Self {
        let app_dir = app_dir.into();
        let state_path = app_dir.join(STATE_FILE);
        let state = std::fs::read_to_string(&state_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mtime = std::fs::metadata(&state_path).and_then(|m| m.modified()).ok();
        Self {
            app_dir,
            state_path: Some(state_path),
            state: Mutex::new(state),
            state_mtime: Mutex::new(mtime),
            services,
            cache: Mutex::new(HashMap::new()),
            limits: Limits::default(),
            request_limits: Limits::request_path(),
        }
    }

    /// Approvals kept in memory only (tests).
    pub fn ephemeral(app_dir: impl Into<PathBuf>, services: Arc<dyn HostServices>) -> Self {
        let mut m = Self::new(app_dir, services);
        m.state_path = None;
        *lock(&m.state) = StateFile::default();
        m
    }

    pub fn user_dir(&self) -> PathBuf {
        self.app_dir.join(PLUGINS_DIR)
    }

    fn save(&self, state: &StateFile) -> Result<()> {
        let Some(path) = &self.state_path else { return Ok(()) };
        let mut v = serde_json::to_value(state).map_err(|e| PluginError::Invalid(e.to_string()))?;
        v["version"] = Value::from(1);
        let text = serde_json::to_string_pretty(&v).map_err(|e| PluginError::Invalid(e.to_string()))?;
        devall_core::fsutil::atomic_write(path, text.as_bytes())?;
        *lock(&self.state_mtime) = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(())
    }

    /// Re-reads `plugins.json` when someone else (the CLI, another window) changed it.
    fn sync(&self, st: &mut StateFile) {
        let Some(path) = &self.state_path else { return };
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut known = lock(&self.state_mtime);
        if mtime.is_some() && mtime != *known {
            if let Some(fresh) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<StateFile>(&t).ok()) {
                *st = fresh;
            }
            *known = mtime;
        }
    }

    fn update<R>(&self, f: impl FnOnce(&mut StateFile) -> Result<R>) -> Result<R> {
        let mut st = lock(&self.state);
        self.sync(&mut st);
        let r = f(&mut st)?;
        self.save(&st)?;
        Ok(r)
    }

    fn approval_key(scope: Scope, project: Option<&ProjectRef>, id: &str) -> Result<String> {
        match scope {
            Scope::User => Ok(id.to_string()),
            Scope::Project => project.map(|p| format!("{}:{id}", p.id)).ok_or_else(|| PluginError::Invalid("no project is open".into())),
        }
    }

    fn approval(&self, scope: Scope, project: Option<&ProjectRef>, id: &str) -> Option<Approval> {
        let key = Self::approval_key(scope, project, id).ok()?;
        let mut st = lock(&self.state);
        self.sync(&mut st);
        match scope {
            Scope::User => st.user.get(&key).cloned(),
            Scope::Project => st.project.get(&key).cloned(),
        }
    }

    /// Loads (or reuses) the compiled plugin in `dir`.
    fn load(&self, dir: &Path) -> Result<Arc<CompiledPlugin>> {
        let mpath = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&mpath).map_err(|e| PluginError::io(&mpath, e))?;
        let manifest = Manifest::parse(&text)?;
        let wasm = dir.join(manifest.wasm_file());
        let meta = std::fs::metadata(&wasm).map_err(|e| PluginError::io(&wasm, e))?;
        let modified = meta.modified().ok();
        if let Some(c) = lock(&self.cache).get(dir)
            && c.manifest_text == text
            && c.modified == modified
            && c.len == meta.len()
        {
            return Ok(c.plugin.clone());
        }
        let plugin = Arc::new(crate::runtime::compile_dir(dir, manifest)?);
        lock(&self.cache).insert(dir.to_path_buf(), CacheEntry { manifest_text: text, modified, len: meta.len(), plugin: plugin.clone() });
        Ok(plugin)
    }

    fn scan(dir: &Path) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut out: Vec<PathBuf> =
            rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && p.join(MANIFEST_FILE).is_file() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
        out.sort();
        out
    }

    fn entries(&self, project: Option<&ProjectRef>) -> Vec<Entry> {
        let mut out: Vec<Entry> = Self::scan(&self.user_dir()).into_iter().map(|dir| Entry { scope: Scope::User, loaded: self.load(&dir), dir }).collect();
        if let Some(p) = project {
            out.extend(Self::scan(&p.root.join(PLUGINS_DIR)).into_iter().map(|dir| Entry { scope: Scope::Project, loaded: self.load(&dir), dir }));
        }
        out
    }

    fn status_of(&self, e: &Entry, project: Option<&ProjectRef>, user_ids: &[String]) -> (Status, Option<Approval>) {
        let Ok(p) = &e.loaded else { return (Status::Broken, None) };
        if e.scope == Scope::Project && user_ids.iter().any(|u| u == p.id()) {
            return (Status::Shadowed, None);
        }
        let approval = self.approval(e.scope, project, p.id());
        let status = match &approval {
            None => Status::Untrusted,
            Some(a) if a.wasm_sha256 != p.wasm_sha256 || a.capabilities != p.manifest.capabilities => Status::Changed,
            Some(a) if !a.enabled => Status::Disabled,
            Some(_) => Status::Ready,
        };
        (status, approval)
    }

    fn user_ids(entries: &[Entry]) -> Vec<String> {
        entries.iter().filter(|e| e.scope == Scope::User).filter_map(|e| e.loaded.as_ref().ok().map(|p| p.id().to_string())).collect()
    }

    /// Installed plugins, then the project's, with their status.
    pub fn list(&self, project: Option<&ProjectRef>) -> Vec<PluginInfo> {
        let entries = self.entries(project);
        let user_ids = Self::user_ids(&entries);
        entries
            .iter()
            .map(|e| {
                let (status, approval) = self.status_of(e, project, &user_ids);
                let folder = e.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                match &e.loaded {
                    Ok(p) => {
                        let granted = approval.as_ref().map(|a| a.granted_secrets.clone()).unwrap_or_default();
                        let secrets = p
                            .manifest
                            .capabilities
                            .secrets_read
                            .iter()
                            .map(|n| SecretStatus { name: n.clone(), granted: granted.contains(n), set: self.services.secret(p.id(), n).is_some() })
                            .collect();
                        PluginInfo {
                            id: p.id().to_string(),
                            scope: e.scope,
                            dir: e.dir.display().to_string(),
                            status,
                            error: match status {
                                Status::Shadowed => Some("an installed plugin has the same id; the installed one is used".into()),
                                _ => None,
                            },
                            manifest: Some(p.manifest.clone()),
                            wasm_sha256: Some(p.wasm_sha256.clone()),
                            approved_sha256: approval.as_ref().map(|a| a.wasm_sha256.clone()),
                            enabled: approval.as_ref().is_some_and(|a| a.enabled),
                            capabilities: p.manifest.capabilities.describe(),
                            secrets,
                            disabled_hooks: approval.map(|a| a.disabled_hooks).unwrap_or_default(),
                        }
                    }
                    Err(err) => PluginInfo {
                        id: folder,
                        scope: e.scope,
                        dir: e.dir.display().to_string(),
                        status,
                        error: Some(err.to_string()),
                        manifest: None,
                        wasm_sha256: None,
                        approved_sha256: None,
                        enabled: false,
                        capabilities: Vec::new(),
                        secrets: Vec::new(),
                        disabled_hooks: Vec::new(),
                    },
                }
            })
            .collect()
    }

    /// The runnable plugin `id`, with its permissions.
    fn active(&self, project: Option<&ProjectRef>, id: &str) -> Result<(Arc<CompiledPlugin>, Approval)> {
        let entries = self.entries(project);
        let user_ids = Self::user_ids(&entries);
        let mut found: Option<&Entry> = None;
        for e in &entries {
            let matches = match &e.loaded {
                Ok(p) => p.id() == id,
                Err(_) => e.dir.file_name().is_some_and(|n| n == id),
            };
            if matches && (found.is_none() || e.scope == Scope::User) {
                found = Some(e);
                if e.scope == Scope::User {
                    break;
                }
            }
        }
        let e = found.ok_or_else(|| PluginError::NotFound(id.to_string()))?;
        let (status, approval) = self.status_of(e, project, &user_ids);
        let plugin = e.loaded.clone()?;
        match (status, approval) {
            (Status::Ready, Some(a)) => Ok((plugin, a)),
            (Status::Disabled, _) => Err(PluginError::Disabled(id.to_string())),
            (Status::Changed, _) => Err(PluginError::Changed(id.to_string())),
            _ => Err(PluginError::NotTrusted(id.to_string())),
        }
    }

    fn context(&self, plugin: &CompiledPlugin, approval: &Approval, project: Option<&ProjectRef>, limits: Limits) -> CallContext {
        let caps = &plugin.manifest.capabilities;
        CallContext {
            permissions: Permissions {
                http_hosts: caps.http.as_ref().map(|h| h.hosts.clone()).unwrap_or_default(),
                secrets: caps.secrets_read.iter().filter(|s| approval.granted_secrets.contains(s)).cloned().collect(),
                fs_project_read: caps.fs_project_read,
            },
            services: self.services.clone(),
            project_root: project.map(|p| p.root.clone()),
            limits,
        }
    }

    /// Calls any contribution of an active plugin (no contribution check).
    pub fn call_raw(&self, project: Option<&ProjectRef>, plugin: &str, kind: &str, id: &str, input: Value, limits: Limits) -> Result<CallOutcome> {
        let (p, approval) = self.active(project, plugin)?;
        let ctx = self.context(&p, &approval, project, limits);
        p.call(kind, id, input, &ctx)
    }

    // ------------------------------------------------------------ install / trust

    /// Reads a plugin folder or `.zip` for the install prompt (nothing is written).
    pub fn preview(&self, source: &Path) -> Result<InstallPreview> {
        let (manifest, _, wasm) = read_source(source)?;
        CompiledPlugin::compile(manifest.clone(), &wasm)?;
        let replaces = self.list(None).into_iter().find(|p| p.scope == Scope::User && p.id == manifest.id).and_then(|p| p.manifest.map(|m| m.version));
        Ok(InstallPreview { capabilities: manifest.capabilities.describe(), wasm_sha256: sha256_hex(&wasm), wasm_bytes: wasm.len() as u64, manifest, replaces })
    }

    /// Installs (or replaces) a user plugin from a folder or `.zip` and approves it with the
    /// secrets the user granted. `expected_sha256` (from [`Self::preview`]) guards against the
    /// source changing between the prompt and the install.
    pub fn install(&self, source: &Path, expected_sha256: Option<&str>, granted_secrets: &[String]) -> Result<PluginInfo> {
        let (manifest, manifest_text, wasm) = read_source(source)?;
        let compiled = CompiledPlugin::compile(manifest.clone(), &wasm)?;
        if let Some(exp) = expected_sha256
            && !exp.eq_ignore_ascii_case(&compiled.wasm_sha256)
        {
            return Err(PluginError::Invalid(format!("{} changed since it was reviewed (expected SHA-256 {exp}, found {})", source.display(), compiled.wasm_sha256)));
        }
        let base = self.user_dir();
        std::fs::create_dir_all(&base).map_err(|e| PluginError::io(&base, e))?;
        let tmp = base.join(format!(".install-{}-{:08x}", manifest.id, now_ms() as u32));
        std::fs::create_dir_all(&tmp).map_err(|e| PluginError::io(&tmp, e))?;
        let write = || -> Result<()> {
            devall_core::fsutil::atomic_write(&tmp.join(MANIFEST_FILE), manifest_text.as_bytes())?;
            devall_core::fsutil::atomic_write(&tmp.join(manifest.wasm_file()), &wasm)?;
            Ok(())
        };
        if let Err(e) = write() {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        let dest = base.join(&manifest.id);
        if dest.exists() {
            trash::delete(&dest).map_err(|e| PluginError::Io(format!("cannot move the old {} to the trash: {e}", dest.display())))?;
        }
        std::fs::rename(&tmp, &dest).map_err(|e| PluginError::io(&dest, e))?;
        lock(&self.cache).remove(&dest);
        let secrets: Vec<String> = granted_secrets.iter().filter(|s| manifest.capabilities.secrets_read.contains(s)).cloned().collect();
        let disabled_hooks = self.approval(Scope::User, None, &manifest.id).map(|a| a.disabled_hooks).unwrap_or_default();
        self.update(|st| {
            st.user.insert(
                manifest.id.clone(),
                Approval {
                    wasm_sha256: compiled.wasm_sha256.clone(),
                    capabilities: manifest.capabilities.clone(),
                    granted_secrets: secrets,
                    enabled: true,
                    disabled_hooks,
                    approved_at: now_ms(),
                },
            );
            Ok(())
        })?;
        self.info(None, Scope::User, &manifest.id)
    }

    fn info(&self, project: Option<&ProjectRef>, scope: Scope, id: &str) -> Result<PluginInfo> {
        self.list(project).into_iter().find(|p| p.scope == scope && p.id == id).ok_or_else(|| PluginError::NotFound(id.to_string()))
    }

    /// Moves an installed plugin to the OS trash and forgets its approval.
    pub fn uninstall(&self, id: &str) -> Result<()> {
        let dir = self.user_dir().join(id);
        if devall_core::fsutil::safe_join(&self.user_dir(), id).is_err() || id.contains(['/', '\\']) {
            return Err(PluginError::Invalid(format!("bad plugin id {id}")));
        }
        if dir.exists() {
            trash::delete(&dir).map_err(|e| PluginError::Io(format!("cannot move {} to the trash: {e}", dir.display())))?;
        }
        lock(&self.cache).remove(&dir);
        self.update(|st| {
            st.user.remove(id);
            Ok(())
        })
    }

    /// Approves a plugin as it is now (project plugins, or an installed plugin that changed or was
    /// copied in by hand). `expected_sha256` must be the hash the user reviewed.
    pub fn trust(&self, project: Option<&ProjectRef>, scope: Scope, id: &str, expected_sha256: &str, granted_secrets: &[String]) -> Result<PluginInfo> {
        let entries = self.entries(project);
        let e = entries
            .iter()
            .find(|e| e.scope == scope && e.loaded.as_ref().is_ok_and(|p| p.id() == id))
            .ok_or_else(|| PluginError::NotFound(id.to_string()))?;
        let p = e.loaded.clone()?;
        if !p.wasm_sha256.eq_ignore_ascii_case(expected_sha256) {
            return Err(PluginError::Invalid(format!("plugin {id} changed while it was being reviewed (SHA-256 is now {})", p.wasm_sha256)));
        }
        let key = Self::approval_key(scope, project, id)?;
        let secrets: Vec<String> = granted_secrets.iter().filter(|s| p.manifest.capabilities.secrets_read.contains(s)).cloned().collect();
        self.update(|st| {
            let map = if scope == Scope::User { &mut st.user } else { &mut st.project };
            let disabled_hooks = map.get(&key).map(|a| a.disabled_hooks.clone()).unwrap_or_default();
            map.insert(
                key.clone(),
                Approval {
                    wasm_sha256: p.wasm_sha256.clone(),
                    capabilities: p.manifest.capabilities.clone(),
                    granted_secrets: secrets,
                    enabled: true,
                    disabled_hooks,
                    approved_at: now_ms(),
                },
            );
            Ok(())
        })?;
        self.info(project, scope, id)
    }

    /// Forgets the approval (the plugin stops running until trusted again).
    pub fn revoke(&self, project: Option<&ProjectRef>, scope: Scope, id: &str) -> Result<()> {
        let key = Self::approval_key(scope, project, id)?;
        self.update(|st| {
            if scope == Scope::User { st.user.remove(&key) } else { st.project.remove(&key) };
            Ok(())
        })
    }

    fn edit_approval(&self, project: Option<&ProjectRef>, scope: Scope, id: &str, f: impl FnOnce(&mut Approval)) -> Result<()> {
        let key = Self::approval_key(scope, project, id)?;
        self.update(|st| {
            let map = if scope == Scope::User { &mut st.user } else { &mut st.project };
            let a = map.get_mut(&key).ok_or_else(|| PluginError::NotTrusted(id.to_string()))?;
            f(a);
            Ok(())
        })
    }

    pub fn set_enabled(&self, project: Option<&ProjectRef>, scope: Scope, id: &str, enabled: bool) -> Result<()> {
        self.edit_approval(project, scope, id, |a| a.enabled = enabled)
    }

    pub fn set_hook_enabled(&self, project: Option<&ProjectRef>, scope: Scope, id: &str, hook: &str, enabled: bool) -> Result<()> {
        self.edit_approval(project, scope, id, |a| {
            a.disabled_hooks.retain(|h| h != hook);
            if !enabled {
                a.disabled_hooks.push(hook.to_string());
            }
        })
    }

    /// Changes which declared secrets the plugin may read.
    pub fn set_granted_secrets(&self, project: Option<&ProjectRef>, scope: Scope, id: &str, secrets: &[String]) -> Result<()> {
        let declared = self.info(project, scope, id)?.manifest.map(|m| m.capabilities.secrets_read).unwrap_or_default();
        self.edit_approval(project, scope, id, |a| a.granted_secrets = secrets.iter().filter(|s| declared.contains(s)).cloned().collect())
    }

    // ------------------------------------------------------------ contributions

    fn ready(&self, project: Option<&ProjectRef>) -> Vec<(Arc<CompiledPlugin>, Approval)> {
        let entries = self.entries(project);
        let user_ids = Self::user_ids(&entries);
        entries
            .iter()
            .filter_map(|e| match self.status_of(e, project, &user_ids) {
                (Status::Ready, Some(a)) => e.loaded.as_ref().ok().map(|p| (p.clone(), a)),
                _ => None,
            })
            .collect()
    }

    /// Contributions of every plugin that is ready to run.
    pub fn catalog(&self, project: Option<&ProjectRef>) -> Catalog {
        let mut c = Catalog::default();
        for (p, a) in self.ready(project) {
            let m = &p.manifest;
            for u in &m.contributes.utilities {
                c.utilities.push(UtilityInfo {
                    id: format!("{}:{}", m.id, u.id),
                    plugin: m.id.clone(),
                    plugin_name: m.name.clone(),
                    name: u.name.clone(),
                    description: u.description.clone(),
                    category: u.category.clone().unwrap_or_else(|| "Plugins".into()),
                    output: u.output.clone().unwrap_or_else(|| "text".into()),
                    no_input: u.no_input,
                    args: u.args.clone(),
                    example: u.example.clone(),
                });
            }
            for i in &m.contributes.importers {
                c.importers.push(ImporterInfo { id: format!("{}:{}", m.id, i.id), plugin: m.id.clone(), name: i.name.clone(), extensions: i.extensions.clone() });
            }
            for g in &m.contributes.codegen {
                c.codegen.push(CodegenInfo { id: format!("{}:{}", m.id, g.id), plugin: m.id.clone(), name: g.name.clone(), language: g.language.clone() });
            }
            for x in &m.contributes.auth {
                c.auth.push(AuthProviderInfo {
                    plugin: m.id.clone(),
                    plugin_name: m.name.clone(),
                    id: x.id.clone(),
                    name: x.name.clone(),
                    description: x.description.clone(),
                    fields: x.fields.clone(),
                });
            }
            for h in &m.contributes.hooks {
                c.hooks.push(HookInfo {
                    plugin: m.id.clone(),
                    id: h.id.clone(),
                    name: h.name.clone(),
                    events: h.events.clone(),
                    hosts: h.hosts.clone(),
                    enabled: !a.disabled_hooks.contains(&h.id),
                });
            }
        }
        c
    }

    fn decode<T: serde::de::DeserializeOwned>(plugin: &str, what: &str, v: Value) -> Result<T> {
        serde_json::from_value(v).map_err(|e| PluginError::Abi { plugin: plugin.to_string(), detail: format!("{what} output does not match the contract: {e}") })
    }

    /// Runs a utility op (`<plugin>:<op>`); missing args take the manifest defaults. Returns the
    /// output text (JSON values pretty-printed) and the plugin's log lines.
    pub fn run_utility(&self, project: Option<&ProjectRef>, id: &str, input: &str, args: &BTreeMap<String, String>) -> Result<(String, Vec<LogLine>)> {
        let (plugin, op) = split_global(id)?;
        let (p, approval) = self.active(project, plugin)?;
        let u = p.manifest.contributes.utilities.iter().find(|u| u.id == op).ok_or_else(|| PluginError::NoContribution { plugin: plugin.into(), kind: "utility", id: op.into() })?;
        let mut all = args.clone();
        for a in &u.args {
            all.entry(a.name.clone()).or_insert_with(|| a.default.clone());
        }
        let input = UtilityInput { input: if u.no_input { String::new() } else { input.to_string() }, args: all };
        let ctx = self.context(&p, &approval, project, self.limits);
        let out = p.call(KIND_UTILITY, op, serde_json::to_value(input).unwrap_or_default(), &ctx)?;
        let o: UtilityOutput = Self::decode(plugin, "utility", out.value)?;
        let text = match o.output {
            Value::String(s) => s,
            other => serde_json::to_string_pretty(&other).unwrap_or_default(),
        };
        Ok((text, out.logs))
    }

    fn hooks_for(&self, project: Option<&ProjectRef>, event: HookEvent, url: &str) -> Vec<(Arc<CompiledPlugin>, Approval, String, String)> {
        let host = url_host_port(url);
        let mut out = Vec::new();
        for (p, a) in self.ready(project) {
            for h in &p.manifest.contributes.hooks {
                let host_ok = h.hosts.is_empty() || host.as_ref().is_some_and(|(n, port)| host_allowed(&h.hosts, n, Some(*port)));
                if h.events.contains(&event) && !a.disabled_hooks.contains(&h.id) && host_ok {
                    out.push((p.clone(), a.clone(), h.id.clone(), h.name.clone()));
                }
            }
        }
        out
    }

    /// Runs every enabled pre-request hook in order (installed plugins first, by id); each sees
    /// the previous one's output. Errors are collected, not raised.
    pub fn run_pre_request(&self, project: Option<&ProjectRef>, request: PluginRequest, request_name: Option<&str>, environment: Option<&str>) -> PreRequestRun {
        let mut run = PreRequestRun { request, ..Default::default() };
        for (p, a, hook, name) in self.hooks_for(project, HookEvent::PreRequest, &run.request.url) {
            let tag = format!("[{}/{}]", p.id(), hook);
            let input = PreRequestInput { request: run.request.clone(), request_name: request_name.map(str::to_string), environment: environment.map(str::to_string) };
            let ctx = self.context(&p, &a, project, self.request_limits);
            match p.call(KIND_PRE_REQUEST, &hook, serde_json::to_value(input).unwrap_or_default(), &ctx) {
                Ok(out) => {
                    run.logs.extend(out.logs.iter().map(|l| format!("{tag} {}", l.message)));
                    match Self::decode::<PreRequestOutput>(p.id(), "preRequest", out.value) {
                        Ok(o) => {
                            run.logs.extend(o.logs.into_iter().map(|l| format!("{tag} {l}")));
                            if let Some(r) = o.request
                                && r != run.request
                            {
                                run.request = r;
                                run.changed = true;
                            }
                        }
                        Err(e) => run.errors.push(format!("{name}: {e}")),
                    }
                }
                Err(e) => run.errors.push(format!("{name}: {e}")),
            }
        }
        run
    }

    /// Runs every enabled post-response hook.
    pub fn run_post_response(&self, project: Option<&ProjectRef>, request: &PluginRequest, response: &PluginResponse, request_name: Option<&str>) -> PostResponseRun {
        let mut run = PostResponseRun::default();
        for (p, a, hook, name) in self.hooks_for(project, HookEvent::PostResponse, &request.url) {
            let tag = format!("[{}/{}]", p.id(), hook);
            let input = PostResponseInput { request: request.clone(), response: response.clone(), request_name: request_name.map(str::to_string) };
            let ctx = self.context(&p, &a, project, self.request_limits);
            match p.call(KIND_POST_RESPONSE, &hook, serde_json::to_value(input).unwrap_or_default(), &ctx) {
                Ok(out) => {
                    run.logs.extend(out.logs.iter().map(|l| format!("{tag} {}", l.message)));
                    match Self::decode::<PostResponseOutput>(p.id(), "postResponse", out.value) {
                        Ok(o) => {
                            run.logs.extend(o.logs.into_iter().map(|l| format!("{tag} {l}")));
                            run.tests.extend(o.tests.into_iter().map(|t| TestResult { name: format!("{name}: {}", t.name), ..t }));
                            run.variables.extend(o.variables);
                        }
                        Err(e) => run.errors.push(format!("{name}: {e}")),
                    }
                }
                Err(e) => run.errors.push(format!("{name}: {e}")),
            }
        }
        run
    }

    /// The most confident plugin importer for `text` (≥ 0.5), if any.
    pub fn detect_import(&self, project: Option<&ProjectRef>, text: &str, file_name: Option<&str>) -> Option<ImportMatch> {
        let mut best: Option<ImportMatch> = None;
        for (p, a) in self.ready(project) {
            for imp in &p.manifest.contributes.importers {
                let ext_hit = file_name.is_some_and(|f| imp.extensions.iter().any(|e| f.to_ascii_lowercase().ends_with(&e.to_ascii_lowercase())));
                let input = ImportInput { text: text.to_string(), file_name: file_name.map(str::to_string) };
                let ctx = self.context(&p, &a, project, self.request_limits);
                let confidence = p
                    .call(KIND_IMPORT_DETECT, &imp.id, serde_json::to_value(input).unwrap_or_default(), &ctx)
                    .ok()
                    .and_then(|o| serde_json::from_value::<DetectOutput>(o.value).ok())
                    .map(|d| d.confidence.clamp(0.0, 1.0))
                    .unwrap_or(0.0);
                let confidence = if ext_hit { confidence.max(0.5) } else { confidence };
                if confidence >= 0.5 && best.as_ref().is_none_or(|b| confidence > b.confidence) {
                    best = Some(ImportMatch { id: format!("{}:{}", p.id(), imp.id), plugin: p.id().to_string(), name: imp.name.clone(), confidence });
                }
            }
        }
        best
    }

    /// Parses `text` with a plugin importer (`<plugin>:<importer>`).
    pub fn import(&self, project: Option<&ProjectRef>, id: &str, text: &str, file_name: Option<&str>) -> Result<ImportOutput> {
        let (plugin, imp) = split_global(id)?;
        let (p, approval) = self.active(project, plugin)?;
        if !p.manifest.contributes.importers.iter().any(|i| i.id == imp) {
            return Err(PluginError::NoContribution { plugin: plugin.into(), kind: "importer", id: imp.into() });
        }
        let input = ImportInput { text: text.to_string(), file_name: file_name.map(str::to_string) };
        let ctx = self.context(&p, &approval, project, self.limits);
        let out = p.call(KIND_IMPORT_PARSE, imp, serde_json::to_value(input).unwrap_or_default(), &ctx)?;
        Self::decode(plugin, "import", out.value)
    }

    /// Generates code for a resolved request (`<plugin>:<target>`).
    pub fn codegen(&self, project: Option<&ProjectRef>, id: &str, request: &PluginRequest) -> Result<String> {
        let (plugin, target) = split_global(id)?;
        let (p, approval) = self.active(project, plugin)?;
        if !p.manifest.contributes.codegen.iter().any(|c| c.id == target) {
            return Err(PluginError::NoContribution { plugin: plugin.into(), kind: "codegen", id: target.into() });
        }
        let ctx = self.context(&p, &approval, project, self.limits);
        let out = p.call(KIND_CODEGEN, target, serde_json::to_value(CodegenInput { request: request.clone() }).unwrap_or_default(), &ctx)?;
        Ok(Self::decode::<CodegenOutput>(plugin, "codegen", out.value)?.code)
    }

    /// Computes auth headers / query parameters with a plugin auth provider.
    pub fn auth(&self, project: Option<&ProjectRef>, plugin: &str, provider: &str, config: &BTreeMap<String, String>, request: &PluginRequest) -> Result<AuthOutput> {
        let (p, approval) = self.active(project, plugin)?;
        if !p.manifest.contributes.auth.iter().any(|a| a.id == provider) {
            return Err(PluginError::NoContribution { plugin: plugin.into(), kind: "auth provider", id: provider.into() });
        }
        let ctx = self.context(&p, &approval, project, self.request_limits);
        let input = AuthInput { request: request.clone(), config: config.clone() };
        let out = p.call(KIND_AUTH, provider, serde_json::to_value(input).unwrap_or_default(), &ctx)?;
        Self::decode(plugin, "auth", out.value)
    }
}

fn url_host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default = if scheme.eq_ignore_ascii_case("https") { 443 } else { 80 };
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit_once('@').map_or(auth, |(_, a)| a);
    match auth.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') || h.ends_with(']') => Some((h.trim_matches(['[', ']']).to_ascii_lowercase(), p.parse().ok()?)),
        _ => Some((auth.trim_matches(['[', ']']).to_ascii_lowercase(), default)),
    }
}

/// `(manifest, manifest text, wasm bytes)` from a plugin folder or `.zip` (with `plugin.yaml` at
/// the root or inside one top-level folder).
fn read_source(source: &Path) -> Result<(Manifest, String, Vec<u8>)> {
    if source.is_dir() {
        let mpath = source.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&mpath).map_err(|e| PluginError::io(&mpath, e))?;
        let m = Manifest::parse(&text)?;
        let wpath = source.join(m.wasm_file());
        let len = std::fs::metadata(&wpath).map_err(|e| PluginError::io(&wpath, e))?.len();
        if len > crate::manifest::MAX_WASM_BYTES {
            return Err(PluginError::InvalidModule { plugin: m.id.clone(), detail: format!("{len} bytes is larger than the limit") });
        }
        let wasm = std::fs::read(&wpath).map_err(|e| PluginError::io(&wpath, e))?;
        return Ok((m, text, wasm));
    }
    let file = std::fs::File::open(source).map_err(|e| PluginError::io(source, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| PluginError::Invalid(format!("{} is not a plugin folder or .zip: {e}", source.display())))?;
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    let manifest_name = names
        .iter()
        .filter(|n| {
            let parts: Vec<&str> = n.split('/').collect();
            parts.last() == Some(&MANIFEST_FILE) && parts.len() <= 2
        })
        .min_by_key(|n| n.len())
        .cloned()
        .ok_or_else(|| PluginError::Invalid(format!("{} has no {MANIFEST_FILE} (at the root or in one top-level folder)", source.display())))?;
    let prefix = manifest_name.strip_suffix(MANIFEST_FILE).unwrap_or_default().to_string();
    let mut read = |name: &str, max: u64| -> Result<Vec<u8>> {
        let mut f = zip.by_name(name).map_err(|e| PluginError::Invalid(format!("{name}: {e}")))?;
        if f.size() > max {
            return Err(PluginError::Invalid(format!("{name} is too large")));
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.by_ref().take(max + 1).read_to_end(&mut buf).map_err(|e| PluginError::Invalid(format!("{name}: {e}")))?;
        if buf.len() as u64 > max {
            return Err(PluginError::Invalid(format!("{name} is too large")));
        }
        Ok(buf)
    };
    let text = String::from_utf8(read(&manifest_name, 1 << 20)?).map_err(|_| PluginError::Manifest("plugin.yaml is not UTF-8".into()))?;
    let m = Manifest::parse(&text)?;
    let wasm = read(&format!("{prefix}{}", m.wasm_file()), crate::manifest::MAX_WASM_BYTES)?;
    Ok((m, text, wasm))
}
