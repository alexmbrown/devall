//! Semantic validation of a [`LoadTestFile`], reported per field so the editor can show issues
//! inline and the CLI can print them. Project-level checks (do the request files exist?) happen
//! when a run is planned.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::duration::DurationSpec;
use crate::model::{Check, Executor, FORMAT_VERSION, LoadTestFile, Stage, ThinkTime};
use crate::threshold::parse_threshold;

/// Hard cap on virtual users per test. Beyond this a single desktop machine mostly measures
/// itself, and HTTP/1.1 connection counts approach the ephemeral port range.
pub const MAX_VUS: u32 = 5_000;
/// Longest allowed test.
pub const MAX_DURATION: Duration = Duration::from_secs(24 * 3600);
/// Highest allowed iteration rate (per second).
pub const MAX_RATE_PER_SEC: f64 = 100_000.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// Dotted path of the offending field, e.g. `executor.stages[1].duration`.
    pub field: String,
    pub message: String,
}

impl Issue {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self { field: field.into(), message: message.into() }
    }
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.field.is_empty() { write!(f, "{}", self.message) } else { write!(f, "{}: {}", self.field, self.message) }
    }
}

pub fn validate(test: &LoadTestFile) -> Vec<Issue> {
    let mut out = Vec::new();
    if test.version > FORMAT_VERSION {
        out.push(Issue::new("version", format!("version {} is newer than this app understands ({FORMAT_VERSION})", test.version)));
    }
    if test.name.trim().is_empty() {
        out.push(Issue::new("name", "name is required"));
    }
    if test.environment.as_deref().is_some_and(|e| e.contains(['/', '\\'])) {
        out.push(Issue::new("environment", "use the environment id (file stem), not a path"));
    }

    // ----- scenario -----
    let sc = &test.scenario;
    match (sc.requests.is_empty(), sc.flow.is_empty()) {
        (true, true) => out.push(Issue::new("scenario", "add at least one request (a weighted `requests` mix or an ordered `flow`)")),
        (false, false) => out.push(Issue::new("scenario", "use either `requests` or `flow`, not both")),
        _ => {}
    }
    for (i, r) in sc.requests.iter().enumerate() {
        check_request_path(&mut out, &format!("scenario.requests[{i}].path"), &r.path);
        if r.weight == 0 {
            out.push(Issue::new(format!("scenario.requests[{i}].weight"), "weight must be at least 1"));
        }
    }
    for (i, p) in sc.flow.iter().enumerate() {
        check_request_path(&mut out, &format!("scenario.flow[{i}]"), p);
    }

    // ----- executor -----
    let ex = &test.executor;
    match ex {
        Executor::ConstantVus { vus, duration } => {
            check_vus(&mut out, "executor.vus", *vus, 1);
            check_duration(&mut out, "executor.duration", duration, true);
        }
        Executor::RampingVus { start_vus, stages } => {
            check_vus(&mut out, "executor.startVus", *start_vus, 0);
            check_stages(&mut out, stages, true);
            if *start_vus == 0 && stages.iter().all(|s| s.target <= 0.0) {
                out.push(Issue::new("executor.stages", "every stage targets 0 VUs, so nothing would run"));
            }
        }
        Executor::ConstantArrivalRate { rate, time_unit, duration, pre_allocated_vus, max_vus } => {
            let unit_ok = check_duration(&mut out, "executor.timeUnit", time_unit, true);
            check_rate(&mut out, "executor.rate", *rate, time_unit, unit_ok, false);
            check_duration(&mut out, "executor.duration", duration, true);
            check_pool(&mut out, *pre_allocated_vus, *max_vus);
        }
        Executor::RampingArrivalRate { start_rate, time_unit, stages, pre_allocated_vus, max_vus } => {
            let unit_ok = check_duration(&mut out, "executor.timeUnit", time_unit, true);
            check_rate(&mut out, "executor.startRate", *start_rate, time_unit, unit_ok, true);
            check_stages(&mut out, stages, false);
            if unit_ok {
                for (i, s) in stages.iter().enumerate() {
                    check_rate(&mut out, &format!("executor.stages[{i}].target"), s.target, time_unit, true, true);
                }
            }
            if *start_rate <= 0.0 && stages.iter().all(|s| s.target <= 0.0) {
                out.push(Issue::new("executor.stages", "every stage targets a rate of 0, so nothing would run"));
            }
            check_pool(&mut out, *pre_allocated_vus, *max_vus);
        }
    }
    if ex.planned_duration() > MAX_DURATION {
        out.push(Issue::new("executor", "tests are limited to 24 hours"));
    }

    // ----- think time -----
    if let Some(tt) = &test.think_time {
        match tt {
            ThinkTime::Fixed(d) => {
                check_duration(&mut out, "thinkTime", d, false);
            }
            ThinkTime::Range { min, max } => {
                let a = check_duration(&mut out, "thinkTime.min", min, false);
                let b = check_duration(&mut out, "thinkTime.max", max, false);
                if a && b && min.get() > max.get() {
                    out.push(Issue::new("thinkTime", "min must not exceed max"));
                }
            }
        }
    }

    // ----- thresholds & checks -----
    let paths = test.scenario.paths();
    for (i, t) in test.thresholds.iter().enumerate() {
        match parse_threshold(t) {
            Err(e) => out.push(Issue::new(format!("thresholds[{i}]"), e.0)),
            Ok(th) => {
                if let Some(scope) = &th.scope
                    && !paths.iter().any(|p| p == scope)
                {
                    out.push(Issue::new(format!("thresholds[{i}]"), format!("'{scope}' is not part of the scenario")));
                }
            }
        }
    }
    for (i, c) in test.checks.iter().enumerate() {
        match c {
            Check::Status { expect } => {
                if expect.is_empty() {
                    out.push(Issue::new(format!("checks[{i}].expect"), "list at least one status, e.g. 2xx or 200"));
                }
                for (j, p) in expect.iter().enumerate() {
                    if !valid_status_pattern(p) {
                        out.push(Issue::new(format!("checks[{i}].expect[{j}]"), format!("'{p}' is not a status pattern (use 200, 2xx or 20x)")));
                    }
                }
            }
            Check::MaxDuration { duration } => {
                check_duration(&mut out, &format!("checks[{i}].duration"), duration, true);
            }
            Check::BodyContains { text } => {
                if text.is_empty() {
                    out.push(Issue::new(format!("checks[{i}].text"), "text is required"));
                }
            }
        }
    }

    // ----- http -----
    if test.http.timeout_ms == Some(0) {
        out.push(Issue::new("http.timeoutMs", "timeout must be at least 1 ms"));
    }
    if test.http.max_connections == Some(0) {
        out.push(Issue::new("http.maxConnections", "must be at least 1"));
    }
    out
}

