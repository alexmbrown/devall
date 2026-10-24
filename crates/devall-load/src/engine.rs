//! The load engine.
//!
//! Runs on its own multi-threaded tokio runtime (separate from the app's) so UI work cannot
//! perturb timing. Arrival-rate executors are driven by a dedicated OS thread that sleeps with
//! the platform's high-resolution timer and hands each iteration, stamped with its *intended*
//! start time, to the runtime; latency is measured from that stamp (coordinated-omission
//! correct). If no VU is free and `maxVus` is reached, the iteration is dropped and counted —
//! never delayed. VU executors loop per VU and measure from the actual send.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devall_core::Resolver;
use devall_core::vars::redact_values;
use devall_http::{PreparedBody, PreparedRequest, prepare};
use reqwest::cookie::{CookieStore, Jar};
use tokio::sync::{Notify, Semaphore};

use crate::error::{LoadError, Result};
use crate::metrics::{ErrorKind, Recorder, RecorderExport, RecorderMetrics, Sample, bucket_seconds};
use crate::model::{Check, Executor};
use crate::plan::Plan;
use crate::schedule::{ArrivalSchedule, vus_at};
use crate::summary::{LoadSnapshot, LoadSummary, Machine, RunPhase, SUMMARY_VERSION};
use crate::threshold::evaluate_all;
use crate::validate::status_matches;

/// Receives live snapshots (called from an engine thread).
pub type ProgressFn = Arc<dyn Fn(LoadSnapshot) + Send + Sync>;

/// Bodies are only buffered (up to this size) when a `bodyContains` check needs them.
const MAX_CHECK_BODY: usize = 1024 * 1024;

/// Stops a run early. Cloning shares the same run.
#[derive(Clone, Default)]
pub struct RunControl {
    inner: Arc<ControlInner>,
}

#[derive(Default)]
struct ControlInner {
    stopped: AtomicBool,
    notify: Notify,
    reason: Mutex<Option<String>>,
}

impl RunControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops issuing iterations; in-flight requests get a short grace period. The summary is
    /// still produced and marked `aborted`.
    pub fn stop(&self) {
        self.stop_with("stopped by user");
    }

    pub fn stop_with(&self, reason: impl Into<String>) {
        if !self.inner.stopped.swap(true, Ordering::SeqCst) {
            *self.inner.reason.lock().unwrap() = Some(reason.into());
        }
        self.inner.notify.notify_waiters();
    }

    pub fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    pub fn reason(&self) -> Option<String> {
        self.inner.reason.lock().unwrap().clone()
    }
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Identifies the run in snapshots and the summary (a ULID when empty).
    pub run_id: String,
    pub snapshot_interval: Duration,
    /// Engine runtime threads (default: CPU count, 2..=8).
    pub worker_threads: Option<usize>,
    /// How long in-flight iterations may run after the planned duration ends.
    pub graceful_stop: Duration,
    /// How long in-flight requests may run after [`RunControl::stop`].
    pub abort_grace: Duration,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            run_id: String::new(),
            snapshot_interval: Duration::from_millis(250),
            worker_threads: None,
            graceful_stop: Duration::from_secs(30),
            abort_grace: Duration::from_secs(2),
        }
    }
}

/// Runs a planned test to completion (or until stopped) and returns its summary. The summary is
/// not written anywhere; see [`crate::store::write_run`].
pub async fn run(plan: Plan, opts: RunOptions, progress: Option<ProgressFn>, control: RunControl) -> Result<LoadSummary> {
    Ok(run_detailed(plan, opts, progress, control).await?.0)
}

