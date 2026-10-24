//! Serde model of `load/<slug>.load.yaml`. Field order is the key order on disk; defaults stay out
//! of the file; unknown keys survive a read → write round trip (`extra`).
//!
//! ```yaml
//! version: 1
//! name: Users API smoke
//! environment: local
//! target: mock          # optional: run against the project's mock server (a "mock baseline")
//! scenario:
//!   requests:
//!     - path: users/list.req.yaml
//!       weight: 3
//!     - path: users/get.req.yaml
//! executor:
//!   type: constantArrivalRate
//!   rate: 50
//!   duration: 30s
//!   preAllocatedVus: 10
//!   maxVus: 100
//! thinkTime: { min: 100ms, max: 500ms }
//! thresholds:
//!   - p95 < 300ms
//!   - errorRate < 1%
//! http:
//!   timeoutMs: 5000
//! ```

use std::time::Duration;

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::duration::DurationSpec;

pub const FORMAT_VERSION: u32 = 1;

/// Unknown keys carried through a read → write round trip.
pub type Extra = IndexMap<String, serde_json::Value>;

fn format_version() -> u32 {
    FORMAT_VERSION
}
fn one() -> u32 {
    1
}
fn is_one(v: &u32) -> bool {
    *v == 1
}
fn is_false(b: &bool) -> bool {
    !*b
}
fn zero_f64() -> f64 {
    0.0
}
fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn default_time_unit() -> DurationSpec {
    DurationSpec::new("1s")
}
fn is_default_time_unit(d: &DurationSpec) -> bool {
    d.as_str() == "1s"
}

/// Writes whole numbers without a trailing `.0` so `rate: 50` stays `rate: 50`.
pub(crate) fn ser_num<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.fract() == 0.0 && v.abs() < 9.0e15 { s.serialize_i64(*v as i64) } else { s.serialize_f64(*v) }
}

/// `load/<slug>.load.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(title = "Load test")]
pub struct LoadTestFile {
    #[serde(default = "format_version")]
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Environment id (file stem in `environments/`). The CLI's `--env` overrides it; the app uses
    /// the active environment when this is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// `mock` sends the load to the project's built-in mock server instead of the requests' own
    /// hosts (the CLI's `--against-mock` does the same for one run).
    #[serde(default, skip_serializing_if = "LoadTarget::is_requests")]
    pub target: LoadTarget,
    pub scenario: Scenario,
    pub executor: Executor,
    /// Pause after every request: between flow steps and, for VU executors, between iterations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_time: Option<ThinkTime>,
    /// Pass/fail rules, e.g. `p95 < 300ms`, `errorRate < 1%`, `rps > 50`, `checks > 99%`,
    /// `p99{users/list.req.yaml} < 1s`. Any failure makes the CLI exit with code 99.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thresholds: Vec<String>,
    /// Per-response assertions feeding the `checks` metric. Default: status below 400.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    #[serde(default, skip_serializing_if = "HttpSettings::is_default")]
    pub http: HttpSettings,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Where a test sends its load.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LoadTarget {
    /// The saved requests' own URLs (resolved with the environment).
    #[default]
    Requests,
    /// The project's built-in mock server (`mocks/`): every request URL's origin is replaced with
    /// the mock's, so the run measures the client and tooling side — a baseline to compare real
    /// runs against.
    Mock,
}

impl LoadTarget {
    pub fn is_requests(&self) -> bool {
        *self == LoadTarget::Requests
    }
}

/// Either a weighted random mix of requests (one request per iteration) or an ordered flow
/// (every step once per iteration, in order). Exactly one of the two lists must be set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Scenario {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<WeightedRequest>,
    /// Request paths (relative to `http/`) run in order each iteration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flow: Vec<String>,
}

impl Scenario {
    pub fn is_flow(&self) -> bool {
        !self.flow.is_empty()
    }

