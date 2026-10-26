//! IPC commands. Paths inside a project are `/`-separated and relative to `http/`; every
//! command takes the project root explicitly so several projects can be open over time without
//! hidden global state.

use std::collections::BTreeSet;
use std::path::PathBuf;

use devall_core::regexes::{RegexEntry, RegexFile};
use devall_core::secrets::{ALL_ENVIRONMENTS, SecretResolver};
use devall_core::transfer::{TransferOptions, TransferReport, TransferSelection};
use devall_core::vars;
use devall_core::{
    EnvironmentEntry, EnvironmentFile, FolderFile, HttpRequestFile, Project, ProjectManifest, Resolver, SecretScope, TreeNode,
};
use serde::Serialize;
use tauri::State;

use crate::state::{AppState, RecentProject};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn open(root: &str) -> CmdResult<Project> {
    Project::open(root).map_err(err)
}

// ---------- app & projects ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    platform: &'static str,
    scratch_path: String,
    keychain_available: bool,
    config_dir: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        scratch_path: state.scratch_dir().display().to_string(),
        keychain_available: state.keychain_available,
        config_dir: state.config_dir.display().to_string(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    root: String,
    manifest: ProjectManifest,
    environments: Vec<EnvironmentEntry>,
    is_scratch: bool,
}

fn info(state: &AppState, p: &Project) -> CmdResult<ProjectInfo> {
    let is_scratch = p.root() == state.scratch_dir();
    Ok(ProjectInfo {
        root: p.root().display().to_string(),
        manifest: p.manifest.clone(),
        environments: p.environments().map_err(err)?,
        is_scratch,
    })
}

#[tauri::command]
pub fn recent_projects(state: State<'_, AppState>) -> Vec<RecentProject> {
    state.recent().into_iter().filter(|r| PathBuf::from(&r.path).join(devall_core::project::MANIFEST).is_file()).collect()
}

#[tauri::command]
pub fn forget_recent(state: State<'_, AppState>, path: String) {
    state.forget_recent(&path);
}

/// A project on this device with its `devall.yaml` id.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectId {
    path: String,
    name: String,
    id: String,
    opened_at: i64,
    /// The scratch pad (never on the recent list).
    scratch: bool,
}

/// The recent projects (newest first), then the scratch pad, with their ids: how a
/// `devall:…?project=<id>` link from outside the app finds its project. Only these folders are
/// read — a link never names a path.
#[tauri::command]
pub fn recent_project_ids(state: State<'_, AppState>) -> Vec<ProjectId> {
    let mut out: Vec<ProjectId> = state
        .recent()
        .into_iter()
        .filter_map(|r| {
            let p = Project::open(&r.path).ok()?;
            Some(ProjectId { path: r.path, name: r.name, id: p.manifest.id, opened_at: r.opened_at, scratch: false })
        })
        .collect();
    if let Ok(p) = Project::open(state.scratch_dir()) {
        out.push(ProjectId { path: p.root().display().to_string(), name: p.manifest.name.clone(), id: p.manifest.id.clone(), opened_at: 0, scratch: true });
    }
    out
}

#[tauri::command]
pub fn open_project(app: tauri::AppHandle, state: State<'_, AppState>, path: String) -> CmdResult<ProjectInfo> {
    let p = open(&path)?;
    if p.root() != state.scratch_dir() {
        state.touch_recent(&p.root().display().to_string(), &p.manifest.name);
    }
    crate::modules::history::project_opened(&app, p.root(), &p.manifest.name);
    info(&state, &p)
}

#[tauri::command]
pub fn create_project(app: tauri::AppHandle, state: State<'_, AppState>, path: String, name: String) -> CmdResult<ProjectInfo> {
    std::fs::create_dir_all(&path).map_err(err)?;
    let p = Project::init(&path, &name).map_err(err)?;
    state.touch_recent(&p.root().display().to_string(), &p.manifest.name);
    crate::modules::history::project_opened(&app, p.root(), &p.manifest.name);
    info(&state, &p)
}

