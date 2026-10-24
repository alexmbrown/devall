//! Shared streaming plumbing: callback sinks, log lines, batching, interactive exec I/O.

use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use serde::Serialize;

/// Receives streamed items. Returns `false` once the receiver is gone, which stops the stream.
pub type Sink<T> = Arc<dyn Fn(T) -> bool + Send + Sync>;

pub fn sink<T>(f: impl Fn(T) -> bool + Send + Sync + 'static) -> Sink<T> {
    Arc::new(f)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// RFC 3339 timestamp when the source provides one (timestamps on).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    pub stream: LogStream,
    pub text: String,
}

/// Messages of a log stream (container logs, pod logs, Cloud Logging tail).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LogMsg {
    Lines { lines: Vec<LogLine> },
    /// The stream ended (container stopped, pod gone, cancelled upstream) or failed.
    End { error: Option<String> },
}

const FLUSH_EVERY: Duration = Duration::from_millis(80);
const MAX_BATCH: usize = 500;
/// Longest single line forwarded to the UI; longer lines are cut (with a marker).
const MAX_LINE: usize = 64 * 1024;

/// Forwards lines from `lines` to `sink` in small batches (≤ 80 ms latency), then sends `End`.
/// Returns when the source ends or the receiver goes away.
pub async fn pump_lines<S>(mut lines: S, sink: &Sink<LogMsg>)
where
    S: Stream<Item = Result<LogLine, String>> + Unpin,
{
    let mut pending: Vec<LogLine> = Vec::new();
    let error = loop {
        let next = if pending.is_empty() {
            Some(lines.next().await)
        } else {
            tokio::time::timeout(FLUSH_EVERY, lines.next()).await.ok()
        };
        match next {
            // Timer fired with pending lines.
            None => {
                if !sink(LogMsg::Lines { lines: std::mem::take(&mut pending) }) {
                    return;
                }
            }
            Some(Some(Ok(line))) => {
                pending.push(line);
                if pending.len() >= MAX_BATCH && !sink(LogMsg::Lines { lines: std::mem::take(&mut pending) }) {
                    return;
                }
            }
            Some(Some(Err(e))) => break Some(e),
            Some(None) => break None,
        }
    };
    if !pending.is_empty() && !sink(LogMsg::Lines { lines: pending }) {
        return;
    }
    sink(LogMsg::End { error });
}

/// Splits `2026-10-21T09:15:02.123456789Z message` into (timestamp, message).
pub fn split_timestamp(line: &str) -> (Option<String>, String) {
    if let Some((head, rest)) = line.split_once(' ')
        && head.len() >= 20
        && head.as_bytes()[0].is_ascii_digit()
        && head.as_bytes().get(10) == Some(&b'T')
        && (head.ends_with('Z') || head.contains('+') || head.get(19..).is_some_and(|t| t.contains('-')))
    {
        return (Some(head.to_string()), rest.to_string());
    }
    (None, line.to_string())
}

pub fn clip_line(mut s: String) -> String {
    if s.len() > MAX_LINE {
        let cut = s.floor_char_boundary(MAX_LINE);
        s.truncate(cut);
        s.push_str(" …[line truncated]");
    }
    s
}

/// Reassembles lines from arbitrary byte chunks (Docker log frames can split or join lines).
#[derive(Default)]
pub struct LineSplitter {
    buf: Vec<u8>,
}

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=i).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }
        // Guard against a producer that never sends a newline.
        if self.buf.len() > MAX_LINE {
            out.push(String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned());
        }
        out
    }

    pub fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() { None } else { Some(String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned()) }
    }
}

/// Decodes UTF-8 from chunks without mangling multi-byte characters split across chunks.
#[derive(Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let valid_up_to = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) => match e.error_len() {
                // Incomplete sequence at the end: keep it for the next chunk.
                None => e.valid_up_to(),
                // Invalid bytes: decode lossily, nothing to keep.
                Some(_) => self.pending.len(),
            },
        };
        let rest = self.pending.split_off(valid_up_to);
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending = rest;
        text
    }
}

/// Shell started when an exec names no command: bash when present, else sh.
pub fn default_shell() -> Vec<String> {
    ["/bin/sh", "-c", "if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi"].iter().map(|s| s.to_string()).collect()
}

/// Input to an interactive exec session (container or pod shell).
#[derive(Debug, Clone)]
pub enum ExecInput {
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ExecEvent {
    /// Terminal output (stdout and stderr merged when a TTY is attached).
    Output { data: String },
    Exit { code: Option<i64>, error: Option<String> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn splits_lines_across_chunks() {
        let mut s = LineSplitter::default();
        assert_eq!(s.push(b"hel"), Vec::<String>::new());
        assert_eq!(s.push(b"lo\r\nwor"), vec!["hello"]);
        assert_eq!(s.push(b"ld\n\nx"), vec!["world", ""]);
        assert_eq!(s.finish().as_deref(), Some("x"));
        assert_eq!(s.finish(), None);
    }

    #[test]
    fn utf8_across_chunks() {
        let text = "héllo → wörld";
        let bytes = text.as_bytes();
        let mut d = Utf8Decoder::default();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            out.push_str(&d.push(chunk));
        }
        assert_eq!(out, text);
    }

    #[test]
    fn timestamps() {
        let (ts, msg) = split_timestamp("2026-10-21T09:15:02.123456789Z GET /health 200");
        assert_eq!(ts.as_deref(), Some("2026-10-21T09:15:02.123456789Z"));
        assert_eq!(msg, "GET /health 200");
        let (ts, msg) = split_timestamp("plain line with spaces");
        assert!(ts.is_none());
        assert_eq!(msg, "plain line with spaces");
    }

    #[tokio::test]
    async fn pump_batches_and_ends() {
        let got: Arc<Mutex<Vec<LogMsg>>> = Arc::default();
        let g = got.clone();
        let s: Sink<LogMsg> = sink(move |m| {
            g.lock().unwrap().push(m);
            true
        });
        let lines = (0..3).map(|i| Ok(LogLine { ts: None, stream: LogStream::Stdout, text: format!("l{i}") }));
        pump_lines(futures::stream::iter(lines), &s).await;
        let got = got.lock().unwrap();
        let n: usize = got.iter().map(|m| if let LogMsg::Lines { lines } = m { lines.len() } else { 0 }).sum();
        assert_eq!(n, 3);
        assert!(matches!(got.last(), Some(LogMsg::End { error: None })));
    }
}
