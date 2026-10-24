//! Threshold expressions: `<metric>[{<request path>}] <op> <value>[unit]`.
//!
//! | Metric | Meaning | Units |
//! |---|---|---|
//! | `p50` `p90` `p95` `p99` `p99.9` `p(97.5)` `med` | latency percentile | `us` `ms` (default) `s` `m` |
//! | `min` `max` `avg`/`mean` | latency | same |
//! | `rps` | completed requests per second | `/s` (default) `/m` |
//! | `errorRate` | failed requests (transport error or status ≥ 400) ÷ requests | `%` or a fraction |
//! | `checks` | passed checks ÷ evaluated checks | `%` or a fraction |
//! | `requests` `errors` `iterations` `dropped` | counts | — |
//!
//! Operators: `<` `<=` `>` `>=` `==` `!=`. A `{request path}` scope (e.g. `p95{users/list.req.yaml}`)
//! is allowed on latency, `rps`, `errorRate`, `requests` and `errors`.

use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Metric {
    /// Latency quantile in 0..=1 (`p95` → 0.95).
    Quantile(f64),
    Min,
    Max,
    Mean,
    Rps,
    ErrorRate,
    Checks,
    Requests,
    Errors,
    Iterations,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// Milliseconds.
    Time,
    /// Fraction 0..=1 (shown as %).
    Ratio,
    /// Per second.
    Rate,
    Count,
}

impl Metric {
    pub fn kind(&self) -> MetricKind {
        match self {
            Metric::Quantile(_) | Metric::Min | Metric::Max | Metric::Mean => MetricKind::Time,
            Metric::Rps => MetricKind::Rate,
            Metric::ErrorRate | Metric::Checks => MetricKind::Ratio,
            Metric::Requests | Metric::Errors | Metric::Iterations | Metric::Dropped => MetricKind::Count,
        }
    }

    fn scopable(&self) -> bool {
        !matches!(self, Metric::Checks | Metric::Iterations | Metric::Dropped)
    }

