//! [`devall_http::plugins::RequestPlugins`] backed by a [`PluginManager`]: plugin auth providers
//! and pre-request / post-response hooks for every send (app and CLI).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use devall_http::checks::AssertionResult;
use devall_http::plugins::{PluginAuthOutput, PostHookOutcome, PreHookOutcome, RequestPlugins};
use devall_http::{HttpResponseData, PreparedBody, PreparedRequest};

use crate::contract::{Header, PluginRequest, PluginResponse};
use crate::manager::{PluginManager, ProjectRef};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub struct HttpBridge {
    manager: Arc<PluginManager>,
    projects: Mutex<HashMap<PathBuf, String>>,
}

impl HttpBridge {
    pub fn new(manager: Arc<PluginManager>) -> Self {
        Self { manager, projects: Mutex::new(HashMap::new()) }
    }

    /// The project (id from `devall.yaml`; the folder path for folders without one).
    pub fn project(&self, root: &Path) -> ProjectRef {
        let mut cache = self.projects.lock().unwrap_or_else(|e| e.into_inner());
        let id = cache
            .entry(root.to_path_buf())
            .or_insert_with(|| devall_core::Project::open(root).map(|p| p.id().to_string()).unwrap_or_else(|_| root.display().to_string()))
            .clone();
        ProjectRef::new(root, id)
    }
}

fn body_of(b: &PreparedBody) -> (Option<String>, bool) {
    match b {
        PreparedBody::None | PreparedBody::Multipart(_) => (None, false),
        PreparedBody::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(t) => (Some(t.to_string()), false),
            Err(_) => (Some(B64.encode(bytes)), true),
        },
        PreparedBody::Form(pairs) => (Some(url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs.iter()).finish()), false),
    }
}

/// The resolved request as plugins see it (multipart bodies are not passed).
pub fn to_plugin_request(r: &PreparedRequest) -> PluginRequest {
    let (body, body_base64) = body_of(&r.body);
    PluginRequest { method: r.method.clone(), url: r.url.clone(), headers: r.headers.iter().map(|(k, v)| Header::new(k, v)).collect(), body, body_base64 }
}

/// Applies a hook's edits; the body is replaced only when the hook changed it.
fn apply(r: &mut PreparedRequest, before: &PluginRequest, after: PluginRequest) -> Result<(), String> {
    r.method = after.method.trim().to_ascii_uppercase();
    r.url = after.url;
    r.headers = after.headers.into_iter().map(|h| (h.name, h.value)).collect();
    if after.body != before.body || after.body_base64 != before.body_base64 {
        r.body = match after.body {
            None => PreparedBody::None,
            Some(b) if after.body_base64 => PreparedBody::Bytes(B64.decode(b).map_err(|e| format!("hook returned an invalid base64 body: {e}"))?),
            Some(b) => PreparedBody::Bytes(b.into_bytes()),
        };
    }
    Ok(())
}

pub fn to_plugin_response(resp: &HttpResponseData) -> PluginResponse {
    let (body, body_base64) = match std::str::from_utf8(&resp.body) {
        Ok(t) => (Some(t.to_string()).filter(|t| !t.is_empty()), false),
        Err(_) => (Some(B64.encode(&resp.body)), true),
    };
    PluginResponse {
        status: resp.status,
        headers: resp.headers.iter().map(|(k, v)| Header::new(k, v)).collect(),
        body,
        body_base64,
        time_ms: resp.timings.total_ms.max(0.0) as u64,
    }
}

impl RequestPlugins for HttpBridge {
    fn has_hooks(&self, project_root: &Path, phase: devall_script::ScriptPhase) -> bool {
        let event = match phase {
            devall_script::ScriptPhase::PreRequest => crate::manifest::HookEvent::PreRequest,
            devall_script::ScriptPhase::PostResponse => crate::manifest::HookEvent::PostResponse,
        };
        self.manager.catalog(Some(&self.project(project_root))).hooks.iter().any(|h| h.enabled && h.events.contains(&event))
    }

    fn authorize(&self, project_root: &Path, plugin: &str, provider: &str, config: &BTreeMap<String, String>, req: &PreparedRequest) -> Result<PluginAuthOutput, String> {
        let project = self.project(project_root);
        let out = self.manager.auth(Some(&project), plugin, provider, config, &to_plugin_request(req)).map_err(|e| e.to_string())?;
        Ok(PluginAuthOutput { headers: out.headers.into_iter().map(|h| (h.name, h.value)).collect(), query: out.query.into_iter().map(|h| (h.name, h.value)).collect() })
    }

    fn pre_request(&self, project_root: &Path, req: &mut PreparedRequest, request_name: &str, environment: Option<&str>) -> PreHookOutcome {
        let project = self.project(project_root);
        let before = to_plugin_request(req);
        let run = self.manager.run_pre_request(Some(&project), before.clone(), Some(request_name), environment);
        let mut out = PreHookOutcome { logs: run.logs, errors: run.errors };
        if run.changed
            && let Err(e) = apply(req, &before, run.request)
        {
            out.errors.push(e);
        }
        out
    }

    fn post_response(&self, project_root: &Path, req: &PreparedRequest, resp: &HttpResponseData, request_name: &str) -> PostHookOutcome {
        let project = self.project(project_root);
        let run = self.manager.run_post_response(Some(&project), &to_plugin_request(req), &to_plugin_response(resp), Some(request_name));
        PostHookOutcome {
            tests: run.tests.into_iter().map(|t| AssertionResult { label: t.name, passed: t.passed, actual: None, message: t.message }).collect(),
            variables: run.variables.into_iter().collect(),
            logs: run.logs,
            errors: run.errors,
        }
    }
}
