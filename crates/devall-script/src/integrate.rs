//! Glue between the project model and the engine: which scripts apply to a request, how a request
//! file maps to the script view and back, and the two entry points for the send pipeline.
//!
//! ```text
//! effective request ──apply_pre_request──▶ edited request + runtime vars ──prepare/send──▶ response
//!                                                                 │
//!                       tests, captures ◀──apply_post_response────┘
//! ```

use std::collections::BTreeSet;

use devall_core::fsutil::{normalize_rel, rel_parent};
use devall_core::vars::SecretLookup;
use devall_core::{Body, HttpRequestFile, KeyValue, Project};

use crate::{
    PostResponseInput, PreRequestInput, RequestView, ResponseInfo, ScriptEngine, ScriptError, ScriptInfo, ScriptPhase, ScriptReport, ScriptSource,
    ScriptTestResult, ScriptVars, VarChange, Variables,
};

/// The scripts that apply to one request, in execution order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptChain {
    pub pre: Vec<ScriptSource>,
    pub post: Vec<ScriptSource>,
}

impl ScriptChain {
    pub fn is_empty(&self) -> bool {
        self.pre.is_empty() && self.post.is_empty()
    }
}

/// Folder scripts from `http/_folder.yaml` down to the request's own folder (outermost first),
/// then the request's own scripts. `rel` is the request path under `http/` (empty for an unsaved
/// draft, which only inherits the root folder's scripts). Blank scripts are left out.
pub fn collect_scripts(project: &Project, rel: &str, request: &HttpRequestFile) -> devall_core::Result<ScriptChain> {
    let rel = normalize_rel(rel);
    let mut folders = Vec::new(); // nearest first, like Project::effective_request
    let mut folder = rel_parent(&rel);
    loop {
        folders.push((folder.clone(), project.read_folder(&folder)?));
        if folder.is_empty() {
            break;
        }
        folder = rel_parent(&folder);
    }
    let mut chain = ScriptChain::default();
    let mut push = |name: String, scripts: &Option<devall_core::Scripts>| {
        if let Some(s) = scripts {
            if !s.pre_request.trim().is_empty() {
                chain.pre.push(ScriptSource::new(name.clone(), s.pre_request.clone()));
            }
            if !s.post_response.trim().is_empty() {
                chain.post.push(ScriptSource::new(name, s.post_response.clone()));
            }
        }
    };
    for (path, f) in folders.iter().rev() {
        push(if path.is_empty() { "collection".into() } else { format!("folder {path}") }, &f.scripts);
    }
    push("request".into(), &request.scripts);
    Ok(chain)
}

