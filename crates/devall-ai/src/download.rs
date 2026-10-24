//! Resumable downloads for model files and llama.cpp archives. Only ever started by an explicit
//! user action.
//!
//! * Bytes stream into `<dest>.part`; a later call resumes with `Range: bytes=<len>-` (a server
//!   that ignores the range and answers 200 restarts the file).
//! * SHA-256 is computed while downloading (the existing `.part` prefix is hashed first on
//!   resume) and compared with the pinned digest, or with Hugging Face's `X-Linked-ETag` (the LFS
//!   SHA-256) when no digest is pinned. A mismatch deletes the partial file.
//! * Cancelling (via the shared flag) keeps the `.part` so the next attempt resumes.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{AiError, Result};

#[derive(Debug, Clone)]
pub struct DownloadSpec {
    pub url: String,
    pub dest: PathBuf,
    pub expected_size: Option<u64>,
    /// Lower-case hex SHA-256.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: f64,
    /// `downloading`, `verifying`, `done`.
    pub phase: &'static str,
    pub resumed_from: u64,
}

/// Cancellation flag shared with the UI.
pub type CancelFlag = Arc<AtomicBool>;

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

fn io(path: &Path, source: std::io::Error) -> AiError {
    AiError::Io { path: path.to_path_buf(), source }
}

fn hash_prefix(path: &Path, hasher: &mut Sha256) -> Result<u64> {
    let mut f = std::fs::File::open(path).map_err(|e| io(path, e))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| io(path, e))?;
        if n == 0 {
            return Ok(n_total);
        }
        hasher.update(&buf[..n]);
        n_total += n as u64;
    }
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("devall/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AiError::Download(e.to_string()))
}

fn header_sha256(resp: &reqwest::Response) -> Option<String> {
    ["x-linked-etag", "etag"].iter().find_map(|h| {
        let v = resp.headers().get(*h)?.to_str().ok()?.trim().trim_start_matches("W/").trim_matches('"').to_ascii_lowercase();
        (v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit())).then_some(v)
    })
}

/// Downloads `spec.url` to `spec.dest` (resuming a previous `.part`), verifying size and SHA-256.
/// An existing complete `dest` is re-used without any network call when its size matches.
pub async fn download(spec: &DownloadSpec, cancel: &CancelFlag, mut on_progress: impl FnMut(&Progress)) -> Result<PathBuf> {
    if let (Ok(meta), Some(size)) = (std::fs::metadata(&spec.dest), spec.expected_size)
        && meta.len() == size
    {
        on_progress(&Progress { downloaded: size, total: Some(size), bytes_per_sec: 0.0, phase: "done", resumed_from: size });
        return Ok(spec.dest.clone());
    }
    if let Some(parent) = spec.dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let part = part_path(&spec.dest);
    let mut existing = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if spec.expected_size.is_some_and(|s| existing > s) {
        let _ = std::fs::remove_file(&part);
        existing = 0;
    }

    let client = http_client()?;
    let mut rb = client.get(&spec.url);
    if existing > 0 {
        rb = rb.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let mut resp = rb.send().await.map_err(|e| AiError::Download(format!("{}: {}", spec.url, crate::error_chain(&e))))?;
    let status = resp.status().as_u16();
    let mut hasher = Sha256::new();
    let append = match status {
        206 if existing > 0 => {
            hash_prefix(&part, &mut hasher)?;
            true
        }
        416 if existing > 0 && spec.expected_size == Some(existing) => {
            // Already fully downloaded into the .part; just verify.
            hash_prefix(&part, &mut hasher)?;
            drop(resp);
            return finish(spec, &part, hasher, None, existing, &mut on_progress);
        }
        200..=299 => {
            existing = 0;
            false
        }
        _ => return Err(AiError::Download(format!("{} returned HTTP {status}", spec.url))),
    };
    let header_digest = header_sha256(&resp);
    let total = spec.expected_size.or_else(|| {
        let len = resp.content_length()?;
        Some(if append { existing + len } else { len })
    });

    let mut file = std::fs::OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(&part).map_err(|e| io(&part, e))?;
    let start = Instant::now();
    let mut last_report = Instant::now() - Duration::from_secs(1);
    let mut downloaded = existing;
    let resumed_from = existing;
    loop {
        if cancel.load(Ordering::Relaxed) {
            file.flush().map_err(|e| io(&part, e))?;
            return Err(AiError::Cancelled);
        }
        let chunk = resp.chunk().await.map_err(|e| AiError::Download(format!("connection lost after {downloaded} bytes ({}); retry to resume", crate::error_chain(&e))))?;
        let Some(chunk) = chunk else { break };
        file.write_all(&chunk).map_err(|e| io(&part, e))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        if last_report.elapsed() >= Duration::from_millis(200) {
            last_report = Instant::now();
            let secs = start.elapsed().as_secs_f64().max(0.001);
            on_progress(&Progress { downloaded, total, bytes_per_sec: (downloaded - resumed_from) as f64 / secs, phase: "downloading", resumed_from });
        }
    }
    file.flush().map_err(|e| io(&part, e))?;
    drop(file);
    finish(spec, &part, hasher, header_digest, downloaded, &mut on_progress)
}

fn finish(spec: &DownloadSpec, part: &Path, hasher: Sha256, header_digest: Option<String>, downloaded: u64, on_progress: &mut impl FnMut(&Progress)) -> Result<PathBuf> {
    on_progress(&Progress { downloaded, total: spec.expected_size, bytes_per_sec: 0.0, phase: "verifying", resumed_from: 0 });
    if let Some(size) = spec.expected_size
        && size != downloaded
    {
        return Err(AiError::Download(format!("incomplete download: {downloaded} of {size} bytes; retry to resume")));
    }
    let actual = hex::encode(hasher.finalize());
    if let Some(expected) = spec.sha256.clone().or(header_digest)
        && !expected.eq_ignore_ascii_case(&actual)
    {
        let _ = std::fs::remove_file(part);
        return Err(AiError::Checksum { file: spec.dest.display().to_string(), expected, actual });
    }
    std::fs::rename(part, &spec.dest).map_err(|e| io(&spec.dest, e))?;
    on_progress(&Progress { downloaded, total: Some(downloaded), bytes_per_sec: 0.0, phase: "done", resumed_from: 0 });
    Ok(spec.dest.clone())
}

// ── Registry of active downloads ────────────────────────────────────────────────────────────────

/// Where a download is. `Paused` (cancelled by the user, `.part` kept), `Done` and `Error` are
/// terminal: they are reported once when the download ends and then dropped from the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadPhase {
    Starting,
    Downloading,
    Verifying,
    /// Unpacking an archive (llama.cpp runtime).
    Installing,
    Done,
    Paused,
    Error,
}

