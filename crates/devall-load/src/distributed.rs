//! Distributed load: one coordinator splits a test across `devall load agent` workers on the
//! user's own machines (Docker, Kubernetes, spare laptops) and merges their results.
//!
//! * **Split.** The arrival rate (or the VUs) is divided by agent weight; each agent runs its
//!   share with the normal engine ([`crate::engine`]).
//! * **Protocol.** HTTP/1.1 + JSON, bearer-token authenticated (`DEVALL_AGENT_TOKEN`), TLS
//!   optional (rustls on the agent, or a TLS terminator in front). `POST /v1/run` takes an
//!   [`AgentPlan`] and answers with an NDJSON stream of [`AgentEvent`]s: periodic snapshots, then
//!   the agent's summary plus its raw measurements — HDR histograms in the V2 format.
//! * **Merge.** Histograms are added (exact percentiles over every sample), counters summed;
//!   thresholds are evaluated on the merged result; the summary records which agents ran.
//! * **Secrets.** A plan carries resolved values only for the requests it needs, and only when
//!   the user explicitly allowed sending resolved secrets to agents
//!   (`--allow-secrets-to-agents`); otherwise a test that uses `{{secret.X}}` is refused.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use devall_core::HttpRequestFile;
use devall_http::{PreparedBody, PreparedPart, PreparedRequest};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::engine::{ProgressFn, RunControl, RunOptions};
use crate::error::{LoadError, Result};
use crate::metrics::{Recorder, RecorderExport, RecorderMetrics, merge_points};
use crate::model::{Executor, LoadTarget, LoadTestFile, Stage};
use crate::plan::{CachedSecrets, Plan, PlannedRequest};
use crate::summary::{AgentRun, ErrorKinds, LoadSnapshot, LoadSummary, Machine, Percentiles, RunPhase, SUMMARY_VERSION, TimelinePoint};
use crate::threshold::evaluate_all;

/// Bumped on incompatible wire changes; agents refuse other versions.
pub const AGENT_PROTOCOL: u32 = 1;
pub const DEFAULT_AGENT_PORT: u16 = 7777;
/// Environment variable holding the shared agent token (agent and coordinator).
pub const TOKEN_ENV: &str = "DEVALL_AGENT_TOKEN";
pub const MIN_TOKEN_LEN: usize = 16;

// ---------- wire format ----------

/// `GET /v1/info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub version: String,
    pub protocol: u32,
    pub machine: Machine,
    pub busy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePart {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WireBody {
    None,
    Bytes { base64: String },
    Form { pairs: Vec<(String, String)> },
    Multipart { parts: Vec<WirePart> },
}

/// A resolved request (no client certificates or files: agents only get bytes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePrepared {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: WireBody,
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub verify_tls: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireRequest {
    pub path: String,
    pub name: String,
    pub method: String,
    /// Folder inheritance applied; re-resolved per iteration when `fixed` is absent.
    pub template: HttpRequestFile,
    pub layers: Vec<IndexMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<WirePrepared>,
    pub probe: WirePrepared,
    pub timeout_ms: u64,
    pub accept_invalid_certs: bool,
}

/// `POST /v1/run`: one agent's share of a test. No `Debug`: it may hold secret values.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlan {
    pub protocol: u32,
    pub run_id: String,
    pub agent_index: usize,
    pub agent_count: usize,
    /// This agent's fraction of the load.
    pub share: f64,
    /// The test with this agent's executor and no thresholds (the coordinator evaluates them).
    pub test: LoadTestFile,
    pub test_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    pub requests: Vec<WireRequest>,
    pub weighted: Vec<(usize, u32)>,
    pub flow: Vec<usize>,
    /// Resolved `{{secret.X}}` values by name — only those the requests use, and only when the
    /// user allowed sending them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, String>,
    /// Values masked in anything the agent reports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_values: Vec<String>,
    /// Wait this long after receiving the plan, so agents start together.
    pub start_delay_ms: u64,
    pub snapshot_interval_ms: u64,
}

