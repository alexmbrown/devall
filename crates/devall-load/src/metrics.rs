//! Recording: HDR histograms (overall, per request, per timeline bucket) and counters.
//!
//! Memory is fixed up front: one 3-significant-digit histogram (~190 KB) for the overall latency,
//! one for the service time (open model), one per request, and a single 2-digit histogram for
//! the bucket in progress, which is folded into a compact [`TimelinePoint`] when the bucket ends.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine as _;
use hdrhistogram::Histogram;
use hdrhistogram::serialization::{Deserializer, Serializer, V2DeflateSerializer};
use serde::{Deserialize, Serialize};

use crate::summary::{ErrorKinds, Percentiles, RequestStats, TimelinePoint, Totals};
use crate::threshold::{Metric, MetricSource};

/// Highest latency tracked (values above saturate): one hour, in microseconds.
const MAX_US: u64 = 3_600_000_000;
const MAX_ERROR_SAMPLES: usize = 5;
/// Timeline buckets kept per run.
pub const MAX_POINTS: u64 = 600;

fn hist(sigfig: u8) -> Histogram<u64> {
    Histogram::new_with_bounds(1, MAX_US, sigfig).expect("valid histogram bounds")
}

fn us(d: Duration) -> u64 {
    (d.as_micros() as u64).clamp(1, MAX_US)
}

fn ms(v: f64) -> f64 {
    (v / 1000.0 * 100.0).round() / 100.0
}

pub fn percentiles(h: &Histogram<u64>) -> Percentiles {
    if h.is_empty() {
        return Percentiles::default();
    }
    let q = |x: f64| ms(h.value_at_quantile(x) as f64);
    Percentiles {
        p50: q(0.5),
        p75: q(0.75),
        p90: q(0.9),
        p95: q(0.95),
        p99: q(0.99),
        p999: q(0.999),
        min: ms(h.min() as f64),
        max: ms(h.max() as f64),
        mean: ms(h.mean()),
    }
}

/// Bucket width so a run of `planned` (plus a grace margin) fits in [`MAX_POINTS`] buckets.
pub fn bucket_seconds(planned: Duration) -> u32 {
    let secs = planned.as_secs() + 30;
    secs.div_ceil(MAX_POINTS).max(1) as u32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Connect,
    Timeout,
    Other,
}

/// One finished request.
#[derive(Debug, Clone)]
pub struct Sample {
    pub req: usize,
    /// From the intended start (open model) or the send (closed model). `None` when the request
    /// never got a response or a timeout (refused connection, invalid request) — recording those
    /// near-zero times would flatter the percentiles.
    pub latency: Option<Duration>,
    /// From the actual send.
    pub service: Option<Duration>,
    pub status: Option<u16>,
    pub error: Option<(ErrorKind, String)>,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub checks_passed: u32,
    pub checks_failed: u32,
}

impl Sample {
    pub fn failed(&self) -> bool {
        self.error.is_some() || self.status.is_some_and(|s| s >= 400)
    }
}

struct ReqAcc {
    latency: Histogram<u64>,
    requests: u64,
    failed: u64,
    statuses: BTreeMap<u16, u64>,
}

struct Window {
    idx: u64,
    hist: Histogram<u64>,
    requests: u64,
    failed: u64,
    dropped: u64,
    vus: u32,
}

pub struct Recorder {
    bucket: Duration,
    latency: Histogram<u64>,
    service: Option<Histogram<u64>>,
    reqs: Vec<ReqAcc>,
    window: Window,
    timeline: Vec<TimelinePoint>,
    /// Timeline points already handed out by [`Recorder::take_new_points`].
    sent: usize,
    pub totals: Totals,
    pub statuses: BTreeMap<u16, u64>,
    pub errors: ErrorKinds,
    pub error_samples: Vec<String>,
    last_vus: u32,
    /// Run length, set by [`Recorder::finish`] (used for rates).
    elapsed: Duration,
}

