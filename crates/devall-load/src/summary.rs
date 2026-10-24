//! Run results: the summary written to `load/runs/<test>/<timestamp>.json` and the live
//! snapshots streamed while a test runs. All latencies are milliseconds rounded to 0.01 ms.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::Executor;
use crate::threshold::{Metric, MetricSource, ThresholdResult};

pub const SUMMARY_VERSION: u32 = 1;

/// Latency distribution in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Percentiles {
    pub p50: f64,
    pub p75: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub p999: f64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

impl Percentiles {
    /// The stored percentile matching `q`, if this summary keeps it.
    pub fn quantile(&self, q: f64) -> Option<f64> {
        let close = |x: f64| (q - x).abs() < 1e-9;
        Some(if close(0.5) {
            self.p50
        } else if close(0.75) {
            self.p75
        } else if close(0.9) {
            self.p90
        } else if close(0.95) {
            self.p95
        } else if close(0.99) {
            self.p99
        } else if close(0.999) {
            self.p999
        } else if close(1.0) {
            self.max
        } else {
            return None;
        })
    }
}

/// Transport-level failures (no HTTP response).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorKinds {
    /// Connection refused/reset, DNS, TLS handshake, ephemeral port exhaustion.
    pub connect: u64,
    pub timeout: u64,
    /// Anything else (invalid request after templating, body read errors, …).
    pub other: u64,
}

impl ErrorKinds {
    pub fn total(&self) -> u64 {
        self.connect + self.timeout + self.other
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    /// Requests that completed (with a response or an error).
    pub requests: u64,
    /// Transport errors plus responses with status ≥ 400.
    pub failed: u64,
    /// `failed / requests` (fraction).
    pub error_rate: f64,
    pub iterations: u64,
    /// Arrival-rate iterations skipped because every VU (up to `maxVus`) was busy.
    pub dropped_iterations: u64,
    /// Completed requests per second over the run.
    pub rps: f64,
    pub iterations_per_sec: f64,
    /// Approximate bytes on the wire (headers + body).
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub checks_passed: u64,
    pub checks_failed: u64,
    /// `checksPassed / (checksPassed + checksFailed)` (fraction; 1 when nothing was checked).
    pub checks_rate: f64,
    /// Most VUs busy at once.
    pub max_vus: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestStats {
    /// Request file relative to `http/`.
    pub path: String,
    pub name: String,
    pub method: String,
    pub requests: u64,
    pub failed: u64,
    pub error_rate: f64,
    pub rps: f64,
    pub latency: Percentiles,
    pub status_codes: BTreeMap<String, u64>,
}

/// One bucket of the timeline (`bucketSeconds` wide; ≤ 600 buckets per run).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePoint {
    /// Bucket start, seconds from the start of the run.
    pub t: u32,
    pub requests: u64,
    pub failed: u64,
    pub rps: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    /// Most VUs busy during the bucket.
    pub vus: u32,
    pub dropped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub commit: String,
}

/// Set on runs that targeted the project's mock server instead of the real hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MockBaseline {
    /// Mock server base URL used for the run.
    pub url: String,
    /// Hosts (`host:port`) the requests normally go to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced_hosts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub os: String,
    pub cpus: u32,
}

impl Machine {
    pub fn current() -> Self {
        Self { os: std::env::consts::OS.to_string(), cpus: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1) }
    }
}

/// One load agent of a distributed run (`devall load run --agents …`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    /// `host:port` (or URL) the coordinator used.
    pub address: String,
    pub weight: u32,
    /// Fraction of the load assigned (weight / total weight).
    pub share: f64,
    /// Whether the agent finished and returned its measurements.
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Agent build (`devall --version`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    #[serde(default)]
    pub machine: Machine,
    pub requests: u64,
    pub failed: u64,
    /// The agent's own p95 (ms) — the summary's percentiles come from the merged histograms.
    pub p95: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aborted_reason: Option<String>,
}