impl std::fmt::Debug for AgentPlan {
    /// Never prints secret values (only their names).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPlan")
            .field("run_id", &self.run_id)
            .field("agent", &format_args!("{}/{}", self.agent_index + 1, self.agent_count))
            .field("share", &self.share)
            .field("executor", &self.test.executor)
            .field("requests", &self.requests.iter().map(|r| r.path.as_str()).collect::<Vec<_>>())
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// One line of the `POST /v1/run` NDJSON stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[allow(clippy::large_enum_variant)]
pub enum AgentEvent {
    Started { run_id: String, info: AgentInfo },
    Snapshot { snapshot: LoadSnapshot },
    /// The agent's own summary (secrets masked) and its raw measurements for merging.
    Finished { summary: LoadSummary, measurements: RecorderExport },
    Error { message: String },
}

// ---------- prepared request conversion ----------

fn to_wire(p: &PreparedRequest, path: &str) -> Result<WirePrepared> {
    if p.client_cert.is_some() {
        return Err(LoadError::Invalid(format!("{path} uses a client certificate (a file on this machine), so it can't run on agents")));
    }
    let body = match &p.body {
        PreparedBody::None => WireBody::None,
        PreparedBody::Bytes(b) => WireBody::Bytes { base64: base64::engine::general_purpose::STANDARD.encode(b) },
        PreparedBody::Form(pairs) => WireBody::Form { pairs: pairs.clone() },
        PreparedBody::Multipart(parts) => {
            if let Some(f) = parts.iter().find_map(|p| p.file.as_ref()) {
                return Err(LoadError::Invalid(format!("{path} uploads {} (a file on this machine), so it can't run on agents", f.display())));
            }
            WireBody::Multipart { parts: parts.iter().map(|p| WirePart { name: p.name.clone(), value: p.value.clone(), content_type: p.content_type.clone() }).collect() }
        }
    };
    Ok(WirePrepared { method: p.method.clone(), url: p.url.clone(), headers: p.headers.clone(), body, timeout_ms: p.timeout_ms, follow_redirects: p.follow_redirects, verify_tls: p.verify_tls })
}

fn from_wire(w: &WirePrepared) -> Result<PreparedRequest> {
    let body = match &w.body {
        WireBody::None => PreparedBody::None,
        WireBody::Bytes { base64 } => PreparedBody::Bytes(base64::engine::general_purpose::STANDARD.decode(base64).map_err(|e| LoadError::Invalid(format!("request body: {e}")))?),
        WireBody::Form { pairs } => PreparedBody::Form(pairs.clone()),
        WireBody::Multipart { parts } => PreparedBody::Multipart(parts.iter().map(|p| PreparedPart { name: p.name.clone(), value: p.value.clone(), file: None, content_type: p.content_type.clone() }).collect()),
    };
    Ok(PreparedRequest {
        method: w.method.clone(),
        url: w.url.clone(),
        headers: w.headers.clone(),
        body,
        timeout_ms: w.timeout_ms,
        follow_redirects: w.follow_redirects,
        verify_tls: w.verify_tls,
        proxy: None,
        client_cert: None,
        digest: None,
        ntlm: None,
    })
}

// ---------- splitting ----------

/// Integer shares of `total` proportional to `weights` that sum to `total` (largest remainder).
pub fn largest_remainder(total: u32, weights: &[u32]) -> Vec<u32> {
    let sum: u64 = weights.iter().map(|w| u64::from(*w)).sum();
    if sum == 0 {
        return vec![0; weights.len()];
    }
    let exact: Vec<f64> = weights.iter().map(|w| total as f64 * f64::from(*w) / sum as f64).collect();
    let mut out: Vec<u32> = exact.iter().map(|x| x.floor() as u32).collect();
    let mut rest = total - out.iter().sum::<u32>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|a, b| (exact[*b] - exact[*b].floor()).total_cmp(&(exact[*a] - exact[*a].floor())).then(a.cmp(b)));
    for i in order {
        if rest == 0 {
            break;
        }
        out[i] += 1;
        rest -= 1;
    }
    out
}

/// Each agent's executor: arrival rates scaled by its share, VUs split so they add up exactly.
pub fn split_executor(ex: &Executor, weights: &[u32]) -> Vec<Executor> {
    let sum: f64 = weights.iter().map(|w| f64::from(*w)).sum::<f64>().max(1.0);
    let shares: Vec<f64> = weights.iter().map(|w| f64::from(*w) / sum).collect();
    let ceil = |n: u32, s: f64| ((f64::from(n) * s).ceil() as u32).max(1);
    let stages = |st: &[Stage], s: f64| st.iter().map(|x| Stage { duration: x.duration.clone(), target: x.target * s }).collect::<Vec<_>>();
    match ex {
        Executor::ConstantVus { vus, duration } => largest_remainder(*vus, weights).into_iter().map(|v| Executor::ConstantVus { vus: v, duration: duration.clone() }).collect(),
        Executor::RampingVus { start_vus, stages: st } => {
            let starts = largest_remainder(*start_vus, weights);
            shares.iter().zip(starts).map(|(s, sv)| Executor::RampingVus { start_vus: sv, stages: stages(st, *s) }).collect()
        }
        Executor::ConstantArrivalRate { rate, time_unit, duration, pre_allocated_vus, max_vus } => shares
            .iter()
            .map(|s| Executor::ConstantArrivalRate {
                rate: rate * s,
                time_unit: time_unit.clone(),
                duration: duration.clone(),
                pre_allocated_vus: ceil(*pre_allocated_vus, *s),
                max_vus: max_vus.map(|m| ceil(m, *s)),
            })
            .collect(),
        Executor::RampingArrivalRate { start_rate, time_unit, stages: st, pre_allocated_vus, max_vus } => shares
            .iter()
            .map(|s| Executor::RampingArrivalRate {
                start_rate: start_rate * s,
                time_unit: time_unit.clone(),
                stages: stages(st, *s),
                pre_allocated_vus: ceil(*pre_allocated_vus, *s),
                max_vus: max_vus.map(|m| ceil(m, *s)),
            })
            .collect(),
    }
}

impl Plan {
    /// Requests that reference `{{secret.X}}`.
    pub fn secret_request_paths(&self) -> Vec<String> {
        self.requests.iter().filter(|r| r.uses_secrets).map(|r| r.path.clone()).collect()
    }