fn check_request_path(out: &mut Vec<Issue>, field: &str, path: &str) {
    if path.trim().is_empty() {
        out.push(Issue::new(field, "pick a request"));
    } else if !path.ends_with(devall_core::project::REQUEST_EXT) {
        out.push(Issue::new(field, format!("'{path}' is not a request file (*.req.yaml under http/)")));
    } else if path.split(['/', '\\']).any(|p| p == "..") {
        out.push(Issue::new(field, "paths cannot contain '..'"));
    }
}

fn check_vus(out: &mut Vec<Issue>, field: &str, vus: u32, min: u32) {
    if vus < min {
        out.push(Issue::new(field, format!("must be at least {min}")));
    } else if vus > MAX_VUS {
        out.push(Issue::new(field, format!("at most {MAX_VUS} VUs per test")));
    }
}

/// Returns whether the duration parsed.
fn check_duration(out: &mut Vec<Issue>, field: &str, d: &DurationSpec, positive: bool) -> bool {
    match d.parse() {
        Ok(v) if positive && v.is_zero() => {
            out.push(Issue::new(field, "must be greater than 0"));
            false
        }
        Ok(v) if v > MAX_DURATION => {
            out.push(Issue::new(field, "must be at most 24h"));
            false
        }
        Ok(_) => true,
        Err(e) => {
            out.push(Issue::new(field, e));
            false
        }
    }
}

fn check_rate(out: &mut Vec<Issue>, field: &str, rate: f64, unit: &DurationSpec, unit_ok: bool, allow_zero: bool) {
    if !rate.is_finite() || rate < 0.0 || (!allow_zero && rate == 0.0) {
        out.push(Issue::new(field, if allow_zero { "must be 0 or more" } else { "must be greater than 0" }));
        return;
    }
    if unit_ok && crate::model::per_sec(rate, unit) > MAX_RATE_PER_SEC {
        out.push(Issue::new(field, format!("at most {MAX_RATE_PER_SEC} iterations per second")));
    }
}

