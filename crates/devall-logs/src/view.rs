//! A view over one file or several merged by timestamp, with a cached filter result.
//!
//! Positions are indices into the view (file order for one file, timestamp-merged order for
//! several). Filtering scans entries in parallel and keeps the matching positions for paging; for a
//! single followed file the cache is extended incrementally as lines arrive.

use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

use crate::analyze::{self, Clusterer, FieldInfo, Histogram, PatternOptions, PatternResult, TopAcc, TopValues};
use crate::file::{EntryReader, LogFile};
use crate::index::Bits;
use crate::parse::{Level, LogEntry};
use crate::query::{Compiled, Filter, Lazy};
use crate::{Result, par};

/// Entries larger than this are truncated in pages (the detail view loads them whole).
pub const PAGE_TEXT_LIMIT: usize = 16 * 1024;
/// Hard cap for a single entry's text in detail views.
pub const DETAIL_TEXT_LIMIT: usize = 4 << 20;
const SRC_SHIFT: u32 = 48;
const IDX_MASK: u64 = (1 << SRC_SHIFT) - 1;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// Matching entries.
    pub total: u64,
    /// All entries in the view.
    pub view_total: u64,
    pub offset: u64,
    pub entries: Vec<LogEntry>,
    /// Some source is still being indexed (totals will grow).
    pub indexing: bool,
    pub elapsed_ms: f64,
}

#[derive(Clone)]
enum Matches {
    All(u64),
    List(Arc<Vec<u64>>),
}

impl Matches {
    fn len(&self) -> u64 {
        match self {
            Matches::All(n) => *n,
            Matches::List(v) => v.len() as u64,
        }
    }

    fn get(&self, i: u64) -> u64 {
        match self {
            Matches::All(_) => i,
            Matches::List(v) => v[i as usize],
        }
    }
}

struct MergeOrder {
    counts: Vec<u64>,
    gens: Vec<u64>,
    order: Arc<Vec<u64>>,
}

struct CacheEntry {
    filter: Filter,
    gens: Vec<u64>,
    counts: Vec<u64>,
    matches: Arc<Vec<u64>>,
}

pub struct LogSet {
    files: Vec<Arc<LogFile>>,
    merge: Mutex<Option<MergeOrder>>,
    cache: Mutex<Option<CacheEntry>>,
}

impl LogSet {
    pub fn single(f: Arc<LogFile>) -> Self {
        Self::merged(vec![f])
    }

    /// Several sources merged by timestamp (stable: each source keeps its own order).
    pub fn merged(files: Vec<Arc<LogFile>>) -> Self {
        Self { files, merge: Mutex::new(None), cache: Mutex::new(None) }
    }

    pub fn files(&self) -> &[Arc<LogFile>] {
        &self.files
    }

    pub fn is_merged(&self) -> bool {
        self.files.len() > 1
    }

    pub fn indexing(&self) -> bool {
        self.files.iter().any(|f| !f.is_done())
    }

    fn counts(&self) -> Vec<u64> {
        self.files.iter().map(|f| f.entry_count()).collect()
    }

    fn gens(&self) -> Vec<u64> {
        self.files.iter().map(|f| f.generation()).collect()
    }