/// [`run`], plus the raw measurements (HDR histograms, counters) for merging with other agents'.
pub async fn run_detailed(plan: Plan, opts: RunOptions, progress: Option<ProgressFn>, control: RunControl) -> Result<(LoadSummary, RecorderExport)> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let threads = opts.worker_threads.unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8));
    std::thread::Builder::new()
        .name("devall-load".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(threads).thread_name("devall-load-worker").enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(LoadError::Engine(e.to_string())));
                    return;
                }
            };
            let out = rt.block_on(execute(plan, opts, progress, control));
            let _ = tx.send(out);
            rt.shutdown_timeout(Duration::from_secs(1));
        })
        .map_err(|e| LoadError::Engine(e.to_string()))?;
    rx.await.map_err(|_| LoadError::Engine("the load engine stopped unexpectedly".into()))?
}

struct Shared {
    plan: Plan,
    clients: Vec<reqwest::Client>,
    rec: Mutex<Recorder>,
    start: Instant,
    control: RunControl,
    /// Planned duration over (or stopped): start no new iterations.
    ending: AtomicBool,
    /// Grace expired: abandon in-flight work.
    aborting: AtomicBool,
    abort_now: Notify,
    /// VUs busy now, and the peak since the last snapshot.
    busy: AtomicU32,
    peak_busy: AtomicU32,
    /// Allocated (open) or targeted (closed) VUs.
    allocated: AtomicU32,
    conn_limit: Option<Arc<Semaphore>>,
    /// Weighted mode: (cumulative weight, request index).
    cumulative: Vec<(u32, usize)>,
    total_weight: u32,
    checks: Vec<Check>,
    keep_body: bool,
}

struct BusyGuard<'a>(&'a Shared);