impl Recorder {
    pub fn new(requests: usize, bucket_seconds: u32, open_model: bool) -> Self {
        Self {
            bucket: Duration::from_secs(bucket_seconds.max(1) as u64),
            latency: hist(3),
            service: open_model.then(|| hist(3)),
            reqs: (0..requests).map(|_| ReqAcc { latency: hist(3), requests: 0, failed: 0, statuses: BTreeMap::new() }).collect(),
            window: Window { idx: 0, hist: hist(2), requests: 0, failed: 0, dropped: 0, vus: 0 },
            timeline: Vec::new(),
            sent: 0,
            totals: Totals::default(),
            statuses: BTreeMap::new(),
            errors: ErrorKinds::default(),
            error_samples: Vec::new(),
            last_vus: 0,
            elapsed: Duration::ZERO,
        }
    }

    pub fn bucket_seconds(&self) -> u32 {
        self.bucket.as_secs() as u32
    }

    pub fn record(&mut self, s: Sample, at: Duration) {
        self.roll(at);
        let failed = s.failed();
        let t = &mut self.totals;
        t.requests += 1;
        t.failed += failed as u64;
        t.bytes_in += s.bytes_in;
        t.bytes_out += s.bytes_out;
        t.checks_passed += s.checks_passed as u64;
        t.checks_failed += s.checks_failed as u64;
        self.window.requests += 1;
        self.window.failed += failed as u64;
        if let Some(l) = s.latency {
            let v = us(l);
            self.latency.saturating_record(v);
            self.window.hist.saturating_record(v);
            if let Some(r) = self.reqs.get_mut(s.req) {
                r.latency.saturating_record(v);
            }
        }
        if let (Some(h), Some(sv)) = (self.service.as_mut(), s.service) {
            h.saturating_record(us(sv));
        }
        if let Some(r) = self.reqs.get_mut(s.req) {
            r.requests += 1;
            r.failed += failed as u64;
            if let Some(code) = s.status {
                *r.statuses.entry(code).or_default() += 1;
            }
        }
        if let Some(code) = s.status {
            *self.statuses.entry(code).or_default() += 1;
        }
        if let Some((kind, msg)) = s.error {
            match kind {
                ErrorKind::Connect => self.errors.connect += 1,
                ErrorKind::Timeout => self.errors.timeout += 1,
                ErrorKind::Other => self.errors.other += 1,
            }
            if self.error_samples.len() < MAX_ERROR_SAMPLES && !self.error_samples.contains(&msg) {
                self.error_samples.push(msg);
            }
        }
    }

    pub fn iteration_done(&mut self, at: Duration) {
        self.roll(at);
        self.totals.iterations += 1;
    }

    pub fn dropped(&mut self, at: Duration) {
        self.roll(at);
        self.totals.dropped_iterations += 1;
        self.window.dropped += 1;
    }

    pub fn observe_vus(&mut self, vus: u32, at: Duration) {
        self.roll(at);
        self.last_vus = vus;
        self.window.vus = self.window.vus.max(vus);
        self.totals.max_vus = self.totals.max_vus.max(vus);
    }

    /// Closes every bucket that ended at or before `at`.
    pub fn roll(&mut self, at: Duration) {
        let idx = (at.as_nanos() / self.bucket.as_nanos()) as u64;
        while self.window.idx < idx {
            self.close_window(self.bucket);
        }
    }

    fn close_window(&mut self, covered: Duration) {
        let w = &mut self.window;
        let secs = covered.as_secs_f64().max(1e-3);
        let q = |x: f64| if w.hist.is_empty() { 0.0 } else { ms(w.hist.value_at_quantile(x) as f64) };
        let point = TimelinePoint {
            t: (w.idx * self.bucket.as_secs()) as u32,
            requests: w.requests,
            failed: w.failed,
            rps: (w.requests as f64 / secs * 100.0).round() / 100.0,
            p50: q(0.5),
            p95: q(0.95),
            p99: q(0.99),
            max: if w.hist.is_empty() { 0.0 } else { ms(w.hist.max() as f64) },
            vus: w.vus.max(self.last_vus),
            dropped: w.dropped,
        };
        self.timeline.push(point);
        w.idx += 1;
        w.hist.reset();
        w.requests = 0;
        w.failed = 0;
        w.dropped = 0;
        w.vus = 0;
    }

