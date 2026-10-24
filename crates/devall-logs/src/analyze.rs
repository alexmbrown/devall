//! Histograms (count per time bucket per level), top field values, field discovery and
//! Drain-like pattern clustering ("Logs Drilldown" patterns): variable tokens (numbers, ids,
//! UUIDs, IPs, hex, timestamps, emails, URLs, numeric path segments) are masked, identical
//! templates are counted, then templates of equal length and first token that agree on most
//! positions are merged with `<*>` wildcards. Templates are processed by count then text, so the
//! result does not depend on input order.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::parse::{Level, LogEntry};
use crate::query::quote;

// ---------- histogram ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    /// Bucket start (Unix ms).
    pub t: i64,
    /// Counts by level, in [`Level::ALL`] order: trace, debug, info, warn, error, fatal, unknown.
    pub counts: [u64; 7],
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Histogram {
    pub start: i64,
    pub end: i64,
    pub bucket_ms: i64,
    pub buckets: Vec<Bucket>,
    /// Entries counted in buckets.
    pub total: u64,
    /// Matching entries without any timestamp.
    pub without_ts: u64,
    /// Largest bucket total (for scaling).
    pub max: u64,
}

const NICE: &[i64] = &[
    1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000, 1_800_000,
    3_600_000, 7_200_000, 10_800_000, 21_600_000, 43_200_000, 86_400_000, 172_800_000, 604_800_000, 1_209_600_000, 2_592_000_000,
    7_776_000_000, 31_536_000_000,
];

/// Smallest "nice" bucket size giving at most `target` buckets over `span` ms.
pub fn nice_bucket(span: i64, target: usize) -> i64 {
    let target = target.max(1) as i64;
    NICE.iter().copied().find(|b| span / b < target).unwrap_or(31_536_000_000)
}

/// Receives one matching entry's effective timestamp and level.
pub type Sink<'a> = &'a mut dyn FnMut(Option<i64>, Level);

/// Builds a histogram. `feed` is called once (twice when `range` is `None`, to find the extent)
/// and must report every matching entry's effective timestamp and level to the sink it's given.
pub fn histogram(range: Option<(i64, i64)>, target: usize, feed: &mut dyn FnMut(Sink<'_>)) -> Histogram {
    let (start, end) = match range {
        Some(r) => r,
        None => {
            let (mut lo, mut hi) = (i64::MAX, i64::MIN);
            feed(&mut |ts, _| {
                if let Some(t) = ts {
                    lo = lo.min(t);
                    hi = hi.max(t);
                }
            });
            if lo > hi {
                let mut without = 0;
                feed(&mut |_, _| without += 1);
                return Histogram { without_ts: without, ..Default::default() };
            }
            (lo, hi + 1)
        }
    };
    let bucket_ms = nice_bucket((end - start).max(1), target);
    let first = start.div_euclid(bucket_ms) * bucket_ms;
    let n = ((end - 1 - first).div_euclid(bucket_ms) + 1).clamp(1, 100_000) as usize;
    let mut buckets: Vec<Bucket> = (0..n).map(|i| Bucket { t: first + i as i64 * bucket_ms, counts: [0; 7] }).collect();
    let (mut total, mut without) = (0u64, 0u64);
    feed(&mut |ts, level| match ts {
        Some(t) if t >= start && t < end => {
            let i = ((t - first) / bucket_ms) as usize;
            if let Some(b) = buckets.get_mut(i) {
                b.counts[level as usize] += 1;
                total += 1;
            }
        }
        Some(_) => {}
        None => without += 1,
    });
    let max = buckets.iter().map(|b| b.counts.iter().sum::<u64>()).max().unwrap_or(0);
    Histogram { start, end, bucket_ms, buckets, total, without_ts: without, max }
}

// ---------- masking ----------

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_hexdigit())
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && [8, 13, 18, 23].iter().all(|&i| b[i] == b'-') && s.split('-').all(is_hex)
}

fn is_ipv4(s: &str) -> bool {
    let host = match s.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|c| c.is_ascii_digit()) => h,
        _ => s,
    };
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|c| c.is_ascii_digit()))
}

fn is_ipv6(s: &str) -> bool {
    let s = s.trim_start_matches('[').split(']').next().unwrap_or(s);
    s.matches(':').count() >= 2 && s.len() >= 3 && s.bytes().all(|c| c.is_ascii_hexdigit() || c == b':' || c == b'.') && s.bytes().any(|c| c.is_ascii_hexdigit())
}

