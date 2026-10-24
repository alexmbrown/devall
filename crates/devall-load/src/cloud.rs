//! Cloud load-generator regions: a test's open-model profile run from the hosted (or a
//! self-hosted) relay's edge — Durable Objects in up to N regions — against hosts the account
//! verified, merged by the relay and turned back into a normal [`LoadSummary`] here, so cloud
//! runs are stored, compared and shown like local ones.
//!
//! This module builds the JSON plan (`POST /v1/load/runs`, relay/PROTOCOL.md §6.4) and reads the
//! answer; the HTTP call itself goes through `devall-relay` in the CLI / app.
//!
//! * **Open model only.** Arrival-rate executors (`constantArrivalRate`, `rampingArrivalRate`);
//!   the edge measures latency from each iteration's intended start like the local engine.
//! * **Fixed requests.** Every request is resolved here once (variables, auth headers); requests
//!   using dynamic values (`{{$uuid}}` …) send the same resolved values every time (warned).
//! * **Secrets.** Same rule as distributed agents: a request needing `{{secret.X}}` is refused
//!   unless the user explicitly allowed sending resolved values (`--allow-secrets-to-cloud`).

use std::collections::BTreeSet;

use base64::Engine as _;
use devall_http::{PreparedBody, PreparedRequest};
use serde::{Deserialize, Serialize};

use crate::error::{LoadError, Result};
use crate::model::{Check, Executor, LoadTarget, ThinkTime, per_sec};
use crate::plan::Plan;
use crate::summary::{AgentRun, ErrorKinds, LoadSummary, Machine, Percentiles, RequestStats, SUMMARY_VERSION, TimelinePoint, Totals};
use crate::threshold::evaluate_all;

/// Durable Object location hints the relay accepts, with display names.
pub const REGIONS: &[(&str, &str)] = &[
    ("wnam", "Western North America"),
    ("enam", "Eastern North America"),
    ("sam", "South America"),
    ("weur", "Western Europe"),
    ("eeur", "Eastern Europe"),
    ("apac", "Asia-Pacific"),
    ("oc", "Oceania"),
    ("afr", "Africa"),
    ("me", "Middle East"),
];

/// Where a target serves its verification token.
pub const VERIFY_PATH: &str = "/.well-known/devall-load-verification.txt";
const MAX_TIMEOUT_MS: u64 = 30_000;
const MAX_THINK_MS: u64 = 5_000;
const MAX_BODY: usize = 64 * 1024;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRequest {
    pub name: String,
    pub path: String,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    pub timeout_ms: u64,
    pub follow_redirects: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudStage {
    pub duration_ms: u64,
    /// Iterations per second at the end of the stage.
    pub target: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudProfile {
    /// Iterations per second at the start.
    pub start_rate: f64,
    pub stages: Vec<CloudStage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CloudCheck {
    Status { expect: Vec<String> },
    MaxDuration { ms: u64 },
    BodyContains { text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkTimeMs {
    pub min: u64,
    pub max: u64,
}

/// `POST /v1/load/runs`. No `Debug`: requests may hold resolved secret values.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPlan {
    pub run_id: String,
    pub name: String,
    pub regions: Vec<String>,
    pub profile: CloudProfile,
    pub max_in_flight: u32,
    pub requests: Vec<CloudRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<Vec<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_time_ms: Option<ThinkTimeMs>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CloudCheck>,
}

impl std::fmt::Debug for CloudPlan {
    /// Never prints headers, URLs or bodies (they may carry resolved secrets).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudPlan")
            .field("run_id", &self.run_id)
            .field("regions", &self.regions)
            .field("profile", &self.profile)
            .field("requests", &self.requests.iter().map(|r| r.path.as_str()).collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default)]
pub struct CloudOptions {
    /// Region ids (`weur,enam`); see [`REGIONS`].
    pub regions: Vec<String>,
    /// The user explicitly allowed sending resolved `{{secret.X}}` values to the relay's edge.
    pub allow_secrets: bool,
    /// Run id (also used to stop the run); empty = a new ULID.
    pub run_id: String,
}

/// A plan ready to send, the origins it needs verified and what the user should know.
#[derive(Debug, Clone)]
pub struct CloudPrepared {
    pub plan: CloudPlan,
    /// `scheme://host[:port]` of every request (each must be verified on the relay).
    pub origins: Vec<String>,
    pub warnings: Vec<String>,
}

/// `weur, enam` → validated ids (order kept, duplicates removed).
pub fn parse_regions(list: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for r in list.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()) {
        if !REGIONS.iter().any(|(id, _)| *id == r) {
            return Err(LoadError::Invalid(format!("unknown region `{r}` — use {}", REGIONS.iter().map(|(id, _)| *id).collect::<Vec<_>>().join(", "))));
        }
        if !out.contains(&r) {
            out.push(r);
        }
    }
    if out.is_empty() {
        return Err(LoadError::Invalid("pick at least one region, e.g. --regions weur,enam".into()));
    }
    Ok(out)
}

/// `scheme://host[:port]` (default ports dropped) of a URL.
pub fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    })
}