    /// One plan per agent. Refused when a request uses secrets and `allow_secrets` is false, when
    /// a request needs files from this machine, or for mock baselines (the mock runs here).
    pub fn agent_plans(&self, weights: &[u32], allow_secrets: bool, run_id: &str, start_delay: Duration, snapshot_interval: Duration) -> Result<Vec<AgentPlan>> {
        if weights.is_empty() {
            return Err(LoadError::Invalid("no agents".into()));
        }
        if self.base_override.is_some() || self.test.target == LoadTarget::Mock {
            return Err(LoadError::Invalid("mock baselines run against this machine's mock server; run them locally, not on agents".into()));
        }
        let with_secrets = self.secret_request_paths();
        if !with_secrets.is_empty() && !allow_secrets {
            return Err(LoadError::SecretsToAgents(format!(
                "{} use{} secrets ({}). Agents would receive the resolved values. Re-run with --allow-secrets-to-agents (in the app: tick “Send resolved secrets to agents”) only if you trust every agent and the network path to it.",
                with_secrets.join(", "),
                if with_secrets.len() == 1 { "s" } else { "" },
                "{{secret.…}}"
            )));
        }
        let mut requests = Vec::with_capacity(self.requests.len());
        for r in &self.requests {
            requests.push(wire_request(r)?);
        }
        let (secrets, secret_values) = if with_secrets.is_empty() {
            (BTreeMap::new(), Vec::new())
        } else {
            (self.secrets.resolved(), self.secret_values.iter().cloned().collect())
        };
        let executors = split_executor(&self.test.executor, weights);
        if let Some(i) = executors.iter().position(|e| e.max_vus() == 0) {
            return Err(LoadError::Invalid(format!("agent {} would get no VUs: {} VUs can't be split across {} agents — use fewer agents or more VUs", i + 1, self.test.executor.max_vus(), weights.len())));
        }
        let total: f64 = weights.iter().map(|w| f64::from(*w)).sum::<f64>().max(1.0);
        Ok(executors
            .into_iter()
            .enumerate()
            .map(|(i, executor)| {
                let mut test = self.test.clone();
                test.executor = executor;
                test.thresholds.clear();
                AgentPlan {
                    protocol: AGENT_PROTOCOL,
                    run_id: run_id.to_string(),
                    agent_index: i,
                    agent_count: weights.len(),
                    share: f64::from(weights[i]) / total,
                    test,
                    test_path: self.test_path.clone(),
                    environment: self.environment.clone(),
                    requests: requests.clone(),
                    weighted: self.weighted.clone(),
                    flow: self.flow.clone(),
                    secrets: secrets.clone(),
                    secret_values: secret_values.clone(),
                    start_delay_ms: start_delay.as_millis() as u64,
                    snapshot_interval_ms: snapshot_interval.as_millis() as u64,
                }
            })
            .collect())
    }

    /// The plan an agent runs. `scratch` stands in for the project root (agents have no project).
    pub fn from_agent_plan(ap: AgentPlan, scratch: &Path) -> Result<Plan> {
        if ap.protocol != AGENT_PROTOCOL {
            return Err(LoadError::Invalid(format!("this agent speaks protocol {AGENT_PROTOCOL}, the coordinator sent {} — use the same devall version on both", ap.protocol)));
        }
        let n = ap.requests.len();
        if ap.weighted.iter().any(|(i, _)| *i >= n) || ap.flow.iter().any(|i| *i >= n) {
            return Err(LoadError::Invalid("the plan references a request it doesn't contain".into()));
        }
        let mut requests = Vec::with_capacity(n);
        for r in ap.requests {
            requests.push(PlannedRequest {
                fixed: r.fixed.as_ref().map(from_wire).transpose()?,
                probe: from_wire(&r.probe)?,
                path: r.path,
                name: r.name,
                method: r.method,
                template: r.template,
                layers: r.layers,
                timeout: Duration::from_millis(r.timeout_ms.max(1)),
                accept_invalid_certs: r.accept_invalid_certs,
                uses_secrets: false,
            });
        }
        let mut secret_values: BTreeSet<String> = ap.secret_values.into_iter().collect();
        secret_values.extend(ap.secrets.values().cloned());
        Ok(Plan {
            test: ap.test,
            test_path: ap.test_path,
            environment: ap.environment,
            project_root: scratch.to_path_buf(),
            requests,
            weighted: ap.weighted,
            flow: ap.flow,
            secrets: Arc::new(CachedSecrets::from_values(ap.secrets)),
            secret_values,
            unresolved: Vec::new(),
            secret_errors: Vec::new(),
            env_protected: false,
            base_override: None,
            mock_baseline: None,
        })
    }
}

fn wire_request(r: &PlannedRequest) -> Result<WireRequest> {
    if r.fixed.is_none() {
        // Re-resolved on the agent every iteration: it can't read files from this machine.
        match &r.template.body {
            devall_core::Body::Binary { .. } => return Err(LoadError::Invalid(format!("{} sends a file body and uses dynamic values ({{{{$…}}}}), so agents can't rebuild it", r.path))),
            devall_core::Body::Multipart { parts } if parts.iter().any(|p| p.enabled && p.file.is_some()) => {
                return Err(LoadError::Invalid(format!("{} uploads a file and uses dynamic values, so agents can't rebuild it", r.path)));
            }
            _ => {}
        }
    }
    Ok(WireRequest {
        path: r.path.clone(),
        name: r.name.clone(),
        method: r.method.clone(),
        template: r.template.clone(),
        layers: r.layers.clone(),
        fixed: r.fixed.as_ref().map(|p| to_wire(p, &r.path)).transpose()?,
        probe: to_wire(&r.probe, &r.path)?,
        timeout_ms: r.timeout.as_millis() as u64,
        accept_invalid_certs: r.accept_invalid_certs,
    })
}

// ---------- coordinator ----------

/// An agent to use, with its relative share of the load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTarget {
    /// `host:port`, `host` (port 7777) or a full `http(s)://…` URL.
    pub address: String,
    #[serde(default = "one")]
    pub weight: u32,
}

fn one() -> u32 {
    1
}