fn check_stages(out: &mut Vec<Issue>, stages: &[Stage], vus: bool) {
    if stages.is_empty() {
        out.push(Issue::new("executor.stages", "add at least one stage"));
    }
    for (i, s) in stages.iter().enumerate() {
        // Zero-length stages are allowed (instant jumps) as long as the total is positive.
        check_duration(out, &format!("executor.stages[{i}].duration"), &s.duration, false);
        if vus {
            if !s.target.is_finite() || s.target < 0.0 || s.target.fract() != 0.0 {
                out.push(Issue::new(format!("executor.stages[{i}].target"), "VU targets are whole numbers ≥ 0"));
            } else if s.target > MAX_VUS as f64 {
                out.push(Issue::new(format!("executor.stages[{i}].target"), format!("at most {MAX_VUS} VUs per test")));
            }
        }
    }
    if !stages.is_empty() && stages.iter().all(|s| s.duration.parse().is_ok()) && stages.iter().map(|s| s.duration.get()).sum::<Duration>().is_zero() {
        out.push(Issue::new("executor.stages", "stages must add up to more than 0s"));
    }
}

fn check_pool(out: &mut Vec<Issue>, pre: u32, max: Option<u32>) {
    check_vus(out, "executor.preAllocatedVus", pre, 1);
    if let Some(m) = max {
        if m < pre {
            out.push(Issue::new("executor.maxVus", "must be at least preAllocatedVus"));
        } else if m > MAX_VUS {
            out.push(Issue::new("executor.maxVus", format!("at most {MAX_VUS} VUs per test")));
        }
    }
}

/// `200`, `2xx`, `20x` (case-insensitive `x`).
pub fn valid_status_pattern(p: &str) -> bool {
    let p = p.trim();
    p.len() == 3 && p.starts_with(['1', '2', '3', '4', '5']) && p.chars().all(|c| c.is_ascii_digit() || c == 'x' || c == 'X')
}

pub fn status_matches(pattern: &str, status: u16) -> bool {
    let s = status.to_string();
    let p = pattern.trim();
    p.len() == 3 && s.len() == 3 && p.chars().zip(s.chars()).all(|(a, b)| a == 'x' || a == 'X' || a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn base() -> LoadTestFile {
        LoadTestFile::template("Smoke", Some("users/list.req.yaml".into()))
    }

    fn fields(t: &LoadTestFile) -> Vec<String> {
        validate(t).into_iter().map(|i| i.field).collect()
    }

    #[test]
    fn template_is_valid() {
        assert!(validate(&base()).is_empty(), "{:?}", validate(&base()));
    }

    #[test]
    fn reports_field_paths() {
        let mut t = base();
        t.name = " ".into();
        t.scenario.flow.push("a.req.yaml".into());
        t.scenario.requests.push(WeightedRequest { path: "x.json".into(), weight: 0 });
        t.thresholds.push("p95 < 3 parsecs".into());
        t.think_time = Some(ThinkTime::Range { min: "2s".into(), max: "1s".into() });
        t.executor = Executor::RampingVus { start_vus: 0, stages: vec![Stage { duration: "10q".into(), target: 6000.0 }] };
        t.checks.push(Check::Status { expect: vec!["2xx".into(), "abc".into()] });
        let f = fields(&t);
        for want in [
            "name",
            "scenario",
            "scenario.requests[1].path",
            "scenario.requests[1].weight",
            "thresholds[2]",
            "thinkTime",
            "executor.stages[0].duration",
            "executor.stages[0].target",
            "checks[0].expect[1]",
        ] {
            assert!(f.iter().any(|x| x == want), "missing {want} in {f:?}");
        }
    }

    #[test]
    fn caps_vus_and_pool() {
        let mut t = base();
        t.executor = Executor::ConstantVus { vus: MAX_VUS + 1, duration: "1m".into() };
        assert_eq!(fields(&t), vec!["executor.vus"]);
        t.executor = Executor::ConstantArrivalRate { rate: 10.0, time_unit: "1s".into(), duration: "1m".into(), pre_allocated_vus: 20, max_vus: Some(10) };
        assert_eq!(fields(&t), vec!["executor.maxVus"]);
        t.executor = Executor::RampingArrivalRate { start_rate: 0.0, time_unit: "0s".into(), stages: vec![], pre_allocated_vus: 1, max_vus: None };
        let f = fields(&t);
        assert!(f.contains(&"executor.timeUnit".to_string()) && f.contains(&"executor.stages".to_string()), "{f:?}");
    }

    #[test]
    fn scoped_threshold_must_reference_scenario() {
        let mut t = base();
        t.thresholds = vec!["p95{users/list.req.yaml} < 1s".into(), "p95{other.req.yaml} < 1s".into()];
        assert_eq!(fields(&t), vec!["thresholds[1]"]);
    }

    #[test]
    fn status_patterns() {
        assert!(status_matches("2xx", 204));
        assert!(status_matches("20x", 201));
        assert!(!status_matches("20x", 210));
        assert!(status_matches("404", 404));
        assert!(!valid_status_pattern("2x"));
        assert!(valid_status_pattern("5XX"));
    }
}
