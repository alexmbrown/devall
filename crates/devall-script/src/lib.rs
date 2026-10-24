//! Pre-request and post-response scripts: sandboxed JavaScript (QuickJS-ng via `rquickjs`) with a
//! Postman-compatible `pm` API, so existing collections migrate with their scripts.
//!
//! # Sandbox
//!
//! Every script runs in a fresh QuickJS runtime + context (nothing leaks between scripts or
//! sends) with a hard wall-clock timeout (interrupt handler, default 5 s, uncatchable), a heap limit
//! (default 64 MiB plus room for large response bodies), a JS stack limit and a cap on console
//! output. There is no filesystem, network, process, OS-environment or module-loader access: only
//! the ECMAScript standard library plus the globals listed below. Secrets are never exposed except
//! through `pm.secrets.get('NAME')` / `pm.vault.get('NAME')` with a *literal* name, and every
//! secret value read that way is masked in logs, errors and test messages.
//!
//! # Supported API
//!
//! | Area | Supported |
//! |---|---|
//! | Variables | `pm.variables.get/set/has/unset/toObject/replaceIn`, `pm.environment.get/set/unset/has/clear/toObject/replaceIn/name`, `pm.collectionVariables.get/set/unset/has/clear/toObject/replaceIn`, `pm.globals.get/set/unset/has/clear/toObject/replaceIn`, `pm.iterationData.get/has/toObject/toJSON` |
//! | Request | `pm.request.method` (assignable), `pm.request.url` (assignable; `toString`, `update`, `getHost`, `getPath`, `getPathWithQuery`, `getQueryString`, `protocol`, `host`, `port`, `path`, `hash`, `addQueryParams`, `removeQueryParams`, `query.add/append/upsert/remove/get/has/all/toObject/each/map/filter/find/count/clear`), `pm.request.headers.add/upsert/remove/get/has/all/toObject/each/…`, `pm.request.addHeader/removeHeader/upsertHeader`, `pm.request.body.raw` (assignable) / `body.update(text \| {mode:'raw', raw})` / `body.mode` / `body.toString()`, `pm.request.name/id` |
//! | Response (post) | `pm.response.code/status/reason()/headers/responseTime/responseSize/size()/text()/json()`, `pm.cookies.get/has/toObject` (from `Set-Cookie`) |
//! | Tests | `pm.test(name, fn)` (sync, `done` callback or promise), `pm.test.skip`, `pm.expect(value, msg?)`, `pm.response.to.*`, legacy `tests["name"] = bool` |
//! | chai subset | chains `to be been is that which and has have with at of same but does still also`; flags `not deep own nested any all`; `equal/equals/eq`, `eql`, `include/includes/contain/contains` (strings, arrays, objects, maps, sets), `property(name, value?)`, `ownProperty`, `lengthOf/length`, `keys`, `members`, `a/an`, `above/gt/greaterThan`, `below/lt/lessThan`, `least/gte`, `most/lte`, `within`, `closeTo/approximately`, `match/matches`, `string`, `oneOf`, `instanceOf/instanceof`, `satisfy`, `throw/throws`, `ok true false null undefined NaN exist empty finite`, `expect.fail` |
//! | Response assertions | `status(code \| reason)`, `header(name, value?)`, `body(text \| RegExp \| object?)`, `jsonBody(path?, value?)`, `jsonSchema(schema)` (draft-07 subset), `ok`, `json`, `withBody`, `info success redirection clientError serverError error accepted badRequest unauthorized forbidden notFound notAcceptable rateLimited` — on `pm.response.to` and `pm.expect(pm.response)` |
//! | Info / flow | `pm.info.eventName/iteration/iterationCount/requestName/requestId`, `pm.execution.skipRequest()` (pre-request), `pm.execution.location` |
//! | Crypto & encoding | `CryptoJS` (also `require('crypto-js')`): `MD5 SHA1 SHA224 SHA256 SHA384 SHA512`, `HmacMD5 HmacSHA1 HmacSHA224 HmacSHA256 HmacSHA384 HmacSHA512`, `algo.<HASH>.create().update().finalize()`, `algo.HMAC.create(algo, key)`, `enc.Hex/Base64/Base64url/Utf8/Latin1 .parse/.stringify`, `lib.WordArray.create/random` — backed by Rust (sha2/sha1/md-5/hmac). `btoa`, `atob`, `crypto.randomUUID()`, `crypto.getRandomValues()`, `Buffer.from(…).toString('base64'\|'hex'\|'utf8'\|'latin1'\|'base64url')`, `TextEncoder`, `TextDecoder` (UTF-8) |
//! | Misc globals | `console.log/info/warn/error/debug/trace/dir/table/assert/count/time/timeEnd/group/groupEnd` (Node-style formatting, `%s %d %i %f %j %o %O`), `setTimeout/setInterval/setImmediate/clear*` (virtual time: callbacks run after the script in due order, no real waiting), top-level `await`, `JSON`, `Date`, a lodash subset as `_` (`get set has isEqual isEmpty pick omit find filter map …`), `tv4.validate` (same JSON-schema subset), `require('crypto-js' \| 'chai' \| 'uuid' \| 'lodash' \| 'atob' \| 'btoa' \| 'buffer' \| 'tv4')` |
//! | Legacy Postman | `postman.setEnvironmentVariable/getEnvironmentVariable/clearEnvironmentVariable/setGlobalVariable/getGlobalVariable/clearGlobalVariable/getResponseHeader/getResponseCookie`, `tests[…]`, `responseBody`, `responseCode`, `responseHeaders`, `responseTime`, `environment`, `globals`, `data`, `iteration`, `request` |
//! | Bruno shims | `bru.getEnvVar/setEnvVar/hasEnvVar/deleteEnvVar/getVar/setVar/hasVar/deleteVar/getCollectionVar/getFolderVar/getRequestVar/getGlobalEnvVar/setGlobalEnvVar/getEnvName/interpolate/sleep/runner.skipRequest`, `req.getUrl/setUrl/getMethod/setMethod/getHeader/getHeaders/setHeader/setHeaders/deleteHeader/getBody/setBody/getName`, `res(path)`, `res.getStatus/getStatusText/getHeader/getHeaders/getBody/getResponseTime/getSize`, `res.status/statusText/headers/body/responseTime`, global `test()` and `expect()` |
//!
//! **Not supported** (each throws or warns with a clear message): `pm.sendRequest`,
//! `pm.execution.setNextRequest` / `postman.setNextRequest` (requests run in tree order),
//! `pm.visualizer` (no-op), `pm.require` packages, `pm.cookies.jar()`, `pm.request.auth`,
//! urlencoded/form-data/GraphQL body editing (`pm.request.body.raw` is empty for those modes and
//! assigning it replaces the body with raw text), CryptoJS ciphers (`AES`, `DES`, …), `SHA3`,
//! `RIPEMD160`, `PBKDF2`, `moment`, `cheerio`, `xml2Json`, `ajv`, `URL`/`URLSearchParams`,
//! `bru.runRequest`/`bru.sendRequest`, `bru.getProcessEnv` (returns `undefined`).
//!
//! # Variable model
//!
//! /dev/all keeps one in-memory **runtime** layer per project (never written to files). Every
//! scope's `set`/`unset` (`variables`, `environment`, `collectionVariables`, `globals`, Bruno's
//! `setVar`/`setEnvVar`) writes that layer; [`VarChange::scope`] records which API was used.
//! Reads see the scope's file values with the runtime layer on top:
//!
//! * `pm.variables.get`: runtime → iteration data → environment → folders (nearest first) → project
//! * `pm.environment.get`: runtime → active environment file
//! * `pm.collectionVariables.get`: runtime → folders → project (`devall.yaml`)
//! * `pm.globals.get`: runtime only; `pm.iterationData.get`: the data row only
//!
//! `unset` removes the runtime override, so a value from a file shows through again. Values are
//! kept as-is inside one script and stored as strings (`JSON.stringify` for objects).
//!
//! # Chains
//!
//! Folder scripts run outermost first, then the request's own ([`ScriptEngine::run_pre_request`],
//! [`ScriptEngine::run_post_response`], [`collect_scripts`]). Each script gets a fresh context but
//! sees every variable change and request edit made by the scripts before it. The chain stops at
//! the first script error; variable changes made before the error are kept (as in Postman), request
//! edits are not applied.
//!
//! # Integration
//!
//! [`apply_pre_request`] / [`apply_post_response`] work on an [`HttpRequestFile`] and the runtime
//! variable map; see their docs for where they slot into the send pipeline.

