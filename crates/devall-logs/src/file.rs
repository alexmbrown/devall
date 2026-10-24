//! Huge-file access: a background indexer records line starts (≈4 bytes/line) and per-entry
//! metadata (timestamp, level); lines are read back on demand with positioned reads, so the file
//! itself is never loaded. Gzip files are decompressed (and UTF-16 files transcoded) to a temp
//! file first. Followed files are polled with [`LogFile::refresh`], which appends new lines and
//! detects truncation and rotation (size shrink or a changed first-KiB fingerprint).

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime};

use serde::Serialize;

use crate::index::{NONE, Packed};
use crate::parse::{self, Detection, Format, Level, LogEntry, Meta};
use crate::time::{NaiveTz, TimeCtx};
use crate::{LogError, Result, par};

/// Bytes per read batch per worker.
const CHUNK: usize = 4 << 20;
/// Lines longer than this are not parsed for metadata (they still become entries).
const MAX_META_LINE: usize = 256 * 1024;
const SAMPLE: usize = 64 * 1024;
const FINGERPRINT: usize = 1024;

#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    pub naive_tz: NaiveTz,
    /// Year for stamps without one (syslog, klog); defaults to the file's modification year.
    pub year: Option<i32>,
    /// Skip detection and parse as this format.
    pub format: Option<Format>,
    /// Display name (defaults to the file name).
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Starting,
    Decompressing,
    Transcoding,
    Indexing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub lines: u64,
    pub entries: u64,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What [`LogFile::refresh`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefreshKind {
    Unchanged,
    Appended,
    Truncated,
    Rotated,
    /// The path is gone (mid-rotation or deleted); the index is kept.
    Missing,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refresh {
    pub kind: RefreshKind,
    pub lines: u64,
    pub entries: u64,
    /// Bumped whenever already-indexed entries changed (truncate, rotate, re-detection).
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    /// On-disk size of the opened file.
    pub size: u64,
    /// Size of the indexed text (after decompression/transcoding).
    pub data_size: u64,
    pub encoding: Encoding,
    pub compressed: bool,
    pub format: Format,
    pub format_label: &'static str,
    pub ts_mode: bool,
    pub lines: u64,
    pub entries: u64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub indexing: bool,
    pub followable: bool,
    pub index_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transform {
    None,
    Gunzip,
    Utf16 { le: bool },
    GunzipUtf16 { le: bool },
}

#[derive(Default)]
struct Index {
    lines: Packed,
    entry_line: Packed,
    /// Effective timestamp: the entry's own, else carried forward from the previous entry.
    entry_ts: Packed,
    entry_level: Vec<u8>,
    /// Data offset just past the last indexed byte.
    indexed_to: u64,
    /// The last indexed line has no newline yet.
    partial_tail: bool,
    last_ts: i64,
    min_ts: i64,
    max_ts: i64,
}

impl Index {
    fn new(start: u64) -> Self {
        Self { indexed_to: start, last_ts: NONE, min_ts: i64::MAX, max_ts: i64::MIN, ..Default::default() }
    }

    fn push(&mut self, li: &LineInfo) {
        let line_idx = self.lines.len() as i64;
        self.lines.push(li.start as i64);
        if li.starts || self.entry_line.is_empty() {
            self.entry_line.push(line_idx);
            if let Some(t) = li.ts {
                self.last_ts = t;
                self.min_ts = self.min_ts.min(t);
                self.max_ts = self.max_ts.max(t);
            }
            self.entry_ts.push(self.last_ts);
            self.entry_level.push(li.level as u8);
        }
    }

    fn shrink(&mut self) {
        self.lines.shrink_to_fit();
        self.entry_line.shrink_to_fit();
        self.entry_ts.shrink_to_fit();
        self.entry_level.shrink_to_fit();
    }

    fn pop_line(&mut self) -> Option<u64> {
        let n = self.lines.len();
        if n == 0 {
            return None;
        }
        let start = self.lines.get(n - 1) as u64;
        self.lines.truncate(n - 1);
        if self.entry_line.last() == Some(n as i64 - 1) {
            let e = self.entry_line.len() - 1;
            self.entry_line.truncate(e);
            self.entry_ts.truncate(e);
            self.entry_level.truncate(e);
        }
        self.last_ts = self.entry_ts.last().unwrap_or(NONE);
        self.indexed_to = start;
        self.partial_tail = false;
        Some(start)
    }
}

#[derive(Debug, Clone, Copy)]
struct LineInfo {
    start: u64,
    starts: bool,
    ts: Option<i64>,
    level: Level,
}

/// Byte span and position of one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntrySpan {
    pub start: u64,
    pub end: u64,
    /// 0-based first line.
    pub line: u64,
    pub ts: Option<i64>,
    pub level: Level,
}

#[derive(Default)]
struct Follow {
    fingerprint: Option<(usize, u64)>,
}

/// An indexed log file (or stream).
pub struct LogFile {
    name: String,
    path: PathBuf,
    data_path: PathBuf,
    _temp: Option<tempfile::TempPath>,
    data_start: u64,
    transform: Transform,
    encoding: Encoding,
    size: u64,
    followable: bool,
    naive: NaiveTz,
    year: i32,
    forced_format: Option<Format>,
    detection: RwLock<(Detection, usize)>,
    index: RwLock<Index>,
    generation: AtomicU64,
    follow: Mutex<Follow>,
    progress: Mutex<Progress>,
    done: AtomicBool,
    cancel: AtomicBool,
    started: Instant,
}

/// Appends to a stream source created with [`LogFile::stream`].
pub struct StreamSink {
    file: Mutex<File>,
}

impl StreamSink {
    /// Appends text (a trailing newline is added when missing).
    pub fn write(&self, text: &str) -> Result<()> {
        let mut f = self.file.lock().unwrap();
        f.write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            f.write_all(b"\n")?;
        }
        Ok(())
    }
}

fn file_year(md: &std::fs::Metadata) -> i32 {
    use chrono::Datelike;
    md.modified().ok().map(|t| chrono::DateTime::<chrono::Utc>::from(t).year()).unwrap_or_else(crate::time::current_year)
}

fn sniff_utf16(b: &[u8]) -> Option<bool> {
    let n = b.len().min(512) & !1;
    if n < 16 {
        return None;
    }
    let (mut even0, mut odd0) = (0, 0);
    for (i, &c) in b[..n].iter().enumerate() {
        if c == 0 {
            if i % 2 == 0 {
                even0 += 1;
            } else {
                odd0 += 1;
            }
        }
    }
    let half = n / 2;
    if odd0 * 10 > half * 4 && even0 * 10 < half {
        Some(true)
    } else if even0 * 10 > half * 4 && odd0 * 10 < half {
        Some(false)
    } else {
        None
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in b {
        h ^= c as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn temp_path() -> Result<tempfile::TempPath> {
    let f = tempfile::Builder::new().prefix("devall-log-").suffix(".log").tempfile()?;
    Ok(f.into_temp_path())
}

struct Counting<R> {
    inner: R,
    n: Arc<AtomicU64>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let k = self.inner.read(buf)?;
        self.n.fetch_add(k as u64, Ordering::Relaxed);
        Ok(k)
    }
}

impl LogFile {
    /// Opens and fully indexes `path` on the calling thread.
    pub fn open(path: impl AsRef<Path>, opts: OpenOptions) -> Result<Arc<LogFile>> {
        let f = Arc::new(Self::prepare(path.as_ref(), opts)?);
        f.run(&mut |_| {})?;
        Ok(f)
    }

    /// Opens `path` and indexes it on a background thread; `on_progress` is called after every
    /// batch (≈ every few MB) and once at the end. Lines are readable as soon as they're indexed.
    pub fn open_background(path: impl AsRef<Path>, opts: OpenOptions, mut on_progress: impl FnMut(&Progress) + Send + 'static) -> Result<Arc<LogFile>> {
        let f = Arc::new(Self::prepare(path.as_ref(), opts)?);
        let worker = f.clone();
        std::thread::Builder::new().name("devall-log-index".into()).spawn(move || {
            let _ = worker.run(&mut on_progress);
        })?;
        Ok(f)
    }

    /// An empty, followable source backed by a temp file; write to it with the returned sink
    /// (Docker/Kubernetes log streams, pasted text).
    pub fn stream(name: &str, opts: OpenOptions) -> Result<(Arc<LogFile>, StreamSink)> {
        let temp = temp_path()?;
        let file = std::fs::OpenOptions::new().append(true).open(&temp)?;
        let path = temp.to_path_buf();
        let mut f = Self::prepare(&path, OpenOptions { name: Some(name.to_string()), ..opts })?;
        f._temp = Some(temp);
        let f = Arc::new(f);
        f.run(&mut |_| {})?;
        Ok((f, StreamSink { file: Mutex::new(file) }))
    }

    fn prepare(path: &Path, opts: OpenOptions) -> Result<Self> {
        let md = std::fs::metadata(path)?;
        if md.is_dir() {
            return Err(LogError::Invalid(format!("{} is a folder", path.display())));
        }
        let mut head = [0u8; 512];
        let n = {
            let mut f = File::open(path)?;
            read_full(&mut f, &mut head)?
        };
        let head = &head[..n];
        let gz = head.starts_with(&[0x1f, 0x8b]);
        let inner: Vec<u8> = if gz {
            let mut d = flate2::read::MultiGzDecoder::new(File::open(path)?);
            let mut buf = vec![0u8; 512];
            let k = read_full(&mut d, &mut buf).unwrap_or(0);
            buf.truncate(k);
            buf
        } else {
            head.to_vec()
        };
        let (encoding, data_start) = if inner.starts_with(&[0xEF, 0xBB, 0xBF]) {
            (Encoding::Utf8Bom, 3)
        } else if inner.starts_with(&[0xFF, 0xFE]) {
            (Encoding::Utf16Le, 2)
        } else if inner.starts_with(&[0xFE, 0xFF]) {
            (Encoding::Utf16Be, 2)
        } else {
            match sniff_utf16(&inner) {
                Some(true) => (Encoding::Utf16Le, 0),
                Some(false) => (Encoding::Utf16Be, 0),
                None => (Encoding::Utf8, 0),
            }
        };
        let utf16 = matches!(encoding, Encoding::Utf16Le | Encoding::Utf16Be);
        let le = encoding == Encoding::Utf16Le;
        let transform = match (gz, utf16) {
            (false, false) => Transform::None,
            (true, false) => Transform::Gunzip,
            (false, true) => Transform::Utf16 { le },
            (true, true) => Transform::GunzipUtf16 { le },
        };
        let temp = if transform == Transform::None { None } else { Some(temp_path()?) };
        let data_path = temp.as_ref().map(|t| t.to_path_buf()).unwrap_or_else(|| path.to_path_buf());
        // Transforms write plain UTF-8 without a BOM.
        let data_start = if transform == Transform::None { data_start } else { 0 };
        let name = opts.name.clone().unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string()));
        Ok(Self {
            name,
            path: path.to_path_buf(),
            data_path,
            _temp: temp,
            data_start,
            transform,
            encoding,
            size: md.len(),
            followable: transform == Transform::None,
            naive: opts.naive_tz,
            year: opts.year.unwrap_or_else(|| file_year(&md)),
            forced_format: opts.format,
            detection: RwLock::new((Detection { format: opts.format.unwrap_or_default(), ts_mode: false }, 0)),
            index: RwLock::new(Index::new(data_start)),
            generation: AtomicU64::new(0),
            follow: Mutex::new(Follow::default()),
            progress: Mutex::new(Progress::default()),
            done: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            started: Instant::now(),
        })
    }

    fn set_progress(&self, f: impl FnOnce(&mut Progress)) -> Progress {
        let mut p = self.progress.lock().unwrap();
        f(&mut p);
        p.elapsed_ms = self.started.elapsed().as_millis() as u64;
        p.clone()
    }

    fn run(&self, on_progress: &mut dyn FnMut(&Progress)) -> Result<()> {
        let r = self.run_inner(on_progress);
        let p = match &r {
            Ok(()) => self.set_progress(|p| {
                p.phase = Phase::Ready;
                p.bytes_done = p.bytes_total;
            }),
            Err(e) => {
                let msg = e.to_string();
                self.set_progress(|p| {
                    p.phase = Phase::Failed;
                    p.error = Some(msg);
                })
            }
        };
        self.done.store(true, Ordering::Release);
        on_progress(&p);
        r
    }

    fn run_inner(&self, on_progress: &mut dyn FnMut(&Progress)) -> Result<()> {
        if self.transform != Transform::None {
            self.materialize(on_progress)?;
        }
        self.detect_now()?;
        let total = std::fs::metadata(&self.data_path)?.len().saturating_sub(self.data_start);
        let p = self.set_progress(|p| {
            p.phase = Phase::Indexing;
            p.bytes_total = total;
            p.bytes_done = 0;
        });
        on_progress(&p);
        self.index_from(self.data_start, true, on_progress)?;
        self.index.write().unwrap().shrink();
        self.update_fingerprint();
        Ok(())
    }

    /// Decompresses / transcodes the source into the temp data file.
    fn materialize(&self, on_progress: &mut dyn FnMut(&Progress)) -> Result<()> {
        let read = Arc::new(AtomicU64::new(0));
        let src = Counting { inner: BufReader::with_capacity(1 << 20, File::open(&self.path)?), n: read.clone() };
        let mut out = std::io::BufWriter::with_capacity(1 << 20, File::create(&self.data_path)?);
        let phase = if matches!(self.transform, Transform::Utf16 { .. }) { Phase::Transcoding } else { Phase::Decompressing };
        let p = self.set_progress(|p| {
            p.phase = phase;
            p.bytes_total = self.size;
        });
        on_progress(&p);
        let mut reader: Box<dyn Read> = match self.transform {
            Transform::Gunzip | Transform::GunzipUtf16 { .. } => Box::new(flate2::read::MultiGzDecoder::new(src)),
            _ => Box::new(src),
        };
        let utf16 = match self.transform {
            Transform::Utf16 { le } | Transform::GunzipUtf16 { le } => Some(le),
            _ => None,
        };
        let mut buf = vec![0u8; 1 << 20];
        let mut carry: Vec<u8> = Vec::new();
        let mut first = true;
        let mut last_report = Instant::now();
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(LogError::Cancelled);
            }
            let k = reader.read(&mut buf)?;
            if k == 0 {
                break;
            }
            match utf16 {
                None => out.write_all(&buf[..k])?,
                Some(le) => {
                    carry.extend_from_slice(&buf[..k]);
                    let mut units: Vec<u16> = carry.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
                    let mut keep = carry.len() % 2;
                    if let Some(&last) = units.last()
                        && (0xD800..0xDC00).contains(&last)
                    {
                        units.pop();
                        keep += 2;
                    }
                    let skip = usize::from(first && units.first() == Some(&0xFEFF));
                    first = false;
                    let s: String = char::decode_utf16(units[skip..].iter().copied()).map(|r| r.unwrap_or('\u{FFFD}')).collect();
                    out.write_all(s.as_bytes())?;
                    carry.drain(..carry.len() - keep);
                }
            }
            if last_report.elapsed().as_millis() > 200 {
                last_report = Instant::now();
                let done = read.load(Ordering::Relaxed);
                let p = self.set_progress(|p| p.bytes_done = done);
                on_progress(&p);
            }
        }
        out.flush()?;
        Ok(())
    }

    /// Format detection from the first 64 KiB.
    fn detect_now(&self) -> Result<()> {
        let mut f = File::open(&self.data_path)?;
        f.seek(SeekFrom::Start(self.data_start))?;
        let mut buf = vec![0u8; SAMPLE];
        let n = read_full(&mut f, &mut buf)?;
        buf.truncate(n);
        let text = String::from_utf8_lossy(&buf);
        let mut lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
        if n == SAMPLE && lines.len() > 1 {
            lines.pop(); // probably cut mid-line
        }
        let sampled = lines.iter().filter(|l| !l.trim().is_empty()).count();
        let ctx = self.ctx();
        let mut det = parse::detect(lines.iter().copied(), &ctx);
        if let Some(f) = self.forced_format {
            det.format = f;
        }
        *self.detection.write().unwrap() = (det, sampled);
        Ok(())
    }

    fn ctx(&self) -> TimeCtx {
        TimeCtx::new(self.naive, self.year)
    }

    /// Indexes from `from` to EOF. With `final_partial`, an unterminated last line is indexed too.
    fn index_from(&self, from: u64, final_partial: bool, on_progress: &mut dyn FnMut(&Progress)) -> Result<(u64, u64)> {
        let det = self.detection.read().unwrap().0;
        let mut file = File::open(&self.data_path)?;
        file.seek(SeekFrom::Start(from))?;
        let threads = par::threads();
        let want = threads * CHUNK;
        let mut carry: Vec<u8> = Vec::new();
        let mut offset = from;
        let (lines0, entries0) = {
            let idx = self.index.read().unwrap();
            (idx.lines.len() as u64, idx.entry_line.len() as u64)
        };
        let mut first_line = lines0 == 0;
        let mut last_report = Instant::now();
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(LogError::Cancelled);
            }
            let mut buf = std::mem::take(&mut carry);
            let have = buf.len();
            buf.resize(have + want, 0);
            let k = read_full(&mut file, &mut buf[have..])?;
            buf.truncate(have + k);
            let eof = k < want;
            if buf.is_empty() {
                break;
            }
            let complete = memchr::memrchr(b'\n', &buf).map(|p| p + 1).unwrap_or(0);
            let body_end = if eof && final_partial { buf.len() } else { complete };
            if body_end == 0 {
                if eof {
                    break;
                }
                carry = buf;
                continue;
            }
            let body = &buf[..body_end];
            let ctx_naive = (self.naive, self.year);
            let pieces = split_lines(body, threads);
            let results: Vec<Vec<LineInfo>> = par::chunks(pieces.len(), 1, |r| {
                let ctx = TimeCtx::new(ctx_naive.0, ctx_naive.1);
                let mut out = Vec::new();
                for p in &pieces[r] {
                    scan_piece(&body[p.clone()], offset + p.start as u64, det, &ctx, &mut out);
                }
                out
            });
            {
                let mut idx = self.index.write().unwrap();
                for v in &results {
                    for li in v {
                        let mut li = *li;
                        if first_line {
                            li.starts = true;
                            first_line = false;
                        }
                        idx.push(&li);
                    }
                }
                idx.indexed_to = offset + body_end as u64;
                idx.partial_tail = body_end > 0 && buf[body_end - 1] != b'\n';
            }
            offset += body_end as u64;
            carry = buf[body_end..].to_vec();
            if last_report.elapsed().as_millis() >= 100 || eof {
                last_report = Instant::now();
                let (l, e) = self.counts();
                let done = offset - self.data_start;
                let p = self.set_progress(|p| {
                    p.bytes_done = done;
                    p.lines = l;
                    p.entries = e;
                });
                on_progress(&p);
            }
            if eof {
                break;
            }
        }
        let (l, e) = self.counts();
        Ok((l - lines0, e.saturating_sub(entries0)))
    }

    fn counts(&self) -> (u64, u64) {
        let idx = self.index.read().unwrap();
        (idx.lines.len() as u64, idx.entry_line.len() as u64)
    }

    fn update_fingerprint(&self) {
        let fp = std::fs::File::open(&self.data_path).ok().and_then(|mut f| {
            f.seek(SeekFrom::Start(self.data_start)).ok()?;
            let mut b = vec![0u8; FINGERPRINT];
            let n = read_full(&mut f, &mut b).ok()?;
            Some((n, fnv(&b[..n])))
        });
        self.follow.lock().unwrap().fingerprint = fp;
    }

    // ---------- public API ----------

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn is_followable(&self) -> bool {
        self.followable
    }

    /// Stops background indexing.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn progress(&self) -> Progress {
        let mut p = self.progress.lock().unwrap().clone();
        if !self.is_done() {
            p.elapsed_ms = self.started.elapsed().as_millis() as u64;
        }
        p
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn detection(&self) -> Detection {
        self.detection.read().unwrap().0
    }

    pub fn format(&self) -> Format {
        self.detection().format
    }

    pub fn line_count(&self) -> u64 {
        self.index.read().unwrap().lines.len() as u64
    }

    pub fn entry_count(&self) -> u64 {
        self.index.read().unwrap().entry_line.len() as u64
    }

    pub fn info(&self) -> FileInfo {
        let idx = self.index.read().unwrap();
        let det = self.detection();
        FileInfo {
            name: self.name.clone(),
            path: self.path.display().to_string(),
            size: std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(self.size),
            data_size: idx.indexed_to.saturating_sub(self.data_start),
            encoding: self.encoding,
            compressed: matches!(self.transform, Transform::Gunzip | Transform::GunzipUtf16 { .. }),
            format: det.format,
            format_label: det.format.label(),
            ts_mode: det.ts_mode,
            lines: idx.lines.len() as u64,
            entries: idx.entry_line.len() as u64,
            first_ts: (idx.min_ts != i64::MAX).then_some(idx.min_ts),
            last_ts: (idx.max_ts != i64::MIN).then_some(idx.max_ts),
            indexing: !self.is_done(),
            followable: self.followable,
            index_bytes: idx.lines.heap_bytes() + idx.entry_line.heap_bytes() + idx.entry_ts.heap_bytes() + idx.entry_level.capacity(),
            error: self.progress.lock().unwrap().error.clone(),
        }
    }

    /// Effective timestamp (own or carried forward) and level of entry `i`.
    pub fn entry_meta(&self, i: u64) -> Option<(Option<i64>, Level)> {
        let idx = self.index.read().unwrap();
        let i = i as usize;
        if i >= idx.entry_line.len() {
            return None;
        }
        let ts = idx.entry_ts.get(i);
        Some(((ts != NONE).then_some(ts), Level::from_u8(idx.entry_level[i])))
    }

    /// Spans of entries in `range` (clamped).
    pub fn spans(&self, range: Range<u64>) -> Vec<EntrySpan> {
        let idx = self.index.read().unwrap();
        let n = idx.entry_line.len() as u64;
        let (a, b) = (range.start.min(n) as usize, range.end.min(n) as usize);
        let nlines = idx.lines.len();
        (a..b)
            .map(|i| {
                let line = idx.entry_line.get(i) as usize;
                let next_line = if i + 1 < idx.entry_line.len() { idx.entry_line.get(i + 1) as usize } else { nlines };
                let start = idx.lines.get(line) as u64;
                let end = if next_line < nlines { idx.lines.get(next_line) as u64 } else { idx.indexed_to };
                let ts = idx.entry_ts.get(i);
                EntrySpan { start, end, line: line as u64, ts: (ts != NONE).then_some(ts), level: Level::from_u8(idx.entry_level[i]) }
            })
            .collect()
    }

    /// Calls `f(entry index, ts, level)` for every entry, in order, without reading the file.
    pub fn for_each_meta(&self, range: Range<u64>, mut f: impl FnMut(u64, Option<i64>, Level)) {
        let idx = self.index.read().unwrap();
        let n = idx.entry_line.len() as u64;
        for i in range.start.min(n)..range.end.min(n) {
            let ts = idx.entry_ts.get(i as usize);
            f(i, (ts != NONE).then_some(ts), Level::from_u8(idx.entry_level[i as usize]));
        }
    }

    /// Like [`Self::for_each_meta`] for arbitrary entry indices (one lock for the whole batch).
    pub fn metas_for(&self, idxs: &[u64], mut f: impl FnMut(u64, Option<i64>, Level)) {
        let idx = self.index.read().unwrap();
        let n = idx.entry_line.len() as u64;
        for &i in idxs {
            if i < n {
                let ts = idx.entry_ts.get(i as usize);
                f(i, (ts != NONE).then_some(ts), Level::from_u8(idx.entry_level[i as usize]));
            }
        }
    }

    /// Raw text of physical lines `range` (0-based, clamped).
    pub fn read_lines(&self, range: Range<u64>) -> Result<Vec<String>> {
        let (offsets, end) = {
            let idx = self.index.read().unwrap();
            let n = idx.lines.len() as u64;
            let (a, b) = (range.start.min(n), range.end.min(n));
            if a >= b {
                return Ok(Vec::new());
            }
            let offs: Vec<u64> = (a..b).map(|i| idx.lines.get(i as usize) as u64).collect();
            let end = if b < n { idx.lines.get(b as usize) as u64 } else { idx.indexed_to };
            (offs, end)
        };
        let base = offsets[0];
        let bytes = self.read_at(base, (end - base) as usize)?;
        Ok(offsets
            .iter()
            .enumerate()
            .map(|(k, &o)| {
                let s = (o - base) as usize;
                let e = offsets.get(k + 1).map(|&x| (x - base) as usize).unwrap_or(bytes.len());
                trim_eol(&String::from_utf8_lossy(&bytes[s.min(bytes.len())..e.min(bytes.len())])).to_string()
            })
            .collect())
    }

    pub(crate) fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut f = File::open(&self.data_path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len];
        let n = read_full(&mut f, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// A windowed reader for scanning many entries.
    pub fn reader(&self) -> Result<EntryReader> {
        Ok(EntryReader { file: File::open(&self.data_path)?, buf: Vec::new(), start: 0 })
    }

    /// Fully parsed entries `range` (index = entry index).
    pub fn entries(&self, range: Range<u64>) -> Result<Vec<LogEntry>> {
        let spans = self.spans(range.clone());
        let mut r = self.reader()?;
        let det = self.detection();
        let ctx = self.ctx();
        spans.iter().enumerate().map(|(k, s)| Ok(self.build(&r.text(s)?, range.start + k as u64, s, det.format, &ctx))).collect()
    }

    pub fn entry(&self, i: u64) -> Result<Option<LogEntry>> {
        Ok(self.entries(i..i + 1)?.pop())
    }

    pub(crate) fn build(&self, text: &str, index: u64, span: &EntrySpan, format: Format, ctx: &TimeCtx) -> LogEntry {
        let mut e = parse::build_entry(text, index, span.line + 1, format, ctx);
        if e.level == Level::Unknown {
            e.level = span.level;
        }
        e
    }

    pub(crate) fn time_ctx(&self) -> TimeCtx {
        self.ctx()
    }

    // ---------- follow ----------

    /// Picks up appended data, truncation and rotation. Cheap when nothing changed.
    pub fn refresh(&self) -> Result<Refresh> {
        let result = |me: &Self, kind| {
            let (l, e) = me.counts();
            Refresh { kind, lines: l, entries: e, generation: me.generation() }
        };
        if !self.followable || !self.is_done() {
            return Ok(result(self, RefreshKind::Unchanged));
        }
        let follow = self.follow.lock().unwrap();
        let md = match std::fs::metadata(&self.data_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result(self, RefreshKind::Missing)),
            Err(e) => return Err(e.into()),
        };
        let size = md.len();
        let (indexed_to, partial) = {
            let idx = self.index.read().unwrap();
            (idx.indexed_to, idx.partial_tail)
        };
        let mut rotated = false;
        if let Some((n, h)) = follow.fingerprint
            && n > 0
            && size >= self.data_start + n as u64
        {
            let head = self.read_at(self.data_start, n)?;
            rotated = head.len() == n && fnv(&head) != h;
        }
        let kind = if rotated {
            Some(RefreshKind::Rotated)
        } else if size < indexed_to {
            Some(RefreshKind::Truncated)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.reset();
            drop(follow);
            self.detect_now()?;
            self.index_from(self.data_start, false, &mut |_| {})?;
            self.update_fingerprint();
            return Ok(result(self, kind));
        }
        if size == indexed_to {
            return Ok(result(self, RefreshKind::Unchanged));
        }
        // Too little was seen to trust the detection (new or streamed file): detect again.
        let sampled = self.detection.read().unwrap().1;
        if sampled < 20 && self.forced_format.is_none() {
            let before = self.detection();
            self.detect_now()?;
            if self.detection() != before && self.entry_count() > 0 {
                self.reset();
                drop(follow);
                self.index_from(self.data_start, false, &mut |_| {})?;
                self.update_fingerprint();
                return Ok(result(self, RefreshKind::Appended));
            }
        }
        let mut resume = indexed_to;
        if partial {
            let mut idx = self.index.write().unwrap();
            if let Some(start) = idx.pop_line() {
                resume = start;
            }
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        let (nl, _) = self.index_from(resume, false, &mut |_| {})?;
        if follow.fingerprint.is_none_or(|(n, _)| n < FINGERPRINT) {
            drop(follow);
            self.update_fingerprint();
        }
        Ok(result(self, if nl > 0 || partial { RefreshKind::Appended } else { RefreshKind::Unchanged }))
    }

    fn reset(&self) {
        *self.index.write().unwrap() = Index::new(self.data_start);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for LogFile {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Reads entries through a sliding buffer so sequential (or nearby) spans cost one read.
pub struct EntryReader {
    file: File,
    buf: Vec<u8>,
    start: u64,
}

const WINDOW: usize = 1 << 20;

impl EntryReader {
    /// Text of an entry, without its trailing newline.
    pub fn text(&mut self, s: &EntrySpan) -> Result<String> {
        Ok(self.bytes(s)?.map(|b| trim_eol(&String::from_utf8_lossy(b)).to_string()).unwrap_or_default())
    }

    /// Calls `f` with the entry text (borrowed when valid UTF-8).
    pub fn with_text<R>(&mut self, s: &EntrySpan, f: impl FnOnce(&str) -> R) -> Result<R> {
        let b = self.bytes(s)?.unwrap_or(&[]);
        Ok(match std::str::from_utf8(b) {
            Ok(t) => f(trim_eol(t)),
            Err(_) => f(trim_eol(&String::from_utf8_lossy(b))),
        })
    }

    fn bytes(&mut self, s: &EntrySpan) -> Result<Option<&[u8]>> {
        let len = (s.end.saturating_sub(s.start)) as usize;
        let have = s.start >= self.start && s.end <= self.start + self.buf.len() as u64;
        if !have {
            self.file.seek(SeekFrom::Start(s.start))?;
            self.buf.resize(len.max(WINDOW), 0);
            let n = read_full(&mut self.file, &mut self.buf)?;
            self.buf.truncate(n);
            self.start = s.start;
        }
        let a = (s.start - self.start) as usize;
        let b = (a + len).min(self.buf.len());
        Ok(Some(&self.buf[a.min(b)..b]))
    }
}

fn trim_eol(s: &str) -> &str {
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.strip_suffix('\r').unwrap_or(s)
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Splits `body` into up to `parts` ranges that end on line boundaries.
fn split_lines(body: &[u8], parts: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let target = body.len().div_ceil(parts.max(1)).max(64 * 1024);
    let mut start = 0;
    while start < body.len() {
        let mut end = (start + target).min(body.len());
        if end < body.len() {
            end = match memchr::memchr(b'\n', &body[end..]) {
                Some(p) => end + p + 1,
                None => body.len(),
            };
        }
        out.push(start..end);
        start = end;
    }
    out
}

fn scan_piece(piece: &[u8], base: u64, det: Detection, ctx: &TimeCtx, out: &mut Vec<LineInfo>) {
    let mut pos = 0;
    while pos < piece.len() {
        let end = memchr::memchr(b'\n', &piece[pos..]).map(|p| pos + p).unwrap_or(piece.len());
        let line = &piece[pos..end];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let (meta, starts) = if line.len() > MAX_META_LINE {
            (Meta { ts: None, level: Level::Unknown, structured: true }, true)
        } else {
            let text: std::borrow::Cow<str> = String::from_utf8_lossy(line);
            let m = parse::line_meta(&text, det.format, ctx);
            let s = parse::starts_entry(&text, &m, det.ts_mode);
            (m, s)
        };
        out.push(LineInfo { start: base + pos as u64, starts, ts: meta.ts, level: meta.level });
        pos = end + 1;
    }
}

/// Seconds since the Unix epoch of a `SystemTime` (helper for callers).
pub fn system_time_ms(t: SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::format_iso;

    fn utc() -> OpenOptions {
        OpenOptions { naive_tz: NaiveTz::Utc, year: Some(2024), ..Default::default() }
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn index_and_random_access() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("app.log");
        let mut text = String::new();
        for i in 0..3000 {
            text.push_str(&format!("2024-01-02T03:04:{:02}Z INFO line {i}\r\n", i % 60));
            if i % 1000 == 0 {
                text.push_str("java.lang.RuntimeException: x\n\tat a.b(C.java:1)\n");
            }
        }
        text.push_str("2024-01-02T03:05:00Z ERROR last line without newline");
        write(&p, &text);
        let f = LogFile::open(&p, utc()).unwrap();
        assert_eq!(f.format(), Format::Prefixed);
        assert_eq!(f.line_count(), 3001 + 6);
        assert_eq!(f.entry_count(), 3001);
        let lines = f.read_lines(0..3).unwrap();
        assert_eq!(lines[0], "2024-01-02T03:04:00Z INFO line 0");
        assert_eq!(lines[1], "java.lang.RuntimeException: x");
        let e = f.entries(0..2).unwrap();
        assert_eq!(e[0].line_count, 3);
        assert_eq!(e[0].raw, "2024-01-02T03:04:00Z INFO line 0\r\njava.lang.RuntimeException: x\n\tat a.b(C.java:1)");
        assert_eq!(e[1].line_no, 4);
        assert_eq!(e[1].message, "line 1");
        let last = f.entry(3000).unwrap().unwrap();
        assert_eq!((last.level, last.message.as_str()), (Level::Error, "last line without newline"));
        assert_eq!(format_iso(last.ts.unwrap()), "2024-01-02T03:05:00.000Z");
        let info = f.info();
        assert!(!info.indexing && info.first_ts.is_some());
    }

    #[test]
    fn gzip_and_utf16() {
        let dir = tempfile::tempdir().unwrap();
        let text = "{\"level\":\"info\",\"msg\":\"a\"}\n{\"level\":\"error\",\"msg\":\"ü€\"}\n";
        let gz = dir.path().join("x.log.gz");
        let mut enc = flate2::write::GzEncoder::new(File::create(&gz).unwrap(), flate2::Compression::fast());
        enc.write_all(text.as_bytes()).unwrap();
        enc.finish().unwrap();
        let f = LogFile::open(&gz, utc()).unwrap();
        assert!(f.info().compressed && !f.is_followable());
        assert_eq!(f.entries(0..2).unwrap()[1].message, "ü€");

        let u16p = dir.path().join("w.log");
        let mut bytes = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&u16p, &bytes).unwrap();
        let f = LogFile::open(&u16p, utc()).unwrap();
        assert_eq!(f.info().encoding, Encoding::Utf16Le);
        let e = f.entries(0..2).unwrap();
        assert_eq!((e[1].level, e[1].message.as_str()), (Level::Error, "ü€"));

        let bom = dir.path().join("bom.log");
        std::fs::write(&bom, [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat()).unwrap();
        let f = LogFile::open(&bom, utc()).unwrap();
        assert_eq!(f.format(), Format::Json);
        assert_eq!(f.entries(0..1).unwrap()[0].message, "a");
    }

    #[test]
    fn follow_append_truncate_rotate() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tail.log");
        write(&p, "2024-01-02T03:04:05Z INFO one\n2024-01-02T03:04:06Z INFO two\n");
        let f = LogFile::open(&p, utc()).unwrap();
        assert_eq!(f.entry_count(), 2);
        assert_eq!(f.refresh().unwrap().kind, RefreshKind::Unchanged);

        // Append, including a partial line that only shows up once terminated.
        append(&p, "2024-01-02T03:04:07Z WARN three\n\tat stack.frame()\n2024-01-02T03:04:08Z INFO fo");
        let r = f.refresh().unwrap();
        assert_eq!((r.kind, r.entries), (RefreshKind::Appended, 3));
        assert_eq!(f.entries(2..3).unwrap()[0].line_count, 2);
        append(&p, "ur\n");
        let r = f.refresh().unwrap();
        assert_eq!(r.entries, 4);
        assert_eq!(f.entries(3..4).unwrap()[0].message, "four");

        // Truncate (copytruncate rotation).
        write(&p, "2024-01-02T04:00:00Z ERROR fresh\n");
        let r = f.refresh().unwrap();
        assert_eq!((r.kind, r.entries), (RefreshKind::Truncated, 1));
        assert!(r.generation > 0);

        // Rename + recreate rotation with a longer file: fingerprint changes.
        std::fs::rename(&p, dir.path().join("tail.log.1")).unwrap();
        assert_eq!(f.refresh().unwrap().kind, RefreshKind::Missing);
        write(&p, "2024-01-02T05:00:00Z INFO rotated a\n2024-01-02T05:00:01Z INFO rotated b\n2024-01-02T05:00:02Z INFO rotated c\n");
        let r = f.refresh().unwrap();
        assert_eq!((r.kind, r.entries), (RefreshKind::Rotated, 3));
        assert_eq!(f.entries(0..1).unwrap()[0].message, "rotated a");

        // A file that ended without a newline gets its last line re-read when it grows.
        let q = dir.path().join("partial.log");
        write(&q, "2024-01-02T03:04:05Z INFO a\n2024-01-02T03:04:06Z INFO b");
        let g = LogFile::open(&q, utc()).unwrap();
        assert_eq!(g.entry_count(), 2);
        append(&q, "c\n");
        let r = g.refresh().unwrap();
        assert_eq!((r.kind, r.entries), (RefreshKind::Appended, 2));
        assert_eq!(g.entries(1..2).unwrap()[0].message, "bc");
    }

    #[test]
    fn streams_redetect() {
        let (f, sink) = LogFile::stream("container", utc()).unwrap();
        assert_eq!(f.entry_count(), 0);
        sink.write("{\"level\":\"warn\",\"msg\":\"hi\"}").unwrap();
        sink.write("{\"level\":\"info\",\"msg\":\"there\"}\n").unwrap();
        f.refresh().unwrap();
        assert_eq!(f.format(), Format::Json);
        let e = f.entries(0..2).unwrap();
        assert_eq!((e[0].level, e[1].message.as_str()), (Level::Warn, "there"));
        assert_eq!(f.name(), "container");
    }

    #[test]
    fn background_progress() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bg.log");
        write(&p, &"plain text line\n".repeat(10_000));
        let (tx, rx) = std::sync::mpsc::channel();
        let f = LogFile::open_background(&p, utc(), move |pr| {
            let _ = tx.send(pr.clone());
        })
        .unwrap();
        let mut last = None;
        while let Ok(pr) = rx.recv_timeout(std::time::Duration::from_secs(10)) {
            let ready = pr.phase == Phase::Ready;
            last = Some(pr);
            if ready {
                break;
            }
        }
        let last = last.unwrap();
        assert_eq!(last.phase, Phase::Ready);
        assert_eq!(last.lines, 10_000);
        assert!(f.is_done());
        assert_eq!(f.entry_count(), 10_000);
    }
}