    /// Closes the partial last bucket and fixes rates to the real run length.
    pub fn finish(&mut self, at: Duration) {
        self.roll(at);
        let start = self.bucket * self.window.idx as u32;
        let covered = at.saturating_sub(start);
        if self.window.requests > 0 || self.window.dropped > 0 || covered >= Duration::from_millis(500) {
            self.close_window(covered.max(Duration::from_millis(1)));
        }
        self.finish_rates(at);
    }

    /// Rates over a run of length `at` (no bucket rolling — merged recorders already hold their
    /// timeline).
    fn finish_rates(&mut self, at: Duration) {
        self.elapsed = at;
        let secs = at.as_secs_f64().max(1e-3);
        let t = &mut self.totals;
        t.error_rate = if t.requests > 0 { t.failed as f64 / t.requests as f64 } else { 0.0 };
        t.rps = round2(t.requests as f64 / secs);
        t.iterations_per_sec = round2(t.iterations as f64 / secs);
        let checked = t.checks_passed + t.checks_failed;
        t.checks_rate = if checked > 0 { t.checks_passed as f64 / checked as f64 } else { 1.0 };
    }

    /// Timeline buckets closed since the last call.
    pub fn take_new_points(&mut self) -> Vec<TimelinePoint> {
        let out = self.timeline[self.sent..].to_vec();
        self.sent = self.timeline.len();
        out
    }

    pub fn timeline(&self) -> &[TimelinePoint] {
        &self.timeline
    }

    pub fn latency(&self) -> Percentiles {
        percentiles(&self.latency)
    }

    pub fn service_time(&self) -> Option<Percentiles> {
        self.service.as_ref().map(percentiles)
    }

    /// Per-request stats; `meta` gives (path, name, method) for each request index.
    pub fn request_stats(&self, meta: &[(String, String, String)]) -> Vec<RequestStats> {
        let secs = self.elapsed.as_secs_f64().max(1e-3);
        self.reqs
            .iter()
            .zip(meta)
            .map(|(r, (path, name, method))| RequestStats {
                path: path.clone(),
                name: name.clone(),
                method: method.clone(),
                requests: r.requests,
                failed: r.failed,
                error_rate: if r.requests > 0 { r.failed as f64 / r.requests as f64 } else { 0.0 },
                rps: round2(r.requests as f64 / secs),
                latency: percentiles(&r.latency),
                status_codes: r.statuses.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            })
            .collect()
    }

    pub fn status_codes(&self) -> BTreeMap<String, u64> {
        self.statuses.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }
}

/// A histogram in the HdrHistogram V2 (deflate) format, base64 — what agents send.
pub fn encode_histogram(h: &Histogram<u64>) -> Result<String, String> {
    let mut buf = Vec::new();
    V2DeflateSerializer::new().serialize(h, &mut buf).map_err(|e| format!("histogram encoding: {e:?}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf))
}

pub fn decode_histogram(s: &str) -> Result<Histogram<u64>, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|e| format!("histogram base64: {e}"))?;
    Deserializer::new().deserialize(&mut bytes.as_slice()).map_err(|e| format!("histogram decoding: {e:?}"))
}

/// One request's measurements, exported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestExport {
    pub path: String,
    pub requests: u64,
    pub failed: u64,
    /// Status code → count (string keys: JSON object keys).
    pub statuses: BTreeMap<String, u64>,
    /// V2 histogram (µs).
    pub latency: String,
}

/// Everything a recorder measured, in a form that merges exactly: HDR histograms (V2 format)
/// are added, counters summed. Agents send it to the coordinator with their summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderExport {
    /// V2 histogram (µs) of all latencies.
    pub latency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    pub requests: Vec<RequestExport>,
    pub totals: Totals,
    pub statuses: BTreeMap<String, u64>,
    pub errors: ErrorKinds,
    pub error_samples: Vec<String>,
    pub timeline: Vec<TimelinePoint>,
    pub bucket_seconds: u32,
    pub elapsed_ms: u64,
}