impl DownloadPhase {
    pub fn is_active(self) -> bool {
        matches!(self, DownloadPhase::Starting | DownloadPhase::Downloading | DownloadPhase::Verifying | DownloadPhase::Installing)
    }

    fn from_progress(phase: &str) -> DownloadPhase {
        match phase {
            "verifying" => DownloadPhase::Verifying,
            "installing" => DownloadPhase::Installing,
            // A finished file inside a multi-file install is still part of an active download.
            _ => DownloadPhase::Downloading,
        }
    }
}

/// Live state of one download, as shown by every view (and sent to the UI as an event).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadState {
    /// Model id, or `runtime:<asset suffix>` for the llama.cpp runtime.
    pub key: String,
    pub phase: DownloadPhase,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub bytes_per_sec: f64,
    /// Result path (done) or error text (error).
    pub message: Option<String>,
}

/// How a download ended.
pub enum Outcome {
    Done(String),
    Failed(AiError),
}

struct Active {
    state: DownloadState,
    cancel: CancelFlag,
}

/// Active downloads by key. The single source of truth for "is X downloading, and how far": the
/// UI asks it on mount instead of relying on the component that started the download.
#[derive(Default)]
pub struct Registry {
    active: std::sync::Mutex<std::collections::HashMap<String, Active>>,
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Active>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers a new download. Errors with the current state when `key` is already running.
    pub fn begin(&self, key: &str, already: u64, total: Option<u64>) -> std::result::Result<(CancelFlag, DownloadState), DownloadState> {
        let mut map = self.lock();
        if let Some(a) = map.get(key) {
            return Err(a.state.clone());
        }
        let state = DownloadState { key: key.to_string(), phase: DownloadPhase::Starting, downloaded: already, total, bytes_per_sec: 0.0, message: None };
        let cancel = CancelFlag::default();
        map.insert(key.to_string(), Active { state: state.clone(), cancel: cancel.clone() });
        Ok((cancel, state))
    }

