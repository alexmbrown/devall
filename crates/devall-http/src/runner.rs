//! Collection runner shared by the app and `devall run`: sends requests in tree order, chains
//! captured values into later requests, evaluates assertions, supports data-driven iterations,
//! bail-on-failure, and renders JUnit XML for CI.

use std::time::Instant;

use devall_core::vars::SecretLookup;
use devall_core::{Project, Resolver, Variables};
use serde::Serialize;

use crate::checks::{self, AssertionResult, CaptureResult};
use crate::oauth2::{BrowserOpener, TokenStore};
use crate::{ClientPool, HttpResponseData, PreparedRequest, prepare_authorized};

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Repeat the whole sequence this many times (ignored when `data` has rows).
    pub iterations: usize,
    /// One variable layer per iteration (CSV/JSON data file rows).
    pub data: Vec<Variables>,
    /// Stop at the first failure.
    pub bail: bool,
    /// Pause between requests.
    pub delay_ms: u64,
    /// Without tests, treat status >= 400 as failure.
    pub fail_on_http_error: bool,
    /// Fail requests that still contain `{{unresolved}}` references instead of sending them.
    pub fail_on_unresolved: bool,
    /// Write missing or changed response snapshots instead of failing their assertions.
    pub update_snapshots: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { iterations: 1, data: Vec::new(), bail: false, delay_ms: 0, fail_on_http_error: false, fail_on_unresolved: true, update_snapshots: false }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunItemResult {
    pub iteration: usize,
    pub path: String,
    pub name: String,
    pub method: String,
    /// Secrets masked.
    pub url: String,
    pub status: Option<u16>,
    pub error: Option<String>,
    pub duration_ms: f64,
    pub size: usize,
    pub tests: Vec<AssertionResult>,
    pub captures: Vec<CaptureResult>,
    pub passed: bool,
    /// Console output and errors from pre-request / post-response scripts.
    pub scripts: devall_script::ScriptReport,
    /// A pre-request script called `pm.execution.skipRequest()`.
    pub skipped: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub duration_ms: f64,
    pub cancelled: bool,
    pub items: Vec<RunItemResult>,
    /// Runtime variables after the run (captured values).
    pub variables: Variables,
}

/// Callbacks while running (progress UI, history recording, cancellation).
pub trait RunHooks: Send + Sync {
    fn on_item(&self, _result: &RunItemResult, _sent: Option<&PreparedRequest>, _response: Option<&HttpResponseData>, _secret_values: &std::collections::BTreeSet<String>) {}
    fn cancelled(&self) -> bool {
        false
    }
}

pub struct NoHooks;
impl RunHooks for NoHooks {}

pub struct RunContext<'a> {
    pub project: &'a Project,
    pub environment: Option<&'a devall_core::EnvironmentFile>,
    pub secrets: &'a dyn SecretLookup,
    pub tokens: &'a dyn TokenStore,
    pub browser: Option<&'a BrowserOpener>,
    pub pool: &'a ClientPool,
}

/// A request that isn't a project file (e.g. parsed from a `.http` or `.hurl` file): run as is,
/// without folder inheritance; only its own scripts apply.
#[derive(Debug, Clone)]
pub struct InlineRequest {
    /// Shown where a project request shows its path, e.g. `api.hurl#2`.
    pub label: String,
    pub request: devall_core::HttpRequestFile,
}