    /// Entries in the view.
    pub fn len(&self) -> u64 {
        if self.is_merged() { self.order().len() as u64 } else { self.files[0].entry_count() }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn order(&self) -> Arc<Vec<u64>> {
        let counts = self.counts();
        let gens = self.gens();
        let mut m = self.merge.lock().unwrap();
        if let Some(o) = m.as_ref()
            && o.counts == counts
            && o.gens == gens
        {
            return o.order.clone();
        }
        let order = Arc::new(merge_order(&self.files, &counts));
        *m = Some(MergeOrder { counts, gens, order: order.clone() });
        order
    }

    /// (source, entry index) of a view position.
    fn resolve(&self, order: Option<&[u64]>, pos: u64) -> (usize, u64) {
        match order {
            Some(o) => {
                let v = o[pos as usize];
                ((v >> SRC_SHIFT) as usize, v & IDX_MASK)
            }
            None => (0, pos),
        }
    }

    fn source_name(&self, src: usize) -> Option<&str> {
        self.is_merged().then(|| self.files[src].name())
    }

    fn matches(&self, filter: &Filter, compiled: &Compiled) -> Result<Matches> {
        if compiled.is_trivial() {
            return Ok(Matches::All(self.len()));
        }
        let counts = self.counts();
        let gens = self.gens();
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(c) = cache.as_mut()
                && c.filter == *filter
                && c.gens == gens
            {
                if c.counts == counts {
                    return Ok(Matches::List(c.matches.clone()));
                }
                if !self.is_merged() && counts[0] >= c.counts[0] {
                    // Appended: re-check the last entry (it may have grown) and scan the rest.
                    let from = c.counts[0].saturating_sub(1);
                    let mut v = (*c.matches).clone();
                    while v.last().is_some_and(|&p| p >= from) {
                        v.pop();
                    }
                    v.extend(eval_par(&self.files[0], compiled, from..counts[0], None)?);
                    c.matches = Arc::new(v);
                    c.counts = counts;
                    return Ok(Matches::List(c.matches.clone()));
                }
            }
        }
        let matches = if self.is_merged() {
            let order = self.order();
            let mut bits = Vec::with_capacity(self.files.len());
            for f in &self.files {
                let n = f.entry_count();
                let mut b = Bits::new(n as usize);
                for i in eval_par(f, compiled, 0..n, Some(f.name()))? {
                    b.set(i as usize);
                }
                bits.push(b);
            }
            order
                .iter()
                .enumerate()
                .filter(|(_, v)| {
                    let (s, i) = ((**v >> SRC_SHIFT) as usize, **v & IDX_MASK);
                    bits[s].get(i as usize)
                })
                .map(|(p, _)| p as u64)
                .collect()
        } else {
            eval_par(&self.files[0], compiled, 0..counts[0], None)?
        };
        let matches = Arc::new(matches);
        *self.cache.lock().unwrap() = Some(CacheEntry { filter: filter.clone(), gens, counts, matches: matches.clone() });
        Ok(Matches::List(matches))
    }

    fn compile(&self, filter: &Filter) -> Result<Compiled> {
        let ctx = self.files.first().map(|f| f.time_ctx()).unwrap_or_default();
        Ok(filter.compile_at(crate::now_ms(), &ctx)?)
    }

    /// A page of matching entries (`offset` counts matches).
    pub fn query(&self, filter: &Filter, offset: u64, limit: usize) -> Result<Page> {
        let t0 = Instant::now();
        let compiled = self.compile(filter)?;
        let m = self.matches(filter, &compiled)?;
        let total = m.len();
        let end = (offset + limit as u64).min(total);
        let positions: Vec<u64> = (offset.min(end)..end).map(|i| m.get(i)).collect();
        let entries = self.load(&positions, PAGE_TEXT_LIMIT)?;
        Ok(Page { total, view_total: self.len(), offset, entries, indexing: self.indexing(), elapsed_ms: t0.elapsed().as_secs_f64() * 1000.0 })
    }

    /// One entry by view position, untruncated (up to 4 MiB).
    pub fn entry_at(&self, pos: u64) -> Result<Option<LogEntry>> {
        if pos >= self.len() {
            return Ok(None);
        }
        Ok(self.load(&[pos], DETAIL_TEXT_LIMIT)?.pop())
    }

    /// Row (match index) of the first match at or after `ts`.
    pub fn locate(&self, filter: &Filter, ts: i64) -> Result<Option<u64>> {
        let compiled = self.compile(filter)?;
        let m = self.matches(filter, &compiled)?;
        let mut found = None;
        let mut row = 0u64;
        self.feed_meta(&m, &mut |t, _| {
            if found.is_none() && t.is_some_and(|t| t >= ts) {
                found = Some(row);
            }
            row += 1;
        });
        Ok(found)
    }

