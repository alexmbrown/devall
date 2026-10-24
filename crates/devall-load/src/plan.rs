//! Planning: turn a test file into everything the engine needs, before any load is sent.
//!
//! * Each scenario request is read with folder inheritance applied (`Project::effective_request`)
//!   and resolved once as a probe (reporting unresolved `{{vars}}` and bad URLs up front).
//! * Secrets are looked up once (the OS keychain is far too slow to hit per request) and cached;
//!   their values are kept for redaction.
//! * Requests whose templates cannot change between iterations are prepared once; the rest
//!   (anything using `{{$uuid}}` & co) are re-resolved for every iteration.
//! * [`Plan::preflight`] applies the guard rails: ephemeral-port churn, VU counts, and the
//!   confirmation required before pointing a high rate at a non-local host.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devall_core::secrets::SecretResolver;
use devall_core::{HttpRequestFile, Project, Resolver, SecretLookup, SecretStore};
use devall_http::{PreparedRequest, prepare};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{LoadError, Result};
use crate::model::{Executor, LoadTestFile, ThinkTime};
use crate::schedule::ArrivalSchedule;
use crate::validate::validate;

/// Arrival rate (per second) at or above which a non-local target needs confirmation.
pub const CONFIRM_RATE_PER_SEC: f64 = 100.0;
/// VU count at or above which a non-local target needs confirmation (VU executors).
pub const CONFIRM_VUS: u32 = 50;
/// Timeout used when neither the test nor the request sets one.
const FALLBACK_TIMEOUT: Duration = Duration::from_secs(60);
const SAFE_METHODS: &[&str] = &["GET", "HEAD", "OPTIONS"];

/// Secret values captured while probing, served from memory during the run.
#[derive(Default)]
pub(crate) struct CachedSecrets {
    values: HashMap<String, Option<String>>,
}

impl CachedSecrets {
    /// Values from a distributed plan (name → value).
    pub(crate) fn from_values(values: BTreeMap<String, String>) -> Self {
        Self { values: values.into_iter().map(|(k, v)| (k, Some(v))).collect() }
    }

    /// Every secret that resolved (name → value).
    pub(crate) fn resolved(&self) -> BTreeMap<String, String> {
        self.values.iter().filter_map(|(k, v)| v.clone().map(|v| (k.clone(), v))).collect()
    }
}

impl SecretLookup for CachedSecrets {
    fn lookup(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned().flatten()
    }
}

/// Records every lookup made through the real resolver so the run never touches the keychain.
struct Recording<'a> {
    inner: &'a dyn SecretLookup,
    seen: Mutex<HashMap<String, Option<String>>>,
}

impl SecretLookup for Recording<'_> {
    fn lookup(&self, name: &str) -> Option<String> {
        if let Some(v) = self.seen.lock().unwrap().get(name) {
            return v.clone();
        }
        let v = self.inner.lookup(name);
        self.seen.lock().unwrap().insert(name.to_string(), v.clone());
        v
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PlannedRequest {
    pub path: String,
    pub name: String,
    pub method: String,
    /// Folder inheritance applied.
    pub template: HttpRequestFile,
    /// Variable layers, highest precedence first: environment, folders (nearest first), project.
    pub layers: Vec<IndexMap<String, String>>,
    /// Prepared once when nothing in the request varies between iterations.
    pub fixed: Option<PreparedRequest>,
    /// The probe resolution (used for host checks and header inspection).
    pub probe: PreparedRequest,
    pub timeout: Duration,
    pub accept_invalid_certs: bool,
    /// The request references `{{secret.X}}` (directly or through variables).
    pub uses_secrets: bool,
}

pub struct Plan {
    pub test: LoadTestFile,
    /// Test file relative to `load/`.
    pub test_path: String,
    pub environment: Option<String>,
    pub project_root: PathBuf,
    pub(crate) requests: Vec<PlannedRequest>,
    /// Weighted mode: (request index, weight).
    pub(crate) weighted: Vec<(usize, u32)>,
    /// Flow mode: request indexes in order.
    pub(crate) flow: Vec<usize>,
    pub(crate) secrets: Arc<CachedSecrets>,
    /// Secret values used by the scenario, for redaction of anything persisted.
    pub secret_values: BTreeSet<String>,
    pub unresolved: Vec<String>,
    pub secret_errors: Vec<String>,
    pub(crate) env_protected: bool,
    /// Origin every request URL is rewritten to (see [`Plan::target_mock`]).
    pub(crate) base_override: Option<url::Url>,
    pub mock_baseline: Option<crate::summary::MockBaseline>,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan").field("test_path", &self.test_path).field("environment", &self.environment).finish_non_exhaustive()
    }
}