fn profile(ex: &Executor) -> Result<CloudProfile> {
    let ms = |d: std::time::Duration| d.as_millis() as u64;
    match ex {
        Executor::ConstantArrivalRate { rate, time_unit, duration, .. } => {
            let r = per_sec(*rate, time_unit);
            Ok(CloudProfile { start_rate: r, stages: vec![CloudStage { duration_ms: ms(duration.get()), target: r }] })
        }
        Executor::RampingArrivalRate { start_rate, time_unit, stages, .. } => Ok(CloudProfile {
            start_rate: per_sec(*start_rate, time_unit),
            stages: stages.iter().map(|s| CloudStage { duration_ms: ms(s.duration.get()), target: per_sec(s.target, time_unit) }).collect(),
        }),
        _ => Err(LoadError::Invalid(format!(
            "cloud regions run open-model tests only (constantArrivalRate or rampingArrivalRate); this test uses {} — switch the executor or run it locally / on agents",
            ex.kind()
        ))),
    }
}

fn wire_body(p: &PreparedRequest, path: &str, headers: &mut Vec<(String, String)>) -> Result<Option<Vec<u8>>> {
    match &p.body {
        PreparedBody::None => Ok(None),
        PreparedBody::Bytes(b) => Ok(Some(b.clone())),
        PreparedBody::Form(pairs) => {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in pairs {
                ser.append_pair(k, v);
            }
            if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                headers.push(("content-type".into(), "application/x-www-form-urlencoded".into()));
            }
            Ok(Some(ser.finish().into_bytes()))
        }
        PreparedBody::Multipart(_) => Err(LoadError::Invalid(format!("{path} sends a multipart body, which cloud regions don't support (run it locally or on agents)"))),
    }
}