    fn load(&self, positions: &[u64], limit: usize) -> Result<Vec<LogEntry>> {
        let order = self.is_merged().then(|| self.order());
        let mut readers: Vec<Option<EntryReader>> = (0..self.files.len()).map(|_| None).collect();
        let ctxs: Vec<_> = self.files.iter().map(|f| (f.format(), f.time_ctx())).collect();
        let mut out = Vec::with_capacity(positions.len());
        for &pos in positions {
            let (s, i) = self.resolve(order.as_deref().map(|v| v.as_slice()), pos);
            let f = &self.files[s];
            let Some(span) = f.spans(i..i + 1).pop() else { continue };
            if readers[s].is_none() {
                readers[s] = Some(f.reader()?);
            }
            let reader = readers[s].as_mut().unwrap();
            let (format, ctx) = &ctxs[s];
            let mut e = reader.with_text(&span, |t| f.build(t, pos, &span, *format, ctx))?;
            e.source = self.source_name(s).map(str::to_string);
            truncate_entry(&mut e, limit);
            out.push(e);
        }
        Ok(out)
    }

    /// Reports (effective ts, level) for every match, in view order.
    fn feed_meta(&self, m: &Matches, f: &mut dyn FnMut(Option<i64>, Level)) {
        match (m, self.is_merged()) {
            (Matches::All(n), false) => self.files[0].for_each_meta(0..*n, |_, t, l| f(t, l)),
            (Matches::List(v), false) => self.files[0].metas_for(v, |_, t, l| f(t, l)),
            (m, true) => {
                let order = self.order();
                let n = m.len();
                let mut start = 0;
                while start < n {
                    let end = (start + 65_536).min(n);
                    // Group a batch by source, then report in view order.
                    let mut per: Vec<Vec<u64>> = vec![Vec::new(); self.files.len()];
                    let mut keys = Vec::with_capacity((end - start) as usize);
                    for k in start..end {
                        let (s, i) = self.resolve(Some(order.as_slice()), m.get(k));
                        keys.push((s, per[s].len()));
                        per[s].push(i);
                    }
                    let mut metas: Vec<Vec<(Option<i64>, Level)>> = vec![Vec::new(); self.files.len()];
                    for (s, idxs) in per.iter().enumerate() {
                        self.files[s].metas_for(idxs, |_, t, l| metas[s].push((t, l)));
                    }
                    for (s, j) in keys {
                        if let Some((t, l)) = metas[s].get(j) {
                            f(*t, *l);
                        }
                    }
                    start = end;
                }
            }
        }
    }

    /// Count per time bucket per level over the matches (≈`target` buckets).
    pub fn histogram(&self, filter: &Filter, target: usize) -> Result<Histogram> {
        let compiled = self.compile(filter)?;
        let m = self.matches(filter, &compiled)?;
        let range = match (filter.from, filter.to) {
            (Some(a), Some(b)) if b > a => Some((a, b)),
            _ => None,
        };
        Ok(analyze::histogram(range, target, &mut |f| self.feed_meta(&m, f)))
    }

    /// Runs `f` over fully parsed matches `sel` (match indices) in parallel chunks.
    fn scan<R: Send>(&self, m: &Matches, sel: Range<u64>, init: impl Fn() -> R + Sync, f: impl Fn(&mut R, u64, &LogEntry) + Sync) -> Result<Vec<R>> {
        let order = self.is_merged().then(|| self.order());
        let order_ref = order.as_deref().map(|v| v.as_slice());
        let ctxs: Vec<_> = self.files.iter().map(|f| (f.format(), f.time_ctx().year, f.time_ctx().naive)).collect();
        let n = (sel.end - sel.start) as usize;
        let results = par::chunks(n, 4096, |r| -> Result<R> {
            let mut acc = init();
            let mut readers: Vec<Option<EntryReader>> = (0..self.files.len()).map(|_| None).collect();
            let tctx: Vec<_> = ctxs.iter().map(|(_, y, nz)| crate::time::TimeCtx::new(*nz, *y)).collect();
            for k in r {
                let pos = m.get(sel.start + k as u64);
                let (s, i) = self.resolve(order_ref, pos);
                let file = &self.files[s];
                let Some(span) = file.spans(i..i + 1).pop() else { continue };
                if readers[s].is_none() {
                    readers[s] = Some(file.reader()?);
                }
                let mut e = readers[s].as_mut().unwrap().with_text(&span, |t| file.build(t, pos, &span, ctxs[s].0, &tctx[s]))?;
                if e.ts.is_none() {
                    e.ts = span.ts;
                }
                e.source = self.source_name(s).map(str::to_string);
                f(&mut acc, pos, &e);
            }
            Ok(acc)
        });
        results.into_iter().collect()
    }