enum Item<'a> {
    Path(&'a str),
    Inline(&'a InlineRequest),
}

/// Runs `paths` (relative to `http/`, in order). `runtime` holds captured variables and is
/// updated in place so the caller can keep them for the session.
pub async fn run(ctx: &RunContext<'_>, paths: &[String], runtime: &mut Variables, opts: &RunOptions, hooks: &dyn RunHooks) -> RunSummary {
    let items: Vec<Item<'_>> = paths.iter().map(|p| Item::Path(p)).collect();
    run_items(ctx, &items, runtime, opts, hooks).await
}

/// Runs in-memory requests in order, like [`run`] (captures chain, assertions, data rows). Nothing
/// is read from or written to `http/`; snapshot assertions can't be used.
pub async fn run_requests(ctx: &RunContext<'_>, requests: &[InlineRequest], runtime: &mut Variables, opts: &RunOptions, hooks: &dyn RunHooks) -> RunSummary {
    let items: Vec<Item<'_>> = requests.iter().map(Item::Inline).collect();
    run_items(ctx, &items, runtime, opts, hooks).await
}

async fn run_items(ctx: &RunContext<'_>, paths: &[Item<'_>], runtime: &mut Variables, opts: &RunOptions, hooks: &dyn RunHooks) -> RunSummary {
    let start = Instant::now();
    let mut summary = RunSummary::default();
    let iterations: Vec<Option<&Variables>> =
        if opts.data.is_empty() { (0..opts.iterations.max(1)).map(|_| None).collect() } else { opts.data.iter().map(Some).collect() };

    'outer: for (iteration, row) in iterations.into_iter().enumerate() {
        for path in paths {
            if hooks.cancelled() {
                summary.cancelled = true;
                break 'outer;
            }
            let item = run_one(ctx, path, iteration, row, runtime, opts, hooks).await;
            summary.total += 1;
            summary.tests_passed += item.tests.iter().filter(|t| t.passed).count();
            summary.tests_failed += item.tests.iter().filter(|t| !t.passed).count();
            if item.passed {
                summary.passed += 1;
            } else {
                summary.failed += 1;
            }
            let failed = !item.passed;
            summary.items.push(item);
            if failed && opts.bail {
                break 'outer;
            }
            if opts.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(opts.delay_ms)).await;
            }
        }
    }
    summary.duration_ms = start.elapsed().as_secs_f64() * 1000.0;
    summary.variables = runtime.clone();
    summary
}