/// Literal names passed to `pm.secrets.get('…')` / `pm.vault.get('…')` in `code`. Only these are
/// looked up and handed to the script (computed names are not supported, by design).
pub fn secret_names(code: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for pat in ["secrets.get(", "vault.get("] {
        let mut offset = 0;
        while let Some(i) = code[offset..].find(pat) {
            let start = offset + i + pat.len();
            offset = start;
            let after = code[start..].trim_start();
            let Some(q) = after.chars().next().filter(|c| matches!(c, '\'' | '"' | '`')) else { continue };
            let Some(end) = after[1..].find(q) else { continue };
            let name = after[1..1 + end].trim();
            if !name.is_empty() && !name.contains("${") && !name.contains('\n') && !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// Everything besides the request that scripts can see. Build it from what the send pipeline
/// already has (environment file, `EffectiveRequest::folder_variables`, manifest variables).
#[derive(Clone, Copy, Default)]
pub struct ScriptEnv<'a> {
    /// The active environment's variables.
    pub environment: Option<&'a Variables>,
    pub environment_name: Option<&'a str>,
    /// Nearest folder first.
    pub folder_variables: &'a [Variables],
    pub project_variables: Option<&'a Variables>,
    /// Data-file row of the current iteration (collection runs).
    pub iteration_data: Option<&'a Variables>,
    /// 0-based.
    pub iteration: usize,
    pub iteration_count: usize,
    /// Request path under `http/` (for `pm.execution.location`).
    pub request_path: &'a str,
    /// Enables `pm.secrets.get('NAME')` for literal names found in the scripts.
    pub secrets: Option<&'a dyn SecretLookup>,
}

impl ScriptEnv<'_> {
    pub fn vars(&self, runtime: &Variables) -> ScriptVars {
        ScriptVars {
            runtime: runtime.clone(),
            iteration_data: self.iteration_data.cloned().unwrap_or_default(),
            environment: self.environment.cloned().unwrap_or_default(),
            folders: self.folder_variables.to_vec(),
            project: self.project_variables.cloned().unwrap_or_default(),
        }
    }

    pub fn info(&self, request: &HttpRequestFile) -> ScriptInfo {
        ScriptInfo {
            request_name: request.name.clone(),
            request_id: request.id.clone(),
            request_path: self.request_path.to_string(),
            iteration: self.iteration,
            iteration_count: self.iteration_count.max(1),
            environment_name: self.environment_name.map(str::to_string),
        }
    }

    fn secrets_for(&self, scripts: &[ScriptSource]) -> Variables {
        let mut out = Variables::new();
        let Some(lookup) = self.secrets else { return out };
        for s in scripts {
            for name in secret_names(&s.code) {
                if !out.contains_key(&name)
                    && let Some(v) = lookup.lookup(&name)
                {
                    out.insert(name, v);
                }
            }
        }
        out
    }
}

fn decode_component(s: &str) -> String {
    let plus = s.replace('+', " ");
    percent_encoding::percent_decode_str(&plus).decode_utf8_lossy().into_owned()
}

impl From<&HttpRequestFile> for RequestView {
    /// The request before variable resolution: enabled params (plus any query typed into the URL
    /// itself), enabled headers and raw body text.
    fn from(req: &HttpRequestFile) -> Self {
        let method = req.method.trim().to_uppercase();
        let raw = req.url.trim();
        let (before_hash, hash) = match raw.split_once('#') {
            Some((a, h)) => (a, Some(h)),
            None => (raw, None),
        };
        let (base, url_query) = match before_hash.split_once('?') {
            Some((b, q)) => (b, Some(q)),
            None => (before_hash, None),
        };
        let mut query: Vec<(String, String)> = url_query
            .map(|q| {
                q.split('&')
                    .filter(|p| !p.is_empty())
                    .map(|p| match p.split_once('=') {
                        Some((k, v)) => (decode_component(k), decode_component(v)),
                        None => (decode_component(p), String::new()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        query.extend(req.params.iter().filter(|p| p.enabled && !p.name.is_empty()).map(|p| (p.name.clone(), p.value.clone())));
        RequestView {
            method: if method.is_empty() { "GET".into() } else { method },
            url: match hash {
                Some(h) => format!("{base}#{h}"),
                None => base.to_string(),
            },
            query,
            headers: req.headers.iter().filter(|h| h.enabled && !h.name.trim().is_empty()).map(|h| (h.name.trim().to_string(), h.value.clone())).collect(),
            body: match &req.body {
                Body::Json { content } | Body::Xml { content } | Body::Text { content, .. } => Some(content.clone()),
                _ => None,
            },
        }
    }
}

fn looks_like_json(t: &str) -> bool {
    let t = t.trim_start();
    (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<serde_json::Value>(t).is_ok()
}

/// Writes the parts of `after` that differ from `before` back into `req`. Disabled params and
/// headers are kept; a raw body set on a request without one becomes JSON (when it parses) or text.
pub(crate) fn apply_view(req: &mut HttpRequestFile, before: &RequestView, after: &RequestView) {
    if after.method != before.method {
        req.method = after.method.clone();
    }
    if after.url != before.url || after.query != before.query {
        req.url = after.url.clone();
        let disabled: Vec<KeyValue> = req.params.iter().filter(|p| !p.enabled).cloned().collect();
        req.params = after.query.iter().map(|(k, v)| KeyValue::new(k, v)).chain(disabled).collect();
    }
    if after.headers != before.headers {
        let disabled: Vec<KeyValue> = req.headers.iter().filter(|h| !h.enabled).cloned().collect();
        req.headers = after.headers.iter().map(|(k, v)| KeyValue::new(k, v)).chain(disabled).collect();
    }
    if after.body != before.body {
        let text = after.body.clone().unwrap_or_default();
        match &mut req.body {
            Body::Json { content } | Body::Xml { content } | Body::Text { content, .. } => *content = text,
            other => {
                *other = match &after.body {
                    None => Body::None,
                    Some(_) if looks_like_json(&text) => Body::Json { content: text },
                    Some(_) => Body::Text { content: text, content_type: None },
                }
            }
        }
    }
}

/// What the pre-request chain did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreOutcome {
    /// Console output and errors for the response panel / run report.
    pub report: ScriptReport,
    /// `pm.test` results declared in pre-request scripts.
    pub tests: Vec<ScriptTestResult>,
    pub changes: Vec<VarChange>,
    /// The request was edited (and the edits applied).
    pub request_changed: bool,
    /// A script called `pm.execution.skipRequest()`.
    pub skip_request: bool,
    /// Secret values read by scripts: add them to the resolver's `secret_values` for redaction.
    pub secret_values: BTreeSet<String>,
    /// The first script error. The request should not be sent.
    pub error: Option<ScriptError>,
}

impl PreOutcome {
    /// No error and no skip.
    pub fn should_send(&self) -> bool {
        self.error.is_none() && !self.skip_request
    }
}

/// What the post-response chain did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PostOutcome {
    pub report: ScriptReport,
    /// Append to the assertion results (same shape as `AssertionResult`).
    pub tests: Vec<ScriptTestResult>,
    pub changes: Vec<VarChange>,
    pub secret_values: BTreeSet<String>,
    pub error: Option<ScriptError>,
}

impl PostOutcome {
    /// A script error or a failed `pm.test`.
    pub fn failed(&self) -> bool {
        self.error.is_some() || self.tests.iter().any(|t| !t.passed)
    }
}

/// Runs the pre-request chain against `request` (normally `EffectiveRequest::request`, i.e. with
/// folder headers/auth applied, **before** `devall_http::prepare`):
///
/// * request edits are written into `request` (only when every script succeeded);
/// * variable changes are applied to `runtime` — the highest-precedence resolver layer — even when
///   a later script failed (Postman semantics). Build the `Resolver` from `runtime` afterwards.
///
/// Returns quickly with an empty outcome when there's nothing to run.
pub fn apply_pre_request(engine: &ScriptEngine, scripts: &[ScriptSource], request: &mut HttpRequestFile, runtime: &mut Variables, env: &ScriptEnv) -> PreOutcome {
    if scripts.iter().all(|s| s.code.trim().is_empty()) {
        return PreOutcome::default();
    }
    let before = RequestView::from(&*request);
    let input = PreRequestInput { request: before.clone(), vars: env.vars(runtime), info: env.info(request), secrets: env.secrets_for(scripts) };
    let out = engine.run_pre_request(scripts, &input);
    *runtime = out.runtime.clone();
    let apply = out.error.is_none() && out.request_changed;
    if apply {
        apply_view(request, &before, &out.request);
    }
    PreOutcome {
        report: ScriptReport::from(&out),
        tests: out.tests,
        changes: out.changes,
        request_changed: apply,
        skip_request: out.skip_request,
        secret_values: out.secret_values,
        error: out.error,
    }
}

/// Runs the post-response chain. `request` is the request that was sent (after pre-request edits,
/// before resolution); `response` comes from [`ResponseInfo::new`]. Variable changes are applied
/// to `runtime`; `tests` should be appended to the request's assertion results.
pub fn apply_post_response(
    engine: &ScriptEngine,
    scripts: &[ScriptSource],
    request: &HttpRequestFile,
    response: &ResponseInfo,
    runtime: &mut Variables,
    env: &ScriptEnv,
) -> PostOutcome {
    if scripts.iter().all(|s| s.code.trim().is_empty()) {
        return PostOutcome::default();
    }
    let input = PostResponseInput {
        request: RequestView::from(request),
        response: ResponseInfo { body: String::new(), ..response.clone() },
        vars: env.vars(runtime),
        info: env.info(request),
        secrets: env.secrets_for(scripts),
    };
    let out = engine.run_chain(ScriptPhase::PostResponse, scripts, &input.request, Some(response), &input.vars, &input.info, &input.secrets);
    *runtime = out.runtime.clone();
    PostOutcome { report: ScriptReport::from(&out), tests: out.tests, changes: out.changes, secret_values: out.secret_values, error: out.error }
}