#[tauri::command]
pub fn open_scratch(app: tauri::AppHandle, state: State<'_, AppState>) -> CmdResult<ProjectInfo> {
    let p = Project::open_or_init(state.scratch_dir(), "Scratch pad").map_err(err)?;
    crate::modules::history::project_opened(&app, p.root(), &p.manifest.name);
    info(&state, &p)
}

#[tauri::command]
pub fn save_manifest(root: String, manifest: ProjectManifest) -> CmdResult<()> {
    let mut p = open(&root)?;
    // The id is the secret-scope key; never let the UI change it. Repositories are edited only
    // through the `repos_*` commands, so a stale manifest in the UI can't drop them.
    let id = p.manifest.id.clone();
    let repos = std::mem::take(&mut p.manifest.repos);
    p.manifest = ProjectManifest { id, repos, ..manifest };
    p.save_manifest().map_err(err)
}

// ---------- tree, requests, folders ----------

#[tauri::command]
pub fn project_tree(root: String) -> CmdResult<TreeNode> {
    open(&root)?.tree().map_err(err)
}

#[tauri::command]
pub fn read_request(root: String, path: String) -> CmdResult<HttpRequestFile> {
    open(&root)?.read_request(&path).map_err(err)
}

#[tauri::command]
pub fn save_request(root: String, path: String, request: HttpRequestFile) -> CmdResult<()> {
    open(&root)?.save_request(&path, &request).map_err(err)
}

#[tauri::command]
pub fn create_request(root: String, folder: String, request: HttpRequestFile) -> CmdResult<String> {
    open(&root)?.create_request(&folder, request).map_err(err)
}

#[tauri::command]
pub fn duplicate_request(root: String, path: String) -> CmdResult<String> {
    open(&root)?.duplicate_request(&path).map_err(err)
}

#[tauri::command]
pub fn create_folder(root: String, parent: String, name: String) -> CmdResult<String> {
    open(&root)?.create_folder(&parent, &name).map_err(err)
}

#[tauri::command]
pub fn read_folder(root: String, path: String) -> CmdResult<FolderFile> {
    open(&root)?.read_folder(&path).map_err(err)
}

#[tauri::command]
pub fn save_folder(root: String, path: String, folder: FolderFile) -> CmdResult<()> {
    open(&root)?.save_folder(&path, &folder).map_err(err)
}

#[tauri::command]
pub fn rename_item(state: State<'_, AppState>, root: String, path: String, name: String) -> CmdResult<String> {
    let to = open(&root)?.rename_item(&path, &name).map_err(err)?;
    // History follows the file.
    if to != path
        && let Ok(h) = state.history(&root)
    {
        let _ = h.rename_request(&path, &to);
    }
    Ok(to)
}

#[tauri::command]
pub fn delete_item(root: String, path: String) -> CmdResult<()> {
    open(&root)?.delete_item(&path).map_err(err)
}

#[tauri::command]
pub fn move_item(state: State<'_, AppState>, root: String, path: String, target: String, index: Option<usize>) -> CmdResult<String> {
    let to = open(&root)?.move_item(&path, &target, index).map_err(err)?;
    if to != path
        && let Ok(h) = state.history(&root)
    {
        let _ = h.rename_request(&path, &to);
    }
    Ok(to)
}

// ---------- environments & secrets ----------

#[tauri::command]
pub fn list_environments(root: String) -> CmdResult<Vec<EnvironmentEntry>> {
    open(&root)?.environments().map_err(err)
}

#[tauri::command]
pub fn create_environment(root: String, name: String) -> CmdResult<String> {
    open(&root)?.create_environment(&name).map_err(err)
}

#[tauri::command]
pub fn save_environment(root: String, id: String, environment: EnvironmentFile) -> CmdResult<()> {
    open(&root)?.save_environment(&id, &environment).map_err(err)
}

#[tauri::command]
pub fn delete_environment(root: String, id: String) -> CmdResult<()> {
    open(&root)?.delete_environment(&id).map_err(err)
}