/// Everything about one run. Contains no secret values (they are redacted before writing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadSummary {
    pub version: u32,
    pub run_id: String,
    /// Test name at the time of the run.
    pub test: String,
    /// Test file relative to `load/`.
    pub test_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Present when the run targeted the project's mock server (a mock baseline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock_baseline: Option<MockBaseline>,
    /// RFC 3339, UTC.
    pub started_at: String,
    pub duration_ms: u64,
    pub planned_duration_ms: u64,
    pub aborted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_reason: Option<String>,
    pub thresholds_passed: bool,
    pub executor: Executor,
    /// Open model: latency is measured from each iteration's intended start time, so it is
    /// corrected for coordinated omission. Closed (VU) models measure from the actual send.
    pub open_model: bool,
    pub totals: Totals,
    pub latency: Percentiles,
    /// Open model only: latency from the actual send (no queueing or scheduling delay), the
    /// "uncorrected" view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_time: Option<Percentiles>,
    pub status_codes: BTreeMap<String, u64>,
    pub error_kinds: ErrorKinds,
    /// A few distinct error messages (secrets redacted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub error_samples: Vec<String>,
    pub requests: Vec<RequestStats>,
    pub thresholds: Vec<ThresholdResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub bucket_seconds: u32,
    pub timeline: Vec<TimelinePoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitInfo>,
    pub machine: Machine,
    /// Distributed runs: the agents that generated the load (`machine` is the coordinator's).
    /// Latency percentiles and thresholds come from the agents' merged HDR histograms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<AgentRun>>,
}

impl LoadSummary {
    /// Passed = finished normally and every threshold held.
    pub fn passed(&self) -> bool {
        !self.aborted && self.thresholds_passed
    }
}

impl MetricSource for LoadSummary {
    fn metric(&self, metric: Metric, scope: Option<&str>) -> Option<f64> {
        if let Some(path) = scope {
            let r = self.requests.iter().find(|r| r.path == path)?;
            return match metric {
                Metric::Quantile(q) => (r.requests > 0).then(|| r.latency.quantile(q)).flatten(),
                Metric::Min => (r.requests > 0).then_some(r.latency.min),
                Metric::Max => (r.requests > 0).then_some(r.latency.max),
                Metric::Mean => (r.requests > 0).then_some(r.latency.mean),
                Metric::Rps => Some(r.rps),
                Metric::ErrorRate => (r.requests > 0).then_some(r.error_rate),
                Metric::Requests => Some(r.requests as f64),
                Metric::Errors => Some(r.failed as f64),
                _ => None,
            };
        }
        let t = &self.totals;
        let has = t.requests > 0;
        match metric {
            Metric::Quantile(q) => has.then(|| self.latency.quantile(q)).flatten(),
            Metric::Min => has.then_some(self.latency.min),
            Metric::Max => has.then_some(self.latency.max),
            Metric::Mean => has.then_some(self.latency.mean),
            Metric::Rps => Some(t.rps),
            Metric::ErrorRate => has.then_some(t.error_rate),
            Metric::Checks => (t.checks_passed + t.checks_failed > 0).then_some(t.checks_rate),
            Metric::Requests => Some(t.requests as f64),
            Metric::Errors => Some(t.failed as f64),
            Metric::Iterations => Some(t.iterations as f64),
            Metric::Dropped => Some(t.dropped_iterations as f64),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RunPhase {
    Running,
    /// Duration elapsed or stop requested: waiting for in-flight requests.
    Stopping,
    Finished,
}

/// Live progress, emitted every ~250 ms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadSnapshot {
    pub run_id: String,
    pub phase: RunPhase,
    pub elapsed_ms: u64,
    pub planned_duration_ms: u64,
    /// VUs busy right now.
    pub vus: u32,
    /// VUs allocated (arrival-rate executors) or targeted (VU executors).
    pub vus_allocated: u32,
    pub requests: u64,
    pub failed: u64,
    pub iterations: u64,
    pub dropped_iterations: u64,
    /// Requests per second over roughly the last second.
    pub rps: f64,
    /// Cumulative latency so far.
    pub latency: Percentiles,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub status_codes: BTreeMap<String, u64>,
    pub error_kinds: ErrorKinds,
    /// Timeline buckets completed since the previous snapshot (append them).
    pub points: Vec<TimelinePoint>,
    pub bucket_seconds: u32,
}