/// Merges timeline points of the same bucket: counts and rates add up; the latency columns keep
/// the worst agent (p50 is request-weighted) — the exact percentiles are in the merged histograms.
pub fn merge_points(parts: &[&[TimelinePoint]]) -> Vec<TimelinePoint> {
    let mut by_t: BTreeMap<u32, (TimelinePoint, f64)> = BTreeMap::new();
    for pts in parts {
        for p in pts.iter() {
            let e = by_t.entry(p.t).or_insert_with(|| (TimelinePoint { t: p.t, ..Default::default() }, 0.0));
            let (m, weighted) = e;
            m.requests += p.requests;
            m.failed += p.failed;
            m.rps = round2(m.rps + p.rps);
            *weighted += p.p50 * p.requests as f64;
            m.p95 = m.p95.max(p.p95);
            m.p99 = m.p99.max(p.p99);
            m.max = m.max.max(p.max);
            m.vus += p.vus;
            m.dropped += p.dropped;
        }
    }
    by_t.into_values()
        .map(|(mut m, weighted)| {
            m.p50 = if m.requests > 0 { (weighted / m.requests as f64 * 100.0).round() / 100.0 } else { 0.0 };
            m
        })
        .collect()
}

impl Recorder {
    /// Exports the measurements (call after [`Recorder::finish`]). `paths` names each request.
    pub fn export(&self, paths: &[String]) -> Result<RecorderExport, String> {
        Ok(RecorderExport {
            latency: encode_histogram(&self.latency)?,
            service: self.service.as_ref().map(encode_histogram).transpose()?,
            requests: self
                .reqs
                .iter()
                .zip(paths)
                .map(|(r, path)| {
                    Ok(RequestExport {
                        path: path.clone(),
                        requests: r.requests,
                        failed: r.failed,
                        statuses: r.statuses.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
                        latency: encode_histogram(&r.latency)?,
                    })
                })
                .collect::<Result<_, String>>()?,
            totals: self.totals.clone(),
            statuses: self.status_codes(),
            errors: self.errors,
            error_samples: self.error_samples.clone(),
            timeline: self.timeline.clone(),
            bucket_seconds: self.bucket_seconds(),
            elapsed_ms: self.elapsed.as_millis() as u64,
        })
    }

    /// One recorder holding every part: histograms added (exact), counters summed, timelines
    /// merged by bucket; rates are over the longest part. `paths` orders the per-request stats.
    pub fn merge(parts: &[RecorderExport], paths: &[String], open_model: bool) -> Result<Recorder, String> {
        let bucket = parts.iter().map(|p| p.bucket_seconds).max().unwrap_or(1);
        let mut rec = Recorder::new(paths.len(), bucket, open_model);
        let add = |into: &mut Histogram<u64>, s: &str| -> Result<(), String> {
            let h = decode_histogram(s)?;
            into.add(&h).map_err(|e| format!("histogram merge: {e:?}"))
        };
        let mut elapsed = Duration::ZERO;
        for p in parts {
            add(&mut rec.latency, &p.latency)?;
            if let (Some(into), Some(s)) = (rec.service.as_mut(), &p.service) {
                add(into, s)?;
            }
            for r in &p.requests {
                if let Some(i) = paths.iter().position(|x| *x == r.path) {
                    let acc = &mut rec.reqs[i];
                    acc.requests += r.requests;
                    acc.failed += r.failed;
                    for (k, v) in &r.statuses {
                        if let Ok(code) = k.parse::<u16>() {
                            *acc.statuses.entry(code).or_default() += v;
                        }
                    }
                    add(&mut acc.latency, &r.latency)?;
                }
            }
            let t = &mut rec.totals;
            t.requests += p.totals.requests;
            t.failed += p.totals.failed;
            t.iterations += p.totals.iterations;
            t.dropped_iterations += p.totals.dropped_iterations;
            t.bytes_in += p.totals.bytes_in;
            t.bytes_out += p.totals.bytes_out;
            t.checks_passed += p.totals.checks_passed;
            t.checks_failed += p.totals.checks_failed;
            // Peaks may not coincide: the sum is an upper bound.
            t.max_vus += p.totals.max_vus;
            for (k, v) in &p.statuses {
                if let Ok(code) = k.parse::<u16>() {
                    *rec.statuses.entry(code).or_default() += v;
                }
            }
            rec.errors.connect += p.errors.connect;
            rec.errors.timeout += p.errors.timeout;
            rec.errors.other += p.errors.other;
            for s in &p.error_samples {
                if rec.error_samples.len() < MAX_ERROR_SAMPLES && !rec.error_samples.contains(s) {
                    rec.error_samples.push(s.clone());
                }
            }
            elapsed = elapsed.max(Duration::from_millis(p.elapsed_ms));
        }
        let timelines: Vec<&[TimelinePoint]> = parts.iter().map(|p| p.timeline.as_slice()).collect();
        rec.timeline = merge_points(&timelines);
        rec.sent = rec.timeline.len();
        rec.finish_rates(elapsed);
        Ok(rec)
    }
}

