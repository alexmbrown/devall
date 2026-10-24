//! WebAssembly plugins (`crates/devall-plugins`): the Plugins page, plugin utilities, plugin import
//! formats and codegen targets, and the request-path bridge (plugin auth + hooks) installed into
//! devall-http at startup.
//!
//! Registration (`lib.rs`): `app.manage(modules::plugins::PluginState::new(data_dir))` in `setup`
//! and the `plugins_*` commands below in `generate_handler!`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use devall_core::{Body, HttpRequestFile, KeyValue, Project, SecretStore};
use devall_import::{ImportPlan, PlanFolder, PlanItem};
use devall_plugins::{
    Catalog, DefaultServices, ImportMatch, ImportOutput, ImportedBody, InstallPreview, LogLine, PluginInfo, PluginManager, ProjectRef, Scope,
};
use serde::Serialize;
use tauri::State;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub struct PluginState {
    pub manager: Arc<PluginManager>,
}

fn keychain() -> Option<Box<dyn SecretStore>> {
    devall_core::KeychainSecretStore::available().then(|| Box::new(devall_core::KeychainSecretStore::new()) as Box<dyn SecretStore>)
}

impl PluginState {
    /// Plugins under `<data_dir>/plugins`; also installs the request bridge into devall-http.
    pub fn new(data_dir: PathBuf) -> Self {
        let manager = Arc::new(PluginManager::new(data_dir, Arc::new(DefaultServices::new(keychain()))));
        devall_http::plugins::install(Arc::new(devall_plugins::HttpBridge::new(manager.clone())));
        Self { manager }
    }
}