async fn run_one(
    ctx: &RunContext<'_>,
    item: &Item<'_>,
    iteration: usize,
    row: Option<&Variables>,
    runtime: &mut Variables,
    opts: &RunOptions,
    hooks: &dyn RunHooks,
) -> RunItemResult {
    let (path, inline) = match item {
        Item::Path(p) => (*p, None),
        Item::Inline(r) => (r.label.as_str(), Some(&r.request)),
    };
    let mut item = RunItemResult {
        iteration,
        path: path.to_string(),
        name: path.to_string(),
        method: String::new(),
        url: String::new(),
        status: None,
        error: None,
        duration_ms: 0.0,
        size: 0,
        tests: Vec::new(),
        captures: Vec::new(),
        passed: false,
        scripts: Default::default(),
        skipped: false,
    };
    let req = match inline {
        Some(r) => r.clone(),
        None => match ctx.project.read_request(path) {
            Ok(r) => r,
            Err(e) => {
                item.error = Some(e.to_string());
                hooks.on_item(&item, None, None, &Default::default());
                return item;
            }
        },
    };
    item.name = req.name.clone();
    let effective = match inline {
        Some(r) => {
            let mut request = r.clone();
            if request.auth.is_inherit() {
                request.auth = devall_core::Auth::None;
            }
            Ok(devall_core::project::EffectiveRequest { request, folder_variables: Vec::new() })
        }
        None => ctx.project.effective_request(path, &req),
    };
    let mut eff = match effective {
        Ok(e) => e,
        Err(e) => {
            item.error = Some(e.to_string());
            hooks.on_item(&item, None, None, &Default::default());
            return item;
        }
    };

    // Pre-request scripts (folders outermost first, then the request) may edit the request and
    // set runtime variables before anything is resolved.
    let chain = match inline {
        Some(r) => {
            let mut c = devall_script::ScriptChain::default();
            if let Some(sc) = &r.scripts {
                if !sc.pre_request.trim().is_empty() {
                    c.pre.push(devall_script::ScriptSource::new("request", sc.pre_request.clone()));
                }
                if !sc.post_response.trim().is_empty() {
                    c.post.push(devall_script::ScriptSource::new("request", sc.post_response.clone()));
                }
            }
            Ok(c)
        }
        None => devall_script::collect_scripts(ctx.project, path, &req),
    };
    let chain = match chain {
        Ok(c) => c,
        Err(e) => {
            item.error = Some(e.to_string());
            hooks.on_item(&item, None, None, &Default::default());
            return item;
        }
    };
    let engine = devall_script::ScriptEngine::new();
    let senv = devall_script::ScriptEnv {
        environment: ctx.environment.map(|e| &e.variables),
        environment_name: ctx.environment.map(|e| e.name.as_str()),
        folder_variables: &eff.folder_variables,
        project_variables: Some(&ctx.project.manifest.variables),
        iteration_data: row,
        iteration,
        iteration_count: if opts.data.is_empty() { opts.iterations.max(1) } else { opts.data.len() },
        request_path: path,
        secrets: Some(ctx.secrets),
    };
    let mut script_secrets = std::collections::BTreeSet::new();
    if !chain.pre.is_empty() {
        let out = devall_script::apply_pre_request(&engine, &chain.pre, &mut eff.request, runtime, &senv);
        item.tests.extend(out.tests.into_iter().map(AssertionResult::from));
        item.scripts.merge(out.report);
        script_secrets = out.secret_values;
        if let Some(e) = out.error {
            item.error = Some(e.to_string());
            hooks.on_item(&item, None, None, &script_secrets);
            return item;
        }
        if out.skip_request {
            item.passed = true;
            item.skipped = true;
            hooks.on_item(&item, None, None, &script_secrets);
            return item;
        }
    }

    let mut r = Resolver::new(ctx.secrets);
    r.push_layer(runtime.clone());
    if let Some(row) = row {
        r.push_layer(row.clone());
    }
    if let Some(env) = ctx.environment {
        r.push_layer(env.variables.clone());
    }
    for fv in &eff.folder_variables {
        r.push_layer(fv.clone());
    }
    r.push_layer(ctx.project.manifest.variables.clone());
    r.secret_values.extend(script_secrets);

    let mut prepared = match prepare_authorized(&eff.request, &mut r, ctx.project.root(), ctx.tokens, ctx.browser).await {
        Ok(p) => p,
        Err(e) => {
            item.error = Some(e.redacted(&r.secret_values).to_string());
            hooks.on_item(&item, None, None, &r.secret_values);
            return item;
        }
    };
    // Plugin pre-request hooks see (and may edit) the resolved request.
    let plugins = crate::plugins::installed();
    if let Some(pl) = plugins.as_ref().filter(|pl| pl.has_hooks(ctx.project.root(), devall_script::ScriptPhase::PreRequest)) {
        let out = pl.pre_request(ctx.project.root(), &mut prepared, &req.name, ctx.environment.map(|e| e.name.as_str()));
        item.scripts.merge(crate::plugins::report(devall_script::ScriptPhase::PreRequest, out.logs));
        if let Some(e) = out.errors.first() {
            item.error = Some(r.redact(&format!("plugin hook: {e}")));
            hooks.on_item(&item, Some(&prepared), None, &r.secret_values);
            return item;
        }
    }
    item.method = prepared.method.clone();
    item.url = r.redact(&prepared.url);
    if opts.fail_on_unresolved && !r.unresolved.is_empty() {
        let list: Vec<String> = r.unresolved.iter().map(|u| format!("{{{{{u}}}}}")).collect();
        item.error = Some(format!("unresolved: {}", list.join(", ")));
        hooks.on_item(&item, Some(&prepared), None, &r.secret_values);
        return item;
    }
    match ctx.pool.execute(&prepared).await {
        Ok(resp) => {
            item.status = Some(resp.status);
            item.duration_ms = resp.timings.total_ms;
            item.size = resp.body_size;
            let cx = checks::CheckContext { project_root: Some(ctx.project.root()), request_path: inline.is_none().then_some(path), update_snapshots: opts.update_snapshots && inline.is_none() };
            item.tests.extend(checks::evaluate_in(&eff.request.tests, &resp, &mut r, &cx));
            let (vars, caps) = checks::capture(&eff.request.captures, &resp);
            runtime.extend(vars);
            item.captures = caps;
            if !chain.post.is_empty() {
                let info = devall_script::ResponseInfo::new(resp.status, &resp.status_text, &resp.headers, &resp.body, resp.timings.total_ms, resp.body_size);
                let out = devall_script::apply_post_response(&engine, &chain.post, &eff.request, &info, runtime, &senv);
                item.tests.extend(out.tests.into_iter().map(AssertionResult::from));
                if let Some(e) = &out.error {
                    // A failing post-response script fails the request like a failed assertion.
                    item.tests.push(AssertionResult { label: format!("{} script ({})", e.phase.label(), e.source), passed: false, actual: None, message: Some(e.to_string()) });
                }
                item.scripts.merge(out.report);
            }
            if let Some(pl) = plugins.as_ref().filter(|pl| pl.has_hooks(ctx.project.root(), devall_script::ScriptPhase::PostResponse)) {
                let out = pl.post_response(ctx.project.root(), &prepared, &resp, &req.name);
                item.tests.extend(out.tests);
                item.tests.extend(crate::plugins::error_rows(&out.errors));
                runtime.extend(out.variables);
                item.scripts.merge(crate::plugins::report(devall_script::ScriptPhase::PostResponse, out.logs));
            }
            let tests_ok = item.tests.iter().all(|t| t.passed);
            let status_ok = !(opts.fail_on_http_error && eff.request.tests.is_empty() && resp.status >= 400);
            item.passed = tests_ok && status_ok;
            hooks.on_item(&item, Some(&prepared), Some(&resp), &r.secret_values);
        }
        Err(e) => {
            item.error = Some(e.redacted(&r.secret_values).to_string());
            hooks.on_item(&item, Some(&prepared), None, &r.secret_values);
        }
    }
    item
}