/// Environment precedence: an explicit override (CLI `--env`) beats the test's `environment`,
/// which beats the caller's fallback (the app's active environment).
pub fn effective_environment(test: &LoadTestFile, override_env: Option<&str>, fallback: Option<&str>) -> Option<String> {
    let clean = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    clean(override_env).or_else(|| clean(test.environment.as_deref())).or_else(|| clean(fallback))
}

/// Validates `test` and resolves its requests against `project`. `env` is the environment id to
/// use (see [`effective_environment`]).
pub fn build_plan(project: &Project, test_path: &str, test: &LoadTestFile, env: Option<&str>, store: &dyn SecretStore) -> Result<Plan> {
    let issues = validate(test);
    if !issues.is_empty() {
        return Err(LoadError::Validation(issues));
    }
    let env = env.map(str::trim).filter(|e| !e.is_empty());
    let env_file = env.map(|id| project.read_environment(id)).transpose()?;
    let resolver_secrets = SecretResolver::new(store, project.id(), env, project.root());
    let recording = Recording { inner: &resolver_secrets, seen: Mutex::new(HashMap::new()) };

    let paths = test.scenario.paths();
    let mut requests = Vec::with_capacity(paths.len());
    let mut unresolved = BTreeSet::new();
    let mut secret_values = BTreeSet::new();
    for path in &paths {
        let rel = devall_core::fsutil::normalize_rel(path);
        let req = project.read_request(&rel).map_err(|e| LoadError::Request { path: rel.clone(), message: e.to_string() })?;
        let eff = project.effective_request(&rel, &req)?;
        let mut layers = Vec::new();
        if let Some(e) = &env_file {
            layers.push(e.variables.clone());
        }
        layers.extend(eff.folder_variables.iter().cloned());
        layers.push(project.manifest.variables.clone());

        let mut r = Resolver::new(&recording);
        for l in &layers {
            r.push_layer(l.clone());
        }
        let probe = prepare(&eff.request, &mut r, project.root()).map_err(|e| LoadError::Request { path: rel.clone(), message: e.to_string() })?;
        for u in &r.unresolved {
            unresolved.insert(format!("{{{{{u}}}}} in {rel}"));
        }
        let uses_secrets = !r.secret_values.is_empty() || r.unresolved.iter().any(|u| u.starts_with("secret."));
        secret_values.extend(r.secret_values.iter().cloned());

        let text = serde_json::to_string(&eff.request).unwrap_or_default();
        let dynamic = text.contains("{{$") || (text.contains("{{") && layers.iter().any(|l| l.values().any(|v| v.contains("{{$"))));
        let timeout_ms = test.http.timeout_ms.unwrap_or(eff.request.settings.timeout_ms);
        let timeout = if timeout_ms == 0 { FALLBACK_TIMEOUT } else { Duration::from_millis(timeout_ms) };
        requests.push(PlannedRequest {
            path: rel,
            name: eff.request.name.clone(),
            method: probe.method.clone(),
            fixed: (!dynamic).then(|| probe.clone()),
            accept_invalid_certs: test.http.insecure || !probe.verify_tls,
            timeout,
            uses_secrets,
            probe,
            template: eff.request,
            layers,
        });
    }

    let index = |p: &str| paths.iter().position(|x| *x == p).expect("path collected above");
    let weighted = test.scenario.requests.iter().map(|w| (index(&w.path), w.weight)).collect();
    let flow = test.scenario.flow.iter().map(|p| index(p)).collect();
    let secret_errors = resolver_secrets.errors.lock().unwrap().clone();
    let cached = CachedSecrets { values: recording.seen.into_inner().unwrap() };
    Ok(Plan {
        test: test.clone(),
        test_path: devall_core::fsutil::normalize_rel(test_path),
        environment: env.map(str::to_string),
        project_root: project.root().to_path_buf(),
        requests,
        weighted,
        flow,
        secrets: Arc::new(cached),
        secret_values,
        unresolved: unresolved.into_iter().collect(),
        secret_errors,
        env_protected: env_file.is_some_and(|e| e.protected),
        base_override: None,
        mock_baseline: None,
    })
}