/// The open project as plugins see it (`None` without a project / for a missing folder).
pub fn project_ref(root: Option<&str>) -> Option<ProjectRef> {
    let root = root.filter(|r| !r.trim().is_empty())?;
    let p = Project::open(root).ok()?;
    Some(ProjectRef::new(p.root(), p.id()))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

#[tauri::command]
pub async fn plugins_list(state: State<'_, PluginState>, root: Option<String>) -> CmdResult<Vec<PluginInfo>> {
    let m = state.manager.clone();
    blocking(move || Ok(m.list(project_ref(root.as_deref()).as_ref()))).await
}

/// Contributions of the plugins that are ready to run.
#[tauri::command]
pub async fn plugins_catalog(state: State<'_, PluginState>, root: Option<String>) -> CmdResult<Catalog> {
    let m = state.manager.clone();
    blocking(move || Ok(m.catalog(project_ref(root.as_deref()).as_ref()))).await
}

#[tauri::command]
pub fn plugins_dir(state: State<'_, PluginState>) -> String {
    state.manager.user_dir().display().to_string()
}

/// Reads a plugin folder / `.zip` for the install prompt.
#[tauri::command]
pub async fn plugins_preview(state: State<'_, PluginState>, source: String) -> CmdResult<InstallPreview> {
    let m = state.manager.clone();
    blocking(move || m.preview(Path::new(&source)).map_err(err)).await
}

/// Installs after the user reviewed the preview; `sha256` must match what they saw.
#[tauri::command]
pub async fn plugins_install(state: State<'_, PluginState>, source: String, sha256: String, grant_secrets: Vec<String>) -> CmdResult<PluginInfo> {
    let m = state.manager.clone();
    blocking(move || m.install(Path::new(&source), Some(&sha256), &grant_secrets).map_err(err)).await
}

#[tauri::command]
pub async fn plugins_uninstall(state: State<'_, PluginState>, id: String) -> CmdResult<()> {
    let m = state.manager.clone();
    blocking(move || m.uninstall(&id).map_err(err)).await
}

/// Trusts a plugin as it is now (`sha256` = the hash shown in the prompt).
#[tauri::command]
pub async fn plugins_trust(state: State<'_, PluginState>, root: Option<String>, scope: Scope, id: String, sha256: String, grant_secrets: Vec<String>) -> CmdResult<PluginInfo> {
    let m = state.manager.clone();
    blocking(move || m.trust(project_ref(root.as_deref()).as_ref(), scope, &id, &sha256, &grant_secrets).map_err(err)).await
}

#[tauri::command]
pub async fn plugins_revoke(state: State<'_, PluginState>, root: Option<String>, scope: Scope, id: String) -> CmdResult<()> {
    let m = state.manager.clone();
    blocking(move || m.revoke(project_ref(root.as_deref()).as_ref(), scope, &id).map_err(err)).await
}

#[tauri::command]
pub async fn plugins_set_enabled(state: State<'_, PluginState>, root: Option<String>, scope: Scope, id: String, enabled: bool) -> CmdResult<()> {
    let m = state.manager.clone();
    blocking(move || m.set_enabled(project_ref(root.as_deref()).as_ref(), scope, &id, enabled).map_err(err)).await
}

#[tauri::command]
pub async fn plugins_set_hook_enabled(state: State<'_, PluginState>, root: Option<String>, scope: Scope, id: String, hook: String, enabled: bool) -> CmdResult<()> {
    let m = state.manager.clone();
    blocking(move || m.set_hook_enabled(project_ref(root.as_deref()).as_ref(), scope, &id, &hook, enabled).map_err(err)).await
}

#[tauri::command]
pub async fn plugins_set_grants(state: State<'_, PluginState>, root: Option<String>, scope: Scope, id: String, secrets: Vec<String>) -> CmdResult<()> {
    let m = state.manager.clone();
    blocking(move || m.set_granted_secrets(project_ref(root.as_deref()).as_ref(), scope, &id, &secrets).map_err(err)).await
}

/// Stores (`Some`) or deletes (`None`) a plugin secret in the OS keychain. Values never come back.
#[tauri::command]
pub async fn plugins_set_secret(id: String, name: String, value: Option<String>) -> CmdResult<()> {
    blocking(move || {
        let store = keychain().ok_or("the OS keychain is not available")?;
        let scope = devall_plugins::secret_scope(&id);
        match value {
            Some(v) => store.set(&scope, &name, &v).map_err(err),
            None => store.delete(&scope, &name).map_err(err),
        }
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UtilityRun {
    pub output: String,
    pub logs: Vec<LogLine>,
}

/// Runs a plugin utility op (`<plugin>:<op>`).
#[tauri::command]
pub async fn plugins_run_utility(state: State<'_, PluginState>, root: Option<String>, op: String, input: String, args: BTreeMap<String, String>) -> CmdResult<UtilityRun> {
    let m = state.manager.clone();
    blocking(move || {
        let (output, logs) = m.run_utility(project_ref(root.as_deref()).as_ref(), &op, &input, &args).map_err(err)?;
        Ok(UtilityRun { output, logs })
    })
    .await
}

// ---------------------------------------------------------------- import / codegen helpers

/// The best plugin importer for pasted text or a (text) file.
pub fn detect_import(manager: &PluginManager, root: Option<&str>, text: &str, file_name: Option<&str>) -> Option<ImportMatch> {
    manager.detect_import(project_ref(root).as_ref(), text, file_name)
}

/// Parses with a plugin importer into the same plan the built-in importers produce.
pub fn import_plan(manager: &PluginManager, root: Option<&str>, importer: &str, text: &str, file_name: Option<&str>) -> CmdResult<ImportPlan> {
    let out = manager.import(project_ref(root).as_ref(), importer, text, file_name).map_err(err)?;
    let name = out.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| importer.split(':').next_back().unwrap_or("Import").to_string());
    Ok(plan_from(out, &name))
}

fn plan_from(out: ImportOutput, name: &str) -> ImportPlan {
    let mut plan = ImportPlan::new(name, devall_import::Format::Plugin);
    plan.warnings = out.warnings;
    for r in out.requests {
        let mut req = HttpRequestFile::new(if r.name.trim().is_empty() { r.url.clone() } else { r.name.clone() });
        req.method = r.method.trim().to_ascii_uppercase();
        req.url = r.url;
        req.headers = r.headers.into_iter().map(|h| KeyValue::new(h.name, h.value)).collect();
        req.docs = r.docs;
        req.body = match r.body {
            None => Body::None,
            Some(ImportedBody::Json { content }) => Body::Json { content },
            Some(ImportedBody::Text { content, content_type }) => Body::Text { content, content_type },
            Some(ImportedBody::Form { fields }) => Body::Form { fields: fields.into_iter().map(|h| KeyValue::new(h.name, h.value)).collect() },
        };
        let folders: Vec<&str> = r.folder.as_deref().map(|f| f.split('/').filter(|s| !s.trim().is_empty()).collect()).unwrap_or_default();
        let mut items = &mut plan.items;
        for f in folders {
            let idx = match items.iter().position(|i| matches!(i, PlanItem::Folder(x) if x.name == f)) {
                Some(i) => i,
                None => {
                    items.push(PlanItem::Folder(PlanFolder::new(f)));
                    items.len() - 1
                }
            };
            items = match &mut items[idx] {
                PlanItem::Folder(x) => &mut x.items,
                PlanItem::Request(_) => unreachable!("index points at a folder"),
            };
        }
        items.push(PlanItem::Request(Box::new(req)));
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use devall_plugins::{Header, ImportedRequest};

    #[test]
    fn import_output_becomes_a_plan_with_folders() {
        let out = ImportOutput {
            name: Some("API".into()),
            requests: vec![
                ImportedRequest { name: "List".into(), method: "get".into(), url: "https://x/items".into(), folder: Some("items/v1".into()), ..Default::default() },
                ImportedRequest {
                    name: "Create".into(),
                    method: "POST".into(),
                    url: "https://x/items".into(),
                    headers: vec![Header::new("A", "b")],
                    body: Some(ImportedBody::Json { content: "{}".into() }),
                    folder: Some("items/v1".into()),
                    ..Default::default()
                },
                ImportedRequest { name: "Health".into(), url: "https://x/health".into(), method: "GET".into(), ..Default::default() },
            ],
            warnings: vec!["w".into()],
        };
        let plan = plan_from(out, "API");
        assert_eq!(plan.request_count(), 3);
        assert_eq!(plan.folder_count(), 2);
        let reqs = plan.requests();
        assert_eq!(reqs[0].0, vec!["items".to_string(), "v1".to_string()]);
        assert_eq!(reqs[0].1.method, "GET");
        assert!(matches!(reqs[1].1.body, Body::Json { .. }));
        assert!(reqs[2].0.is_empty());
        assert_eq!(plan.warnings, vec!["w"]);
    }
}
