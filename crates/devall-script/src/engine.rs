//! Runs scripts in a fresh QuickJS runtime each: prelude (globals) → user code → microtasks and
//! virtual timers → collect results. Everything here is synchronous; callers on an async runtime
//! can call it directly (scripts are usually sub-millisecond, bounded by [`ScriptEngine::timeout`])
//! or move owned inputs to `spawn_blocking`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use devall_core::vars::redact_values;
use rquickjs::context::EvalOptions;
use rquickjs::promise::PromiseState;
use rquickjs::{CatchResultExt, CaughtError, Coerced, Context, Ctx, FromJs, Function, Object, Runtime, Value};
use serde::{Deserialize, Serialize};

use crate::natives::{self, LogSink};
use crate::{
    PostResponseInput, PreRequestInput, RequestView, ResponseInfo, ScriptError, ScriptErrorKind, ScriptInfo, ScriptOutput, ScriptPhase, ScriptSource,
    ScriptTestResult, ScriptVars, VarChange, Variables, apply_changes,
};

const PRELUDE: &str = include_str!("prelude.js");
/// File name user code is evaluated under; stack frames `script:LINE:COL` locate errors.
const SCRIPT_FILE: &str = "script";
/// Extra time and heap granted after a failure to read the script's results.
const GRACE: Duration = Duration::from_millis(500);
const GRACE_MEMORY: usize = 16 * 1024 * 1024;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// Script execution limits. Cheap to clone, `Send + Sync`; every run gets its own runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEngine {
    /// Wall-clock limit per script (including promises and timers).
    pub timeout: Duration,
    /// Heap limit per script; large inputs (response bodies) get extra room on top. 0 = unlimited.
    pub memory_limit: usize,
    /// JavaScript stack limit (deep recursion fails with a RangeError).
    pub max_stack_size: usize,
    /// Console lines kept per script.
    pub max_log_lines: usize,
    /// Longer console lines are truncated (bytes).
    pub max_log_line_len: usize,
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self { timeout: DEFAULT_TIMEOUT, memory_limit: DEFAULT_MEMORY_LIMIT, max_stack_size: 512 * 1024, max_log_lines: 1000, max_log_line_len: 16 * 1024 }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsInput<'a> {
    event: &'static str,
    request: &'a RequestView,
    response: Option<JsResponse<'a>>,
    vars: JsVars<'a>,
    info: &'a ScriptInfo,
    secrets: &'a Variables,
}

/// The response without its body (the body is passed as a separate string).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsResponse<'a> {
    code: u16,
    status: &'a str,
    headers: &'a [(String, String)],
    response_time: f64,
    size: usize,
}

#[derive(Serialize)]
struct JsVars<'a> {
    runtime: &'a Variables,
    data: &'a Variables,
    environment: &'a Variables,
    collection: &'a Variables,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Collected {
    request: Option<RequestView>,
    #[serde(default)]
    changes: Vec<VarChange>,
    #[serde(default)]
    tests: Vec<JsTest>,
    #[serde(default)]
    skip_request: bool,
}

#[derive(Deserialize)]
struct JsTest {
    name: String,
    passed: bool,
    #[serde(default)]
    skipped: bool,
    message: Option<String>,
}

struct RunResult {
    logs: Vec<crate::LogLine>,
    error: Option<ScriptError>,
    collected: Option<Collected>,
}