/// Where each of `names` (`baseUrl`, `secret.API_TOKEN`) is referenced in the project's files.
#[tauri::command]
pub async fn env_usages(root: String, names: Vec<String>) -> CmdResult<Vec<devall_core::usages::VarUsage>> {
    let root = open(&root)?.root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || devall_core::usages::find_usages(&root, &names)).await.map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    name: String,
    /// Where the value currently comes from, or `None` when it isn't set anywhere.
    source: Option<&'static str>,
}

/// Status of every secret declared by the environment (values are never returned).
#[tauri::command]
pub fn secret_status(state: State<'_, AppState>, root: String, env: String) -> CmdResult<Vec<SecretStatus>> {
    let p = open(&root)?;
    let e = p.read_environment(&env).map_err(err)?;
    let r = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), Some(&env), p.root(), Some(&e), &state.providers);
    Ok(e.secrets.iter().map(|n| SecretStatus { name: n.clone(), source: r.source_of(n) }).collect())
}

/// Stores a secret value in the OS keychain. `env` = `*` stores it for every environment.
#[tauri::command]
pub fn set_secret(state: State<'_, AppState>, root: String, env: String, name: String, value: String) -> CmdResult<()> {
    let p = open(&root)?;
    let env = if env.is_empty() { ALL_ENVIRONMENTS.to_string() } else { env };
    state.secrets.set(&SecretScope::new(p.id(), env), name.trim(), &value).map_err(err)
}

#[tauri::command]
pub fn delete_secret(state: State<'_, AppState>, root: String, env: String, name: String) -> CmdResult<()> {
    let p = open(&root)?;
    let env = if env.is_empty() { ALL_ENVIRONMENTS.to_string() } else { env };
    state.secrets.delete(&SecretScope::new(p.id(), env), &name).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VarInfo {
    name: String,
    /// Resolved value; secrets are masked.
    value: Option<String>,
    source: String,
}

/// Every variable visible to a request at `path` with environment `env`, nearest source winning.
/// Used for `{{` autocomplete and hover previews.
#[tauri::command]
pub fn variable_catalog(state: State<'_, AppState>, root: String, path: String, env: Option<String>) -> CmdResult<Vec<VarInfo>> {
    let p = open(&root)?;
    let env_file = env.as_deref().filter(|e| !e.is_empty()).map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let eff = p.effective_request(&path, &HttpRequestFile::new("")).map_err(err)?;
    let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env.as_deref(), p.root(), env_file.as_ref(), &state.providers);
    let runtime = state.runtime_vars(&root);
    let mut r = resolver_for(&p, &runtime, env.as_deref(), env_file.as_ref(), &eff.folder_variables, &secrets);
    let mut out: Vec<VarInfo> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |name: &str, raw: &str, source: String, r: &mut Resolver| {
        if seen.insert(name.to_string()) {
            let value = r.resolve(raw);
            out.push(VarInfo { name: name.to_string(), value: Some(r.redact(&value)), source });
        }
    };
    for (k, v) in &runtime {
        push(k, v, "captured (this session)".into(), &mut r);
    }
    let root_key = p.root().to_string_lossy().to_string();
    let forwards = devall_infra::forward::global_overrides();
    for (k, v) in &forwards.variables(&root_key, env.as_deref()) {
        let source = forwards.source_of(&root_key, env.as_deref(), k).map_or_else(|| "port-forward".to_string(), |s| format!("port-forward: {s}"));
        push(k, v, source, &mut r);
    }
    if let Some(e) = &env_file {
        for (k, v) in &e.variables {
            push(k, v, format!("environment: {}", e.name), &mut r);
        }
    }
    for (i, fv) in eff.folder_variables.iter().enumerate() {
        for (k, v) in fv {
            push(k, v, if i + 1 == eff.folder_variables.len() { "root folder".into() } else { "folder".into() }, &mut r);
        }
    }
    for (k, v) in &p.manifest.variables {
        push(k, v, "project".into(), &mut r);
    }
    if let Some(e) = &env_file {
        for s in &e.secrets {
            out.push(VarInfo {
                name: format!("secret.{s}"),
                value: secrets.source_of(s).map(|_| "••••••".to_string()),
                source: secrets.source_of(s).unwrap_or("not set").to_string(),
            });
        }
    }
    for d in vars::DYNAMIC_NAMES {
        out.push(VarInfo { name: d.to_string(), value: None, source: "dynamic".into() });
    }
    Ok(out)
}