/// `h1:7777,h2:7777` plus optional weights `2,1`.
pub fn parse_agents(list: &str, weights: Option<&str>) -> Result<Vec<AgentTarget>> {
    let addrs: Vec<String> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    if addrs.is_empty() {
        return Err(LoadError::Invalid("--agents needs at least one host:port".into()));
    }
    let ws: Vec<u32> = match weights {
        Some(w) => w.split(',').map(|x| x.trim().parse::<u32>().map_err(|_| LoadError::Invalid(format!("agent weight `{x}` is not a whole number")))).collect::<Result<_>>()?,
        None => vec![1; addrs.len()],
    };
    if ws.len() != addrs.len() {
        return Err(LoadError::Invalid(format!("{} agents but {} weights", addrs.len(), ws.len())));
    }
    if ws.contains(&0) {
        return Err(LoadError::Invalid("agent weights must be at least 1".into()));
    }
    Ok(addrs.into_iter().zip(ws).map(|(address, weight)| AgentTarget { address, weight }).collect())
}

/// Expands `dns:<name>[:port]` entries (a Kubernetes headless Service: one address per pod).
pub async fn resolve_agents(agents: Vec<AgentTarget>) -> Result<Vec<AgentTarget>> {
    let mut out = Vec::new();
    for a in agents {
        let Some(rest) = a.address.strip_prefix("dns:") else {
            out.push(a);
            continue;
        };
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse::<u16>().unwrap_or(DEFAULT_AGENT_PORT)),
            _ => (rest.to_string(), DEFAULT_AGENT_PORT),
        };
        let mut ips: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await.map_err(|e| LoadError::Agent { address: a.address.clone(), message: format!("DNS lookup failed: {e}") })?.collect();
        ips.sort();
        ips.dedup();
        if ips.is_empty() {
            return Err(LoadError::Agent { address: a.address.clone(), message: "no addresses".into() });
        }
        out.extend(ips.into_iter().map(|ip| AgentTarget { address: ip.to_string(), weight: a.weight }));
    }
    Ok(out)
}

/// Base URL for an agent address.
pub fn agent_url(address: &str) -> String {
    let a = address.trim().trim_end_matches('/');
    if a.starts_with("http://") || a.starts_with("https://") {
        return a.to_string();
    }
    let has_port = match a.rsplit_once(':') {
        Some((h, p)) => p.parse::<u16>().is_ok() && (!h.contains(':') || h.ends_with(']')),
        None => false,
    };
    if has_port { format!("http://{a}") } else { format!("http://{a}:{DEFAULT_AGENT_PORT}") }
}

pub struct DistributedOptions {
    pub agents: Vec<AgentTarget>,
    pub token: String,
    /// The user explicitly allowed sending resolved secrets to agents.
    pub allow_secrets: bool,
    /// PEM CA for `https://` agents with a private certificate.
    pub ca_cert: Option<PathBuf>,
    pub run_id: String,
    /// How long agents wait before starting, so they start together.
    pub start_delay: Duration,
    pub snapshot_interval: Duration,
}

impl Default for DistributedOptions {
    fn default() -> Self {
        Self { agents: Vec::new(), token: String::new(), allow_secrets: false, ca_cert: None, run_id: String::new(), start_delay: Duration::from_millis(500), snapshot_interval: Duration::from_millis(500) }
    }
}

/// HTTP client for talking to agents (rustls; optional private CA).
pub fn agent_client(ca_cert: Option<&Path>) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).tcp_nodelay(true);
    if let Some(ca) = ca_cert {
        let pem = std::fs::read(ca).map_err(|e| LoadError::Invalid(format!("agent CA {}: {e}", ca.display())))?;
        for cert in reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| LoadError::Invalid(format!("agent CA {}: {e}", ca.display())))? {
            b = b.add_root_certificate(cert);
        }
    }
    b.build().map_err(|e| LoadError::Engine(e.to_string()))
}

fn agent_err(address: &str, message: impl Into<String>) -> LoadError {
    LoadError::Agent { address: address.to_string(), message: message.into() }
}

async fn http_error(address: &str, resp: reqwest::Response) -> LoadError {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| body.chars().take(300).collect());
    match status.as_u16() {
        401 => agent_err(address, "the agent rejected the token (check DEVALL_AGENT_TOKEN on both sides)"),
        409 => agent_err(address, format!("busy: {detail}")),
        _ => agent_err(address, format!("HTTP {status}: {detail}")),
    }
}

/// `GET /v1/info` of one agent.
pub async fn agent_info(client: &reqwest::Client, address: &str, token: &str) -> Result<AgentInfo> {
    let url = format!("{}/v1/info", agent_url(address));
    let resp = client.get(&url).bearer_auth(token).timeout(Duration::from_secs(10)).send().await.map_err(|e| agent_err(address, format!("unreachable ({})", devall_http::error_chain(&e))))?;
    if !resp.status().is_success() {
        return Err(http_error(address, resp).await);
    }
    let body = resp.bytes().await.map_err(|e| agent_err(address, format!("connection lost ({})", devall_http::error_chain(&e))))?;
    serde_json::from_slice::<AgentInfo>(&body).map_err(|e| agent_err(address, format!("not a devall load agent ({e})")))
}

enum Incoming {
    Event(usize, Box<AgentEvent>),
    Failed(usize, String),
    Done(usize),
}

#[derive(Default)]
struct AgentProgress {
    snapshot: Option<LoadSnapshot>,
    points: Vec<TimelinePoint>,
    finished: Option<(LoadSummary, RecorderExport)>,
    error: Option<String>,
    info: Option<AgentInfo>,
    done: bool,
}