mod engine;
mod integrate;
mod natives;

use std::collections::BTreeSet;
use std::fmt;

pub use devall_core::{HttpRequestFile, Variables};
use serde::{Deserialize, Serialize};

pub use engine::{DEFAULT_MEMORY_LIMIT, DEFAULT_TIMEOUT, ScriptEngine};
pub use integrate::{PostOutcome, PreOutcome, ScriptChain, ScriptEnv, apply_post_response, apply_pre_request, collect_scripts, secret_names};

/// One script to run: `name` says where it comes from (shown with logs and errors).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptSource {
    /// `request`, `collection` (the root `http/_folder.yaml`) or `folder <path>`.
    pub name: String,
    pub code: String,
}

impl ScriptSource {
    pub fn new(name: impl Into<String>, code: impl Into<String>) -> Self {
        Self { name: name.into(), code: code.into() }
    }
}

/// Which hook a script runs in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptPhase {
    #[default]
    PreRequest,
    PostResponse,
}

impl ScriptPhase {
    /// Postman's `pm.info.eventName`.
    pub fn event_name(self) -> &'static str {
        match self {
            ScriptPhase::PreRequest => "prerequest",
            ScriptPhase::PostResponse => "test",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ScriptPhase::PreRequest => "pre-request",
            ScriptPhase::PostResponse => "post-response",
        }
    }
}

