//! OpenAPI commands: lint, spec ↔ mock ↔ traffic drift, and folders linked to a spec (re-import
//! in place when it changes) — thin wrappers over `crates/devall-openapi` and
//! `devall_import::sync`.
//!
//! Registration: `pub mod openapi;` and the `openapi_*` commands in `generate_handler!`. Uses the
//! existing `ProxyState` (inspector captures) and `AppState` (request history); no state of its
//! own. Link changes are written to the folder's `_folder.yaml` (`openapi: { source, autoSync }`).

use std::path::{Path, PathBuf};

use devall_import::sync::{self, LinkedFolder, OpenApiLink, SyncReport};
use devall_openapi::{Drift, DriftReport, LintReport, MockInput, Spec, TrafficOptions, TrafficSample};
use devall_proxy::ExchangeId;
use serde::Deserialize;
use serde_json::Value;
use tauri::State;

use crate::modules::proxy::ProxyState;
use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

/// A project-relative or absolute path.
fn resolve(root: &str, path: &str) -> CmdResult<PathBuf> {
    let p = Path::new(path);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    devall_core::fsutil::safe_join(Path::new(root), path).map_err(err)
}

fn open(root: &str) -> CmdResult<devall_core::Project> {
    devall_core::Project::open(root).map_err(err)
}

/// OpenAPI files in the project (YAML/JSON that look like OpenAPI 3), project-relative.
#[tauri::command]
pub async fn openapi_specs(root: String) -> CmdResult<Vec<String>> {
    blocking(move || Ok(devall_openapi::find_specs(Path::new(&root)))).await
}

/// Lints a spec file (`path`, project-relative or absolute) or pasted `text`.
#[tauri::command]
pub async fn openapi_lint(root: Option<String>, path: Option<String>, text: Option<String>) -> CmdResult<LintReport> {
    blocking(move || {
        let text = match (path.filter(|p| !p.trim().is_empty()), text) {
            (Some(p), _) => {
                let full = resolve(root.as_deref().unwrap_or(""), &p)?;
                std::fs::read_to_string(&full).map_err(|e| format!("{}: {e}", full.display()))?
            }
            (None, Some(t)) => t,
            _ => return Err("nothing to lint".into()),
        };
        Ok(devall_openapi::lint_text(&text))
    })
    .await
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DriftOptions {
    /// Check `mocks/mappings/`.
    pub mocks: bool,
    /// Check the inspector's captures (all of them, or `inspectorIds`).
    pub inspector: bool,
    pub inspector_ids: Vec<ExchangeId>,
    /// Check the project's request history (latest 500 sends).
    pub history: bool,
    /// HAR file to check (absolute or project-relative).
    pub har: Option<String>,
    /// Check traffic to every host, not only the spec's servers.
    pub all_hosts: bool,
}

fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
}

fn body_text(raw: &[u8], headers: &[(String, String)]) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let decoded = match header(headers, "content-encoding") {
        Some(enc) => devall_proxy::decode_body(raw, &enc).ok()?,
        None => raw.to_vec(),
    };
    String::from_utf8(decoded).ok()
}

fn exchange_sample(ex: &devall_proxy::Exchange) -> TrafficSample {
    let resp = ex.response.as_ref();
    TrafficSample {
        id: ex.id.to_string(),
        url: ex.request.url.clone(),
        method: ex.request.method.clone(),
        status: resp.map(|r| r.status),
        request_content_type: header(&ex.request.headers, "content-type"),
        request_body: body_text(&ex.request.body.raw, &ex.request.headers),
        response_content_type: resp.and_then(|r| header(&r.headers, "content-type")),
        response_body: resp.and_then(|r| body_text(&r.body.raw, &r.headers)),
    }
}

fn history_samples(state: &AppState, root: &str) -> CmdResult<Vec<TrafficSample>> {
    let h = state.history(root)?;
    let mut out = Vec::new();
    for s in h.list(None, 500).map_err(err)? {
        let Some((e, body)) = h.get(&s.id).map_err(err)? else { continue };
        let sent_headers: Vec<(String, String)> = e
            .sent
            .get("headers")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        out.push(TrafficSample {
            id: e.id.clone(),
            url: e.url.clone(),
            method: e.method.clone(),
            status: e.status,
            request_content_type: header(&sent_headers, "content-type"),
            request_body: e.sent.get("body").and_then(Value::as_str).map(str::to_string),
            response_content_type: e.content_type.clone().or_else(|| header(&e.response_headers, "content-type")),
            response_body: String::from_utf8(body).ok(),
        });
    }
    Ok(out)
}