impl Plan {
    /// The relay's run plan for this test. Refused for closed-model executors, mock baselines,
    /// client certificates / Digest / NTLM, multipart bodies, and — unless `allow_secrets` — any
    /// request that resolved a `{{secret.X}}`.
    pub fn cloud_plan(&self, opts: &CloudOptions) -> Result<CloudPrepared> {
        if self.base_override.is_some() || self.test.target == LoadTarget::Mock {
            return Err(LoadError::Invalid("mock baselines run against this machine's mock server; run them locally, not from cloud regions".into()));
        }
        if opts.regions.is_empty() {
            return Err(LoadError::Invalid("pick at least one region".into()));
        }
        for r in &opts.regions {
            if !REGIONS.iter().any(|(id, _)| id == r) {
                return Err(LoadError::Invalid(format!("unknown region `{r}`")));
            }
        }
        let profile = profile(&self.test.executor)?;
        let with_secrets = self.secret_request_paths();
        if !with_secrets.is_empty() && !opts.allow_secrets {
            return Err(LoadError::SecretsToAgents(format!(
                "{} use{} secrets ({}). The relay's edge would receive the resolved values to send them. Re-run with --allow-secrets-to-cloud (in the app: tick “Send resolved secrets to the relay”) only if you trust the relay you use.",
                with_secrets.join(", "),
                if with_secrets.len() == 1 { "s" } else { "" },
                "{{secret.…}}"
            )));
        }
        let mut warnings = Vec::new();
        let mut origins: Vec<String> = Vec::new();
        let mut requests = Vec::with_capacity(self.requests.len());
        for r in &self.requests {
            let p = match &r.fixed {
                Some(p) => p,
                None => {
                    warnings.push(format!("{} uses dynamic values ({{{{$…}}}}); cloud regions send the values resolved once, for every request", r.path));
                    &r.probe
                }
            };
            if p.client_cert.is_some() {
                return Err(LoadError::Invalid(format!("{} uses a client certificate (a file on this machine), so it can't run from cloud regions", r.path)));
            }
            if p.digest.is_some() || p.ntlm.is_some() {
                return Err(LoadError::Invalid(format!("{} uses Digest/NTLM authentication (a per-connection handshake), which cloud regions don't do", r.path)));
            }
            if p.proxy.is_some() {
                warnings.push(format!("{}: the request's proxy setting doesn't apply from cloud regions", r.path));
            }
            if !p.verify_tls || r.accept_invalid_certs {
                warnings.push(format!("{}: cloud regions always verify TLS certificates", r.path));
            }
            let mut headers = p.headers.clone();
            let body = wire_body(p, &r.path, &mut headers)?;
            if body.as_ref().is_some_and(|b| b.len() > MAX_BODY) {
                return Err(LoadError::Invalid(format!("{}: request bodies from cloud regions are limited to 64 KiB", r.path)));
            }
            let mut timeout = r.timeout.as_millis() as u64;
            if timeout > MAX_TIMEOUT_MS {
                warnings.push(format!("{}: timeout capped at 30 s from cloud regions", r.path));
                timeout = MAX_TIMEOUT_MS;
            }
            let origin = origin_of(&p.url).ok_or_else(|| LoadError::Invalid(format!("{}: `{}` isn't an absolute http(s) URL", r.path, devall_core::vars::redact_values(&p.url, &self.secret_values))))?;
            if !origins.contains(&origin) {
                origins.push(origin);
            }
            requests.push(CloudRequest {
                name: r.name.clone(),
                path: r.path.clone(),
                method: p.method.clone(),
                url: p.url.clone(),
                headers,
                body_base64: body.map(|b| base64::engine::general_purpose::STANDARD.encode(b)),
                timeout_ms: timeout.max(100),
                follow_redirects: p.follow_redirects,
            });
        }
        let (weights, flow) = if self.flow.is_empty() {
            let mut w = vec![0u32; requests.len()];
            for (i, weight) in &self.weighted {
                if let Some(x) = w.get_mut(*i) {
                    *x = x.saturating_add(*weight);
                }
            }
            (Some(w.into_iter().map(|x| x.clamp(1, 1000)).collect()), None)
        } else {
            (None, Some(self.flow.clone()))
        };
        let think_time_ms = self.test.think_time.as_ref().map(|t| {
            let (a, b) = match t {
                ThinkTime::Fixed(d) => (d.get(), d.get()),
                ThinkTime::Range { min, max } => (min.get(), max.get().max(min.get())),
            };
            let (a, b) = (a.as_millis() as u64, b.as_millis() as u64);
            if b > MAX_THINK_MS {
                warnings.push("think time capped at 5 s from cloud regions".into());
            }
            ThinkTimeMs { min: a.min(MAX_THINK_MS), max: b.min(MAX_THINK_MS) }
        });
        let checks = self
            .test
            .checks
            .iter()
            .map(|c| match c {
                Check::Status { expect } => CloudCheck::Status { expect: expect.clone() },
                Check::MaxDuration { duration } => CloudCheck::MaxDuration { ms: duration.get().as_millis() as u64 },
                Check::BodyContains { text } => CloudCheck::BodyContains { text: text.clone() },
            })
            .collect();
        if self.test.http.http2 {
            warnings.push("http2: cloud regions use whatever the edge negotiates with the target".into());
        }
        let run_id = if opts.run_id.is_empty() { ulid::Ulid::generate().to_string() } else { opts.run_id.clone() };
        Ok(CloudPrepared {
            plan: CloudPlan {
                run_id,
                name: self.test.name.clone(),
                regions: opts.regions.clone(),
                profile,
                max_in_flight: self.test.executor.max_vus().clamp(1, 2000),
                requests,
                weights,
                flow,
                think_time_ms,
                checks,
            },
            origins,
            warnings,
        })
    }
}