/// The request as scripts see it: **before** `{{variables}}` are resolved (like Postman), with
/// folder headers merged when built from an effective request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestView {
    pub method: String,
    /// URL without the query string (may keep a `#fragment`).
    pub url: String,
    /// Enabled query parameters in order, decoded (as in the params table).
    #[serde(default)]
    pub query: Vec<(String, String)>,
    /// Enabled headers in order.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Raw body text for JSON/text/XML bodies; `None` for no body and non-raw bodies (form,
    /// multipart, GraphQL, file).
    #[serde(default)]
    pub body: Option<String>,
}

impl RequestView {
    /// `url?query` (query values unencoded, as shown in the URL bar).
    pub fn full_url(&self) -> String {
        let (base, hash) = match self.url.split_once('#') {
            Some((b, h)) => (b, Some(h)),
            None => (self.url.as_str(), None),
        };
        let mut out = base.to_string();
        if !self.query.is_empty() {
            out.push('?');
            out.push_str(&self.query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
        }
        if let Some(h) = hash {
            out.push('#');
            out.push_str(h);
        }
        out
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The response as post-response scripts see it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseInfo {
    pub code: u16,
    /// Reason phrase, e.g. `OK`.
    pub status: String,
    /// Wire order, duplicates preserved.
    pub headers: Vec<(String, String)>,
    /// Body as text (lossy UTF-8), possibly truncated to [`MAX_SCRIPT_BODY`].
    pub body: String,
    /// Total time in milliseconds.
    pub response_time: f64,
    /// Body size in bytes.
    pub size: usize,
}

/// Larger bodies are truncated before they reach a script.
pub const MAX_SCRIPT_BODY: usize = 32 * 1024 * 1024;

impl ResponseInfo {
    /// Builds the view from the fields of `devall_http::HttpResponseData`:
    /// `ResponseInfo::new(r.status, &r.status_text, &r.headers, &r.body, r.timings.total_ms, r.body_size)`.
    pub fn new(code: u16, status: &str, headers: &[(String, String)], body: &[u8], response_time_ms: f64, size: usize) -> Self {
        let body = &body[..body.len().min(MAX_SCRIPT_BODY)];
        Self {
            code,
            status: status.to_string(),
            headers: headers.to_vec(),
            body: String::from_utf8_lossy(body).into_owned(),
            response_time: response_time_ms,
            size,
        }
    }
}

/// Variable layers visible to a script (see the crate docs for precedence).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptVars {
    /// Session runtime variables (captures and script sets) — highest precedence.
    pub runtime: Variables,
    /// The current data-file row in a collection run.
    pub iteration_data: Variables,
    /// The active environment's variables (from its file).
    pub environment: Variables,
    /// Folder variables, nearest folder first (as in `EffectiveRequest::folder_variables`).
    pub folders: Vec<Variables>,
    /// Project (`devall.yaml`) variables.
    pub project: Variables,
}

impl ScriptVars {
    /// Project vars overlaid by folder vars, nearest folder winning (Postman's collection scope).
    pub fn collection(&self) -> Variables {
        let mut out = self.project.clone();
        for f in self.folders.iter().rev() {
            out.extend(f.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        out
    }
}

/// `pm.info` and friends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptInfo {
    pub request_name: String,
    pub request_id: String,
    /// Location under `http/`, e.g. `users/get-user.req.yaml` (empty for unsaved drafts).
    pub request_path: String,
    /// 0-based iteration in a collection run.
    pub iteration: usize,
    pub iteration_count: usize,
    pub environment_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreRequestInput {
    pub request: RequestView,
    pub vars: ScriptVars,
    pub info: ScriptInfo,
    /// Secret values for `pm.secrets.get(name)` (see [`secret_names`]); masked in output.
    pub secrets: Variables,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostResponseInput {
    /// The request as it was sent (after pre-request edits, before variable resolution).
    pub request: RequestView,
    pub response: ResponseInfo,
    pub vars: ScriptVars,
    pub info: ScriptInfo,
    pub secrets: Variables,
}

/// Which `pm` API a variable change came through. All scopes write the one runtime layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VarScope {
    /// `pm.variables`, Bruno `bru.setVar`.
    Variables,
    /// `pm.environment` (a runtime override — never written to the environment file).
    Environment,
    /// `pm.collectionVariables`.
    CollectionVariables,
    /// `pm.globals`.
    Globals,
}

/// One `set` (`value: Some`) or `unset` (`value: None`), in execution order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VarChange {
    pub scope: VarScope,
    pub name: String,
    pub value: Option<String>,
}

/// Applies changes, in order, to a runtime variable map.
pub fn apply_changes(runtime: &mut Variables, changes: &[VarChange]) {
    for c in changes {
        match &c.value {
            Some(v) => {
                runtime.insert(c.name.clone(), v.clone());
            }
            None => {
                runtime.shift_remove(&c.name);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    #[default]
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

impl LogLevel {
    fn parse(s: &str) -> Self {
        match s {
            "info" => LogLevel::Info,
            "warn" => LogLevel::Warn,
            "error" => LogLevel::Error,
            "debug" => LogLevel::Debug,
            _ => LogLevel::Log,
        }
    }
}

/// A `console.*` line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub phase: ScriptPhase,
    /// [`ScriptSource::name`] of the script that logged it.
    pub source: String,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptErrorKind {
    /// The script doesn't parse.
    Syntax,
    /// An uncaught exception (including failed `pm.expect` outside `pm.test`).
    Runtime,
    /// Ran past the time limit (also: a top-level `await` that never settled).
    Timeout,
    /// Exceeded the memory limit.
    Memory,
    /// Engine failure (a bug or resource exhaustion outside the script's control).
    Internal,
}

/// A script that failed. `line`/`column` are 1-based positions in the script's own source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptError {
    pub phase: ScriptPhase,
    pub source: String,
    pub kind: ScriptErrorKind,
    /// E.g. `ReferenceError: foo is not defined`.
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl std::error::Error for ScriptError {}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} script ({})", self.phase.label(), self.source)?;
        if let Some(line) = self.line {
            write!(f, " line {line}")?;
            if let Some(col) = self.column {
                write!(f, ":{col}")?;
            }
        }
        write!(f, ": {}", self.message)
    }
}

/// A `pm.test` result. Same JSON shape as `devall_http::checks::AssertionResult`
/// (`label`, `passed`, `actual`, `message`) plus `skipped` and `source`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptTestResult {
    /// The test name.
    pub label: String,
    /// Skipped tests count as passed.
    pub passed: bool,
    /// Always `None` (kept for shape compatibility with assertion results).
    pub actual: Option<String>,
    /// Failure message, e.g. `AssertionError: expected 404 to equal 200`.
    pub message: Option<String>,
    #[serde(default)]
    pub skipped: bool,
    pub phase: ScriptPhase,
    /// [`ScriptSource::name`] of the script that declared the test.
    pub source: String,
}

/// Result of running a chain of scripts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptOutput {
    /// The request after edits (pre-request; unchanged for post-response or after an error).
    pub request: RequestView,
    pub request_changed: bool,
    /// Every variable set/unset, in order (kept even when a later script failed).
    pub changes: Vec<VarChange>,
    /// The runtime layer with `changes` applied.
    pub runtime: Variables,
    pub tests: Vec<ScriptTestResult>,
    pub logs: Vec<LogLine>,
    /// First failure; the chain stops there.
    pub error: Option<ScriptError>,
    /// `pm.execution.skipRequest()` / `bru.runner.skipRequest()` was called.
    pub skip_request: bool,
    /// Scripts that ran (blank ones are skipped).
    pub scripts_run: usize,
    /// Secret values the scripts read (redact them wherever request data is shown or stored).
    #[serde(skip)]
    pub secret_values: BTreeSet<String>,
}

pub type PreRequestOutput = ScriptOutput;
pub type PostResponseOutput = ScriptOutput;

/// Console output and errors of one send (pre-request + post-response), for the response panel.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptReport {
    pub logs: Vec<LogLine>,
    pub errors: Vec<ScriptError>,
    /// The pre-request script asked to skip the request.
    pub skipped: bool,
    /// Number of scripts that ran.
    pub scripts_run: usize,
}

impl ScriptReport {
    pub fn is_empty(&self) -> bool {
        self.logs.is_empty() && self.errors.is_empty() && !self.skipped && self.scripts_run == 0
    }
    pub fn merge(&mut self, other: ScriptReport) {
        self.logs.extend(other.logs);
        self.errors.extend(other.errors);
        self.skipped |= other.skipped;
        self.scripts_run += other.scripts_run;
    }
}

impl From<&ScriptOutput> for ScriptReport {
    fn from(o: &ScriptOutput) -> Self {
        ScriptReport { logs: o.logs.clone(), errors: o.error.iter().cloned().collect(), skipped: o.skip_request, scripts_run: o.scripts_run }
    }
}

#[cfg(test)]
mod tests;