impl ScriptEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_memory_limit(mut self, bytes: usize) -> Self {
        self.memory_limit = bytes;
        self
    }

    /// Runs pre-request scripts in order (folders outermost first, then the request).
    pub fn run_pre_request(&self, scripts: &[ScriptSource], input: &PreRequestInput) -> ScriptOutput {
        self.run_chain(ScriptPhase::PreRequest, scripts, &input.request, None, &input.vars, &input.info, &input.secrets)
    }

    /// Runs post-response scripts in order (folders outermost first, then the request).
    pub fn run_post_response(&self, scripts: &[ScriptSource], input: &PostResponseInput) -> ScriptOutput {
        self.run_chain(ScriptPhase::PostResponse, scripts, &input.request, Some(&input.response), &input.vars, &input.info, &input.secrets)
    }

    /// Parses `code` without running it; `None` when it compiles.
    pub fn check_syntax(&self, code: &str) -> Option<ScriptError> {
        const WRAP: &str = "(async function(){";
        let rt = Runtime::new().ok()?;
        rt.set_memory_limit(self.memory_limit.max(code.len().saturating_mul(16)));
        let deadline = Instant::now() + self.timeout;
        rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
        let ctx = Context::full(&rt).ok()?;
        ctx.with(|ctx| {
            let mut opts = EvalOptions::default();
            opts.strict = false;
            opts.filename = Some(SCRIPT_FILE.into());
            // An unevaluated async function body: compiles everything (top-level await included)
            // without executing a single statement.
            let src = format!("{WRAP}{code}\n}})");
            let r: rquickjs::Result<Value> = ctx.eval_with_options(src, opts);
            let e = r.catch(&ctx).err()?;
            let mut err = self.classify(&ctx, ScriptPhase::PreRequest, "script", e, false, true);
            if err.line == Some(1) {
                err.column = err.column.map(|c| c.saturating_sub(WRAP.len() as u32).max(1));
            }
            Some(err)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_chain(
        &self,
        phase: ScriptPhase,
        scripts: &[ScriptSource],
        request: &RequestView,
        response: Option<&ResponseInfo>,
        vars: &ScriptVars,
        info: &ScriptInfo,
        secrets: &Variables,
    ) -> ScriptOutput {
        let mut out = ScriptOutput { request: request.clone(), runtime: vars.runtime.clone(), ..Default::default() };
        let collection = vars.collection();
        let body = response.map(|r| r.body.as_str()).unwrap_or("");
        for s in scripts.iter().filter(|s| !s.code.trim().is_empty()) {
            let input = JsInput {
                event: phase.event_name(),
                request: &out.request,
                response: response.map(|r| JsResponse { code: r.code, status: &r.status, headers: &r.headers, response_time: r.response_time, size: r.size }),
                vars: JsVars { runtime: &out.runtime, data: &vars.iteration_data, environment: &vars.environment, collection: &collection },
                info,
                secrets,
            };
            let json = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
            let r = self.run_one(phase, s, &json, body);
            out.scripts_run += 1;
            out.logs.extend(r.logs);
            if let Some(c) = r.collected {
                apply_changes(&mut out.runtime, &c.changes);
                out.changes.extend(c.changes);
                out.tests.extend(c.tests.into_iter().map(|t| ScriptTestResult {
                    label: t.name,
                    passed: t.passed || t.skipped,
                    actual: None,
                    message: t.message,
                    skipped: t.skipped,
                    phase,
                    source: s.name.clone(),
                }));
                if phase == ScriptPhase::PreRequest {
                    out.skip_request |= c.skip_request;
                    if r.error.is_none()
                        && let Some(req) = c.request
                    {
                        out.request = req;
                    }
                }
            }
            if let Some(e) = r.error {
                out.error = Some(e);
                // A failed chain doesn't edit the request (it shouldn't be sent anyway).
                out.request = request.clone();
                break;
            }
        }
        out.request_changed = out.request != *request;
        let values: BTreeSet<String> = secrets.values().filter(|v| !v.is_empty()).cloned().collect();
        if !values.is_empty() {
            for l in &mut out.logs {
                l.message = redact_values(&l.message, &values);
            }
            for t in &mut out.tests {
                t.message = t.message.as_deref().map(|m| redact_values(m, &values));
            }
            if let Some(e) = &mut out.error {
                e.message = redact_values(&e.message, &values);
            }
        }
        out.secret_values = values;
        out
    }

    fn run_one(&self, phase: ScriptPhase, source: &ScriptSource, input_json: &str, body: &str) -> RunResult {
        let sink = Rc::new(RefCell::new(LogSink::new(phase, &source.name, self.max_log_lines, self.max_log_line_len)));
        let internal = |message: String| ScriptError { phase, source: source.name.clone(), kind: ScriptErrorKind::Internal, message, line: None, column: None };
        let rt = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => return RunResult { logs: Vec::new(), error: Some(internal(format!("could not start the script engine: {e}"))), collected: None },
        };
        // Strings live in the heap as well (twice: JSON text and parsed values), so big inputs
        // get headroom on top of the configured limit.
        let limit = if self.memory_limit == 0 { 0 } else { self.memory_limit.saturating_add((input_json.len() + body.len()).saturating_mul(6)) };
        rt.set_memory_limit(limit);
        rt.set_max_stack_size(self.max_stack_size);
        let deadline = Rc::new(Cell::new(Instant::now() + self.timeout));
        let timed_out = Rc::new(Cell::new(false));
        {
            let (deadline, timed_out) = (deadline.clone(), timed_out.clone());
            rt.set_interrupt_handler(Some(Box::new(move || {
                let hit = Instant::now() >= deadline.get();
                if hit {
                    timed_out.set(true);
                }
                hit
            })));
        }
        let ctx = match Context::full(&rt) {
            Ok(c) => c,
            Err(e) => return RunResult { logs: Vec::new(), error: Some(internal(format!("could not start the script engine: {e}"))), collected: None },
        };

        let mut error = ctx.with(|ctx| self.execute(&ctx, phase, source, input_json, body, sink.clone(), &deadline, &timed_out).err());

        // Variables set and tests recorded before a failure still count (as in Postman), so
        // collect them even after a timeout or out-of-memory error.
        deadline.set(Instant::now() + GRACE);
        if limit != 0 {
            rt.set_memory_limit(limit.saturating_add(GRACE_MEMORY));
        }
        rt.run_gc();
        let collected = match ctx.with(|ctx| collect(&ctx)) {
            Ok(c) => Some(c),
            Err(msg) => {
                if error.is_none() {
                    error = Some(internal(msg));
                }
                None
            }
        };
        drop(ctx);
        drop(rt);
        let logs = match Rc::try_unwrap(sink) {
            Ok(s) => s.into_inner().finish(),
            Err(s) => s.borrow().lines.clone(),
        };
        RunResult { logs, error, collected }
    }

    #[allow(clippy::too_many_arguments)]
    fn execute<'js>(
        &self,
        ctx: &Ctx<'js>,
        phase: ScriptPhase,
        source: &ScriptSource,
        input_json: &str,
        body: &str,
        sink: Rc<RefCell<LogSink>>,
        deadline: &Cell<Instant>,
        timed_out: &Cell<bool>,
    ) -> Result<(), ScriptError> {
        let fail = |e: CaughtError<'js>, compiling: bool| self.classify(ctx, phase, &source.name, e, timed_out.get(), compiling);

        let setup = || -> rquickjs::Result<()> {
            let g = ctx.globals();
            g.set("__devall_native", natives::install(ctx, sink)?)?;
            g.set("__devall_input", input_json)?;
            g.set("__devall_body", body)?;
            let mut opts = EvalOptions::default();
            opts.filename = Some("devall:prelude".into());
            ctx.eval_with_options::<(), _>(PRELUDE, opts)
        };
        if let Err(e) = setup().catch(ctx) {
            let mut err = fail(e, false);
            if err.kind == ScriptErrorKind::Runtime {
                err.kind = ScriptErrorKind::Internal;
                err.message = format!("script environment failed to start: {}", err.message);
                err.line = None;
                err.column = None;
            }
            return Err(err);
        }

        let mut opts = EvalOptions::default();
        opts.strict = false; // Postman scripts are sloppy-mode scripts
        opts.promise = true; // top-level await
        opts.backtrace_barrier = true;
        opts.filename = Some(SCRIPT_FILE.into());
        let result: Value = ctx.eval_with_options(source.code.as_str(), opts).catch(ctx).map_err(|e| fail(e, true))?;

        // Drive promises and virtual timers until nothing is left to run.
        let next_timer: Function = ctx.globals().get::<_, Object>("__devall").and_then(|h| h.get("nextTimer")).catch(ctx).map_err(|e| fail(e, false))?;
        let expired = || timed_out.get() || Instant::now() >= deadline.get();
        loop {
            while !expired() && ctx.execute_pending_job() {}
            if expired() {
                timed_out.set(true);
                return Err(self.timeout_error(phase, &source.name));
            }
            let more: bool = next_timer.call(()).catch(ctx).map_err(|e| fail(e, false))?;
            if !more {
                break;
            }
        }
        if let Some(p) = result.as_promise() {
            match p.state() {
                PromiseState::Resolved => {}
                PromiseState::Rejected => {
                    if let Some(Err(e)) = p.result::<Value>() {
                        return Err(fail(CaughtError::from_error(ctx, e), false));
                    }
                }
                PromiseState::Pending => {
                    return Err(ScriptError {
                        phase,
                        source: source.name.clone(),
                        kind: ScriptErrorKind::Timeout,
                        message: "the script never finished: a top-level await is waiting for a promise that can't settle".into(),
                        line: None,
                        column: None,
                    });
                }
            }
        }
        Ok(())
    }

    fn timeout_error(&self, phase: ScriptPhase, source: &str) -> ScriptError {
        ScriptError {
            phase,
            source: source.to_string(),
            kind: ScriptErrorKind::Timeout,
            message: format!("timed out after {} ms", self.timeout.as_millis()),
            line: None,
            column: None,
        }
    }

    fn classify<'js>(&self, ctx: &Ctx<'js>, phase: ScriptPhase, source: &str, e: CaughtError<'js>, timed_out: bool, compiling: bool) -> ScriptError {
        let (name, message, stack) = match &e {
            CaughtError::Exception(ex) => {
                let name = ex.as_object().get::<_, Option<Coerced<String>>>("name").ok().flatten().map(|c| c.0);
                (name, ex.message().unwrap_or_default(), ex.stack())
            }
            CaughtError::Value(v) => (None, format!("Uncaught {}", display_value(ctx, v)), None),
            CaughtError::Error(err) => (None, err.to_string(), None),
        };
        let (line, column) = match stack.as_deref().and_then(locate) {
            Some((l, c)) => (Some(l), Some(c)),
            None => (None, None),
        };
        let out_of_memory = message == "out of memory" || matches!(e, CaughtError::Error(rquickjs::Error::Allocation));
        let (kind, message) = if timed_out || message == "interrupted" {
            (ScriptErrorKind::Timeout, format!("timed out after {} ms", self.timeout.as_millis()))
        } else if out_of_memory {
            (ScriptErrorKind::Memory, format!("ran out of memory (limit {} MiB)", self.memory_limit.div_ceil(1024 * 1024)))
        } else {
            let kind = if compiling && name.as_deref() == Some("SyntaxError") { ScriptErrorKind::Syntax } else { ScriptErrorKind::Runtime };
            let message = match name.filter(|n| !n.is_empty() && !message.starts_with(n.as_str())) {
                Some(n) => format!("{n}: {message}"),
                None => message,
            };
            (kind, message)
        };
        ScriptError { phase, source: source.to_string(), kind, message, line, column }
    }
}