/// Merges live snapshots: counts add up, latency shows the worst agent (exact numbers come at
/// the end, from the merged histograms).
fn merged_snapshot(run_id: &str, planned_ms: u64, parts: &[AgentProgress], points: Vec<TimelinePoint>, phase: RunPhase) -> LoadSnapshot {
    let snaps: Vec<&LoadSnapshot> = parts.iter().filter_map(|p| p.snapshot.as_ref()).collect();
    let sum = |f: &dyn Fn(&LoadSnapshot) -> u64| snaps.iter().map(|s| f(s)).sum::<u64>();
    let max_f = |f: &dyn Fn(&Percentiles) -> f64| snaps.iter().map(|s| f(&s.latency)).fold(0.0, f64::max);
    let requests = sum(&|s| s.requests);
    let mut codes: BTreeMap<String, u64> = BTreeMap::new();
    let mut errors = ErrorKinds::default();
    for s in &snaps {
        for (k, v) in &s.status_codes {
            *codes.entry(k.clone()).or_default() += v;
        }
        errors.connect += s.error_kinds.connect;
        errors.timeout += s.error_kinds.timeout;
        errors.other += s.error_kinds.other;
    }
    let mean = if requests > 0 { snaps.iter().map(|s| s.latency.mean * s.requests as f64).sum::<f64>() / requests as f64 } else { 0.0 };
    let min = snaps.iter().filter(|s| s.requests > 0).map(|s| s.latency.min).reduce(f64::min).unwrap_or(0.0);
    LoadSnapshot {
        run_id: run_id.to_string(),
        phase,
        elapsed_ms: snaps.iter().map(|s| s.elapsed_ms).max().unwrap_or(0),
        planned_duration_ms: planned_ms,
        vus: snaps.iter().map(|s| s.vus).sum(),
        vus_allocated: snaps.iter().map(|s| s.vus_allocated).sum(),
        requests,
        failed: sum(&|s| s.failed),
        iterations: sum(&|s| s.iterations),
        dropped_iterations: sum(&|s| s.dropped_iterations),
        rps: (snaps.iter().map(|s| s.rps).sum::<f64>() * 100.0).round() / 100.0,
        latency: Percentiles {
            p50: max_f(&|p| p.p50),
            p75: max_f(&|p| p.p75),
            p90: max_f(&|p| p.p90),
            p95: max_f(&|p| p.p95),
            p99: max_f(&|p| p.p99),
            p999: max_f(&|p| p.p999),
            min,
            max: max_f(&|p| p.max),
            mean: (mean * 100.0).round() / 100.0,
        },
        bytes_in: sum(&|s| s.bytes_in),
        bytes_out: sum(&|s| s.bytes_out),
        status_codes: codes,
        error_kinds: errors,
        points,
        bucket_seconds: snaps.iter().map(|s| s.bucket_seconds).max().unwrap_or(1),
    }
}

/// Timeline buckets every healthy agent has closed (each agent reports a bucket once it ends),
/// merged and not yet emitted; with `all`, whatever is left.
fn ready_points(parts: &[AgentProgress], emitted: &mut Option<u32>, all: bool) -> Vec<TimelinePoint> {
    let live: Vec<&AgentProgress> = parts.iter().filter(|p| p.error.is_none()).collect();
    if live.is_empty() {
        return Vec::new();
    }
    let upto = if all { Some(u32::MAX) } else { live.iter().map(|p| p.points.last().map(|x| x.t)).min().flatten() };
    let Some(upto) = upto else { return Vec::new() };
    let lists: Vec<&[TimelinePoint]> = live.iter().map(|p| p.points.as_slice()).collect();
    let out: Vec<TimelinePoint> = merge_points(&lists).into_iter().filter(|p| emitted.is_none_or(|e| p.t > e) && p.t <= upto).collect();
    if let Some(last) = out.last() {
        *emitted = Some(last.t);
    }
    out
}

