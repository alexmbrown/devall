//! Import / export / code generation commands — a thin layer over `crates/devall-import`.
//!
//! Import reads Postman (v2.0/v2.1 + environments), Insomnia (v4 JSON, v5 YAML), Bruno folders
//! and `.bru` files, OpenCollection YAML, OpenAPI 3.0/3.1 + Swagger 2.0, HAR, `.http`/`.rest`,
//! Hurl and cURL. Export writes OpenAPI 3.1, Postman v2.1 and `.http`. Code generation covers
//! 15 languages from the same resolved request the app would send.
//!
//! # Integration (for whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    `devall-import = { path = "../../../crates/devall-import" }`
//!    (or add `devall-import = { path = "crates/devall-import" }` to the root
//!    `[workspace.dependencies]` and use `devall-import.workspace = true`).
//! 2. `src/modules/mod.rs`: `pub mod import;`
//! 3. No managed state of its own (`codegen` uses the existing `AppState` for secrets and runtime
//!    variables, and `crate::commands::resolver_for`).
//! 4. `src/lib.rs`, inside `tauri::generate_handler![…]`:
//!    ```text
//!    modules::import::import_detect,
//!    modules::import::import_preview,
//!    modules::import::import_apply,
//!    modules::import::export_openapi,
//!    modules::import::export_postman,
//!    modules::import::export_http_file,
//!    modules::import::export_hurl,
//!    modules::import::export_write,
//!    modules::import::codegen_languages,
//!    modules::import::codegen,
//!    ```
//! 5. Capabilities: `dialog:allow-open` and `dialog:allow-save` (already in `capabilities/default.json`).
//! 6. Frontend: `ImportDialog`, `ExportDialog`, `GenerateCodeModal` from `src/features/import`
//!    (typed wrappers in `src/lib/api/import.ts`). Detected secret values are stored with the
//!    existing `set_secret` command, only when the user confirms.

use std::path::{Path, PathBuf};

use devall_core::secrets::SecretResolver;
use devall_core::vars::redact_values;
use devall_core::{Auth, HttpRequestFile, Project};
use devall_http::PrepareOptions;
use devall_import::codegen::{self, Language, LanguageInfo};
use devall_import::export::{self, ExportOutput};
use devall_import::{ApplyOptions, ApplyReport, Format, ParseOptions, PlanSummary};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// What the UI hands over: pasted text, or a file/folder path (from the picker or a drop).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSource {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub options: ParseOptions,
    /// A plugin importer (`<plugin>:<importer>`) instead of the built-in formats.
    #[serde(default)]
    pub plugin: Option<String>,
    /// The open project (for project plugins).
    #[serde(default)]
    pub root: Option<String>,
}

/// Largest text file offered to plugin importers.
const MAX_PLUGIN_INPUT: u64 = 16 * 1024 * 1024;