// ---------- the relay's answer ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRegionResult {
    pub region: String,
    pub name: String,
    /// Data center the region actually ran in, when the relay could tell (IATA code).
    #[serde(default)]
    pub colo: Option<String>,
    pub share: f64,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    pub requests: u64,
    pub failed: u64,
    #[serde(default)]
    pub dropped_iterations: u64,
    pub p50: f64,
    pub p95: f64,
}

/// `POST /v1/load/runs` answer: the summary fields the edge can fill (secrets never stored there).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudResult {
    pub run_id: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub planned_duration_ms: u64,
    pub aborted: bool,
    #[serde(default)]
    pub abort_reason: Option<String>,
    pub totals: Totals,
    pub latency: Percentiles,
    #[serde(default)]
    pub service_time: Option<Percentiles>,
    #[serde(default)]
    pub status_codes: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    pub error_kinds: ErrorKinds,
    #[serde(default)]
    pub error_samples: Vec<String>,
    #[serde(default)]
    pub requests: Vec<RequestStats>,
    pub bucket_seconds: u32,
    #[serde(default)]
    pub timeline: Vec<TimelinePoint>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub regions: Vec<CloudRegionResult>,
}

/// Address recorded for a region in `summary.agents` (`cloud:weur (AMS)`).
pub fn region_address(r: &CloudRegionResult) -> String {
    match &r.colo {
        Some(c) => format!("cloud:{} ({c})", r.region),
        None => format!("cloud:{}", r.region),
    }
}