impl<'a> BusyGuard<'a> {
    fn new(sh: &'a Shared) -> Self {
        let now = sh.busy.fetch_add(1, Ordering::SeqCst) + 1;
        sh.peak_busy.fetch_max(now, Ordering::SeqCst);
        Self(sh)
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Shared {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    fn secrets_redact(&self, text: &str) -> String {
        redact_values(text, &self.plan.secret_values)
    }

    fn pick(&self) -> usize {
        if self.cumulative.len() == 1 || self.total_weight == 0 {
            return self.cumulative.first().map(|c| c.1).unwrap_or(0);
        }
        let r = fastrand::u32(0..self.total_weight);
        self.cumulative.iter().find(|(c, _)| r < *c).map(|c| c.1).unwrap_or(0)
    }

    /// Waits for `d` unless the run is stopped or aborted first.
    async fn pause(&self, d: Duration) {
        if d.is_zero() {
            return;
        }
        let stop = self.control.inner.notify.notified();
        let abort = self.abort_now.notified();
        if self.control.is_stopped() || self.aborting.load(Ordering::SeqCst) {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(d) => {}
            _ = stop => {}
            _ = abort => {}
        }
    }

    /// One iteration, abandoned if the grace period after the end runs out.
    async fn guarded_iteration(&self, intended: Option<Instant>) {
        let abort = self.abort_now.notified();
        if self.aborting.load(Ordering::SeqCst) {
            return;
        }
        tokio::select! {
            _ = self.iteration(intended) => {}
            _ = abort => {}
        }
    }

    async fn iteration(&self, intended: Option<Instant>) {
        if self.plan.flow.is_empty() {
            self.send(self.pick(), intended, None).await;
        } else {
            let jar = Jar::default();
            for (step, &idx) in self.plan.flow.iter().enumerate() {
                if step > 0 {
                    if self.control.is_stopped() {
                        return;
                    }
                    if let Some(tt) = &self.plan.test.think_time {
                        self.pause(tt.sample()).await;
                    }
                    if self.control.is_stopped() {
                        return;
                    }
                }
                self.send(idx, if step == 0 { intended } else { None }, Some(&jar)).await;
            }
        }
        let at = self.elapsed();
        self.rec.lock().unwrap().iteration_done(at);
    }

    fn prepare_now(&self, idx: usize) -> std::result::Result<PreparedRequest, String> {
        let p = &self.plan.requests[idx];
        let mut r = Resolver::new(self.plan.secrets.as_ref());
        for l in &p.layers {
            r.push_layer(l.clone());
        }
        let mut prepared = prepare(&p.template, &mut r, &self.plan.project_root).map_err(|e| e.to_string())?;
        if let Some(base) = &self.plan.base_override {
            prepared.url = crate::plan::rebase_url(&prepared.url, base);
        }
        Ok(prepared)
    }

    async fn send(&self, idx: usize, intended: Option<Instant>, jar: Option<&Jar>) {
        let wanted = intended.unwrap_or_else(Instant::now);
        let planned = &self.plan.requests[idx];
        let req: Cow<'_, PreparedRequest> = match &planned.fixed {
            Some(p) => Cow::Borrowed(p),
            None => match self.prepare_now(idx) {
                Ok(p) => Cow::Owned(p),
                Err(msg) => {
                    self.record_error(idx, ErrorKind::Other, &msg, None, None, 0);
                    return;
                }
            },
        };
        let _permit = match &self.conn_limit {
            Some(s) => s.clone().acquire_owned().await.ok(),
            None => None,
        };
        let bytes_out = estimate_request_bytes(&req);
        let started = Instant::now();
        let outcome = self.exchange(idx, &req, jar).await;
        let end = Instant::now();
        let latency = end.duration_since(wanted);
        let service = end.duration_since(started);
        match outcome {
            Ok((status, bytes_in, body)) => {
                let (passed, failed) = self.run_checks(Some(status), latency, body.as_deref());
                let sample = Sample {
                    req: idx,
                    latency: Some(latency),
                    service: Some(service),
                    status: Some(status),
                    error: None,
                    bytes_in,
                    bytes_out,
                    checks_passed: passed,
                    checks_failed: failed,
                };
                let at = end.duration_since(self.start);
                self.rec.lock().unwrap().record(sample, at);
            }
            Err((kind, msg)) => {
                // Timeouts are real (long) latencies; refused connections are not.
                let timing = (kind == ErrorKind::Timeout).then_some((latency, service));
                self.record_error(idx, kind, &msg, timing.map(|t| t.0), timing.map(|t| t.1), bytes_out);
                if kind == ErrorKind::Connect && looks_like_port_exhaustion(&msg) {
                    self.control.stop_with("aborted: the machine ran out of ephemeral ports (connections are not being reused)");
                }
            }
        }
    }

    fn record_error(&self, idx: usize, kind: ErrorKind, msg: &str, latency: Option<Duration>, service: Option<Duration>, bytes_out: u64) {
        let (passed, failed) = self.run_checks(None, latency.unwrap_or_default(), None);
        let mut text = self.secrets_redact(msg);
        if text.len() > 300 {
            text.truncate(text.floor_char_boundary(300));
            text.push('…');
        }
        let sample = Sample {
            req: idx,
            latency,
            service,
            status: None,
            error: Some((kind, format!("{}: {text}", self.plan.requests[idx].path))),
            bytes_in: 0,
            bytes_out,
            checks_passed: passed,
            checks_failed: failed,
        };
        let at = self.elapsed();
        self.rec.lock().unwrap().record(sample, at);
    }

    /// (checks passed, checks failed) for one response. Without configured checks, the default
    /// check is "got a response with status < 400".
    fn run_checks(&self, status: Option<u16>, latency: Duration, body: Option<&[u8]>) -> (u32, u32) {
        if self.checks.is_empty() {
            return if status.is_some_and(|s| s < 400) { (1, 0) } else { (0, 1) };
        }
        let mut passed = 0;
        for c in &self.checks {
            let ok = match (c, status) {
                (_, None) => false,
                (Check::Status { expect }, Some(s)) => expect.iter().any(|p| status_matches(p, s)),
                (Check::MaxDuration { duration }, Some(_)) => latency <= duration.get(),
                (Check::BodyContains { text }, Some(_)) => body.is_some_and(|b| contains(b, text.as_bytes())),
            };
            passed += ok as u32;
        }
        (passed, self.checks.len() as u32 - passed)
    }

    /// Sends one request and drains the response. Returns (status, approx bytes in, body if kept).
    async fn exchange(&self, idx: usize, req: &PreparedRequest, jar: Option<&Jar>) -> std::result::Result<(u16, u64, Option<Vec<u8>>), (ErrorKind, String)> {
        let planned = &self.plan.requests[idx];
        let client = &self.clients[idx];
        let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|_| (ErrorKind::Other, format!("bad method {}", req.method)))?;
        let url = reqwest::Url::parse(&req.url).map_err(|e| (ErrorKind::Other, format!("invalid URL {} ({e})", req.url)))?;
        let mut rb = client.request(method, url.clone()).timeout(planned.timeout);
        for (k, v) in &req.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if let Some(jar) = jar
            && req.header("cookie").is_none()
            && let Some(c) = jar.cookies(&url)
        {
            rb = rb.header(reqwest::header::COOKIE, c);
        }
        rb = match &req.body {
            PreparedBody::None => rb,
            PreparedBody::Bytes(data) => rb.body(data.clone()),
            PreparedBody::Form(pairs) => {
                let encoded = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs.iter()).finish();
                let rb = if req.header("content-type").is_none() { rb.header("Content-Type", "application/x-www-form-urlencoded") } else { rb };
                rb.body(encoded)
            }
            PreparedBody::Multipart(parts) => {
                let mut form = reqwest::multipart::Form::new();
                for p in parts {
                    let mut part = match &p.file {
                        Some(path) => {
                            let bytes = tokio::fs::read(path).await.map_err(|e| (ErrorKind::Other, format!("cannot read {}: {e}", path.display())))?;
                            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            reqwest::multipart::Part::bytes(bytes).file_name(name)
                        }
                        None => reqwest::multipart::Part::text(p.value.clone()),
                    };
                    if let Some(ct) = &p.content_type {
                        part = part.mime_str(ct).map_err(|e| (ErrorKind::Other, format!("bad content type {ct}: {e}")))?;
                    }
                    form = form.part(p.name.clone(), part);
                }
                rb.multipart(form)
            }
        };
        let mut resp = rb.send().await.map_err(classify)?;
        let status = resp.status().as_u16();
        let mut bytes_in = 17 + resp.headers().iter().map(|(k, v)| (k.as_str().len() + v.len() + 4) as u64).sum::<u64>();
        if let Some(jar) = jar {
            let final_url = resp.url().clone();
            jar.set_cookies(&mut resp.headers().get_all(reqwest::header::SET_COOKIE).iter(), &final_url);
        }
        let mut body = self.keep_body.then(Vec::new);
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    bytes_in += chunk.len() as u64;
                    if let Some(b) = body.as_mut()
                        && b.len() < MAX_CHECK_BODY
                    {
                        let room = MAX_CHECK_BODY - b.len();
                        b.extend_from_slice(&chunk[..chunk.len().min(room)]);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let kind = if e.is_timeout() { ErrorKind::Timeout } else { ErrorKind::Other };
                    return Err((kind, format!("reading body: {}", devall_http::error_chain(&e))));
                }
            }
        }
        Ok((status, bytes_in, body))
    }
}

