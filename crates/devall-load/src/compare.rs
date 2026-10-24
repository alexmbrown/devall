//! Run-over-run comparison: deltas of the key metrics with regression flags.
//!
//! A metric regresses when it moves in the bad direction by more than `tolerance` (relative,
//! default 10%) *and* by more than a small absolute floor (1 ms, 0.5 percentage points, 1/s), so
//! sub-millisecond jitter on fast endpoints never shows up as a regression.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::summary::{LoadSummary, Percentiles};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Better {
    Lower,
    Higher,
    /// Informational (e.g. request counts).
    Neither,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Regression,
    Improvement,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricDelta {
    pub key: String,
    pub label: String,
    /// `ms`, `%`, `/s` or empty.
    pub unit: String,
    pub base: f64,
    pub target: f64,
    pub delta: f64,
    /// Relative change in percent; `None` when the base is 0.
    pub delta_pct: Option<f64>,
    pub better: Better,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestDelta {
    pub path: String,
    pub name: String,
    pub metrics: Vec<MetricDelta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdDelta {
    pub expr: String,
    pub base_ok: Option<bool>,
    pub target_ok: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunRef {
    pub run_id: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mock_baseline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub aborted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub base: RunRef,
    pub target: RunRef,
    pub metrics: Vec<MetricDelta>,
    pub requests: Vec<RequestDelta>,
    pub thresholds: Vec<ThresholdDelta>,
    pub regressions: u32,
    pub improvements: u32,
    /// Reasons the comparison may be apples-to-oranges.
    pub warnings: Vec<String>,
    /// Present when exactly one run is a mock baseline: where the real run's time goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineBreakdown>,
}

/// A real run against a mock baseline of the same test: the mock run's latency is what the
/// client, the load generator and the local network stack cost; the rest of the real run's
/// latency is the server (plus the network path to it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BaselineBreakdown {
    /// `base` or `target`: which run is the mock baseline.
    pub mock_side: String,
    pub rows: Vec<BreakdownRow>,
    /// Per request (p95), when both runs have it.
    pub requests: Vec<RequestBreakdown>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    pub key: String,
    pub label: String,
    /// Real run latency (ms).
    pub real: f64,
    /// Mock baseline latency (ms): client + tooling overhead.
    pub mock: f64,
    /// `real − mock`, at least 0: server + network (ms).
    pub server: f64,
    /// `mock / real` (0..=1): the share of the real latency that is client-side.
    pub client_share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestBreakdown {
    pub path: String,
    pub name: String,
    pub real_p95: f64,
    pub mock_p95: f64,
    pub server_p95: f64,
}

fn breakdown_row(key: &str, label: &str, real: f64, mock: f64) -> BreakdownRow {
    BreakdownRow {
        key: key.into(),
        label: label.into(),
        real: r2(real),
        mock: r2(mock),
        server: r2((real - mock).max(0.0)),
        client_share: if real > 0.0 { r2((mock / real).clamp(0.0, 1.0) * 100.0) / 100.0 } else { 0.0 },
    }
}

/// The breakdown for a real run against a mock baseline (`None` unless exactly one is a mock).
pub fn baseline_breakdown(base: &LoadSummary, target: &LoadSummary) -> Option<BaselineBreakdown> {
    let (mock, real, side) = match (base.mock_baseline.is_some(), target.mock_baseline.is_some()) {
        (true, false) => (base, target, "base"),
        (false, true) => (target, base, "target"),
        _ => return None,
    };
    let (m, r) = (&mock.latency, &real.latency);
    let rows = vec![
        breakdown_row("p50", "p50", r.p50, m.p50),
        breakdown_row("p95", "p95", r.p95, m.p95),
        breakdown_row("p99", "p99", r.p99, m.p99),
        breakdown_row("mean", "mean", r.mean, m.mean),
    ];
    let requests = real
        .requests
        .iter()
        .filter_map(|rr| {
            let mr = mock.requests.iter().find(|x| x.path == rr.path)?;
            Some(RequestBreakdown { path: rr.path.clone(), name: rr.name.clone(), real_p95: r2(rr.latency.p95), mock_p95: r2(mr.latency.p95), server_p95: r2((rr.latency.p95 - mr.latency.p95).max(0.0)) })
        })
        .collect();
    Some(BaselineBreakdown { mock_side: side.into(), rows, requests })
}

#[derive(Debug, Clone, Copy)]
pub struct CompareOptions {
    /// Relative change (0.10 = 10%) before a metric counts as changed.
    pub tolerance: f64,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self { tolerance: 0.10 }
    }
}

fn delta(key: &str, label: &str, unit: &str, base: f64, target: f64, better: Better, opts: CompareOptions) -> MetricDelta {
    let d = target - base;
    let floor = match unit {
        "ms" => 1.0,
        "%" => 0.5,
        "/s" => 1.0,
        _ => 0.0,
    };
    let rel = if base.abs() > f64::EPSILON { d / base } else if d.abs() > 0.0 { f64::INFINITY * d.signum() } else { 0.0 };
    let changed = d.abs() > floor && rel.abs() > opts.tolerance;
    let verdict = match better {
        _ if !changed => Verdict::Unchanged,
        Better::Neither => Verdict::Unchanged,
        Better::Lower if d > 0.0 => Verdict::Regression,
        Better::Higher if d < 0.0 => Verdict::Regression,
        _ => Verdict::Improvement,
    };
    MetricDelta {
        key: key.into(),
        label: label.into(),
        unit: unit.into(),
        base: r2(base),
        target: r2(target),
        delta: r2(d),
        delta_pct: (base.abs() > f64::EPSILON).then(|| r2(d / base * 100.0)),
        better,
        verdict,
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn latency_deltas(a: &Percentiles, b: &Percentiles, opts: CompareOptions) -> Vec<MetricDelta> {
    vec![
        delta("p50", "p50", "ms", a.p50, b.p50, Better::Lower, opts),
        delta("p90", "p90", "ms", a.p90, b.p90, Better::Lower, opts),
        delta("p95", "p95", "ms", a.p95, b.p95, Better::Lower, opts),
        delta("p99", "p99", "ms", a.p99, b.p99, Better::Lower, opts),
        delta("p999", "p99.9", "ms", a.p999, b.p999, Better::Lower, opts),
        delta("max", "max", "ms", a.max, b.max, Better::Lower, opts),
        delta("mean", "mean", "ms", a.mean, b.mean, Better::Lower, opts),
    ]
}

fn run_ref(s: &LoadSummary) -> RunRef {
    RunRef {
        run_id: s.run_id.clone(),
        started_at: s.started_at.clone(),
        environment: s.environment.clone(),
        mock_baseline: s.mock_baseline.is_some(),
        commit: s.git.as_ref().map(|g| g.commit.clone()),
        aborted: s.aborted,
    }
}

/// Compares `target` against `base` (usually: newer run vs. an older baseline).
pub fn compare(base: &LoadSummary, target: &LoadSummary, opts: CompareOptions) -> Comparison {
    let (a, b) = (&base.totals, &target.totals);
    let mut metrics = latency_deltas(&base.latency, &target.latency, opts);
    metrics.push(delta("rps", "Throughput", "/s", a.rps, b.rps, Better::Higher, opts));
    metrics.push(delta("errorRate", "Error rate", "%", a.error_rate * 100.0, b.error_rate * 100.0, Better::Lower, opts));
    metrics.push(delta("checks", "Checks passed", "%", a.checks_rate * 100.0, b.checks_rate * 100.0, Better::Higher, opts));
    metrics.push(delta("dropped", "Dropped iterations", "", a.dropped_iterations as f64, b.dropped_iterations as f64, Better::Lower, opts));
    metrics.push(delta("requests", "Requests", "", a.requests as f64, b.requests as f64, Better::Neither, opts));

    let requests = target
        .requests
        .iter()
        .filter_map(|tr| {
            let br = base.requests.iter().find(|r| r.path == tr.path)?;
            let mut m = latency_deltas(&br.latency, &tr.latency, opts);
            m.retain(|d| ["p50", "p95", "p99"].contains(&d.key.as_str()));
            m.push(delta("rps", "Throughput", "/s", br.rps, tr.rps, Better::Higher, opts));
            m.push(delta("errorRate", "Error rate", "%", br.error_rate * 100.0, tr.error_rate * 100.0, Better::Lower, opts));
            Some(RequestDelta { path: tr.path.clone(), name: tr.name.clone(), metrics: m })
        })
        .collect::<Vec<_>>();

    let mut exprs: Vec<&str> = base.thresholds.iter().map(|t| t.expr.as_str()).collect();
    for t in &target.thresholds {
        if !exprs.contains(&t.expr.as_str()) {
            exprs.push(&t.expr);
        }
    }
    let thresholds: Vec<ThresholdDelta> = exprs
        .iter()
        .map(|e| ThresholdDelta {
            expr: e.to_string(),
            base_ok: base.thresholds.iter().find(|t| t.expr == *e).map(|t| t.ok),
            target_ok: target.thresholds.iter().find(|t| t.expr == *e).map(|t| t.ok),
        })
        .collect();

    let mut regressions = metrics.iter().filter(|m| m.verdict == Verdict::Regression).count() as u32;
    regressions += requests.iter().flat_map(|r| &r.metrics).filter(|m| m.verdict == Verdict::Regression).count() as u32;
    regressions += thresholds.iter().filter(|t| t.base_ok == Some(true) && t.target_ok == Some(false)).count() as u32;
    let improvements = metrics.iter().filter(|m| m.verdict == Verdict::Improvement).count() as u32
        + requests.iter().flat_map(|r| &r.metrics).filter(|m| m.verdict == Verdict::Improvement).count() as u32;

    let baseline = baseline_breakdown(base, target);
    let mut warnings = Vec::new();
    if baseline.is_some() {
        warnings.push("One run is a mock baseline: the differences are what the real server and network add, not regressions.".to_string());
    } else if base.mock_baseline.is_some() && target.mock_baseline.is_some() {
        warnings.push("Both runs are mock baselines: this compares the client side only.".to_string());
    }
    if base.executor != target.executor {
        warnings.push("The executor settings differ between these runs.".to_string());
    }
    if base.environment != target.environment && baseline.is_none() {
        warnings.push(format!(
            "Different environments: {} vs {}.",
            base.environment.as_deref().unwrap_or("none"),
            target.environment.as_deref().unwrap_or("none")
        ));
    }
    if base.test_path != target.test_path {
        warnings.push("These runs belong to different tests.".to_string());
    }
    if base.aborted || target.aborted {
        warnings.push("At least one run was stopped early.".to_string());
    }
    if base.machine != target.machine {
        warnings.push("The runs come from different machines; expect some variance.".to_string());
    }

    let (mut metrics, mut requests) = (metrics, requests);
    let (mut regressions, mut improvements) = (regressions, improvements);
    if baseline.is_some() {
        // Real vs mock differs by design; no verdicts.
        for m in metrics.iter_mut().chain(requests.iter_mut().flat_map(|r| r.metrics.iter_mut())) {
            m.verdict = Verdict::Unchanged;
        }
        regressions = 0;
        improvements = 0;
    }
    Comparison { base: run_ref(base), target: run_ref(target), metrics, requests, thresholds, regressions, improvements, warnings, baseline }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floors_and_tolerance() {
        let o = CompareOptions::default();
        // +50% but only +0.5 ms: below the 1 ms floor.
        assert_eq!(delta("p95", "p95", "ms", 1.0, 1.5, Better::Lower, o).verdict, Verdict::Unchanged);
        // +5%: within tolerance.
        assert_eq!(delta("p95", "p95", "ms", 100.0, 105.0, Better::Lower, o).verdict, Verdict::Unchanged);
        assert_eq!(delta("p95", "p95", "ms", 100.0, 150.0, Better::Lower, o).verdict, Verdict::Regression);
        assert_eq!(delta("p95", "p95", "ms", 150.0, 100.0, Better::Lower, o).verdict, Verdict::Improvement);
        assert_eq!(delta("rps", "rps", "/s", 100.0, 50.0, Better::Higher, o).verdict, Verdict::Regression);
        assert_eq!(delta("err", "err", "%", 0.0, 2.0, Better::Lower, o).verdict, Verdict::Regression);
        let d = delta("err", "err", "%", 0.0, 2.0, Better::Lower, o);
        assert_eq!(d.delta_pct, None);
        assert_eq!(delta("n", "n", "", 10.0, 1000.0, Better::Neither, o).verdict, Verdict::Unchanged);
    }

    #[test]
    fn real_run_against_a_mock_baseline() {
        let text = r#"{"version":1,"runId":"r","test":"t","testPath":"t.load.yaml","startedAt":"2026-01-01T00:00:00Z","durationMs":1000,"plannedDurationMs":1000,
            "aborted":false,"thresholdsPassed":true,"executor":{"type":"constantVus","vus":1,"duration":"1s"},"openModel":false,
            "totals":{"requests":10,"failed":0,"errorRate":0,"iterations":10,"droppedIterations":0,"rps":10,"iterationsPerSec":10,"bytesIn":0,"bytesOut":0,"checksPassed":0,"checksFailed":0,"checksRate":1,"maxVus":1},
            "latency":{"p50":100,"p75":120,"p90":150,"p95":200,"p99":300,"p999":300,"min":50,"max":300,"mean":110},
            "statusCodes":{},"errorKinds":{"connect":0,"timeout":0,"other":0},
            "requests":[{"path":"a.req.yaml","name":"a","method":"GET","requests":10,"failed":0,"errorRate":0,"rps":10,"latency":{"p50":100,"p75":120,"p90":150,"p95":200,"p99":300,"p999":300,"min":50,"max":300,"mean":110},"statusCodes":{}}],
            "thresholds":[],"bucketSeconds":1,"timeline":[],"machine":{"os":"x","cpus":1}}"#;
        let real: LoadSummary = serde_json::from_str(text).unwrap();
        let mut mock = real.clone();
        mock.mock_baseline = Some(crate::summary::MockBaseline { url: "http://127.0.0.1:1".into(), replaced_hosts: vec!["api:443".into()] });
        mock.latency.p50 = 2.0;
        mock.latency.p95 = 4.0;
        mock.requests[0].latency.p95 = 5.0;
        let c = compare(&mock, &real, CompareOptions::default());
        let b = c.baseline.unwrap();
        assert_eq!(b.mock_side, "base");
        let p95 = b.rows.iter().find(|r| r.key == "p95").unwrap();
        assert_eq!((p95.real, p95.mock, p95.server, p95.client_share), (200.0, 4.0, 196.0, 0.02));
        assert_eq!(b.requests[0].server_p95, 195.0);
        assert_eq!(c.regressions, 0, "no verdicts against a mock");
        assert!(c.base.mock_baseline && !c.target.mock_baseline);
        assert!(c.warnings.iter().any(|w| w.contains("mock baseline")));
        assert!(compare(&real, &real, CompareOptions::default()).baseline.is_none());
    }
}