const UNITS: &[&str] = &["ms", "s", "us", "µs", "ns", "m", "h", "d", "%", "b", "kb", "mb", "gb", "k", "x", "px"];

fn is_number(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    let digits_end = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',' || c == '_')).unwrap_or(t.len());
    if digits_end == 0 || !t.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    let unit = &t[digits_end..];
    unit.is_empty() || UNITS.iter().any(|u| u.eq_ignore_ascii_case(unit))
}

fn is_timeish(s: &str) -> bool {
    let digits = s.bytes().filter(|c| c.is_ascii_digit()).count();
    digits >= 4 && s.bytes().all(|c| c.is_ascii_digit() || matches!(c, b':' | b'-' | b'.' | b'T' | b'Z' | b'+' | b'/' | b',')) && s.contains([':', '-', '/'])
}

fn is_id(s: &str) -> bool {
    s.len() >= 6
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        && s.bytes().any(|c| c.is_ascii_digit())
        && s.bytes().any(|c| c.is_ascii_alphabetic())
}

fn classify(v: &str) -> Option<&'static str> {
    if v.is_empty() {
        return None;
    }
    if is_uuid(v) {
        return Some("<UUID>");
    }
    if is_ipv4(v) || is_ipv6(v) {
        return Some("<IP>");
    }
    if is_number(v) {
        return Some("<NUM>");
    }
    if (v.len() >= 8 && is_hex(v) && v.bytes().any(|c| c.is_ascii_digit())) || (v.starts_with("0x") && is_hex(&v[2..])) {
        return Some("<HEX>");
    }
    if is_timeish(v) {
        return Some("<TS>");
    }
    if v.contains("://") {
        return Some("<URL>");
    }
    if let Some(at) = v.find('@')
        && at > 0
        && v[at + 1..].contains('.')
    {
        return Some("<EMAIL>");
    }
    if is_id(v) {
        return Some("<ID>");
    }
    None
}

fn mask_value(v: &str, out: &mut String) {
    if let Some(p) = classify(v) {
        out.push_str(p);
        return;
    }
    if v.contains('/') && v.len() > 1 {
        // Paths: mask variable segments (`/users/42/orders` → `/users/<NUM>/orders`).
        for (i, seg) in v.split('/').enumerate() {
            if i > 0 {
                out.push('/');
            }
            match classify(seg) {
                Some(p) => out.push_str(p),
                None => out.push_str(seg),
            }
        }
        return;
    }
    out.push_str(v);
}

const PRE: &[char] = &['(', '[', '{', '"', '\'', '<', '`'];
const POST: &[char] = &[')', ']', '}', '"', '\'', ',', ';', ':', '.', '!', '?', '>', '`'];

fn mask_token(tok: &str, out: &mut String) {
    let core_start = tok.find(|c: char| !PRE.contains(&c)).unwrap_or(tok.len());
    let pre = &tok[..core_start];
    let rest = &tok[core_start..];
    let core_end = rest.rfind(|c: char| !POST.contains(&c)).map(|i| i + rest[i..].chars().next().map_or(1, char::len_utf8)).unwrap_or(0);
    let (core, post) = rest.split_at(core_end);
    out.push_str(pre);
    if core.is_empty() {
        out.push_str(post);
        return;
    }
    if classify(core).is_none()
        && let Some(eq) = core.find(['=', ':'])
        && eq > 0
        && eq + 1 < core.len()
        && !core.contains("://")
    {
        out.push_str(&core[..=eq]);
        mask_value(&core[eq + 1..], out);
    } else {
        mask_value(core, out);
    }
    out.push_str(post);
}

/// The masked template of a message line.
pub fn mask(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for (i, tok) in line.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if i >= 64 {
            out.push_str("<*>");
            break;
        }
        mask_token(tok, &mut out);
    }
    out
}

// ---------- clustering ----------

#[derive(Debug, Clone)]
struct Acc {
    count: u64,
    levels: [u64; 7],
    first_ts: Option<i64>,
    last_ts: Option<i64>,
    sample: String,
    sample_index: u64,
    spark: Vec<u32>,
}

impl Acc {
    fn merge(&mut self, o: &Acc) {
        self.count += o.count;
        for i in 0..7 {
            self.levels[i] += o.levels[i];
        }
        self.first_ts = min_opt(self.first_ts, o.first_ts);
        self.last_ts = max_opt(self.last_ts, o.last_ts);
        if o.sample_index < self.sample_index {
            self.sample = o.sample.clone();
            self.sample_index = o.sample_index;
        }
        for (a, b) in self.spark.iter_mut().zip(&o.spark) {
            *a += b;
        }
    }
}

fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pattern {
    /// Stable id (hash of the template).
    pub id: String,
    pub template: String,
    pub count: u64,
    /// Fraction of clustered entries.
    pub share: f64,
    /// By level, in [`Level::ALL`] order.
    pub levels: [u64; 7],
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub sample: String,
    /// View position of the sample entry.
    pub sample_index: u64,
    pub sparkline: Vec<u32>,
    /// Query that selects this pattern (`msg~"…"`).
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatternResult {
    pub patterns: Vec<Pattern>,
    /// Entries clustered.
    pub total: u64,
    /// Only the most recent `max_entries` matches were clustered.
    pub sampled: bool,
    pub spark_start: i64,
    pub spark_bucket_ms: i64,
    /// Distinct templates before merging.
    pub templates: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct PatternOptions {
    pub max_patterns: usize,
    pub max_entries: usize,
    pub spark_buckets: usize,
    /// Fraction of positions that must agree to merge two templates.
    pub similarity: f64,
}

impl Default for PatternOptions {
    fn default() -> Self {
        Self { max_patterns: 200, max_entries: 500_000, spark_buckets: 30, similarity: 0.6 }
    }
}

/// Accumulates masked templates; mergeable across threads.
#[derive(Debug, Clone)]
pub struct Clusterer {
    map: HashMap<String, Acc>,
    spark_start: i64,
    spark_bucket: i64,
    spark_n: usize,
    total: u64,
}

impl Clusterer {
    /// `spark` = (start, end) of the sparkline range.
    pub fn new(spark: Option<(i64, i64)>, buckets: usize) -> Self {
        let (start, bucket, n) = match spark {
            Some((s, e)) if e > s && buckets > 0 => {
                let b = ((e - s) as f64 / buckets as f64).ceil().max(1.0) as i64;
                (s, b, buckets)
            }
            _ => (0, 1, 0),
        };
        Self { map: HashMap::new(), spark_start: start, spark_bucket: bucket, spark_n: n, total: 0 }
    }

    pub fn add(&mut self, message: &str, ts: Option<i64>, level: Level, index: u64) {
        let line = message.split('\n').next().unwrap_or("");
        let template = mask(line);
        let spark_n = self.spark_n;
        let acc = self.map.entry(template).or_insert_with(|| Acc {
            count: 0,
            levels: [0; 7],
            first_ts: None,
            last_ts: None,
            sample: line.chars().take(500).collect(),
            sample_index: index,
            spark: vec![0; spark_n],
        });
        acc.count += 1;
        acc.levels[level as usize] += 1;
        acc.first_ts = min_opt(acc.first_ts, ts);
        acc.last_ts = max_opt(acc.last_ts, ts);
        if index < acc.sample_index {
            acc.sample = line.chars().take(500).collect();
            acc.sample_index = index;
        }
        if let Some(t) = ts
            && self.spark_n > 0
            && t >= self.spark_start
        {
            let i = ((t - self.spark_start) / self.spark_bucket) as usize;
            if let Some(s) = acc.spark.get_mut(i) {
                *s += 1;
            }
        }
        self.total += 1;
    }

    pub fn merge(&mut self, other: Clusterer) {
        self.total += other.total;
        for (k, v) in other.map {
            match self.map.get_mut(&k) {
                Some(a) => a.merge(&v),
                None => {
                    self.map.insert(k, v);
                }
            }
        }
    }

    pub fn finish(self, opts: &PatternOptions) -> PatternResult {
        let templates = self.map.len();
        let mut items: Vec<(String, Acc)> = self.map.into_iter().collect();
        items.sort_by(|a, b| b.1.count.cmp(&a.1.count).then_with(|| a.0.cmp(&b.0)));
        struct Cluster {
            tokens: Vec<String>,
            acc: Acc,
        }
        let mut clusters: Vec<Cluster> = Vec::new();
        let mut groups: HashMap<(usize, String), Vec<usize>> = HashMap::new();
        for (template, acc) in items {
            let tokens: Vec<String> = template.split(' ').map(str::to_string).collect();
            let key = (tokens.len(), group_key(tokens.first().map(String::as_str).unwrap_or("")));
            let members = groups.entry(key).or_default();
            let mut best: Option<(usize, f64)> = None;
            for &ci in members.iter() {
                let s = similarity(&clusters[ci].tokens, &tokens);
                let need = if tokens.len() <= 2 { 1.0 } else { opts.similarity };
                if s >= need && best.is_none_or(|(_, bs)| s > bs) {
                    best = Some((ci, s));
                }
            }
            match best {
                Some((ci, _)) => {
                    let c = &mut clusters[ci];
                    for (a, b) in c.tokens.iter_mut().zip(&tokens) {
                        if a != b {
                            *a = "<*>".to_string();
                        }
                    }
                    c.acc.merge(&acc);
                }
                None => {
                    if members.len() < 500 {
                        members.push(clusters.len());
                    }
                    clusters.push(Cluster { tokens, acc });
                }
            }
        }
        clusters.sort_by(|a, b| b.acc.count.cmp(&a.acc.count).then_with(|| a.tokens.cmp(&b.tokens)));
        let total = self.total.max(1);
        let patterns = clusters
            .into_iter()
            .take(opts.max_patterns)
            .map(|c| {
                let template = c.tokens.join(" ");
                Pattern {
                    id: format!("{:016x}", fnv(&template)),
                    query: format!("msg~{}", quote(&template_regex(&c.tokens))),
                    template,
                    count: c.acc.count,
                    share: c.acc.count as f64 / total as f64,
                    levels: c.acc.levels,
                    first_ts: c.acc.first_ts,
                    last_ts: c.acc.last_ts,
                    sample: c.acc.sample,
                    sample_index: c.acc.sample_index,
                    sparkline: c.acc.spark,
                }
            })
            .collect();
        PatternResult { patterns, total: self.total, sampled: false, spark_start: self.spark_start, spark_bucket_ms: self.spark_bucket, templates }
    }
}

fn group_key(first: &str) -> String {
    if first.contains('<') { "<*>".into() } else { first.to_string() }
}

fn similarity(a: &[String], b: &[String]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return if a.is_empty() && b.is_empty() { 1.0 } else { 0.0 };
    }
    let same = a.iter().zip(b).filter(|(x, y)| x == y || *x == "<*>").count();
    same as f64 / a.len() as f64
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in s.as_bytes() {
        h ^= c as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

const PLACEHOLDERS: &[&str] = &["<*>", "<NUM>", "<UUID>", "<IP>", "<HEX>", "<TS>", "<EMAIL>", "<URL>", "<ID>"];

fn token_regex(tok: &str) -> String {
    if PLACEHOLDERS.contains(&tok) {
        return r"\S+".into();
    }
    let mut out = String::new();
    let mut rest = tok;
    while !rest.is_empty() {
        let next = PLACEHOLDERS.iter().filter_map(|p| rest.find(p).map(|i| (i, *p))).min();
        match next {
            Some((i, p)) => {
                out.push_str(&regex::escape(&rest[..i]));
                out.push_str(r"\S*?");
                rest = &rest[i + p.len()..];
            }
            None => {
                out.push_str(&regex::escape(rest));
                break;
            }
        }
    }
    out
}

/// Regex matching messages of a template (first line).
pub fn template_regex(tokens: &[String]) -> String {
    format!(r"^{}(\s|$)", tokens.iter().map(|t| token_regex(t)).collect::<Vec<_>>().join(r"\s+"))
}

// ---------- field values ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ValueCount {
    pub value: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NumStats {
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub p50: f64,
    pub p95: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TopValues {
    pub field: String,
    pub scanned: u64,
    pub present: u64,
    /// Distinct values seen (stops counting at 10 000).
    pub distinct: u64,
    pub values: Vec<ValueCount>,
    pub numeric: Option<NumStats>,
    pub sampled: bool,
}

fn value_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Null => {}
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| value_strings(x, out)),
        other => out.push(other.to_string()),
    }
}

/// Values of `field` in an entry (special names: level, msg, source, format, trace).
pub fn field_values(e: &LogEntry, field: &str) -> Vec<String> {
    let mut out = Vec::new();
    match field {
        "level" => out.push(e.level.as_str().to_string()),
        "msg" | "message" => out.push(e.first_line().to_string()),
        "source" => out.extend(e.source.clone()),
        "format" => out.push(e.format.as_str().to_string()),
        "trace" | "correlation" => out.extend(e.correlation_id.clone()),
        _ => {
            if let Some(v) = e.field(field) {
                value_strings(v, &mut out);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct TopAcc {
    counts: HashMap<String, u64>,
    nums: Vec<f64>,
    scanned: u64,
    present: u64,
    overflow: bool,
}

const MAX_DISTINCT: usize = 10_000;

impl TopAcc {
    pub fn add(&mut self, values: Vec<String>) {
        self.scanned += 1;
        if values.is_empty() {
            return;
        }
        self.present += 1;
        for v in values {
            if let Ok(n) = v.parse::<f64>()
                && n.is_finite()
            {
                self.nums.push(n);
            }
            let len = self.counts.len();
            match self.counts.get_mut(&v) {
                Some(c) => *c += 1,
                None if len < MAX_DISTINCT => {
                    self.counts.insert(v, 1);
                }
                None => self.overflow = true,
            }
        }
    }

    pub fn merge(&mut self, o: TopAcc) {
        self.scanned += o.scanned;
        self.present += o.present;
        self.overflow |= o.overflow;
        self.nums.extend(o.nums);
        for (k, v) in o.counts {
            let len = self.counts.len();
            match self.counts.get_mut(&k) {
                Some(c) => *c += v,
                None if len < MAX_DISTINCT => {
                    self.counts.insert(k, v);
                }
                None => self.overflow = true,
            }
        }
    }

    pub fn finish(mut self, field: &str, limit: usize) -> TopValues {
        let distinct = self.counts.len() as u64;
        let mut values: Vec<ValueCount> = self.counts.into_iter().map(|(value, count)| ValueCount { value, count }).collect();
        values.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        values.truncate(limit);
        let numeric = (!self.nums.is_empty() && self.nums.len() as u64 * 10 >= self.present * 9).then(|| {
            self.nums.sort_by(|a, b| a.total_cmp(b));
            let n = self.nums.len();
            let pct = |p: f64| self.nums[((n as f64 - 1.0) * p).round() as usize];
            NumStats { min: self.nums[0], max: self.nums[n - 1], avg: self.nums.iter().sum::<f64>() / n as f64, p50: pct(0.5), p95: pct(0.95) }
        });
        TopValues { field: field.to_string(), scanned: self.scanned, present: self.present, distinct, values, numeric, sampled: self.overflow }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldInfo {
    pub name: String,
    /// Sampled entries that have the field.
    pub count: u64,
    pub distinct: u64,
    pub numeric: bool,
    pub top: Vec<ValueCount>,
}

/// Field names (top level, plus one nested level as `a.b`) with counts and top values.
pub fn discover_fields(entries: &[LogEntry], limit: usize) -> Vec<FieldInfo> {
    let mut accs: HashMap<String, TopAcc> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for e in entries {
        let mut seen: Vec<(String, Vec<String>)> = Vec::new();
        for (k, v) in &e.fields {
            match v {
                Value::Object(o) => {
                    for (k2, v2) in o {
                        if !v2.is_object() {
                            let mut vals = Vec::new();
                            value_strings(v2, &mut vals);
                            seen.push((format!("{k}.{k2}"), vals));
                        }
                    }
                }
                _ => {
                    let mut vals = Vec::new();
                    value_strings(v, &mut vals);
                    seen.push((k.clone(), vals));
                }
            }
        }
        for (name, vals) in seen {
            if !accs.contains_key(&name) {
                order.push(name.clone());
            }
            let acc = accs.entry(name).or_default();
            acc.add(vals);
        }
    }
    let mut out: Vec<FieldInfo> = order
        .into_iter()
        .map(|name| {
            let acc = accs.remove(&name).unwrap_or_default();
            let t = acc.finish(&name, 5);
            FieldInfo { name, count: t.present, distinct: t.distinct, numeric: t.numeric.is_some(), top: t.values }
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking() {
        assert_eq!(mask("GET /users/42/orders took 15ms"), "GET /users/<NUM>/orders took <NUM>");
        assert_eq!(mask("user=alice id=7f3a9b2c41 ip 10.0.0.12:8080"), "user=alice id=<HEX> ip <IP>");
        assert_eq!(mask("req 550e8400-e29b-41d4-a716-446655440000 from bob@example.com"), "req <UUID> from <EMAIL>");
        assert_eq!(mask("(retry 3) failed: connection refused."), "(retry <NUM>) failed: connection refused.");
        assert_eq!(mask("at 2024-01-02T03:04:05Z order ord_8x7Kq2 ok"), "at <TS> order <ID> ok");
        assert_eq!(mask("fetch https://api.example.com/x?id=1 -> 200"), "fetch <URL> -> <NUM>");
        assert_eq!(mask("status:503 dur:1.25s"), "status:<NUM> dur:<NUM>");
    }

    #[test]
    fn clustering_is_stable_and_merges() {
        let mut lines = Vec::new();
        for i in 0..300u64 {
            lines.push((format!("GET /api/users/{i} took {}ms", i % 50), Level::Info));
            if i % 3 == 0 {
                lines.push((format!("cache miss for key user:{i}"), Level::Debug));
            }
            if i % 10 == 0 {
                lines.push((format!("payment {} failed for customer {}", ["declined", "timeout", "fraud"][i as usize % 3], i), Level::Error));
            }
        }
        let run = |order: &[(String, Level)]| {
            let mut c = Clusterer::new(Some((0, 300_000)), 10);
            for (i, (l, lvl)) in order.iter().enumerate() {
                c.add(l, Some(i as i64 * 600), *lvl, i as u64);
            }
            c.finish(&PatternOptions::default())
        };
        let a = run(&lines);
        let t: Vec<(&str, u64)> = a.patterns.iter().map(|p| (p.template.as_str(), p.count)).collect();
        assert_eq!(
            t,
            vec![
                ("GET /api/users/<NUM> took <NUM>", 300),
                ("cache miss for key user:<NUM>", 100),
                ("payment <*> failed for customer <NUM>", 30),
            ]
        );
        assert_eq!(a.patterns[2].levels[Level::Error as usize], 30);
        assert_eq!(a.patterns[0].sparkline.iter().sum::<u32>(), 300);
        assert_eq!(a.total, 430);

        // Same clusters (and ids) regardless of input order.
        let mut rev = lines.clone();
        rev.reverse();
        let b = run(&rev);
        let ids = |r: &PatternResult| r.patterns.iter().map(|p| (p.id.clone(), p.count)).collect::<Vec<_>>();
        assert_eq!(ids(&a), ids(&b));

        // The generated query selects exactly the pattern's lines.
        let re = regex::Regex::new(&template_regex(&a.patterns[2].template.split(' ').map(String::from).collect::<Vec<_>>())).unwrap();
        assert_eq!(lines.iter().filter(|(l, _)| re.is_match(l)).count(), 30);
        let q = crate::query::Query::parse(&a.patterns[0].query).unwrap();
        let (_, e) = crate::parse::parse_text("GET /api/users/7 took 3ms\ncache miss for key user:1", &crate::time::TimeCtx::utc());
        assert!(q.matches(&e[0]) && !q.matches(&e[1]));
    }

    #[test]
    fn histogram_buckets() {
        let pts: Vec<(Option<i64>, Level)> = vec![(Some(0), Level::Info), (Some(59_999), Level::Error), (Some(60_000), Level::Info), (None, Level::Warn)];
        let h = histogram(None, 10, &mut |f| {
            for (t, l) in &pts {
                f(*t, *l);
            }
        });
        assert_eq!(h.bucket_ms, 10_000);
        assert_eq!(h.buckets.len(), 7);
        assert_eq!(h.buckets[0].counts[Level::Info as usize], 1);
        assert_eq!(h.buckets[5].counts[Level::Error as usize], 1);
        assert_eq!(h.buckets[6].counts[Level::Info as usize], 1);
        assert_eq!((h.total, h.without_ts, h.max), (3, 1, 1));
        assert_eq!(nice_bucket(3_600_000, 100), 60_000);
    }

    #[test]
    fn top_values_and_fields() {
        let (_, entries) = crate::parse::parse_text(
            "{\"status\":200,\"user\":{\"id\":1}}\n{\"status\":200}\n{\"status\":500,\"user\":{\"id\":2}}\n{\"x\":1}",
            &crate::time::TimeCtx::utc(),
        );
        let mut acc = TopAcc::default();
        for e in &entries {
            acc.add(field_values(e, "status"));
        }
        let t = acc.finish("status", 10);
        assert_eq!(t.values[0], ValueCount { value: "200".into(), count: 2 });
        assert_eq!((t.present, t.scanned, t.distinct), (3, 4, 2));
        assert_eq!(t.numeric.as_ref().unwrap().max, 500.0);
        let f = discover_fields(&entries, 10);
        assert_eq!(f[0].name, "status");
        assert!(f.iter().any(|x| x.name == "user.id" && x.count == 2));
    }
}
