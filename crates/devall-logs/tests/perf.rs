//! Indexes a generated ~200 MB JSON-lines file and times the main operations.
//! Size: `DEVALL_LOGS_PERF_MB` (default 200). Run with `--nocapture` to see the timings.

use std::io::Write;
use std::time::Instant;

use devall_logs::analyze::PatternOptions;
use devall_logs::file::{LogFile, OpenOptions};
use devall_logs::time::NaiveTz;
use devall_logs::{Filter, Level, LogSet};

const LINES_PER_BLOCK: usize = 8192;

/// One block of JSON lines; returns the bytes and the offsets of each line's `HH:MM`.
fn template() -> (Vec<u8>, Vec<usize>) {
    let mut buf = Vec::new();
    let mut marks = Vec::new();
    for i in 0..LINES_PER_BLOCK {
        let (level, status) = match i % 50 {
            0 => ("error", 503),
            1..=4 => ("warn", 429),
            _ => ("info", 200),
        };
        let msg = match i % 4 {
            0 => format!("GET /api/users/{} done in {}ms", i * 7 % 1000, i % 97),
            1 => format!("cache miss for key session:{:08x}", i * 2_654_435_761usize % 0xffff_ffff),
            2 => format!("POST /api/orders/{} created for customer {}", i, i % 313),
            _ => format!("worker {} processed batch {} ({} items)", i % 8, i, i % 64),
        };
        let prefix = "{\"ts\":\"2024-01-02T";
        buf.extend_from_slice(prefix.as_bytes());
        marks.push(buf.len());
        let line = format!(
            "03:00:{:02}.{:03}Z\",\"level\":\"{level}\",\"msg\":\"{msg}\",\"status\":{status},\"user\":{{\"id\":{}}},\"trace_id\":\"{:016x}\",\"host\":\"web-{}\"}}\n",
            (i * 60 / LINES_PER_BLOCK) % 60,
            i % 1000,
            i % 500,
            i * 11,
            i % 4
        );
        buf.extend_from_slice(line.as_bytes());
    }
    (buf, marks)
}

#[test]
fn index_and_query_large_file() {
    let mb: usize = std::env::var("DEVALL_LOGS_PERF_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.log");
    let t0 = Instant::now();
    let (tpl, marks) = template();
    let blocks = (mb << 20).div_ceil(tpl.len()).max(1);
    {
        let mut f = std::io::BufWriter::with_capacity(4 << 20, std::fs::File::create(&path).unwrap());
        let mut block = tpl.clone();
        for b in 0..blocks {
            let (h, m) = (3 + b / 60, b % 60);
            let hm = format!("{h:02}:{m:02}");
            for &o in &marks {
                block[o..o + 5].copy_from_slice(hm.as_bytes());
            }
            f.write_all(&block).unwrap();
        }
        f.flush().unwrap();
    }
    let size = std::fs::metadata(&path).unwrap().len();
    let lines = (blocks * LINES_PER_BLOCK) as u64;
    eprintln!("generated {:.0} MB ({lines} lines) in {:?}", size as f64 / 1048576.0, t0.elapsed());

    let t = Instant::now();
    let file = LogFile::open(&path, OpenOptions { naive_tz: NaiveTz::Utc, year: Some(2024), ..Default::default() }).unwrap();
    let index_time = t.elapsed();
    let info = file.info();
    eprintln!(
        "indexed in {index_time:?} ({:.0} MB/s); index memory {:.1} MB ({:.1} bytes/line)",
        size as f64 / 1048576.0 / index_time.as_secs_f64(),
        info.index_bytes as f64 / 1048576.0,
        info.index_bytes as f64 / lines as f64
    );
    assert_eq!(file.line_count(), lines);
    assert_eq!(file.entry_count(), lines);
    assert_eq!(info.format, devall_logs::Format::Json);

    let set = LogSet::single(file.clone());
    let t = Instant::now();
    let last = set.query(&Filter::default(), lines - 100, 100).unwrap();
    eprintln!("last page (unfiltered) in {:?}", t.elapsed());
    assert_eq!(last.entries.len(), 100);

    let t = Instant::now();
    let errors = set.query(&Filter::query("status>=500"), 0, 100).unwrap();
    eprintln!("field query `status>=500` over all entries in {:?} → {}", t.elapsed(), errors.total);
    let per_block = |pred: fn(usize) -> bool| (0..LINES_PER_BLOCK).filter(|i| pred(i % 50)).count() as u64 * blocks as u64;
    assert_eq!(errors.total, per_block(|r| r == 0));
    assert!(errors.entries.iter().all(|e| e.level == Level::Error));

    let t = Instant::now();
    let text = set.query(&Filter::query("\"cache miss\" web-1"), 0, 10).unwrap();
    eprintln!("text query in {:?} → {}", t.elapsed(), text.total);
    assert!(text.total > 0);

    let t = Instant::now();
    let lv = set.query(&Filter { levels: vec![Level::Warn], ..Default::default() }, 0, 10).unwrap();
    eprintln!("level filter (index only) in {:?} → {}", t.elapsed(), lv.total);
    assert_eq!(lv.total, per_block(|r| (1..=4).contains(&r)));

    let t = Instant::now();
    let h = set.histogram(&Filter::default(), 120).unwrap();
    eprintln!("histogram in {:?} ({} buckets of {} ms)", t.elapsed(), h.buckets.len(), h.bucket_ms);
    assert_eq!(h.total, lines);

    let t = Instant::now();
    let p = set.patterns(&Filter::default(), &PatternOptions { max_entries: 200_000, ..Default::default() }).unwrap();
    eprintln!("patterns over {} entries in {:?} → {} patterns", p.total, t.elapsed(), p.patterns.len());
    assert!(p.patterns.len() <= 10, "{:?}", p.patterns.iter().map(|x| &x.template).collect::<Vec<_>>());
    assert!(p.patterns.iter().any(|x| x.template == "GET /api/users/<NUM> done in <NUM>"));
}