    /// Applies a progress report; returns the new state (`None` when `key` isn't active).
    pub fn progress(&self, key: &str, p: &Progress) -> Option<DownloadState> {
        let mut map = self.lock();
        let a = map.get_mut(key)?;
        a.state.phase = DownloadPhase::from_progress(p.phase);
        a.state.downloaded = p.downloaded;
        if p.total.is_some() {
            a.state.total = p.total;
        }
        a.state.bytes_per_sec = p.bytes_per_sec;
        Some(a.state.clone())
    }

    /// Removes `key` and returns its terminal state.
    pub fn finish(&self, key: &str, outcome: Outcome) -> DownloadState {
        let mut state = self.lock().remove(key).map(|a| a.state).unwrap_or_else(|| DownloadState {
            key: key.to_string(),
            phase: DownloadPhase::Starting,
            downloaded: 0,
            total: None,
            bytes_per_sec: 0.0,
            message: None,
        });
        state.bytes_per_sec = 0.0;
        match outcome {
            Outcome::Done(path) => {
                state.phase = DownloadPhase::Done;
                if let Some(t) = state.total {
                    state.downloaded = t;
                }
                state.message = Some(path);
            }
            Outcome::Failed(AiError::Cancelled) => {
                state.phase = DownloadPhase::Paused;
                state.message = None;
            }
            Outcome::Failed(e) => {
                state.phase = DownloadPhase::Error;
                state.message = Some(e.to_string());
            }
        }
        state
    }