fn display_value<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> String {
    if let Some(s) = v.as_string() {
        return s.to_string().unwrap_or_default();
    }
    if v.is_object()
        && let Ok(Some(s)) = ctx.json_stringify(v.clone())
        && let Ok(s) = s.to_string()
    {
        return s;
    }
    Coerced::<String>::from_js(ctx, v.clone()).map(|c| c.0).unwrap_or_else(|_| "value".into())
}

/// First `script:LINE:COL` frame in a QuickJS stack trace.
fn locate(stack: &str) -> Option<(u32, u32)> {
    let key = format!("{SCRIPT_FILE}:");
    let mut offset = 0;
    while let Some(i) = stack[offset..].find(&key) {
        let at = offset + i;
        let boundary = at == 0 || matches!(stack.as_bytes()[at - 1], b'(' | b' ');
        let after = &stack[at + key.len()..];
        if boundary && let Some(pos) = parse_pos(after) {
            return Some(pos);
        }
        offset = at + key.len();
    }
    None
}

fn parse_pos(s: &str) -> Option<(u32, u32)> {
    let digits = |s: &str| -> Option<(u32, usize)> {
        let n = s.bytes().take_while(u8::is_ascii_digit).count();
        Some((s[..n].parse().ok()?, n))
    };
    let (line, n) = digits(s)?;
    let rest = s[n..].strip_prefix(':')?;
    let (col, _) = digits(rest)?;
    Some((line, col))
}

fn collect(ctx: &Ctx<'_>) -> Result<Collected, String> {
    let json: rquickjs::Result<String> = ctx.globals().get::<_, Object>("__devall").and_then(|h| h.get::<_, Function>("collect")).and_then(|f| f.call(()));
    match json.catch(ctx) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("could not read the script's results: {e}")),
        Err(e) => Err(format!("could not read the script's results: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_frames() {
        assert_eq!(locate("    at <eval> (script:3:1)\n"), Some((3, 1)));
        assert_eq!(locate("    at script:12:6\n"), Some((12, 6)));
        assert_eq!(locate("    at f (devall:prelude:9:2)\n    at script (script:4:10)\n"), Some((4, 10)));
        assert_eq!(locate("    at myscript:1:1\n"), None);
    }
}