/// `https://api.example.com/v1/users?x=1` rebased on `http://127.0.0.1:8080` →
/// `http://127.0.0.1:8080/v1/users?x=1`. URLs that don't parse are returned unchanged.
pub fn rebase_url(url: &str, base: &url::Url) -> String {
    match url::Url::parse(url) {
        Ok(u) if u.has_host() => format!("{}{}", base.origin().ascii_serialization(), &u[url::Position::BeforePath..]),
        _ => url.to_string(),
    }
}

/// Ephemeral-port budget of an OS: ports in the dynamic range and how long a client-closed
/// connection keeps its port (TIME_WAIT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortBudget {
    pub os: &'static str,
    pub ports: u32,
    pub time_wait: Duration,
}

/// Windows defaults: 49152–65535 and a 4-minute TIME_WAIT. Exhaustion breaks every outbound
/// connection on the machine, so this is the budget the guard rails always check.
pub const WINDOWS_PORTS: PortBudget = PortBudget { os: "Windows", ports: 16_384, time_wait: Duration::from_secs(240) };

impl PortBudget {
    pub fn current() -> Self {
        if cfg!(windows) {
            WINDOWS_PORTS
        } else if cfg!(target_os = "macos") {
            PortBudget { os: "macOS", ports: 16_384, time_wait: Duration::from_secs(30) }
        } else {
            PortBudget { os: "Linux", ports: 28_232, time_wait: Duration::from_secs(60) }
        }
    }