pub(crate) fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Thresholds are evaluated against the live histograms, so any percentile (`p(97.5)`) works.
pub struct RecorderMetrics<'a> {
    pub rec: &'a Recorder,
    /// Request path for each request index.
    pub paths: &'a [String],
}

impl MetricSource for RecorderMetrics<'_> {
    fn metric(&self, metric: Metric, scope: Option<&str>) -> Option<f64> {
        let rec = self.rec;
        let secs = rec.elapsed.as_secs_f64().max(1e-3);
        let lat = |h: &Histogram<u64>| -> Option<f64> {
            if h.is_empty() {
                return None;
            }
            Some(match metric {
                Metric::Quantile(q) => h.value_at_quantile(q) as f64 / 1000.0,
                Metric::Min => h.min() as f64 / 1000.0,
                Metric::Max => h.max() as f64 / 1000.0,
                Metric::Mean => h.mean() / 1000.0,
                _ => return None,
            })
        };
        if let Some(path) = scope {
            let i = self.paths.iter().position(|p| p == path)?;
            let r = rec.reqs.get(i)?;
            return match metric {
                Metric::Quantile(_) | Metric::Min | Metric::Max | Metric::Mean => lat(&r.latency),
                Metric::Rps => Some(r.requests as f64 / secs),
                Metric::ErrorRate => (r.requests > 0).then(|| r.failed as f64 / r.requests as f64),
                Metric::Requests => Some(r.requests as f64),
                Metric::Errors => Some(r.failed as f64),
                _ => None,
            };
        }
        let t = &rec.totals;
        match metric {
            Metric::Quantile(_) | Metric::Min | Metric::Max | Metric::Mean => lat(&rec.latency),
            Metric::Rps => Some(t.requests as f64 / secs),
            Metric::ErrorRate => (t.requests > 0).then(|| t.failed as f64 / t.requests as f64),
            Metric::Checks => {
                let n = t.checks_passed + t.checks_failed;
                (n > 0).then(|| t.checks_passed as f64 / n as f64)
            }
            Metric::Requests => Some(t.requests as f64),
            Metric::Errors => Some(t.failed as f64),
            Metric::Iterations => Some(t.iterations as f64),
            Metric::Dropped => Some(t.dropped_iterations as f64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(req: usize, ms: u64, at: &mut Recorder, t: Duration) {
        at.record(
            Sample {
                req,
                latency: Some(Duration::from_millis(ms)),
                service: Some(Duration::from_millis(ms)),
                status: Some(200),
                error: None,
                bytes_in: 10,
                bytes_out: 5,
                checks_passed: 1,
                checks_failed: 0,
            },
            t,
        );
    }

    #[test]
    fn buckets_roll_and_percentiles_hold() {
        let mut r = Recorder::new(2, 1, true);
        for i in 0..100u64 {
            ok((i % 2) as usize, i + 1, &mut r, Duration::from_millis(i * 25)); // 0..2.5s
        }
        r.observe_vus(7, Duration::from_millis(2600));
        r.dropped(Duration::from_millis(2700));
        r.record(
            Sample { req: 0, latency: None, service: None, status: None, error: Some((ErrorKind::Connect, "refused".into())), bytes_in: 0, bytes_out: 0, checks_passed: 0, checks_failed: 1 },
            Duration::from_millis(2800),
        );
        r.finish(Duration::from_millis(2900));
        let tl = r.timeline();
        assert_eq!(tl.len(), 3);
        assert_eq!(tl.iter().map(|p| p.requests).sum::<u64>(), 101);
        assert_eq!(tl[0].requests, 40);
        assert_eq!(tl[0].rps, 40.0);
        assert_eq!(tl[2].vus, 7);
        assert_eq!(tl[2].dropped, 1);
        let p = r.latency();
        assert!((p.p50 - 50.0).abs() <= 0.1, "{p:?}");
        assert!((p.p99 - 99.0).abs() <= 0.1, "{p:?}");
        assert!((p.max - 100.0).abs() < 0.1, "{p:?}");
        assert_eq!(r.totals.failed, 1);
        assert_eq!(r.errors.connect, 1);
        assert_eq!(r.error_samples, vec!["refused"]);
        assert!((r.totals.checks_rate - 100.0 / 101.0).abs() < 1e-9);
        let paths = vec!["a.req.yaml".to_string(), "b.req.yaml".to_string()];
        let m = RecorderMetrics { rec: &r, paths: &paths };
        assert_eq!(m.metric(Metric::Requests, Some("b.req.yaml")), Some(50.0));
        assert!(m.metric(Metric::Quantile(0.975), None).unwrap() > 97.0);
        assert_eq!(m.metric(Metric::Dropped, None), Some(1.0));
        let stats = r.request_stats(&[
            ("a.req.yaml".into(), "A".into(), "GET".into()),
            ("b.req.yaml".into(), "B".into(), "POST".into()),
        ]);
        assert_eq!(stats[0].requests, 51);
        assert_eq!(stats[0].failed, 1);
        assert_eq!(stats[1].status_codes.get("200"), Some(&50));
        // New points are handed out once.
        assert_eq!(r.take_new_points().len(), 3);
        assert!(r.take_new_points().is_empty());
    }

    /// Two recorders exported, serialized as JSON, merged: identical percentiles and counts to
    /// one recorder that saw every sample.
    #[test]
    fn merged_exports_equal_one_recorder() {
        let paths = vec!["a.req.yaml".to_string(), "b.req.yaml".to_string()];
        let mut whole = Recorder::new(2, 1, true);
        let mut parts = [Recorder::new(2, 1, true), Recorder::new(2, 1, true)];
        let mut seed = 7u64;
        for i in 0..20_000u64 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let ms = 1 + (seed >> 33) % 900;
            let at = Duration::from_millis(i / 4);
            ok((i % 2) as usize, ms, &mut whole, at);
            ok((i % 2) as usize, ms, &mut parts[(seed >> 20) as usize % 2], at);
        }
        let end = Duration::from_millis(5_000);
        whole.finish(end);
        let exports: Vec<RecorderExport> = parts
            .iter_mut()
            .map(|r| {
                r.finish(end);
                let e = r.export(&paths).unwrap();
                serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap()
            })
            .collect();
        let merged = Recorder::merge(&exports, &paths, true).unwrap();
        assert_eq!(merged.latency(), whole.latency(), "HDR merge is exact");
        assert_eq!(merged.service_time(), whole.service_time());
        assert_eq!(merged.totals.requests, 20_000);
        assert_eq!(merged.totals.rps, whole.totals.rps);
        let m = merged.request_stats(&[("a.req.yaml".into(), "A".into(), "GET".into()), ("b.req.yaml".into(), "B".into(), "GET".into())]);
        let w = whole.request_stats(&[("a.req.yaml".into(), "A".into(), "GET".into()), ("b.req.yaml".into(), "B".into(), "GET".into())]);
        assert_eq!(m, w);
        assert_eq!(merged.timeline().iter().map(|p| p.requests).sum::<u64>(), 20_000);
        assert_eq!(merged.timeline().len(), whole.timeline().len());
        assert!(decode_histogram("not base64!").is_err());
    }

    #[test]
    fn bucket_width_fits_600_points() {
        assert_eq!(bucket_seconds(Duration::from_secs(60)), 1);
        assert_eq!(bucket_seconds(Duration::from_secs(570)), 1);
        assert_eq!(bucket_seconds(Duration::from_secs(3600)), 7);
    }
}