    /// Every distinct request path, in first-use order.
    pub fn paths(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let all = self.requests.iter().map(|r| &r.path).chain(self.flow.iter());
        for p in all {
            if !out.contains(p) {
                out.push(p.clone());
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeightedRequest {
    /// Request file relative to `http/`, e.g. `users/list.req.yaml`.
    pub path: String,
    /// Relative share of iterations (default 1).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub weight: u32,
}

/// How load is generated. VU executors are closed-model (a VU waits for its response before the
/// next iteration); arrival-rate executors are open-model (iterations start on schedule whatever
/// the response times, so latency is measured from the intended start — no coordinated omission).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Executor {
    /// A fixed number of VUs loop for `duration`.
    ConstantVus { vus: u32, duration: DurationSpec },
    /// VUs ramp linearly between stage targets.
    RampingVus {
        #[serde(default, skip_serializing_if = "is_zero_u32")]
        start_vus: u32,
        stages: Vec<Stage>,
    },
    /// `rate` iterations per `timeUnit`, for `duration`.
    ConstantArrivalRate {
        #[serde(serialize_with = "ser_num")]
        rate: f64,
        #[serde(default = "default_time_unit", skip_serializing_if = "is_default_time_unit")]
        time_unit: DurationSpec,
        duration: DurationSpec,
        pre_allocated_vus: u32,
        /// Defaults to `preAllocatedVus`. Iterations that find no free VU are dropped and reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_vus: Option<u32>,
    },
    /// Iteration rate ramps linearly between stage targets (per `timeUnit`).
    RampingArrivalRate {
        #[serde(default = "zero_f64", skip_serializing_if = "is_zero_f64", serialize_with = "ser_num")]
        start_rate: f64,
        #[serde(default = "default_time_unit", skip_serializing_if = "is_default_time_unit")]
        time_unit: DurationSpec,
        stages: Vec<Stage>,
        pre_allocated_vus: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_vus: Option<u32>,
    },
}

impl Executor {
    pub fn kind(&self) -> &'static str {
        match self {
            Executor::ConstantVus { .. } => "constantVus",
            Executor::RampingVus { .. } => "rampingVus",
            Executor::ConstantArrivalRate { .. } => "constantArrivalRate",
            Executor::RampingArrivalRate { .. } => "rampingArrivalRate",
        }
    }

    /// Arrival-rate (open model) executors.
    pub fn is_open(&self) -> bool {
        matches!(self, Executor::ConstantArrivalRate { .. } | Executor::RampingArrivalRate { .. })
    }

    /// Planned run time (stages summed). Invalid durations count as zero.
    pub fn planned_duration(&self) -> Duration {
        match self {
            Executor::ConstantVus { duration, .. } | Executor::ConstantArrivalRate { duration, .. } => duration.get(),
            Executor::RampingVus { stages, .. } | Executor::RampingArrivalRate { stages, .. } => stages.iter().map(|s| s.duration.get()).sum(),
        }
    }

    /// Most VUs this executor can use.
    pub fn max_vus(&self) -> u32 {
        match self {
            Executor::ConstantVus { vus, .. } => *vus,
            Executor::RampingVus { start_vus, stages } => stages.iter().map(|s| s.target.max(0.0).round() as u32).fold(*start_vus, u32::max),
            Executor::ConstantArrivalRate { pre_allocated_vus, max_vus, .. } | Executor::RampingArrivalRate { pre_allocated_vus, max_vus, .. } => {
                max_vus.unwrap_or(*pre_allocated_vus).max(*pre_allocated_vus)
            }
        }
    }

    /// Highest iteration rate per second (open models only).
    pub fn peak_rate_per_sec(&self) -> Option<f64> {
        match self {
            Executor::ConstantArrivalRate { rate, time_unit, .. } => Some(per_sec(*rate, time_unit)),
            Executor::RampingArrivalRate { start_rate, time_unit, stages, .. } => {
                Some(stages.iter().map(|s| s.target).fold(*start_rate, f64::max)).map(|r| per_sec(r, time_unit))
            }
            _ => None,
        }
    }
}

pub(crate) fn per_sec(rate: f64, unit: &DurationSpec) -> f64 {
    let secs = unit.get().as_secs_f64();
    if secs > 0.0 { rate / secs } else { 0.0 }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    pub duration: DurationSpec,
    /// VUs (ramping VUs) or iterations per `timeUnit` (ramping arrival rate) reached at the end
    /// of the stage.
    #[serde(serialize_with = "ser_num")]
    pub target: f64,
}

/// `thinkTime: 1s` or `thinkTime: { min: 500ms, max: 2s }` (uniformly random).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ThinkTime {
    Fixed(DurationSpec),
    Range { min: DurationSpec, max: DurationSpec },
}

impl ThinkTime {
    pub fn sample(&self) -> Duration {
        match self {
            ThinkTime::Fixed(d) => d.get(),
            ThinkTime::Range { min, max } => {
                let (a, b) = (min.get(), max.get());
                if b <= a {
                    return a;
                }
                a + (b - a).mul_f64(fastrand::f64())
            }
        }
    }

    pub fn mean(&self) -> Duration {
        match self {
            ThinkTime::Fixed(d) => d.get(),
            ThinkTime::Range { min, max } => (min.get() + max.get()) / 2,
        }
    }
}

/// A per-response assertion. The `checks` metric is passed / evaluated across all of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Check {
    /// Status matches one of the patterns: `200`, `2xx`, `20x`.
    Status { expect: Vec<String> },
    /// Response (from the intended start) completes within `duration`.
    MaxDuration { duration: DurationSpec },
    /// Response body contains `text`.
    BodyContains { text: String },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HttpSettings {
    /// Per-request timeout. Defaults to each request's own timeout setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Caps requests in flight (and so open HTTP/1.1 connections). Waiting for a connection counts
    /// toward latency in arrival-rate tests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_connections: Option<u32>,
    /// HTTP/2 with prior knowledge (h2c for `http://`). Default: HTTP/1.1 with keep-alive.
    #[serde(default, skip_serializing_if = "is_false")]
    pub http2: bool,
    /// Accept invalid TLS certificates for every request.
    #[serde(default, skip_serializing_if = "is_false")]
    pub insecure: bool,
}

impl HttpSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl LoadTestFile {
    /// A sensible starting point: a gentle open-model smoke test of `request`.
    pub fn template(name: impl Into<String>, request: Option<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            name: name.into(),
            description: None,
            environment: None,
            target: LoadTarget::Requests,
            scenario: Scenario { requests: request.map(|path| vec![WeightedRequest { path, weight: 1 }]).unwrap_or_default(), flow: Vec::new() },
            executor: Executor::ConstantArrivalRate {
                rate: 10.0,
                time_unit: default_time_unit(),
                duration: DurationSpec::new("30s"),
                pre_allocated_vus: 10,
                max_vus: Some(50),
            },
            think_time: None,
            thresholds: vec!["p95 < 500ms".into(), "errorRate < 1%".into()],
            checks: Vec::new(),
            http: HttpSettings::default(),
            extra: Extra::new(),
        }
    }
}