    /// Asks a running download to stop (it keeps its `.part` and reports `Paused`).
    pub fn cancel(&self, key: &str) -> bool {
        match self.lock().get(key) {
            Some(a) => {
                a.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn get(&self, key: &str) -> Option<DownloadState> {
        self.lock().get(key).map(|a| a.state.clone())
    }

    pub fn is_active(&self, key: &str) -> bool {
        self.lock().contains_key(key)
    }

    /// Every active download, ordered by key.
    pub fn snapshot(&self) -> Vec<DownloadState> {
        let mut v: Vec<DownloadState> = self.lock().values().map(|a| a.state.clone()).collect();
        v.sort_by(|a, b| a.key.cmp(&b.key));
        v
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn registry_tracks_one_download_per_key() {
        let r = Registry::default();
        let (flag, st) = r.begin("m1", 1_000, Some(10_000)).unwrap();
        assert_eq!((st.phase, st.downloaded), (DownloadPhase::Starting, 1_000));
        // A second start is rejected with the live state (what a remounted view must show).
        let busy = r.begin("m1", 0, None).unwrap_err();
        assert_eq!(busy.key, "m1");
        assert!(r.is_active("m1"));

        let p = Progress { downloaded: 4_000, total: Some(10_000), bytes_per_sec: 2_000.0, phase: "downloading", resumed_from: 1_000 };
        let st = r.progress("m1", &p).unwrap();
        assert_eq!((st.phase, st.downloaded, st.total), (DownloadPhase::Downloading, 4_000, Some(10_000)));
        assert_eq!(r.get("m1").unwrap().downloaded, 4_000);
        assert!(r.progress("other", &p).is_none());

        let st = r.progress("m1", &Progress { phase: "verifying", ..p.clone() }).unwrap();
        assert_eq!(st.phase, DownloadPhase::Verifying);
        assert_eq!(r.snapshot().len(), 1);

        assert!(r.cancel("m1"));
        assert!(flag.load(Ordering::Relaxed));
        assert!(!r.cancel("nope"));
        let end = r.finish("m1", Outcome::Failed(AiError::Cancelled));
        assert_eq!((end.phase, end.downloaded), (DownloadPhase::Paused, 4_000));
        assert!(!r.is_active("m1") && r.snapshot().is_empty());

        // Can start again after it ended; errors and success are terminal too.
        r.begin("m1", 4_000, Some(10_000)).unwrap();
        let end = r.finish("m1", Outcome::Failed(AiError::Download("boom".into())));
        assert_eq!(end.phase, DownloadPhase::Error);
        assert!(end.message.unwrap().contains("boom"));
        r.begin("m1", 0, Some(10_000)).unwrap();
        let end = r.finish("m1", Outcome::Done("/models/m1.gguf".into()));
        assert_eq!((end.phase, end.downloaded, end.message.as_deref()), (DownloadPhase::Done, 10_000, Some("/models/m1.gguf")));
        assert!(!end.phase.is_active());
    }

    #[tokio::test]
    async fn registry_follows_a_real_download() {
        let body = vec![3u8; 300_000];
        let (url, _) = file_server(body.clone(), false).await;
        let dir = tempfile::tempdir().unwrap();
        let spec = DownloadSpec { url, dest: dir.path().join("r.bin"), expected_size: Some(body.len() as u64), sha256: Some(sha(&body)) };
        let reg = Registry::default();
        let (cancel, _) = reg.begin("r", 0, spec.expected_size).unwrap();
        let mut seen = Vec::new();
        let res = download(&spec, &cancel, |p| seen.push(reg.progress("r", p).unwrap().phase)).await;
        let end = reg.finish("r", match res {
            Ok(p) => Outcome::Done(p.display().to_string()),
            Err(e) => Outcome::Failed(e),
        });
        assert_eq!(end.phase, DownloadPhase::Done);
        assert_eq!(end.downloaded, body.len() as u64);
        assert!(seen.contains(&DownloadPhase::Verifying));
        assert!(reg.snapshot().is_empty());
    }

    /// Serves `body` at any path, honouring `Range: bytes=N-` unless `ignore_range`. Records the
    /// Range headers it saw.
    pub(crate) async fn file_server(body: Vec<u8>, ignore_range: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let body = Arc::new(body);
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let body = body.clone();
                let seen = seen2.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let range = req.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|r| r.trim_end_matches('-').to_string()));
                    seen.lock().unwrap().push(range.clone().unwrap_or_default());
                    let start: usize = if ignore_range { 0 } else { range.and_then(|r| r.parse().ok()).unwrap_or(0) };
                    let head = if start >= body.len() && start > 0 {
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                    } else if start > 0 {
                        format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() - 1, body.len(), body.len() - start)
                    } else {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                    };
                    let _ = s.write_all(head.as_bytes()).await;
                    if start < body.len() {
                        let _ = s.write_all(&body[start..]).await;
                    }
                    let _ = s.shutdown().await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn sha(b: &[u8]) -> String {
        hex::encode(Sha256::digest(b))
    }

    #[tokio::test]
    async fn resumes_a_partial_download_with_range() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (url, seen) = file_server(body.clone(), false).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.gguf");
        // Pretend a previous attempt stopped after 70 000 bytes.
        std::fs::write(part_path(&dest), &body[..70_000]).unwrap();
        let spec = DownloadSpec { url: format!("{url}/m.gguf"), dest: dest.clone(), expected_size: Some(body.len() as u64), sha256: Some(sha(&body)) };
        let cancel = CancelFlag::default();
        let mut phases = Vec::new();
        download(&spec, &cancel, |p| phases.push(p.phase)).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_path(&dest).exists());
        assert_eq!(seen.lock().unwrap().as_slice(), ["70000"]);
        assert_eq!(phases.last(), Some(&"done"));

        // Already complete: no request at all.
        download(&spec, &cancel, |_| {}).await.unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn restarts_when_server_ignores_range_and_rejects_bad_checksum() {
        let body = b"0123456789abcdefghij".repeat(1000);
        let (url, _) = file_server(body.clone(), true).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("x.bin");
        std::fs::write(part_path(&dest), b"garbage-prefix").unwrap();
        let spec = DownloadSpec { url: url.clone(), dest: dest.clone(), expected_size: Some(body.len() as u64), sha256: Some(sha(&body)) };
        download(&spec, &CancelFlag::default(), |_| {}).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);

        let dest2 = dir.path().join("y.bin");
        let bad = DownloadSpec { url, dest: dest2.clone(), expected_size: None, sha256: Some("00".repeat(32)) };
        let err = download(&bad, &CancelFlag::default(), |_| {}).await.unwrap_err();
        assert!(matches!(err, AiError::Checksum { .. }), "{err}");
        assert!(!dest2.exists() && !part_path(&dest2).exists());
    }

    #[tokio::test]
    async fn cancel_keeps_the_partial_file() {
        let body = vec![7u8; 50_000];
        let (url, _) = file_server(body, false).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("c.bin");
        let cancel = CancelFlag::default();
        cancel.store(true, Ordering::Relaxed);
        let spec = DownloadSpec { url, dest: dest.clone(), expected_size: Some(50_000), sha256: None };
        assert!(matches!(download(&spec, &cancel, |_| {}).await, Err(AiError::Cancelled)));
        assert!(part_path(&dest).exists());
        assert!(!dest.exists());
    }
}
