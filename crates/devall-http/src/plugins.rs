//! Integration points for WebAssembly plugins (`crates/devall-plugins`) without a dependency on the
//! plugin runtime: the desktop app and the CLI [`install`] a [`RequestPlugins`] at startup, and
//! devall-http calls it for `auth: { type: plugin }` (in [`crate::prepare_with`]) and around every
//! send of a collection run (pre-request / post-response hooks). With nothing installed, plugin
//! auth fails with a clear message and hooks are skipped.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use devall_script::{LogLevel, LogLine, ScriptPhase, ScriptReport};

use crate::checks::AssertionResult;
use crate::{HttpResponseData, PreparedRequest};

/// What an auth provider adds to a request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginAuthOutput {
    /// Set (replacing same-named headers).
    pub headers: Vec<(String, String)>,
    /// Appended to the URL's query.
    pub query: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct PreHookOutcome {
    pub logs: Vec<String>,
    /// A failing hook fails the send (like a failing pre-request script).
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PostHookOutcome {
    pub tests: Vec<AssertionResult>,
    /// Runtime variables to set.
    pub variables: Vec<(String, String)>,
    pub logs: Vec<String>,
    /// Shown as failed test rows.
    pub errors: Vec<String>,
}

pub trait RequestPlugins: Send + Sync {
    /// Whether any enabled hook would run for this phase (lets callers skip copying bodies).
    fn has_hooks(&self, project_root: &Path, phase: devall_script::ScriptPhase) -> bool;
    /// Runs auth provider `provider` of plugin `plugin` for a resolved request.
    fn authorize(&self, project_root: &Path, plugin: &str, provider: &str, config: &BTreeMap<String, String>, req: &PreparedRequest) -> Result<PluginAuthOutput, String>;
    /// Runs every enabled pre-request hook; may edit `req`.
    fn pre_request(&self, project_root: &Path, req: &mut PreparedRequest, request_name: &str, environment: Option<&str>) -> PreHookOutcome;
    /// Runs every enabled post-response hook.
    fn post_response(&self, project_root: &Path, req: &PreparedRequest, resp: &HttpResponseData, request_name: &str) -> PostHookOutcome;
}

static INSTALLED: RwLock<Option<Arc<dyn RequestPlugins>>> = RwLock::new(None);

/// Installs the process-wide plugin host (replacing any previous one).
pub fn install(plugins: Arc<dyn RequestPlugins>) {
    *INSTALLED.write().unwrap_or_else(|e| e.into_inner()) = Some(plugins);
}

pub fn installed() -> Option<Arc<dyn RequestPlugins>> {
    INSTALLED.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Hook log lines as script console lines (source `plugin`).
pub fn report(phase: ScriptPhase, logs: Vec<String>) -> ScriptReport {
    ScriptReport { logs: logs.into_iter().map(|message| LogLine { phase, source: "plugin".into(), level: LogLevel::Info, message }).collect(), ..Default::default() }
}

/// Failed hooks as failing test rows.
pub fn error_rows(errors: &[String]) -> Vec<AssertionResult> {
    errors.iter().map(|e| AssertionResult { label: "Plugin hook".into(), passed: false, actual: None, message: Some(e.clone()) }).collect()
}

/// Applies an auth provider's output to a prepared request.
pub(crate) fn apply_auth(req: &mut PreparedRequest, out: PluginAuthOutput) -> Result<(), String> {
    for (k, v) in out.headers {
        req.headers.retain(|(h, _)| !h.eq_ignore_ascii_case(&k));
        req.headers.push((k, v));
    }
    if !out.query.is_empty() {
        let mut url = url::Url::parse(&req.url).map_err(|e| e.to_string())?;
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in &out.query {
                q.append_pair(k, v);
            }
        }
        req.url = url.to_string();
    }
    Ok(())
}