    /// Drain-like message patterns over the (most recent `max_entries`) matches.
    pub fn patterns(&self, filter: &Filter, opts: &PatternOptions) -> Result<PatternResult> {
        let compiled = self.compile(filter)?;
        let m = self.matches(filter, &compiled)?;
        let n = m.len();
        let start = n.saturating_sub(opts.max_entries as u64);
        let spark = match (filter.from, filter.to) {
            (Some(a), Some(b)) if b > a => Some((a, b)),
            _ => {
                let (mut lo, mut hi) = (i64::MAX, i64::MIN);
                self.feed_meta(&m, &mut |t, _| {
                    if let Some(t) = t {
                        lo = lo.min(t);
                        hi = hi.max(t);
                    }
                });
                (lo <= hi).then_some((lo, hi + 1))
            }
        };
        let parts = self.scan(&m, start..n, || Clusterer::new(spark, opts.spark_buckets), |c, pos, e| c.add(&e.message, e.ts, e.level, pos))?;
        let mut all = Clusterer::new(spark, opts.spark_buckets);
        for p in parts {
            all.merge(p);
        }
        let mut r = all.finish(opts);
        r.sampled = start > 0;
        Ok(r)
    }

    /// Most frequent values of `field` among the (most recent 200k) matches.
    pub fn top_values(&self, filter: &Filter, field: &str, limit: usize) -> Result<TopValues> {
        let compiled = self.compile(filter)?;
        let m = self.matches(filter, &compiled)?;
        let n = m.len();
        let start = n.saturating_sub(200_000);
        let parts = self.scan(&m, start..n, TopAcc::default, |acc, _, e| acc.add(analyze::field_values(e, field)))?;
        let mut all = TopAcc::default();
        for p in parts {
            all.merge(p);
        }
        let mut t = all.finish(field, limit);
        t.sampled |= start > 0;
        Ok(t)
    }

    /// Fields seen in a sample of up to `sample` matches spread over the result.
    pub fn fields(&self, filter: &Filter, sample: usize) -> Result<Vec<FieldInfo>> {
        let compiled = self.compile(filter)?;
        let m = self.matches(filter, &compiled)?;
        let n = m.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let step = (n / sample.max(1) as u64).max(1);
        let positions: Vec<u64> = (0..n).step_by(step as usize).take(sample).map(|k| m.get(k)).collect();
        let entries = self.load(&positions, PAGE_TEXT_LIMIT)?;
        Ok(analyze::discover_fields(&entries, 200))
    }

    /// Streams matches from match index `from` in view order, in batches (parsed in parallel);
    /// returns how many were streamed.
    pub fn batches(&self, filter: &Filter, from: u64, batch: usize, mut f: impl FnMut(Vec<LogEntry>) -> Result<()>) -> Result<u64> {
        let compiled = self.compile(filter)?;
        let m = self.matches(filter, &compiled)?;
        let n = m.len();
        let mut start = from.min(n);
        while start < n {
            let end = (start + batch.max(1) as u64).min(n);
            let parts = self.scan(&m, start..end, Vec::new, |v: &mut Vec<LogEntry>, _, e| v.push(e.clone()))?;
            f(parts.into_iter().flatten().collect())?;
            start = end;
        }
        Ok(n - from.min(n))
    }
}

fn truncate_entry(e: &mut LogEntry, limit: usize) {
    for s in [&mut e.raw, &mut e.message] {
        if s.len() > limit {
            let cut = s.floor_char_boundary(limit);
            let extra = s.len() - cut;
            s.truncate(cut);
            s.push_str(&format!("… [+{extra} bytes]"));
        }
    }
}

/// Matching entry indices of `file` in `range`, scanned in parallel.
fn eval_par(file: &LogFile, compiled: &Compiled, range: Range<u64>, source: Option<&str>) -> Result<Vec<u64>> {
    let n = (range.end.saturating_sub(range.start)) as usize;
    let base = range.start;
    let parts = par::chunks(n, 16_384, |r| eval_range(file, compiled, base + r.start as u64..base + r.end as u64, source));
    let mut out = Vec::new();
    for p in parts {
        out.extend(p?);
    }
    Ok(out)
}