/// Checks mocks and/or traffic against the spec at `spec` (project-relative or absolute).
#[tauri::command]
pub async fn openapi_drift(proxy: State<'_, ProxyState>, app: State<'_, AppState>, root: String, spec: String, options: DriftOptions) -> CmdResult<DriftReport> {
    // Gather traffic on this thread (state handles aren't Send); the checks run on a worker.
    let inspector: Vec<TrafficSample> = if options.inspector {
        let ex = crate::modules::proxy::captured(&proxy, &options.inspector_ids)?;
        ex.iter().filter(|e| e.response.is_some()).map(exchange_sample).collect()
    } else {
        Vec::new()
    };
    let history = if options.history { history_samples(&app, &root)? } else { Vec::new() };
    blocking(move || {
        let path = resolve(&root, &spec)?;
        let s = Spec::load(&path).map_err(err)?;
        let mut d = Drift::new(&s);
        if options.mocks {
            let pm = devall_mock::load_project(Path::new(&root));
            let inputs: Vec<MockInput> = pm.mappings.into_iter().map(|m| MockInput { id: m.id, name: m.name, file: m.file, mapping: m.mapping }).collect();
            d.add_mocks(&inputs, &devall_openapi::mock_body_reader(Path::new(&root)));
        }
        let opts = TrafficOptions { all_hosts: options.all_hosts };
        if !inspector.is_empty() {
            d.add_traffic("inspector", &inspector, &opts);
        }
        if !history.is_empty() {
            d.add_traffic("history", &history, &opts);
        }
        if let Some(har) = options.har.filter(|h| !h.trim().is_empty()) {
            let hp = resolve(&root, &har)?;
            let text = std::fs::read_to_string(&hp).map_err(|e| format!("{}: {e}", hp.display()))?;
            let samples = devall_openapi::har::parse_har(&text).map_err(err)?;
            d.add_traffic(&hp.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "HAR".into()), &samples, &opts);
        }
        Ok(d.finish())
    })
    .await
}

// ---------- linked specs ----------

/// Folders linked to a spec.
#[tauri::command]
pub async fn openapi_links(root: String) -> CmdResult<Vec<LinkedFolder>> {
    blocking(move || sync::links(&open(&root)?).map_err(err)).await
}

/// Links `folder` (under `http/`) to `source` (a path or URL). `resync` re-imports right away.
#[tauri::command]
pub async fn openapi_link(root: String, folder: String, source: String, auto_sync: bool, resync: Option<bool>) -> CmdResult<(OpenApiLink, Option<SyncReport>)> {
    let link = {
        let (root, folder, source) = (root.clone(), folder.clone(), source.clone());
        blocking(move || sync::link(&open(&root)?, &folder, &source, auto_sync).map_err(err)).await?
    };
    let report = if resync.unwrap_or(false) { Some(openapi_resync(root, folder).await?) } else { None };
    Ok((link, report))
}

#[tauri::command]
pub async fn openapi_unlink(root: String, folder: String) -> CmdResult<()> {
    blocking(move || sync::unlink(&open(&root)?, &folder).map_err(err)).await
}

/// Fetches a spec URL (a user-triggered network call).
async fn fetch(url: &str) -> CmdResult<String> {
    let req = devall_http::PreparedRequest {
        method: "GET".into(),
        url: url.to_string(),
        headers: vec![("Accept".into(), "application/yaml, application/json;q=0.9, */*;q=0.5".into())],
        body: devall_http::PreparedBody::None,
        timeout_ms: 30_000,
        follow_redirects: true,
        verify_tls: true,
        proxy: None,
        client_cert: None,
        digest: None,
        ntlm: None,
    };
    let resp = devall_http::ClientPool::new().execute(&req).await.map_err(err)?;
    if resp.status >= 400 {
        return Err(format!("{url} answered {} {}", resp.status, resp.status_text));
    }
    String::from_utf8(resp.body).map_err(|_| format!("{url} did not return text"))
}

/// Re-imports the folder's linked spec in place (requests updated by name, new ones added;
/// requests whose operation is gone are listed in `stale`, not deleted).
#[tauri::command]
pub async fn openapi_resync(root: String, folder: String) -> CmdResult<SyncReport> {
    let link = {
        let (root, folder) = (root.clone(), folder.clone());
        blocking(move || sync::read_link(&open(&root)?, &folder).map_err(err)).await?
    }
    .ok_or_else(|| format!("http/{folder} is not linked to an OpenAPI spec"))?;
    let text = if link.is_url() { Some(fetch(&link.source).await?) } else { None };
    blocking(move || {
        let mut p = open(&root)?;
        match text {
            Some(t) => sync::resync_text(&mut p, &folder, &t),
            None => sync::resync(&mut p, &folder),
        }
        .map_err(err)
    })
    .await
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSyncResult {
    pub folder: String,
    pub report: Option<SyncReport>,
    pub error: Option<String>,
}

/// Re-syncs the auto-sync folders whose spec is among `paths` (project-relative paths from the
/// `project-changed` event). Per-folder failures are reported, not raised.
#[tauri::command]
pub async fn openapi_autosync(root: String, paths: Vec<String>) -> CmdResult<Vec<AutoSyncResult>> {
    blocking(move || {
        let mut p = open(&root)?;
        let affected = sync::affected_by(&p, &paths).map_err(err)?;
        Ok(affected
            .iter()
            .map(|l| match sync::resync(&mut p, &l.folder) {
                Ok(r) => AutoSyncResult { folder: l.folder.clone(), report: Some(r), error: None },
                Err(e) => AutoSyncResult { folder: l.folder.clone(), report: None, error: Some(e.to_string()) },
            })
            .collect())
    })
    .await
}