/// Parses a data file for data-driven runs: CSV (header row) or JSON (array of objects).
pub fn parse_data_file(text: &str, is_json: bool) -> Result<Vec<Variables>, String> {
    if is_json {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("data file is not valid JSON: {e}"))?;
        let arr = v.as_array().ok_or("JSON data file must be an array of objects")?;
        return arr
            .iter()
            .map(|row| {
                let obj = row.as_object().ok_or("every JSON data row must be an object")?;
                Ok(obj.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect())
            })
            .collect();
    }
    let mut lines = csv_records(text).into_iter();
    let header = lines.next().ok_or("CSV data file is empty")?;
    Ok(lines.filter(|r| !(r.len() == 1 && r[0].is_empty())).map(|rec| header.iter().cloned().zip(rec.into_iter().chain(std::iter::repeat(String::new()))).collect()).collect())
}

/// Minimal RFC 4180 CSV reader (quoted fields, escaped quotes, CRLF).
fn csv_records(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                out.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        out.push(row);
    }
    out
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// JUnit XML (one test case per request execution) for CI systems.
pub fn to_junit(summary: &RunSummary, suite: &str) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites name=\"{}\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\">\n  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\">\n",
        xml(suite),
        summary.total,
        summary.failed,
        summary.duration_ms / 1000.0,
        xml(suite),
        summary.total,
        summary.failed,
        summary.duration_ms / 1000.0
    ));
    for i in &summary.items {
        let name = if summary.items.iter().any(|x| x.iteration > 0) { format!("{} [#{}]", i.name, i.iteration + 1) } else { i.name.clone() };
        out.push_str(&format!("    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\">\n", xml(&i.path), xml(&name), i.duration_ms / 1000.0));
        if let Some(e) = &i.error {
            out.push_str(&format!("      <error message=\"{}\"/>\n", xml(e)));
        } else if !i.passed {
            let failed: Vec<String> = i.tests.iter().filter(|t| !t.passed).map(|t| format!("{} (actual: {})", t.label, t.actual.as_deref().unwrap_or("—"))).collect();
            let msg = if failed.is_empty() { format!("HTTP {}", i.status.unwrap_or(0)) } else { failed.join("; ") };
            out.push_str(&format!("      <failure message=\"{}\"/>\n", xml(&msg)));
        }
        out.push_str(&format!("      <system-out>{} {} → {}</system-out>\n", xml(&i.method), xml(&i.url), i.status.map(|s| s.to_string()).unwrap_or_else(|| "error".into())));
        out.push_str("    </testcase>\n");
    }
    out.push_str("  </testsuite>\n</testsuites>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth2::MemoryTokenStore;
    use devall_core::vars::NoSecrets;
    use devall_core::{AssertOp, AssertTarget, Assertion, Capture, CaptureSource, HttpRequestFile};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// `/login` returns a token; `/me` echoes the Authorization header as JSON.
    async fn server() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let auth = req.lines().find_map(|l| l.strip_prefix("authorization: ").or_else(|| l.strip_prefix("Authorization: "))).unwrap_or("").to_string();
                    let body = if req.starts_with("POST /login") { r#"{"token":"abc"}"#.to_string() } else { format!(r#"{{"auth":"{auth}"}}"#) };
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn chains_captures_and_reports() {
        let base = server().await;
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Run").unwrap();
        let mut login = HttpRequestFile::new("Login");
        login.method = "POST".into();
        login.url = format!("{base}/login");
        login.captures = vec![Capture { name: "token".into(), source: CaptureSource::JsonPath { path: "$.token".into() }, enabled: true }];
        let mut me = HttpRequestFile::new("Me");
        me.url = format!("{base}/me");
        me.headers = vec![devall_core::KeyValue::new("Authorization", "Bearer {{token}}")];
        me.tests = vec![
            Assertion { target: AssertTarget::JsonPath { path: "$.auth".into() }, op: AssertOp::Eq, value: Some("Bearer {{token}}".into()), enabled: true },
            Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some("201".into()), enabled: true },
        ];
        let a = p.create_request("", login).unwrap();
        let b = p.create_request("", me).unwrap();

        let tokens = MemoryTokenStore::default();
        let pool = ClientPool::new();
        let ctx = RunContext { project: &p, environment: None, secrets: &NoSecrets, tokens: &tokens, browser: None, pool: &pool };
        let mut runtime = Variables::new();
        let s = run(&ctx, &[a.clone(), b.clone()], &mut runtime, &RunOptions::default(), &NoHooks).await;
        assert_eq!(runtime["token"], "abc");
        assert_eq!(s.total, 2);
        assert_eq!(s.passed, 1);
        assert!(s.items[1].tests[0].passed, "{:?}", s.items[1].tests);
        assert!(!s.items[1].tests[1].passed);
        let junit = to_junit(&s, "Run");
        assert!(junit.contains("tests=\"2\" failures=\"1\""));
        assert!(junit.contains("<failure message=\"status eq 201 (actual: 200)\"/>"), "{junit}");

        // Bail stops after the first failure; data rows drive iterations.
        let mut runtime = Variables::new();
        let opts = RunOptions { bail: true, data: vec![Variables::new(), Variables::new()], ..Default::default() };
        let s = run(&ctx, &[b.clone(), a.clone()], &mut runtime, &opts, &NoHooks).await;
        assert_eq!(s.total, 1, "unresolved {{token}} fails first and bails");
        assert!(s.items[0].error.as_deref().unwrap().contains("unresolved"));
    }

    #[test]
    fn data_files() {
        let rows = parse_data_file("user,note\r\nada,\"hello, \"\"world\"\"\"\nbob,\n", false).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["note"], "hello, \"world\"");
        assert_eq!(rows[1]["note"], "");
        let rows = parse_data_file(r#"[{"id":1,"name":"x"},{"id":2,"name":"y"}]"#, true).unwrap();
        assert_eq!(rows[1]["id"], "2");
        assert!(parse_data_file("{}", true).is_err());
    }
}