fn classify(e: reqwest::Error) -> (ErrorKind, String) {
    let msg = devall_http::error_chain(&e);
    let kind = if e.is_timeout() {
        ErrorKind::Timeout
    } else if e.is_connect() {
        ErrorKind::Connect
    } else {
        ErrorKind::Other
    };
    (kind, msg)
}

fn looks_like_port_exhaustion(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    ["os error 10048", "os error 10055", "only one usage of each socket address", "address already in use", "cannot assign requested address", "os error 99)"]
        .iter()
        .any(|n| m.contains(n))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// Request line + headers + body; approximate (the client adds a few headers of its own).
fn estimate_request_bytes(req: &PreparedRequest) -> u64 {
    let head = req.method.len() + req.url.len() + 12 + req.headers.iter().map(|(k, v)| k.len() + v.len() + 4).sum::<usize>() + 2;
    let body = match &req.body {
        PreparedBody::None => 0,
        PreparedBody::Bytes(b) => b.len(),
        PreparedBody::Form(pairs) => pairs.iter().map(|(k, v)| k.len() + v.len() + 2).sum(),
        PreparedBody::Multipart(parts) => parts.iter().map(|p| p.name.len() + p.value.len() + 64).sum(),
    };
    (head + body) as u64
}

fn build_client(plan: &Plan, accept_invalid: bool, follow_redirects: bool, pool: usize) -> Result<reqwest::Client> {
    let b = reqwest::Client::builder()
        .tls_danger_accept_invalid_certs(accept_invalid)
        .redirect(if follow_redirects { reqwest::redirect::Policy::limited(10) } else { reqwest::redirect::Policy::none() })
        // Keep every connection a VU opened: closing them would churn ephemeral ports.
        .pool_max_idle_per_host(pool)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        // Measure wire bytes and keep generator CPU low: no transparent decompression.
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate();
    let b = if plan.test.http.http2 { b.http2_prior_knowledge() } else { b.http1_only() };
    b.build().map_err(|e| LoadError::Engine(format!("cannot create HTTP client: {}", devall_http::error_chain(&e))))
}

async fn execute(plan: Plan, opts: RunOptions, progress: Option<ProgressFn>, control: RunControl) -> Result<(LoadSummary, RecorderExport)> {
    let run_id = if opts.run_id.is_empty() { ulid::Ulid::generate().to_string() } else { opts.run_id.clone() };
    let executor = plan.test.executor.clone();
    let planned = executor.planned_duration();
    let max_vus = executor.max_vus();
    let preflight = plan.preflight();

    let pool = max_vus.max(plan.test.http.max_connections.unwrap_or(0)) as usize + 8;
    let mut clients = Vec::with_capacity(plan.requests.len());
    let mut built: Vec<((bool, bool), reqwest::Client)> = Vec::new();
    for r in &plan.requests {
        let key = (r.accept_invalid_certs, r.probe.follow_redirects);
        let client = match built.iter().find(|(k, _)| *k == key) {
            Some((_, c)) => c.clone(),
            None => {
                let c = build_client(&plan, key.0, key.1, pool)?;
                built.push((key, c.clone()));
                c
            }
        };
        clients.push(client);
    }

    let mut cumulative = Vec::new();
    let mut acc = 0u32;
    for (idx, w) in &plan.weighted {
        acc += *w;
        cumulative.push((acc, *idx));
    }
    let checks = plan.test.checks.clone();
    let keep_body = checks.iter().any(|c| matches!(c, Check::BodyContains { .. }));
    let bucket = bucket_seconds(planned);
    let n_requests = plan.requests.len();
    let open = executor.is_open();
    let started_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let sh = Arc::new(Shared {
        conn_limit: plan.test.http.max_connections.map(|n| Arc::new(Semaphore::new(n as usize))),
        plan,
        clients,
        rec: Mutex::new(Recorder::new(n_requests, bucket, open)),
        start: Instant::now(),
        control: control.clone(),
        ending: AtomicBool::new(false),
        aborting: AtomicBool::new(false),
        abort_now: Notify::new(),
        busy: AtomicU32::new(0),
        peak_busy: AtomicU32::new(0),
        allocated: AtomicU32::new(0),
        cumulative,
        total_weight: acc,
        checks,
        keep_body,
    });

    let ticker_done = Arc::new(Notify::new());
    let ticker = tokio::spawn(snapshot_loop(sh.clone(), progress.clone(), run_id.clone(), opts.snapshot_interval, ticker_done.clone()));

    match &executor {
        Executor::ConstantVus { vus, duration } => {
            run_closed(sh.clone(), *vus, vec![(duration.get(), *vus)], &opts).await;
        }
        Executor::RampingVus { start_vus, stages } => {
            let stages = stages.iter().map(|s| (s.duration.get(), s.target.max(0.0).round() as u32)).collect();
            run_closed(sh.clone(), *start_vus, stages, &opts).await;
        }
        Executor::ConstantArrivalRate { pre_allocated_vus, .. } | Executor::RampingArrivalRate { pre_allocated_vus, .. } => {
            let schedule = ArrivalSchedule::for_executor(&executor).expect("arrival-rate executor");
            run_open(sh.clone(), schedule, planned, *pre_allocated_vus, max_vus, &opts).await;
        }
    }

    let end = sh.elapsed();
    ticker_done.notify_one();
    let _ = ticker.await;

    let aborted = control.is_stopped();
    let abort_reason = aborted.then(|| control.reason().unwrap_or_else(|| "stopped".into()));
    let mut rec = sh.rec.lock().unwrap();
    rec.observe_vus(sh.peak_busy.swap(0, Ordering::SeqCst), end);
    rec.finish(end);
    let final_points = rec.take_new_points();

    let paths = sh.plan.request_paths();
    let thresholds = evaluate_all(&sh.plan.test.thresholds, &RecorderMetrics { rec: &rec, paths: &paths });
    let thresholds_passed = thresholds.iter().all(|t| t.ok);
    let mut warnings = preflight.warnings.clone();
    if rec.totals.dropped_iterations > 0 {
        warnings.push(format!(
            "{} iterations were dropped because all {max_vus} VUs were busy. Raise maxVus, or the target can't keep up with this rate.",
            rec.totals.dropped_iterations
        ));
    }
    for e in &sh.plan.secret_errors {
        warnings.push(format!("Keychain: {e}"));
    }

    let summary = LoadSummary {
        version: SUMMARY_VERSION,
        run_id: run_id.clone(),
        test: sh.plan.test.name.clone(),
        test_path: sh.plan.test_path.clone(),
        environment: sh.plan.environment.clone(),
        mock_baseline: sh.plan.mock_baseline.clone(),
        started_at,
        duration_ms: end.as_millis() as u64,
        planned_duration_ms: planned.as_millis() as u64,
        aborted,
        abort_reason,
        thresholds_passed,
        executor,
        open_model: open,
        totals: rec.totals.clone(),
        latency: rec.latency(),
        service_time: rec.service_time(),
        status_codes: rec.status_codes(),
        error_kinds: rec.errors,
        error_samples: rec.error_samples.clone(),
        requests: rec.request_stats(&sh.plan.request_meta()),
        thresholds,
        warnings: warnings.iter().map(|w| sh.secrets_redact(w)).collect(),
        bucket_seconds: rec.bucket_seconds(),
        timeline: rec.timeline().to_vec(),
        git: crate::store::git_info(&sh.plan.project_root),
        machine: Machine::current(),
        agents: None,
    };
    let export = rec.export(&paths).map_err(LoadError::Engine)?;

    if let Some(p) = &progress {
        p(LoadSnapshot {
            run_id,
            phase: RunPhase::Finished,
            elapsed_ms: end.as_millis() as u64,
            planned_duration_ms: planned.as_millis() as u64,
            vus: 0,
            vus_allocated: sh.allocated.load(Ordering::SeqCst),
            requests: rec.totals.requests,
            failed: rec.totals.failed,
            iterations: rec.totals.iterations,
            dropped_iterations: rec.totals.dropped_iterations,
            rps: rec.totals.rps,
            latency: summary.latency,
            bytes_in: rec.totals.bytes_in,
            bytes_out: rec.totals.bytes_out,
            status_codes: summary.status_codes.clone(),
            error_kinds: rec.errors,
            points: final_points,
            bucket_seconds: summary.bucket_seconds,
        });
    }
    Ok((summary, export))
}

/// Waits for `done` (true when finished) up to `grace`, then abandons in-flight work.
async fn drain<F: Future<Output = ()>>(sh: &Shared, grace: Duration, done: F) {
    tokio::pin!(done);
    if tokio::time::timeout(grace, &mut done).await.is_err() {
        sh.aborting.store(true, Ordering::SeqCst);
        sh.abort_now.notify_waiters();
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut done).await;
    }
}