    pub fn name(&self) -> String {
        match self {
            Metric::Quantile(q) => format!("p{}", trim(q * 100.0)),
            Metric::Min => "min".into(),
            Metric::Max => "max".into(),
            Metric::Mean => "avg".into(),
            Metric::Rps => "rps".into(),
            Metric::ErrorRate => "errorRate".into(),
            Metric::Checks => "checks".into(),
            Metric::Requests => "requests".into(),
            Metric::Errors => "errors".into(),
            Metric::Iterations => "iterations".into(),
            Metric::Dropped => "dropped".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Op {
    pub fn holds(self, actual: f64, limit: f64) -> bool {
        const EPS: f64 = 1e-9;
        match self {
            Op::Lt => actual < limit,
            Op::Le => actual <= limit + EPS,
            Op::Gt => actual > limit,
            Op::Ge => actual + EPS >= limit,
            Op::Eq => (actual - limit).abs() <= EPS,
            Op::Ne => (actual - limit).abs() > EPS,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Eq => "==",
            Op::Ne => "!=",
        }
    }
}

/// A parsed threshold. `value` is in canonical units: ms, fraction, per second, or a count.
#[derive(Debug, Clone, PartialEq)]
pub struct Threshold {
    pub expr: String,
    pub metric: Metric,
    pub scope: Option<String>,
    pub op: Op,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThresholdParseError(pub String);

impl fmt::Display for ThresholdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ThresholdParseError {}

pub fn parse_threshold(expr: &str) -> Result<Threshold, ThresholdParseError> {
    let fail = |msg: String| ThresholdParseError(format!("'{}': {msg}", expr.trim()));
    let s = expr.trim();
    if s.is_empty() {
        return Err(fail("empty threshold".into()));
    }
    // Split at the operator (outside any {scope}).
    let mut depth = 0i32;
    let mut op_at = None;
    for (i, c) in s.char_indices() {
        match c {
            '{' | '(' => depth += 1,
            '}' | ')' => depth -= 1,
            '<' | '>' | '=' | '!' if depth == 0 => {
                op_at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(op_at) = op_at else {
        if depth > 0 {
            return Err(fail("unterminated {request} scope".into()));
        }
        return Err(fail("missing comparison (use <, <=, >, >=, == or !=)".into()));
    };
    let lhs = s[..op_at].trim();
    let rest = &s[op_at..];
    let (op, op_len) = if rest.starts_with("<=") {
        (Op::Le, 2)
    } else if rest.starts_with(">=") {
        (Op::Ge, 2)
    } else if rest.starts_with("==") {
        (Op::Eq, 2)
    } else if rest.starts_with("!=") {
        (Op::Ne, 2)
    } else if rest.starts_with('<') {
        (Op::Lt, 1)
    } else if rest.starts_with('>') {
        (Op::Gt, 1)
    } else if rest.starts_with('=') {
        (Op::Eq, 1)
    } else {
        return Err(fail("bad comparison operator".into()));
    };
    let rhs = rest[op_len..].trim();

    let (name, scope) = match lhs.find('{') {
        Some(i) => {
            let Some(inner) = lhs[i + 1..].strip_suffix('}') else {
                return Err(fail("unterminated {request} scope".into()));
            };
            let inner = inner.trim();
            if inner.is_empty() {
                return Err(fail("empty {request} scope".into()));
            }
            (lhs[..i].trim(), Some(inner.replace('\\', "/")))
        }
        None => (lhs, None),
    };
    if name.is_empty() {
        return Err(fail("missing metric".into()));
    }
    let metric = parse_metric(name).ok_or_else(|| {
        fail(format!("unknown metric '{name}' (use p50…p99.9, p(N), med, min, max, avg, rps, errorRate, checks, requests, errors, iterations, dropped)"))
    })?;
    if scope.is_some() && !metric.scopable() {
        return Err(fail(format!("'{}' cannot be scoped to a request", metric.name())));
    }
    let value = parse_value(rhs, metric.kind()).map_err(fail)?;
    Ok(Threshold { expr: s.to_string(), metric, scope, op, value })
}

fn parse_metric(name: &str) -> Option<Metric> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "med" | "median" => Metric::Quantile(0.5),
        "min" => Metric::Min,
        "max" => Metric::Max,
        "avg" | "mean" => Metric::Mean,
        "rps" | "reqrate" => Metric::Rps,
        "errorrate" | "error_rate" | "failrate" => Metric::ErrorRate,
        "checks" => Metric::Checks,
        "requests" | "reqs" => Metric::Requests,
        "errors" | "failed" => Metric::Errors,
        "iterations" => Metric::Iterations,
        "dropped" | "droppediterations" => Metric::Dropped,
        _ => {
            let p = lower.strip_prefix('p')?;
            let p = p.strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(p);
            let pct: f64 = match p {
                "999" => 99.9,
                "9999" => 99.99,
                _ => p.parse().ok()?,
            };
            if !(pct > 0.0 && pct <= 100.0) {
                return None;
            }
            // Rounded so `p99.9` is exactly 0.999 (and matches stored percentiles).
            Metric::Quantile((pct * 10_000.0).round() / 1_000_000.0)
        }
    })
}

fn parse_value(text: &str, kind: MetricKind) -> Result<f64, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("missing value".into());
    }
    let num_len = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(t.len());
    let n: f64 = t[..num_len].parse().map_err(|_| format!("'{t}' is not a number"))?;
    if !n.is_finite() || n < 0.0 {
        return Err(format!("'{t}' must be a non-negative number"));
    }
    let unit = t[num_len..].trim();
    match kind {
        MetricKind::Time => match unit {
            "" | "ms" => Ok(n),
            "us" | "µs" => Ok(n / 1000.0),
            "s" => Ok(n * 1000.0),
            "m" | "min" => Ok(n * 60_000.0),
            other => Err(format!("unknown time unit '{other}' (use us, ms, s, m)")),
        },
        MetricKind::Ratio => match unit {
            "%" => Ok(n / 100.0),
            "" if n <= 1.0 => Ok(n),
            "" => Err(format!("use a percentage like {n}% or a fraction like {}", trim(n / 100.0))),
            other => Err(format!("unknown unit '{other}' (use %)")),
        },
        MetricKind::Rate => match unit {
            "" | "/s" => Ok(n),
            "/m" | "/min" => Ok(n / 60.0),
            other => Err(format!("unknown rate unit '{other}' (use /s or /m)")),
        },
        MetricKind::Count => match unit {
            "" => Ok(n),
            other => Err(format!("counts take no unit (got '{other}')")),
        },
    }
}

/// Anything thresholds can be evaluated against (the live recorder or a stored summary).
pub trait MetricSource {
    /// Value in canonical units (ms, fraction, per second, count); `None` when there is no data
    /// (no samples, unknown request scope, or a percentile a stored summary does not keep).
    fn metric(&self, metric: Metric, scope: Option<&str>) -> Option<f64>;
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdResult {
    pub expr: String,
    pub ok: bool,
    /// Measured value in display units (`unit`); `None` when it could not be measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<f64>,
    /// Limit in display units.
    pub limit: f64,
    /// `ms`, `%`, `/s` or empty for counts.
    pub unit: String,
    /// One-line explanation, e.g. `p95 = 312.4ms (want < 300ms)`.
    pub message: String,
}

impl Threshold {
    pub fn display_unit(&self) -> &'static str {
        match self.metric.kind() {
            MetricKind::Time => "ms",
            MetricKind::Ratio => "%",
            MetricKind::Rate => "/s",
            MetricKind::Count => "",
        }
    }

    fn to_display(&self, v: f64) -> f64 {
        let v = if self.metric.kind() == MetricKind::Ratio { v * 100.0 } else { v };
        (v * 100.0).round() / 100.0
    }

    pub fn evaluate(&self, src: &dyn MetricSource) -> ThresholdResult {
        let label = match &self.scope {
            Some(s) => format!("{}{{{s}}}", self.metric.name()),
            None => self.metric.name(),
        };
        let unit = self.display_unit();
        let limit = self.to_display(self.value);
        let want = format!("want {} {}{unit}", self.op.symbol(), trim(limit));
        match src.metric(self.metric, self.scope.as_deref()) {
            Some(actual) => {
                let ok = self.op.holds(actual, self.value);
                let shown = self.to_display(actual);
                ThresholdResult {
                    expr: self.expr.clone(),
                    ok,
                    actual: Some(shown),
                    limit,
                    unit: unit.into(),
                    message: format!("{label} = {}{unit} ({want})", trim(shown)),
                }
            }
            None => ThresholdResult {
                expr: self.expr.clone(),
                ok: false,
                actual: None,
                limit,
                unit: unit.into(),
                message: format!("{label}: no data ({want})"),
            },
        }
    }
}

/// Parses every expression; parse errors become failed results so nothing is silently ignored.
pub fn evaluate_all(exprs: &[String], src: &dyn MetricSource) -> Vec<ThresholdResult> {
    exprs
        .iter()
        .map(|e| match parse_threshold(e) {
            Ok(t) => t.evaluate(src),
            Err(err) => ThresholdResult { expr: e.clone(), ok: false, actual: None, limit: 0.0, unit: String::new(), message: err.0 },
        })
        .collect()
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_examples() {
        let t = parse_threshold("p95 < 300ms").unwrap();
        assert_eq!((t.metric, t.op, t.value), (Metric::Quantile(0.95), Op::Lt, 300.0));
        let t = parse_threshold("p99 < 1s").unwrap();
        assert_eq!(t.value, 1000.0);
        let t = parse_threshold("errorRate < 1%").unwrap();
        assert_eq!((t.metric, t.value), (Metric::ErrorRate, 0.01));
        let t = parse_threshold("rps > 50").unwrap();
        assert_eq!((t.metric, t.op, t.value), (Metric::Rps, Op::Gt, 50.0));
        let t = parse_threshold("checks > 99%").unwrap();
        assert_eq!((t.metric, t.value), (Metric::Checks, 0.99));
    }

    #[test]
    fn parses_variants() {
        assert_eq!(parse_threshold("p99.9<=2s").unwrap().metric, Metric::Quantile(0.999));
        assert_eq!(parse_threshold("p(97.5) < 10").unwrap().metric, Metric::Quantile(0.975));
        assert_eq!(parse_threshold("p999 < 10").unwrap().metric, Metric::Quantile(0.999));
        assert_eq!(parse_threshold("med < 10").unwrap().metric, Metric::Quantile(0.5));
        assert_eq!(parse_threshold("avg < 250us").unwrap().value, 0.25);
        assert_eq!(parse_threshold("errorRate <= 0.05").unwrap().value, 0.05);
        assert_eq!(parse_threshold("rps >= 600/m").unwrap().value, 10.0);
        assert_eq!(parse_threshold("dropped == 0").unwrap().op, Op::Eq);
        let t = parse_threshold("p95{users/list.req.yaml} < 300ms").unwrap();
        assert_eq!(t.scope.as_deref(), Some("users/list.req.yaml"));
        assert_eq!(t.metric, Metric::Quantile(0.95));
    }

    #[test]
    fn rejects_with_helpful_messages() {
        let cases = [
            ("p95 300ms", "missing comparison"),
            ("p95 < 300xs", "unknown time unit"),
            ("latency < 3", "unknown metric"),
            ("errorRate < 5", "percentage"),
            ("requests > 5ms", "no unit"),
            ("checks{a.req.yaml} > 99%", "cannot be scoped"),
            ("p95{a.req.yaml < 3", "unterminated"),
            ("p95 <", "missing value"),
            ("p101 < 3", "unknown metric"),
            ("", "empty"),
        ];
        for (expr, needle) in cases {
            let err = parse_threshold(expr).unwrap_err().0;
            assert!(err.contains(needle), "{expr}: {err}");
        }
    }

    struct Fixed;
    impl MetricSource for Fixed {
        fn metric(&self, m: Metric, scope: Option<&str>) -> Option<f64> {
            match (m, scope) {
                (Metric::Quantile(q), None) if (q - 0.95).abs() < 1e-9 => Some(312.4),
                (Metric::Quantile(_), Some("a.req.yaml")) => Some(10.0),
                (Metric::ErrorRate, None) => Some(0.004),
                (Metric::Rps, None) => Some(49.5),
                _ => None,
            }
        }
    }

    #[test]
    fn evaluates_with_readable_messages() {
        let r = parse_threshold("p95 < 300ms").unwrap().evaluate(&Fixed);
        assert!(!r.ok);
        assert_eq!(r.message, "p95 = 312.4ms (want < 300ms)");
        let r = parse_threshold("errorRate < 1%").unwrap().evaluate(&Fixed);
        assert!(r.ok, "{r:?}");
        assert_eq!(r.actual, Some(0.4));
        assert_eq!(r.unit, "%");
        assert!(!parse_threshold("rps > 50").unwrap().evaluate(&Fixed).ok);
        assert!(parse_threshold("p99{a.req.yaml} < 20").unwrap().evaluate(&Fixed).ok);
        let r = parse_threshold("p99{b.req.yaml} < 20").unwrap().evaluate(&Fixed);
        assert!(!r.ok && r.message.contains("no data"), "{r:?}");
        let all = evaluate_all(&["p95 < 1s".into(), "nonsense".into()], &Fixed);
        assert!(all[0].ok && !all[1].ok);
    }
}