pub(crate) fn resolver_for<'a>(
    p: &Project,
    runtime: &devall_core::Variables,
    env_id: Option<&str>,
    env: Option<&EnvironmentFile>,
    folder_vars: &[devall_core::Variables],
    secrets: &'a SecretResolver<'a>,
) -> Resolver<'a> {
    let mut r = Resolver::new(secrets);
    // Captured values from earlier responses win, then running port-forwards (e.g. baseUrl →
    // http://127.0.0.1:<port>), then environment, folders, project.
    r.push_layer(runtime.clone());
    r.push_layer(devall_infra::forward::global_overrides().variables(&p.root().to_string_lossy(), env_id));
    if let Some(e) = env {
        r.push_layer(e.variables.clone());
    }
    for fv in folder_vars {
        r.push_layer(fv.clone());
    }
    r.push_layer(p.manifest.variables.clone());
    r
}

// ---------- utilities ----------

#[tauri::command]
pub fn import_curl(command: String) -> CmdResult<HttpRequestFile> {
    devall_core::curl::parse_curl(&command).map_err(err)
}

#[tauri::command]
pub fn util_list() -> Vec<devall_utils::OpInfo> {
    devall_utils::list()
}

/// Async + blocking pool: network ops (dns.lookup, tls.chain), jq and bcrypt must not freeze the UI.
#[tauri::command]
pub async fn util_run(op: String, input: String, args: serde_json::Value) -> CmdResult<String> {
    tokio::task::spawn_blocking(move || devall_utils::run(&op, &input, &args).map_err(err)).await.map_err(err)?
}

// ---------- files the user picked in a dialog ----------

const MAX_TEXT_FILE: u64 = 50 * 1024 * 1024;

#[tauri::command]
pub fn read_text_file(path: String) -> CmdResult<String> {
    let meta = std::fs::metadata(&path).map_err(err)?;
    if meta.len() > MAX_TEXT_FILE {
        return Err(format!("{path} is larger than 50 MB"));
    }
    let bytes = std::fs::read(&path).map_err(err)?;
    Ok(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string())
}

#[tauri::command]
pub fn write_text_file(path: String, content: String) -> CmdResult<()> {
    devall_core::fsutil::atomic_write(std::path::Path::new(&path), content.as_bytes()).map_err(err)
}

// ---------- secret scanning & secret managers ----------

#[tauri::command]
pub fn scan_text(text: String) -> Vec<devall_core::scan::Finding> {
    devall_core::scan::scan_text(&text)
}

#[tauri::command]
pub fn scan_project(root: String) -> Vec<devall_core::scan::FileFindings> {
    devall_core::scan::scan_project(std::path::Path::new(&root))
}

#[tauri::command]
pub fn secret_providers() -> Vec<devall_core::providers::ProviderInfo> {
    devall_core::providers::PROVIDERS.to_vec()
}

/// Resolves a secret-manager reference once to check it works (the value is never returned).
#[tauri::command]
pub async fn secret_provider_test(state: State<'_, AppState>, reference: String) -> CmdResult<usize> {
    let r = reference.clone();
    let v = tokio::task::spawn_blocking(move || devall_core::providers::fetch(&r)).await.map_err(err)??;
    state.providers.clear();
    Ok(v.chars().count())
}

// ---------- move to project (scratch pad → real project) ----------