fn grace(sh: &Shared, opts: &RunOptions) -> Duration {
    if sh.control.is_stopped() { opts.abort_grace } else { opts.graceful_stop }
}

async fn run_closed(sh: Arc<Shared>, start_vus: u32, stages: Vec<(Duration, u32)>, opts: &RunOptions) {
    let total: Duration = stages.iter().map(|s| s.0).sum();
    let target = Arc::new(AtomicU32::new(0));
    let mut vus: Vec<Option<tokio::task::JoinHandle<()>>> = Vec::new();
    let deadline = tokio::time::Instant::from_std(sh.start + total);
    loop {
        let stop = sh.control.inner.notify.notified();
        let t = sh.elapsed();
        if t >= total || sh.control.is_stopped() {
            break;
        }
        let tv = vus_at(start_vus, &stages, t);
        target.store(tv, Ordering::SeqCst);
        sh.allocated.store(tv, Ordering::SeqCst);
        for i in 0..tv as usize {
            if vus.len() <= i {
                vus.push(None);
            }
            if !vus[i].as_ref().is_some_and(|h| !h.is_finished()) {
                vus[i] = Some(tokio::spawn(vu_loop(sh.clone(), i as u32, target.clone())));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            _ = tokio::time::sleep_until(deadline) => {}
            _ = stop => {}
        }
    }
    sh.ending.store(true, Ordering::SeqCst);
    target.store(0, Ordering::SeqCst);
    let handles: Vec<_> = vus.into_iter().flatten().collect();
    let g = grace(&sh, opts);
    drain(&sh, g, async {
        for h in handles {
            let _ = h.await;
        }
    })
    .await;
}

async fn vu_loop(sh: Arc<Shared>, idx: u32, target: Arc<AtomicU32>) {
    let _busy = BusyGuard::new(&sh);
    let active = |sh: &Shared| !sh.ending.load(Ordering::SeqCst) && !sh.control.is_stopped() && idx < target.load(Ordering::SeqCst);
    while active(&sh) {
        sh.guarded_iteration(None).await;
        if !active(&sh) {
            break;
        }
        if let Some(tt) = &sh.plan.test.think_time {
            sh.pause(tt.sample()).await;
        }
    }
}

async fn run_open(sh: Arc<Shared>, schedule: ArrivalSchedule, planned: Duration, pre_allocated: u32, max_vus: u32, opts: &RunOptions) {
    let sem = Arc::new(Semaphore::new(pre_allocated as usize));
    sh.allocated.store(pre_allocated, Ordering::SeqCst);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::runtime::Handle::current();
    let (sh2, sem2) = (sh.clone(), sem.clone());
    let spawned = std::thread::Builder::new().name("devall-load-scheduler".into()).spawn(move || {
        scheduler(&sh2, &handle, schedule, &sem2, planned, max_vus);
        let _ = tx.send(());
    });
    if spawned.is_ok() {
        let _ = rx.await;
    } else {
        sh.control.stop_with("could not start the scheduler thread");
    }
    sh.ending.store(true, Ordering::SeqCst);
    let allocated = sh.allocated.load(Ordering::SeqCst);
    let g = grace(&sh, opts);
    drain(&sh, g, async {
        let _ = sem.acquire_many(allocated).await;
    })
    .await;
}

/// Runs on its own OS thread: `std::thread::sleep` uses high-resolution timers (unlike the
/// runtime's millisecond timer wheel), so iterations start close to their intended time.
fn scheduler(sh: &Arc<Shared>, handle: &tokio::runtime::Handle, schedule: ArrivalSchedule, sem: &Arc<Semaphore>, planned: Duration, max_vus: u32) {
    let sleep_until = |due: Instant| -> bool {
        loop {
            if sh.control.is_stopped() {
                return false;
            }
            let now = Instant::now();
            if now >= due {
                return true;
            }
            std::thread::sleep((due - now).min(Duration::from_millis(20)));
        }
    };
    for offset in schedule {
        let due = sh.start + offset;
        if !sleep_until(due) {
            return;
        }
        let permit = match sem.clone().try_acquire_owned() {
            Ok(p) => Some(p),
            Err(_) if sh.allocated.load(Ordering::SeqCst) < max_vus => {
                sh.allocated.fetch_add(1, Ordering::SeqCst);
                sem.add_permits(1);
                sem.clone().try_acquire_owned().ok()
            }
            Err(_) => None,
        };
        match permit {
            Some(permit) => {
                let sh = sh.clone();
                handle.spawn(async move {
                    let _permit = permit;
                    let _busy = BusyGuard::new(&sh);
                    sh.guarded_iteration(Some(due)).await;
                });
            }
            None => {
                let at = sh.elapsed();
                sh.rec.lock().unwrap().dropped(at);
            }
        }
    }
    // Hold the run open until the planned end so rates are computed over the real duration.
    sleep_until(sh.start + planned);
}

async fn snapshot_loop(sh: Arc<Shared>, progress: Option<ProgressFn>, run_id: String, every: Duration, done: Arc<Notify>) {
    let planned = sh.plan.test.executor.planned_duration();
    let mut history: std::collections::VecDeque<(Duration, u64)> = std::collections::VecDeque::new();
    let mut tick = tokio::time::interval(every.max(Duration::from_millis(50)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = done.notified() => return,
        }
        let at = sh.elapsed();
        let busy = sh.busy.load(Ordering::SeqCst);
        let peak = sh.peak_busy.swap(busy, Ordering::SeqCst).max(busy);
        let snap = {
            let mut rec = sh.rec.lock().unwrap();
            rec.observe_vus(peak, at);
            if progress.is_none() {
                continue;
            }
            history.push_back((at, rec.totals.requests));
            while history.len() > 2 && at.saturating_sub(history[0].0) > Duration::from_millis(1100) {
                history.pop_front();
            }
            let (t0, n0) = history[0];
            let dt = at.saturating_sub(t0).as_secs_f64();
            let rps = if dt > 0.0 { (rec.totals.requests - n0) as f64 / dt } else { 0.0 };
            LoadSnapshot {
                run_id: run_id.clone(),
                phase: if sh.ending.load(Ordering::SeqCst) || sh.control.is_stopped() { RunPhase::Stopping } else { RunPhase::Running },
                elapsed_ms: at.as_millis() as u64,
                planned_duration_ms: planned.as_millis() as u64,
                vus: busy,
                vus_allocated: sh.allocated.load(Ordering::SeqCst),
                requests: rec.totals.requests,
                failed: rec.totals.failed,
                iterations: rec.totals.iterations,
                dropped_iterations: rec.totals.dropped_iterations,
                rps: (rps * 100.0).round() / 100.0,
                latency: rec.latency(),
                bytes_in: rec.totals.bytes_in,
                bytes_out: rec.totals.bytes_out,
                status_codes: rec.status_codes(),
                error_kinds: rec.errors,
                points: rec.take_new_points(),
                bucket_seconds: rec.bucket_seconds(),
            }
        };
        if let Some(p) = &progress {
            p(snap);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_exhaustion_messages() {
        assert!(looks_like_port_exhaustion("error trying to connect: tcp connect error: Only one usage of each socket address (protocol/network address/port) is normally permitted. (os error 10048)"));
        assert!(looks_like_port_exhaustion("tcp connect error: Cannot assign requested address (os error 99)"));
        assert!(!looks_like_port_exhaustion("tcp connect error: Connection refused (os error 111)"));
    }

    #[test]
    fn body_search() {
        assert!(contains(b"hello world", b"o w"));
        assert!(!contains(b"hello", b"world"));
        assert!(contains(b"x", b""));
    }
}