    /// Sustained new connections per second before the range runs out.
    pub fn sustainable_rate(&self) -> f64 {
        self.ports as f64 / self.time_wait.as_secs_f64()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub reason: String,
    /// Non-local hosts the test targets.
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub environment: Option<String>,
    pub open_model: bool,
    pub planned_duration_ms: u64,
    /// Peak iterations per second (arrival-rate executors).
    pub peak_rate: Option<f64>,
    /// Iterations the profile asks for (arrival-rate executors).
    pub expected_iterations: Option<u64>,
    pub max_vus: u32,
    /// Every target host (`host:port`).
    pub hosts: Vec<String>,
    pub unresolved: Vec<String>,
    pub secret_errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Set when the run must be confirmed (non-local target at a high rate, or a protected
    /// environment receiving writes).
    pub requires_confirmation: Option<Confirmation>,
    /// Requests that use `{{secret.X}}`: distributing them sends resolved values to agents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_requests: Vec<String>,
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `localhost`, `*.localhost`, loopback and unspecified addresses.
pub fn is_local_host(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

impl Plan {
    /// Sends every request to `mock_url` (the project's mock server) instead of its own host:
    /// scheme, host and port are replaced, path and query kept. The summary is tagged as a mock
    /// baseline.
    pub fn target_mock(&mut self, mock_url: &str) -> Result<()> {
        let base = url::Url::parse(mock_url).map_err(|e| LoadError::Invalid(format!("mock server URL {mock_url}: {e}")))?;
        let mut replaced: Vec<String> = Vec::new();
        for r in &mut self.requests {
            if let Ok(u) = url::Url::parse(&r.probe.url)
                && let Some(h) = u.host_str()
            {
                let label = match u.port_or_known_default() {
                    Some(p) => format!("{h}:{p}"),
                    None => h.to_string(),
                };
                if !replaced.contains(&label) {
                    replaced.push(label);
                }
            }
            r.probe.url = rebase_url(&r.probe.url, &base);
            if let Some(f) = &mut r.fixed {
                f.url = rebase_url(&f.url, &base);
            }
        }
        self.mock_baseline = Some(crate::summary::MockBaseline { url: base.origin().ascii_serialization(), replaced_hosts: replaced });
        self.base_override = Some(base);
        Ok(())
    }

    pub fn is_open_model(&self) -> bool {
        self.test.executor.is_open()
    }

    /// (path, name, method) per request index.
    pub fn request_meta(&self) -> Vec<(String, String, String)> {
        self.requests.iter().map(|r| (r.path.clone(), r.name.clone(), r.method.clone())).collect()
    }

    pub fn request_paths(&self) -> Vec<String> {
        self.requests.iter().map(|r| r.path.clone()).collect()
    }

    /// Requests per iteration that send `Connection: close` (each one costs a fresh connection
    /// and leaves a port in TIME_WAIT).
    fn closing_requests_per_iteration(&self) -> f64 {
        let closes = |i: usize| self.requests[i].probe.header("connection").is_some_and(|v| v.to_ascii_lowercase().contains("close"));
        if !self.flow.is_empty() {
            return self.flow.iter().filter(|i| closes(**i)).count() as f64;
        }
        let total: u32 = self.weighted.iter().map(|(_, w)| *w).sum();
        if total == 0 {
            return 0.0;
        }
        self.weighted.iter().filter(|(i, _)| closes(*i)).map(|(_, w)| *w as f64).sum::<f64>() / total as f64
    }

    fn requests_per_iteration(&self) -> f64 {
        if self.flow.is_empty() { 1.0 } else { self.flow.len() as f64 }
    }

    pub fn preflight(&self) -> Preflight {
        let ex = &self.test.executor;
        let planned = ex.planned_duration();
        let peak_rate = ex.peak_rate_per_sec();
        let max_vus = ex.max_vus();
        let mut hosts: Vec<String> = Vec::new();
        let mut remote: Vec<String> = Vec::new();
        for r in &self.requests {
            if let Ok(u) = url::Url::parse(&r.probe.url) {
                let host = u.host_str().unwrap_or_default().to_string();
                let label = match u.port_or_known_default() {
                    Some(p) => format!("{host}:{p}"),
                    None => host.clone(),
                };
                if !hosts.contains(&label) {
                    hosts.push(label.clone());
                }
                if !is_local_host(&host) && !remote.contains(&label) {
                    remote.push(label);
                }
            }
        }

        let mut warnings = Vec::new();
        // ----- ephemeral ports -----
        let closing = self.closing_requests_per_iteration();
        if closing > 0.0 {
            let think = self.test.think_time.as_ref().map(ThinkTime::mean).unwrap_or_default();
            let (churn, how) = match peak_rate {
                Some(rate) => (rate * closing, "at the peak rate"),
                // Closed model: assume ~10 ms per request plus think time.
                None => {
                    let per_iter = (Duration::from_millis(10).mul_f64(self.requests_per_iteration()) + think).as_secs_f64();
                    (max_vus as f64 * closing / per_iter.max(1e-3), "if responses take ~10 ms")
                }
            };
            let mut budgets = vec![PortBudget::current()];
            if budgets[0] != WINDOWS_PORTS {
                budgets.push(WINDOWS_PORTS);
            }
            if let Some(b) = budgets.into_iter().find(|b| churn * planned.min(b.time_wait).as_secs_f64() > b.ports as f64 * 0.5) {
                let exhaust_after = Duration::from_secs_f64(b.ports as f64 / churn.max(1e-3));
                warnings.push(format!(
                    "Requests send `Connection: close`, so this test opens about {churn:.0} new connections/s {how}. {} has {} ephemeral ports and keeps each closed one in TIME_WAIT for {} (≈{:.0} new connections/s sustained); the ports would run out after ~{}, breaking all outbound networking on the machine. Remove the header or lower the load.",
                    b.os,
                    thousands(b.ports as u64),
                    crate::duration::format_duration(b.time_wait),
                    b.sustainable_rate(),
                    crate::duration::format_duration(Duration::from_secs(exhaust_after.as_secs())),
                ));
            }
        }
        let concurrent = self.test.http.max_connections.map_or(max_vus, |m| m.min(max_vus));
        if !self.test.http.http2 && concurrent as f64 > WINDOWS_PORTS.ports as f64 * 0.25 {
            warnings.push(format!(
                "Up to {concurrent} HTTP/1.1 connections may be open at once, a large share of the {} ephemeral ports on Windows. Consider `http.maxConnections` or `http.http2`.",
                thousands(WINDOWS_PORTS.ports as u64)
            ));
        }
        // ----- configuration hints -----
        if ex.is_open() && self.flow.is_empty() && self.test.think_time.is_some() {
            warnings.push("thinkTime has no effect here: arrival-rate tests start iterations on schedule, and a weighted mix runs one request per iteration.".into());
        }
        if let Some(tt) = &self.test.think_time
            && !planned.is_zero()
            && tt.mean() >= planned
        {
            warnings.push("thinkTime is as long as the whole test; most VUs will only run once.".into());
        }
        if let Executor::ConstantArrivalRate { pre_allocated_vus, max_vus: None, .. } | Executor::RampingArrivalRate { pre_allocated_vus, max_vus: None, .. } = ex
            && let Some(rate) = peak_rate
            && rate > *pre_allocated_vus as f64 * 20.0
        {
            warnings.push(format!(
                "{pre_allocated_vus} VUs can only sustain {rate:.0} iterations/s if each takes under {:.0} ms; set maxVus so slow responses don't turn into dropped iterations.",
                *pre_allocated_vus as f64 * 1000.0 / rate
            ));
        }
        for u in &self.unresolved {
            warnings.push(format!("Unresolved variable {u}"));
        }
        if self.mock_baseline.is_some() && self.test.http.http2 {
            warnings.push("The mock server speaks HTTP/1.1 only; turn off http.http2 for a mock baseline.".into());
        }

        // ----- confirmation -----
        let mut reasons = Vec::new();
        if !remote.is_empty() {
            let heavy = match peak_rate {
                Some(rate) => rate >= CONFIRM_RATE_PER_SEC,
                None => max_vus >= CONFIRM_VUS,
            };
            if heavy {
                let load = match peak_rate {
                    Some(r) => format!("up to {r:.0} iterations/s"),
                    None => format!("{max_vus} concurrent VUs"),
                };
                reasons.push(format!("This test sends {load} to {}, which is not on this machine. Only load-test systems you own or are allowed to test.", remote.join(", ")));
            }
        }
        if self.env_protected {
            let writes: Vec<&str> = self.requests.iter().map(|r| r.method.as_str()).filter(|m| !SAFE_METHODS.contains(m)).collect();
            if !writes.is_empty() {
                reasons.push(format!(
                    "Environment \"{}\" is protected and this test sends {} requests.",
                    self.environment.as_deref().unwrap_or_default(),
                    writes.join("/")
                ));
            }
        }
        let requires_confirmation = (!reasons.is_empty()).then(|| Confirmation { reason: reasons.join(" "), hosts: remote.clone() });

        Preflight {
            environment: self.environment.clone(),
            open_model: ex.is_open(),
            planned_duration_ms: planned.as_millis() as u64,
            peak_rate: peak_rate.map(|r| (r * 100.0).round() / 100.0),
            expected_iterations: ArrivalSchedule::for_executor(ex).map(|s| s.expected_iterations().ceil() as u64),
            max_vus,
            hosts,
            unresolved: self.unresolved.clone(),
            secret_errors: self.secret_errors.clone(),
            warnings,
            requires_confirmation,
            secret_requests: self.secret_request_paths(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hosts() {
        for h in ["localhost", "api.localhost", "127.0.0.1", "127.1.2.3", "[::1]", "::1", "0.0.0.0"] {
            assert!(is_local_host(h), "{h}");
        }
        for h in ["example.com", "10.0.0.5", "192.168.1.2", "localhost.example.com"] {
            assert!(!is_local_host(h), "{h}");
        }
    }

    #[test]
    fn environment_precedence() {
        let mut t = LoadTestFile::template("x", None);
        assert_eq!(effective_environment(&t, None, Some("local")).as_deref(), Some("local"));
        t.environment = Some("staging".into());
        assert_eq!(effective_environment(&t, None, Some("local")).as_deref(), Some("staging"));
        assert_eq!(effective_environment(&t, Some("prod"), Some("local")).as_deref(), Some("prod"));
        assert_eq!(effective_environment(&t, Some(" "), None).as_deref(), Some("staging"));
    }

    #[test]
    fn windows_budget() {
        assert!((WINDOWS_PORTS.sustainable_rate() - 68.27).abs() < 0.01);
        assert_eq!(thousands(16_384), "16,384");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }
}