/// One keychain value that can follow moved items. Values never cross IPC, only names.
#[derive(Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SecretCopy {
    /// Environment id in the source project (`*` = every environment).
    from_env: String,
    to_env: String,
    name: String,
    /// The destination already has a value that copying would replace.
    #[serde(default)]
    exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferResult {
    report: TransferReport,
    /// Keychain values the source holds for the moved secrets, offered for copying.
    secrets: Vec<SecretCopy>,
}

/// Copies (or moves) requests, folders, environments and notes into another project.
#[tauri::command]
pub fn transfer_to_project(state: State<'_, AppState>, from_root: String, to_root: String, selection: TransferSelection, options: TransferOptions) -> CmdResult<TransferResult> {
    let from = open(&from_root)?;
    let mut to = open(&to_root)?;
    let report = devall_core::transfer::transfer(&from, &mut to, &selection, &options).map_err(err)?;
    let has = |project: &str, env: &str, name: &str| matches!(state.secrets.get(&SecretScope::new(project, env), name), Ok(Some(_)));
    let mut secrets: Vec<SecretCopy> = Vec::new();
    let mut push = |from_env: &str, to_env: &str, name: &str| {
        if has(from.id(), from_env, name) && !secrets.iter().any(|s| s.from_env == from_env && s.name == name) {
            secrets.push(SecretCopy { from_env: from_env.into(), to_env: to_env.into(), name: name.into(), exists: has(to.id(), to_env, name) });
        }
    };
    for e in &report.environments {
        for name in &e.secrets {
            push(&e.from, &e.to, name);
        }
    }
    let all: BTreeSet<&String> = report.secret_refs.iter().chain(report.environments.iter().flat_map(|e| e.secrets.iter())).collect();
    for name in all {
        push(ALL_ENVIRONMENTS, ALL_ENVIRONMENTS, name);
    }
    Ok(TransferResult { report, secrets })
}

/// Copies keychain values from one project's scope to another's (after the user confirmed).
#[tauri::command]
pub fn transfer_secrets(state: State<'_, AppState>, from_root: String, to_root: String, copies: Vec<SecretCopy>) -> CmdResult<usize> {
    let (from, to) = (open(&from_root)?, open(&to_root)?);
    let mut n = 0;
    for c in copies {
        if let Some(v) = state.secrets.get(&SecretScope::new(from.id(), c.from_env.as_str()), &c.name).map_err(err)? {
            state.secrets.set(&SecretScope::new(to.id(), c.to_env.as_str()), &c.name, &v).map_err(err)?;
            n += 1;
        }
    }
    Ok(n)
}

// ---------- response snapshots ----------

/// Records a response body as a snapshot file (default `http/__snapshots__/<request>.snap.json`)
/// and returns its path relative to the project root.
#[tauri::command]
pub fn snapshot_save(root: String, path: String, body: String, content_type: Option<String>, ignore: Vec<String>, file: Option<String>) -> CmdResult<String> {
    let p = open(&root)?;
    let file = file.map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).unwrap_or_else(|| devall_http::snapshot::default_path(&path));
    let snap = devall_http::snapshot::make(body.as_bytes(), content_type.as_deref(), Some(&path), &ignore)?;
    devall_http::snapshot::write(p.root(), &file, &snap)?;
    Ok(file)
}

// ---------- saved regexes (utilities/regex/*.regex.yaml) ----------

#[tauri::command]
pub fn regex_list(root: String) -> CmdResult<Vec<RegexEntry>> {
    open(&root)?.regexes().map_err(err)
}

#[tauri::command]
pub fn regex_read(root: String, path: String) -> CmdResult<RegexFile> {
    open(&root)?.read_regex(&path).map_err(err)
}

#[tauri::command]
pub fn regex_save(root: String, path: Option<String>, file: RegexFile) -> CmdResult<String> {
    open(&root)?.save_regex(path.as_deref(), &file).map_err(err)
}

#[tauri::command]
pub fn regex_delete(root: String, path: String) -> CmdResult<()> {
    open(&root)?.delete_regex(&path).map_err(err)
}