fn read_text_file(path: &Path) -> CmdResult<String> {
    let len = std::fs::metadata(path).map_err(err)?.len();
    if len > MAX_PLUGIN_INPUT {
        return Err(format!("{} is too large for a plugin importer", path.display()));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

impl ImportSource {
    fn parse(&self, plugins: &devall_plugins::PluginManager) -> CmdResult<devall_import::ImportPlan> {
        let path = self.path.as_deref().filter(|p| !p.trim().is_empty()).map(PathBuf::from);
        if let Some(importer) = self.plugin.as_deref().filter(|p| !p.is_empty()) {
            let (text, name) = match &path {
                Some(p) => (read_text_file(p)?, p.file_name().map(|n| n.to_string_lossy().into_owned())),
                None => (self.text.clone().unwrap_or_default(), None),
            };
            return crate::modules::plugins::import_plan(plugins, self.root.as_deref(), importer, &text, name.as_deref());
        }
        devall_import::parse_source(self.text.as_deref(), path.as_deref(), &self.options).map_err(err)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectResult {
    pub format: Option<Format>,
    pub label: Option<String>,
    /// The argument was an existing file or folder path.
    pub is_path: bool,
    pub is_dir: bool,
    /// A plugin importer that recognised it (only when no built-in format did).
    pub plugin: Option<devall_plugins::ImportMatch>,
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

/// Detects the format of pasted text, or of a file/folder when the argument is an existing path.
/// Plugin importers are asked when no built-in format matches.
#[tauri::command]
pub async fn import_detect(plugins: State<'_, crate::modules::plugins::PluginState>, text_or_path: String, root: Option<String>) -> CmdResult<DetectResult> {
    let manager = plugins.manager.clone();
    blocking(move || {
        let trimmed = text_or_path.trim();
        let as_path = (!trimmed.contains('\n') && !trimmed.is_empty()).then(|| Path::new(trimmed)).filter(|p| p.exists());
        let (format, is_path, is_dir) = match as_path {
            Some(p) => (devall_import::detect_path(p).or_else(|| if p.is_file() { read_text_file(p).ok().and_then(|t| devall_import::detect(&t)) } else { None }), true, p.is_dir()),
            None => (devall_import::detect(&text_or_path), false, false),
        };
        let plugin = match (format, as_path) {
            (Some(_), _) => None,
            (None, Some(p)) if p.is_file() => read_text_file(p)
                .ok()
                .and_then(|t| crate::modules::plugins::detect_import(&manager, root.as_deref(), &t, p.file_name().and_then(|n| n.to_str()))),
            (None, Some(_)) => None,
            (None, None) => crate::modules::plugins::detect_import(&manager, root.as_deref(), &text_or_path, None),
        };
        let label = format.map(|f| f.label().to_string()).or_else(|| plugin.as_ref().map(|m| format!("{} (plugin)", m.name)));
        Ok(DetectResult { label, format: format.or(plugin.as_ref().map(|_| Format::Plugin)), is_path, is_dir, plugin })
    })
    .await
}

/// Parses the source and returns a preview: folder/request tree, environments, variable and
/// secret *names* (never values) and warnings.
#[tauri::command]
pub async fn import_preview(plugins: State<'_, crate::modules::plugins::PluginState>, source: ImportSource) -> CmdResult<PlanSummary> {
    let manager = plugins.manager.clone();
    blocking(move || Ok(source.parse(&manager)?.summary())).await
}

/// Parses the source again and writes it into the project below `target_folder` (relative to
/// `http/`, empty = root). The report's `secrets` carry the detected credential values so the UI
/// can offer to store them with `set_secret`; they are never written to project files.
#[tauri::command]
pub async fn import_apply(
    plugins: State<'_, crate::modules::plugins::PluginState>,
    root: String,
    source: ImportSource,
    target_folder: String,
    options: Option<ApplyOptions>,
) -> CmdResult<ApplyReport> {
    let manager = plugins.manager.clone();
    blocking(move || {
        let plan = source.parse(&manager)?;
        let mut project = Project::open(&root).map_err(err)?;
        devall_import::apply(plan, &mut project, &target_folder, &options.unwrap_or_default()).map_err(err)
    })
    .await
}

fn folder_arg(folder: Option<String>) -> Option<String> {
    folder.filter(|f| !f.trim().is_empty())
}

/// OpenAPI 3.1 YAML for the project or one folder; `env` limits `servers` to one environment.
#[tauri::command]
pub async fn export_openapi(root: String, folder: Option<String>, env: Option<String>) -> CmdResult<ExportOutput> {
    blocking(move || {
        let p = Project::open(&root).map_err(err)?;
        export::to_openapi(&p, folder_arg(folder).as_deref(), env.as_deref().filter(|e| !e.is_empty())).map_err(err)
    })
    .await
}

/// Postman Collection v2.1 JSON; `attachments` holds one `*.postman_environment.json` per environment.
#[tauri::command]
pub async fn export_postman(root: String, folder: Option<String>) -> CmdResult<ExportOutput> {
    blocking(move || {
        let p = Project::open(&root).map_err(err)?;
        export::to_postman(&p, folder_arg(folder).as_deref()).map_err(err)
    })
    .await
}

/// JetBrains-compatible `.http` file; `attachments` holds `http-client.env.json` and
/// `http-client.private.env.json` (secret names with empty values).
#[tauri::command]
pub async fn export_http_file(root: String, folder: Option<String>) -> CmdResult<ExportOutput> {
    blocking(move || {
        let p = Project::open(&root).map_err(err)?;
        export::to_http_file(&p, folder_arg(folder).as_deref()).map_err(err)
    })
    .await
}

/// Hurl file (+ one `<environment>.hurl.env` variables file per environment); assertions and
/// captures become `[Asserts]` / `[Captures]`.
#[tauri::command]
pub async fn export_hurl(root: String, folder: Option<String>) -> CmdResult<ExportOutput> {
    blocking(move || {
        let p = Project::open(&root).map_err(err)?;
        export::to_hurl(&p, folder_arg(folder).as_deref()).map_err(err)
    })
    .await
}

/// Writes an export to a path the user picked in the save dialog.
#[tauri::command]
pub async fn export_write(dest: String, content: String) -> CmdResult<()> {
    blocking(move || {
        let path = PathBuf::from(&dest);
        if !path.is_absolute() {
            return Err(format!("not an absolute path: {dest}"));
        }
        if path.is_dir() {
            return Err(format!("{dest} is a folder"));
        }
        match path.parent() {
            Some(parent) if parent.is_dir() => {}
            _ => return Err(format!("folder does not exist for {dest}")),
        }
        devall_core::fsutil::atomic_write(&path, content.as_bytes()).map_err(err)
    })
    .await
}

#[tauri::command]
pub fn codegen_languages() -> Vec<LanguageInfo> {
    codegen::languages()
}

/// Resolves the editor's request like a send (for code generation). OAuth 2.0 requests get a
/// placeholder access token. Returns the request and the secret values to mask.
fn prepare_for_codegen(
    state: &AppState,
    root: &str,
    path: &str,
    request: &HttpRequestFile,
    env: Option<String>,
) -> CmdResult<(devall_http::PreparedRequest, std::collections::BTreeSet<String>)> {
    let p = Project::open(root).map_err(err)?;
    let env = env.filter(|e| !e.is_empty());
    let env_file = env.as_deref().map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let eff = p.effective_request(path, request).map_err(err)?;
    let secrets = SecretResolver::for_environment(state.secrets.as_ref(), p.id(), env.as_deref(), p.root(), env_file.as_ref(), &state.providers);
    let runtime = state.runtime_vars(root);
    let mut r = crate::commands::resolver_for(&p, &runtime, env.as_deref(), env_file.as_ref(), &eff.folder_variables, &secrets);
    let opts = PrepareOptions {
        oauth2_token: matches!(eff.request.auth, Auth::OAuth2(_)).then(|| "<oauth2-access-token>".to_string()),
        ..Default::default()
    };
    let prepared = devall_http::prepare_with(&eff.request, &mut r, p.root(), &opts).map_err(|e| err(e.redacted(&r.secret_values)))?;
    Ok((prepared, r.secret_values.clone()))
}

/// Generates code for the editor's current request (`path` locates it for folder inheritance; may
/// be empty for drafts), resolved like a send with environment `env`. Secret values are masked
/// unless `include_secrets` is set. OAuth 2.0 requests get a placeholder access token.
#[tauri::command]
pub async fn codegen(
    state: State<'_, AppState>,
    root: String,
    path: String,
    request: HttpRequestFile,
    env: Option<String>,
    language: Language,
    include_secrets: bool,
) -> CmdResult<String> {
    let (prepared, secret_values) = prepare_for_codegen(&state, &root, &path, &request, env)?;
    if include_secrets {
        return Ok(codegen::generate(&prepared, language));
    }
    let masked = codegen::redact(&prepared, &secret_values);
    Ok(redact_values(&codegen::generate(&masked, language), &secret_values))
}

/// Like [`codegen`] with a plugin codegen target (`<plugin>:<target>`). The plugin sees the
/// request with secret values masked unless `include_secrets` is set.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn codegen_plugin(
    state: State<'_, AppState>,
    plugins: State<'_, crate::modules::plugins::PluginState>,
    root: String,
    path: String,
    request: HttpRequestFile,
    env: Option<String>,
    target: String,
    include_secrets: bool,
) -> CmdResult<String> {
    let (prepared, secret_values) = prepare_for_codegen(&state, &root, &path, &request, env)?;
    let req = if include_secrets { prepared } else { codegen::redact(&prepared, &secret_values) };
    let manager = plugins.manager.clone();
    blocking(move || {
        let project = crate::modules::plugins::project_ref(Some(&root));
        let code = manager.codegen(project.as_ref(), &target, &devall_plugins::to_plugin_request(&req)).map_err(err)?;
        Ok(if include_secrets { code } else { redact_values(&code, &secret_values) })
    })
    .await
}