/// Runs `plan` on the agents and merges the results into one summary (thresholds evaluated on
/// the merged histograms; `summary.agents` lists who ran what).
pub async fn run_distributed(plan: Plan, opts: DistributedOptions, progress: Option<ProgressFn>, control: RunControl) -> Result<LoadSummary> {
    if opts.token.trim().is_empty() {
        return Err(LoadError::Invalid(format!("no agent token: set {TOKEN_ENV} (the same value the agents run with)")));
    }
    let agents = resolve_agents(opts.agents.clone()).await?;
    if agents.is_empty() {
        return Err(LoadError::Invalid("no agents".into()));
    }
    let run_id = if opts.run_id.is_empty() { ulid::Ulid::generate().to_string() } else { opts.run_id.clone() };
    let client = agent_client(opts.ca_cert.as_deref())?;
    let token = opts.token.trim().to_string();

    // Every agent must answer, be idle and speak our protocol before any load is sent.
    let mut agent_infos = Vec::new();
    for a in &agents {
        let info = agent_info(&client, &a.address, &token).await?;
        if info.protocol != AGENT_PROTOCOL {
            return Err(agent_err(&a.address, format!("protocol {} (this coordinator speaks {AGENT_PROTOCOL}); use the same devall version", info.protocol)));
        }
        if info.busy {
            return Err(agent_err(&a.address, format!("busy with run {}", info.run_id.unwrap_or_default())));
        }
        agent_infos.push(info);
    }

    let weights: Vec<u32> = agents.iter().map(|a| a.weight.max(1)).collect();
    let plans = plan.agent_plans(&weights, opts.allow_secrets, &run_id, opts.start_delay, opts.snapshot_interval)?;
    let preflight = plan.preflight();
    let planned_ms = plan.test.executor.planned_duration().as_millis() as u64;
    let started_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let t0 = Instant::now();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Incoming>();
    for (i, (a, ap)) in agents.iter().zip(plans).enumerate() {
        let (client, token, tx, address) = (client.clone(), token.clone(), tx.clone(), a.address.clone());
        tokio::spawn(async move {
            match stream_agent(&client, &address, &token, &ap, i, &tx).await {
                Ok(()) => {
                    let _ = tx.send(Incoming::Done(i));
                }
                Err(e) => {
                    let _ = tx.send(Incoming::Failed(i, e));
                }
            }
        });
    }
    drop(tx);

    let mut parts: Vec<AgentProgress> = agents.iter().map(|_| AgentProgress::default()).collect();
    for (p, info) in parts.iter_mut().zip(&agent_infos) {
        p.info = Some(info.clone());
    }
    let mut emitted: Option<u32> = None;
    let mut stop_sent = false;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    loop {
        let next = tokio::select! {
            m = rx.recv() => m,
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if control.is_stopped() && !stop_sent {
                    stop_sent = true;
                    send_stop(&client, &agents, &token).await;
                }
                continue;
            }
        };
        let Some(msg) = next else { break };
        let msg = match msg {
            Incoming::Event(i, ev) => Ok((i, *ev)),
            other => Err(other),
        };
        match msg {
            Ok((i, AgentEvent::Snapshot { snapshot })) => {
                parts[i].points.extend(snapshot.points.iter().copied());
                parts[i].snapshot = Some(snapshot);
                if let Some(p) = &progress
                    && last_emit.elapsed() >= Duration::from_millis(200)
                {
                    last_emit = Instant::now();
                    let pts = ready_points(&parts, &mut emitted, false);
                    let phase = if control.is_stopped() { RunPhase::Stopping } else { RunPhase::Running };
                    p(merged_snapshot(&run_id, planned_ms, &parts, pts, phase));
                }
            }
            Ok((i, AgentEvent::Started { info, .. })) => parts[i].info = Some(info),
            Ok((i, AgentEvent::Finished { summary, measurements })) => parts[i].finished = Some((summary, measurements)),
            Ok((i, AgentEvent::Error { message })) => parts[i].error = Some(message),
            Err(Incoming::Failed(i, e)) => {
                parts[i].error.get_or_insert(e);
                parts[i].done = true;
            }
            Err(Incoming::Done(i)) => {
                parts[i].done = true;
                if parts[i].finished.is_none() && parts[i].error.is_none() {
                    parts[i].error = Some("the agent closed the stream without results".into());
                }
            }
            Err(Incoming::Event(..)) => {}
        }
        // One agent failing mid-run makes the others' numbers a partial picture: stop them.
        if parts.iter().any(|p| p.error.is_some()) && !stop_sent && parts.iter().any(|p| !p.done) {
            stop_sent = true;
            control.stop_with("an agent failed");
            send_stop(&client, &agents, &token).await;
        }
    }

    // ---------- merge ----------
    let finished: Vec<&(LoadSummary, RecorderExport)> = parts.iter().filter_map(|p| p.finished.as_ref()).collect();
    if finished.is_empty() {
        let why: Vec<String> = agents.iter().zip(&parts).filter_map(|(a, p)| p.error.as_ref().map(|e| format!("{}: {e}", a.address))).collect();
        return Err(LoadError::Engine(format!("no agent returned results — {}", why.join("; "))));
    }
    let paths = plan.request_paths();
    let exports: Vec<RecorderExport> = finished.iter().map(|f| f.1.clone()).collect();
    let mut rec = Recorder::merge(&exports, &paths, plan.test.executor.is_open()).map_err(LoadError::Engine)?;
    let thresholds = evaluate_all(&plan.test.thresholds, &RecorderMetrics { rec: &rec, paths: &paths });
    let failures: Vec<String> = agents.iter().zip(&parts).filter_map(|(a, p)| p.error.as_ref().map(|e| format!("{}: {e}", a.address))).collect();
    let agent_abort = finished.iter().find_map(|f| f.0.abort_reason.clone().filter(|_| f.0.aborted));
    let aborted = !failures.is_empty() || control.is_stopped() || agent_abort.is_some();
    let abort_reason = if !failures.is_empty() {
        Some(format!("agent failure — {}", failures.join("; ")))
    } else if control.is_stopped() {
        Some(control.reason().unwrap_or_else(|| "stopped".into()))
    } else {
        agent_abort
    };
    let total_w: f64 = weights.iter().map(|w| f64::from(*w)).sum::<f64>().max(1.0);
    let agent_runs: Vec<AgentRun> = agents
        .iter()
        .zip(&parts)
        .zip(&weights)
        .map(|((a, p), w)| AgentRun {
            address: a.address.clone(),
            weight: *w,
            share: (f64::from(*w) / total_w * 1000.0).round() / 1000.0,
            ok: p.finished.is_some() && p.error.is_none(),
            error: p.error.clone(),
            version: p.info.as_ref().map(|i| i.version.clone()).unwrap_or_default(),
            machine: p.info.as_ref().map(|i| i.machine.clone()).unwrap_or_default(),
            requests: p.finished.as_ref().map(|f| f.0.totals.requests).unwrap_or(0),
            failed: p.finished.as_ref().map(|f| f.0.totals.failed).unwrap_or(0),
            p95: p.finished.as_ref().map(|f| f.0.latency.p95).unwrap_or(0.0),
            aborted_reason: p.finished.as_ref().and_then(|f| f.0.abort_reason.clone()),
        })
        .collect();
    let mut warnings = preflight.warnings.clone();
    for (a, p) in agents.iter().zip(&parts) {
        if let Some((s, _)) = &p.finished {
            for w in &s.warnings {
                let w = format!("{}: {w}", a.address);
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
        }
    }
    if rec.totals.dropped_iterations > 0 {
        warnings.push(format!("{} iterations were dropped because the agents' VUs were all busy. Raise maxVus or add agents.", rec.totals.dropped_iterations));
    }
    let meta = plan.request_meta();
    let redact = |s: String| devall_core::vars::redact_values(&s, &plan.secret_values);
    let elapsed = finished.iter().map(|f| f.0.duration_ms).max().unwrap_or_else(|| t0.elapsed().as_millis() as u64);
    rec.totals.max_vus = rec.totals.max_vus.max(finished.iter().map(|f| f.0.totals.max_vus).sum());
    let summary = LoadSummary {
        version: SUMMARY_VERSION,
        run_id: run_id.clone(),
        test: plan.test.name.clone(),
        test_path: plan.test_path.clone(),
        environment: plan.environment.clone(),
        mock_baseline: None,
        started_at,
        duration_ms: elapsed,
        planned_duration_ms: planned_ms,
        aborted,
        abort_reason: abort_reason.map(redact),
        thresholds_passed: thresholds.iter().all(|t| t.ok),
        executor: plan.test.executor.clone(),
        open_model: plan.test.executor.is_open(),
        totals: rec.totals.clone(),
        latency: rec.latency(),
        service_time: rec.service_time(),
        status_codes: rec.status_codes(),
        error_kinds: rec.errors,
        error_samples: rec.error_samples.iter().cloned().map(redact).collect(),
        requests: rec.request_stats(&meta),
        thresholds,
        warnings: warnings.into_iter().map(redact).collect(),
        bucket_seconds: rec.bucket_seconds(),
        timeline: rec.timeline().to_vec(),
        git: crate::store::git_info(&plan.project_root),
        machine: Machine::current(),
        agents: Some(agent_runs),
    };
    if let Some(p) = &progress {
        let pts = ready_points(&parts, &mut emitted, true);
        let mut snap = merged_snapshot(&run_id, planned_ms, &parts, pts, RunPhase::Finished);
        snap.latency = summary.latency;
        snap.requests = summary.totals.requests;
        snap.rps = summary.totals.rps;
        p(snap);
    }
    Ok(summary)
}