impl Plan {
    /// A [`LoadSummary`] from the relay's merged result: thresholds evaluated here, regions
    /// recorded as `agents` (`cloud:<region> (<colo>)`), secrets redacted.
    pub fn cloud_summary(&self, result: CloudResult, extra_warnings: &[String]) -> LoadSummary {
        let redact = |s: String| devall_core::vars::redact_values(&s, &self.secret_values);
        let mut warnings: Vec<String> = extra_warnings.to_vec();
        for w in result.warnings {
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
        warnings.push("Ran from cloud regions: Durable Object placement (locationHint) is best-effort; each region's data center is listed when the relay detected it.".into());
        let agents: Vec<AgentRun> = result
            .regions
            .iter()
            .map(|r| AgentRun {
                address: region_address(r),
                weight: 1,
                share: r.share,
                ok: r.ok,
                error: r.error.clone().map(redact),
                version: "edge".into(),
                machine: Machine { os: format!("cloudflare {}", r.name), cpus: 0 },
                requests: r.requests,
                failed: r.failed,
                p95: r.p95,
                aborted_reason: None,
            })
            .collect();
        let mut summary = LoadSummary {
            version: SUMMARY_VERSION,
            run_id: result.run_id,
            test: self.test.name.clone(),
            test_path: self.test_path.clone(),
            environment: self.environment.clone(),
            mock_baseline: None,
            started_at: result.started_at,
            duration_ms: result.duration_ms,
            planned_duration_ms: result.planned_duration_ms,
            aborted: result.aborted,
            abort_reason: result.abort_reason.map(redact),
            thresholds_passed: true,
            executor: self.test.executor.clone(),
            open_model: true,
            totals: result.totals,
            latency: result.latency,
            service_time: result.service_time,
            status_codes: result.status_codes,
            error_kinds: result.error_kinds,
            error_samples: result.error_samples.into_iter().map(redact).collect(),
            requests: result.requests,
            thresholds: Vec::new(),
            warnings: warnings.into_iter().map(redact).collect(),
            bucket_seconds: result.bucket_seconds.max(1),
            timeline: result.timeline,
            git: crate::store::git_info(&self.project_root),
            machine: Machine::current(),
            agents: Some(agents),
        };
        let thresholds = evaluate_all(&self.test.thresholds, &summary);
        summary.thresholds_passed = thresholds.iter().all(|t| t.ok);
        summary.thresholds = thresholds;
        summary
    }

    /// Distinct `scheme://host[:port]` the test's requests go to (to verify before a cloud run).
    pub fn request_origins(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for r in &self.requests {
            let url = r.fixed.as_ref().map(|p| p.url.as_str()).unwrap_or(r.probe.url.as_str());
            if let Some(o) = origin_of(url)
                && seen.insert(o.clone())
            {
                out.push(o);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_regions_and_origins() {
        assert_eq!(parse_regions("weur, ENAM,weur").unwrap(), vec!["weur", "enam"]);
        assert!(parse_regions("mars").is_err());
        assert!(parse_regions(" , ").is_err());
        assert_eq!(origin_of("https://api.example.com/x?y=1").as_deref(), Some("https://api.example.com"));
        assert_eq!(origin_of("http://127.0.0.1:3000/").as_deref(), Some("http://127.0.0.1:3000"));
        assert_eq!(origin_of("https://api.example.com:443/").as_deref(), Some("https://api.example.com"));
        assert!(origin_of("not a url").is_none());
    }

    #[test]
    fn profiles_follow_the_executor() {
        let ex: Executor = serde_yaml_ng::from_str("type: constantArrivalRate\nrate: 120\ntimeUnit: 1m\nduration: 30s\npreAllocatedVus: 5").unwrap();
        let p = profile(&ex).unwrap();
        assert_eq!(p.start_rate, 2.0);
        assert_eq!(p.stages, vec![CloudStage { duration_ms: 30_000, target: 2.0 }]);
        let ramp: Executor = serde_yaml_ng::from_str("type: rampingArrivalRate\nstartRate: 1\nstages:\n  - {duration: 10s, target: 5}\n  - {duration: 5s, target: 0}\npreAllocatedVus: 5").unwrap();
        let p = profile(&ramp).unwrap();
        assert_eq!(p.start_rate, 1.0);
        assert_eq!(p.stages.len(), 2);
        let vus: Executor = serde_yaml_ng::from_str("type: constantVus\nvus: 2\nduration: 5s").unwrap();
        assert!(profile(&vus).unwrap_err().to_string().contains("open-model"));
    }

    #[test]
    fn reads_the_relay_answer() {
        let json = r#"{"version":1,"runId":"r1","test":"t","startedAt":"2026-10-22T10:00:00Z","durationMs":5000,"plannedDurationMs":5000,"aborted":false,"abortReason":null,"openModel":true,
          "totals":{"requests":20,"failed":0,"errorRate":0,"iterations":20,"droppedIterations":0,"rps":4,"iterationsPerSec":4,"bytesIn":100,"bytesOut":50,"checksPassed":20,"checksFailed":0,"checksRate":1,"maxVus":2},
          "latency":{"p50":1,"p75":1,"p90":2,"p95":2,"p99":3,"p999":3,"min":0.5,"max":3,"mean":1.2},"serviceTime":{"p50":1,"p75":1,"p90":2,"p95":2,"p99":3,"p999":3,"min":0.5,"max":3,"mean":1.2},
          "statusCodes":{"200":20},"errorKinds":{"connect":0,"timeout":0,"other":0},"errorSamples":[],
          "requests":[{"path":"a.req.yaml","name":"a","method":"GET","requests":20,"failed":0,"errorRate":0,"rps":4,"latency":{"p50":1,"p75":1,"p90":2,"p95":2,"p99":3,"p999":3,"min":0.5,"max":3,"mean":1.2},"statusCodes":{"200":20}}],
          "bucketSeconds":1,"timeline":[{"t":0,"requests":4,"failed":0,"rps":4,"p50":1,"p95":2,"p99":3,"max":3,"vus":1,"dropped":0}],"warnings":[],
          "regions":[{"region":"weur","name":"Western Europe","colo":"AMS","share":0.5,"ok":true,"error":null,"requests":10,"failed":0,"droppedIterations":0,"p50":1,"p95":2},
                     {"region":"enam","name":"Eastern North America","colo":null,"share":0.5,"ok":true,"error":null,"requests":10,"failed":0,"droppedIterations":0,"p50":1,"p95":2}],
          "histogram":{"buckets":[[1000,20]],"count":20,"min":500,"max":3000,"sum":24000}}"#;
        let r: CloudResult = serde_json::from_str(json).unwrap();
        assert_eq!(r.totals.requests, 20);
        assert_eq!(region_address(&r.regions[0]), "cloud:weur (AMS)");
        assert_eq!(region_address(&r.regions[1]), "cloud:enam");
    }
}