fn eval_range(file: &LogFile, compiled: &Compiled, range: Range<u64>, source: Option<&str>) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    if compiled.query.is_all() {
        file.for_each_meta(range, |i, ts, l| {
            if compiled.meta_pass(ts, l) {
                out.push(i);
            }
        });
        return Ok(out);
    }
    let format = file.format();
    let ctx = file.time_ctx();
    let mut reader = file.reader()?;
    let mut start = range.start;
    while start < range.end {
        let end = (start + 8192).min(range.end);
        for (k, sp) in file.spans(start..end).iter().enumerate() {
            if !compiled.meta_pass(sp.ts, sp.level) {
                continue;
            }
            let i = start + k as u64;
            let hit = reader.with_text(sp, |text| {
                let lazy = Lazy::new(text, sp.ts, sp.level, sp.line + 1, source, |t| {
                    let mut e = file.build(t, i, sp, format, &ctx);
                    e.source = source.map(str::to_string);
                    e
                });
                compiled.query.matches(&lazy)
            })?;
            if hit {
                out.push(i);
            }
        }
        start = end;
    }
    Ok(out)
}

/// k-way merge by effective timestamp; ties and entries before the first timestamp keep source
/// order. Packs (source << 48 | index).
fn merge_order(files: &[Arc<LogFile>], counts: &[u64]) -> Vec<u64> {
    let total: u64 = counts.iter().sum();
    let mut out = Vec::with_capacity(total as usize);
    // Per-source cursors over effective timestamps, read in blocks.
    struct Cursor {
        next: u64,
        end: u64,
        buf: Vec<Option<i64>>,
        buf_start: u64,
    }
    let mut cursors: Vec<Cursor> = counts.iter().map(|&c| Cursor { next: 0, end: c, buf: Vec::new(), buf_start: 0 }).collect();
    let fill = |files: &[Arc<LogFile>], s: usize, c: &mut Cursor| {
        c.buf.clear();
        c.buf_start = c.next;
        let end = (c.next + 65_536).min(c.end);
        files[s].for_each_meta(c.next..end, |_, t, _| c.buf.push(t));
    };
    let ts_of = |c: &Cursor| c.buf.get((c.next - c.buf_start) as usize).copied().flatten().unwrap_or(i64::MIN);
    let mut heap = BinaryHeap::new();
    for (s, c) in cursors.iter_mut().enumerate() {
        if c.next < c.end {
            fill(files, s, c);
            heap.push(Reverse((ts_of(c), s, c.next)));
        }
    }
    while let Some(Reverse((_, s, i))) = heap.pop() {
        out.push(((s as u64) << SRC_SHIFT) | i);
        let c = &mut cursors[s];
        c.next += 1;
        if c.next < c.end {
            if c.next >= c.buf_start + c.buf.len() as u64 {
                fill(files, s, c);
            }
            heap.push(Reverse((ts_of(c), s, c.next)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::OpenOptions;
    use crate::time::NaiveTz;

    fn opts() -> OpenOptions {
        OpenOptions { naive_tz: NaiveTz::Utc, year: Some(2024), ..Default::default() }
    }

    fn file(dir: &std::path::Path, name: &str, text: &str) -> Arc<LogFile> {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        LogFile::open(&p, opts()).unwrap()
    }

    #[test]
    fn query_paging_histogram_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let mut text = String::new();
        for i in 0..2000 {
            let level = if i % 100 == 0 { "error" } else { "info" };
            text.push_str(&format!(
                "{{\"ts\":\"2024-01-02T03:{:02}:{:02}Z\",\"level\":\"{level}\",\"msg\":\"request {} done in {}ms\",\"status\":{}}}\n",
                (i / 60) % 60,
                i % 60,
                i,
                i % 37,
                if i % 100 == 0 { 500 } else { 200 }
            ));
        }
        let set = LogSet::single(file(dir.path(), "a.log", &text));
        assert_eq!(set.len(), 2000);
        let all = set.query(&Filter::default(), 1990, 50).unwrap();
        assert_eq!((all.total, all.entries.len(), all.entries[0].index), (2000, 10, 1990));

        let f = Filter::query("status>=500");
        let p = set.query(&f, 0, 5).unwrap();
        assert_eq!(p.total, 20);
        assert_eq!(p.entries[1].index, 100);
        assert_eq!(p.entries[1].level, Level::Error);
        let lv = Filter { levels: vec![Level::Error], ..Default::default() };
        assert_eq!(set.query(&lv, 0, 1).unwrap().total, 20);
        let timed = Filter { from: Some(1_704_164_400_000), to: Some(1_704_164_460_000), ..Default::default() }; // 03:00:00–03:01:00
        assert_eq!(set.query(&timed, 0, 1).unwrap().total, 60);
        assert!(set.query(&Filter::query("status>="), 0, 1).is_err());

        let h = set.histogram(&Filter::default(), 40).unwrap();
        assert_eq!(h.total, 2000);
        assert_eq!(h.buckets.iter().map(|b| b.counts[Level::Error as usize]).sum::<u64>(), 20);

        let pats = set.patterns(&Filter::default(), &PatternOptions::default()).unwrap();
        assert_eq!(pats.patterns.len(), 1);
        assert_eq!(pats.patterns[0].template, "request <NUM> done in <NUM>");
        assert_eq!(pats.patterns[0].count, 2000);

        let top = set.top_values(&Filter::default(), "status", 5).unwrap();
        assert_eq!(top.values[0].value, "200");
        assert_eq!(top.values[1].count, 20);
        let fields = set.fields(&Filter::default(), 100).unwrap();
        assert!(fields.iter().any(|f| f.name == "status" && f.numeric));
        assert_eq!(set.locate(&f, 1_704_164_400_000 + 200_000).unwrap(), Some(2)); // #200 is at 03:03:20

        let mut seen = 0;
        set.batches(&f, 0, 7, |b| {
            seen += b.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, 20);
    }

    #[test]
    fn merge_two_files_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let a = file(dir.path(), "api.log", "2024-01-02T03:04:01Z INFO api one\n2024-01-02T03:04:03Z ERROR api three\n2024-01-02T03:04:05Z INFO api five\n");
        let b = file(
            dir.path(),
            "db.log",
            "{\"time\":\"2024-01-02T03:04:02Z\",\"level\":\"info\",\"msg\":\"db two\"}\n{\"time\":\"2024-01-02T03:04:04Z\",\"level\":\"warn\",\"msg\":\"db four\"}\n",
        );
        let set = LogSet::merged(vec![a, b]);
        let p = set.query(&Filter::default(), 0, 10).unwrap();
        let msgs: Vec<&str> = p.entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["api one", "db two", "api three", "db four", "api five"]);
        assert_eq!(p.entries[1].source.as_deref(), Some("db.log"));
        let f = Filter::query("source=db.log OR level=error");
        let p = set.query(&f, 0, 10).unwrap();
        assert_eq!(p.entries.iter().map(|e| e.index).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(set.histogram(&Filter::default(), 10).unwrap().total, 5);
    }

    #[test]
    fn follow_extends_cached_filter() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.log");
        std::fs::write(&p, "2024-01-02T03:04:01Z ERROR boom\n2024-01-02T03:04:02Z INFO ok\n").unwrap();
        let f = LogFile::open(&p, opts()).unwrap();
        let set = LogSet::single(f.clone());
        let flt = Filter::query("boom OR crash");
        assert_eq!(set.query(&flt, 0, 10).unwrap().total, 1);
        use std::io::Write;
        let mut h = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        h.write_all(b"2024-01-02T03:04:03Z ERROR crash\n\tat frame boom()\n2024-01-02T03:04:04Z INFO fine\n").unwrap();
        f.refresh().unwrap();
        let page = set.query(&flt, 0, 10).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.entries[1].line_count, 2);
    }

    #[test]
    fn truncation() {
        let mut e = LogEntry { raw: "é".repeat(20_000), message: "x".into(), ..Default::default() };
        truncate_entry(&mut e, 101);
        assert!(e.raw.starts_with("éé") && e.raw.contains("… [+"));
        assert_eq!(e.message, "x");
    }
}