async fn send_stop(client: &reqwest::Client, agents: &[AgentTarget], token: &str) {
    for a in agents {
        let _ = client.post(format!("{}/v1/stop", agent_url(&a.address))).bearer_auth(token).timeout(Duration::from_secs(5)).send().await;
    }
}

/// Posts the plan and forwards the NDJSON events.
async fn stream_agent(client: &reqwest::Client, address: &str, token: &str, plan: &AgentPlan, i: usize, tx: &tokio::sync::mpsc::UnboundedSender<Incoming>) -> std::result::Result<(), String> {
    let body = serde_json::to_vec(plan).map_err(|e| e.to_string())?;
    let mut resp = client
        .post(format!("{}/v1/run", agent_url(address)))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("unreachable ({})", devall_http::error_chain(&e)))?;
    if !resp.status().is_success() {
        return Err(http_error(address, resp).await.to_string());
    }
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = resp.chunk().await.map_err(|e| format!("connection lost ({})", devall_http::error_chain(&e)))?;
        let Some(chunk) = chunk else { break };
        buf.extend_from_slice(&chunk);
        while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let line = &line[..line.len() - 1];
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let ev: AgentEvent = serde_json::from_slice(line).map_err(|e| format!("unreadable event: {e}"))?;
            let _ = tx.send(Incoming::Event(i, Box::new(ev)));
        }
    }
    Ok(())
}

// ---------- deployment helpers ----------

/// `devall load agent k8s`: a Secret (the token), a Deployment of agents and a headless Service
/// (one DNS record per pod, for `--agents dns:<service>.<namespace>.svc:7777`).
#[derive(Debug, Clone)]
pub struct K8sOptions {
    pub name: String,
    pub namespace: Option<String>,
    pub replicas: u32,
    pub image: String,
    pub port: u16,
    pub token: String,
    pub cpu: String,
    pub memory: String,
}

impl Default for K8sOptions {
    fn default() -> Self {
        Self {
            name: "devall-load-agent".into(),
            namespace: None,
            replicas: 2,
            image: "ghcr.io/OWNER/devall-agent:latest".into(),
            port: DEFAULT_AGENT_PORT,
            token: String::new(),
            cpu: "1".into(),
            memory: "512Mi".into(),
        }
    }
}

fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// The manifests as one multi-document YAML string.
pub fn k8s_manifests(o: &K8sOptions) -> String {
    let ns = o.namespace.as_deref().map(|n| format!("\n  namespace: {}", yaml_str(n))).unwrap_or_default();
    let ns_name = o.namespace.as_deref().unwrap_or("default");
    let name = &o.name;
    let labels = format!("\n    app.kubernetes.io/name: {name}\n    app.kubernetes.io/component: load-agent");
    let agents = [format!("dns:{name}.{ns_name}.svc:{}", o.port)];
    format!(
        r#"# /dev/all load agents — generated by `devall load agent k8s`.
# Apply:   kubectl apply -f agents.yaml
# In-cluster coordinator:  devall load run <test> --agents {agents} --agents-token-env {TOKEN_ENV}
#   (the headless Service resolves to every agent pod).
# From your machine: port-forward each pod, e.g.
#   kubectl -n {ns_name} get pods -l app.kubernetes.io/name={name}
#   kubectl -n {ns_name} port-forward pod/<pod> 17777:{port}   (then --agents 127.0.0.1:17777,…)
# The token Secret below holds the shared bearer token; keep this file out of git.
apiVersion: v1
kind: Secret
metadata:
  name: {name}-token{ns}
  labels:{labels}
type: Opaque
stringData:
  token: {token}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {name}{ns}
  labels:{labels}
spec:
  replicas: {replicas}
  selector:
    matchLabels:
      app.kubernetes.io/name: {name}
  template:
    metadata:
      labels:
        app.kubernetes.io/name: {name}
        app.kubernetes.io/component: load-agent
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
      containers:
        - name: agent
          image: {image}
          command: ["devall"]
          args: ["load", "agent", "--listen", "0.0.0.0:{port}"]
          env:
            - name: {TOKEN_ENV}
              valueFrom:
                secretKeyRef:
                  name: {name}-token
                  key: token
          ports:
            - name: agent
              containerPort: {port}
          readinessProbe:
            httpGet:
              path: /healthz
              port: agent
            periodSeconds: 5
          livenessProbe:
            httpGet:
              path: /healthz
              port: agent
            periodSeconds: 15
          resources:
            requests:
              cpu: {cpu}
              memory: {memory}
            limits:
              memory: {memory}
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
---
apiVersion: v1
kind: Service
metadata:
  name: {name}{ns}
  labels:{labels}
spec:
  clusterIP: None
  selector:
    app.kubernetes.io/name: {name}
  ports:
    - name: agent
      port: {port}
      targetPort: agent
"#,
        agents = agents.join(","),
        port = o.port,
        token = yaml_str(&o.token),
        replicas = o.replicas,
        image = yaml_str(&o.image),
        cpu = yaml_str(&o.cpu),
        memory = yaml_str(&o.memory),
    )
}

/// A random token for agents (256 bits, hex).
pub fn random_token() -> String {
    (0..4).map(|_| format!("{:016x}", fastrand::u64(..))).collect()
}

/// Default run options for an agent executing `ap`.
pub(crate) fn agent_run_options(ap: &AgentPlan) -> RunOptions {
    RunOptions { run_id: ap.run_id.clone(), snapshot_interval: Duration::from_millis(ap.snapshot_interval_ms.clamp(100, 5_000)), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_add_up() {
        assert_eq!(largest_remainder(10, &[1, 1, 1]), vec![4, 3, 3]);
        assert_eq!(largest_remainder(7, &[2, 1]), vec![5, 2]);
        assert_eq!(largest_remainder(1, &[1, 1]), vec![1, 0]);
        let ex = Executor::ConstantArrivalRate { rate: 90.0, time_unit: "1s".into(), duration: "10s".into(), pre_allocated_vus: 10, max_vus: Some(30) };
        let parts = split_executor(&ex, &[2, 1]);
        match (&parts[0], &parts[1]) {
            (Executor::ConstantArrivalRate { rate: a, pre_allocated_vus: pa, max_vus: ma, .. }, Executor::ConstantArrivalRate { rate: b, max_vus: mb, .. }) => {
                assert_eq!((*a, *b), (60.0, 30.0));
                assert_eq!((*pa, *ma, *mb), (7, Some(20), Some(10)));
            }
            _ => panic!(),
        }
        let vus = split_executor(&Executor::ConstantVus { vus: 5, duration: "1m".into() }, &[1, 1]);
        assert_eq!(vus.iter().map(Executor::max_vus).sum::<u32>(), 5);
        let ramp = split_executor(&Executor::RampingArrivalRate { start_rate: 10.0, time_unit: "1s".into(), stages: vec![Stage { duration: "5s".into(), target: 100.0 }], pre_allocated_vus: 4, max_vus: None }, &[1, 3]);
        assert_eq!(ramp[1].peak_rate_per_sec(), Some(75.0));
    }

    #[test]
    fn agent_addresses() {
        assert_eq!(agent_url("10.0.0.5"), "http://10.0.0.5:7777");
        assert_eq!(agent_url("10.0.0.5:9000"), "http://10.0.0.5:9000");
        assert_eq!(agent_url("https://agents.example.com/"), "https://agents.example.com");
        assert_eq!(agent_url("[::1]:7777"), "http://[::1]:7777");
        let a = parse_agents("h1:7777, h2", Some("2,1")).unwrap();
        assert_eq!(a[1], AgentTarget { address: "h2".into(), weight: 1 });
        assert!(parse_agents("h1,h2", Some("1")).is_err());
        assert!(parse_agents("", None).is_err());
        assert!(parse_agents("h1", Some("0")).is_err());
    }

    #[test]
    fn k8s_manifests_are_parseable_yaml() {
        let o = K8sOptions { replicas: 3, image: "registry.local/devall:1.0".into(), token: "t0k3n-with-\"quotes\"".into(), namespace: Some("load".into()), ..Default::default() };
        let text = k8s_manifests(&o);
        let docs: Vec<serde_yaml_ng::Value> = serde_yaml_ng::Deserializer::from_str(&text).map(|d| serde::Deserialize::deserialize(d).unwrap()).collect();
        assert_eq!(docs.len(), 3);
        assert_eq!(docs[0]["stringData"]["token"].as_str(), Some("t0k3n-with-\"quotes\""));
        assert_eq!(docs[1]["spec"]["replicas"].as_u64(), Some(3));
        assert_eq!(docs[1]["metadata"]["namespace"].as_str(), Some("load"));
        assert_eq!(docs[2]["spec"]["clusterIP"].as_str(), Some("None"));
        assert!(text.contains("--agents dns:devall-load-agent.load.svc:7777"));
    }
}
