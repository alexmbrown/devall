//! Body plumbing: a tee that captures what streams through (up to a cap) without buffering the
//! transfer, a bandwidth throttle, and `Content-Encoding` decoding for display.

use std::future::Future;
use std::io::Read;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Body, Frame, SizeHint};

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type ProxyBody = BoxBody<Bytes, BoxError>;

/// Decoded bodies larger than this are refused (decompression-bomb guard).
const MAX_DECODED: u64 = 256 * 1024 * 1024;

pub(crate) fn full(b: impl Into<Bytes>) -> ProxyBody {
    Full::new(b.into()).map_err(|never| match never {}).boxed()
}

pub(crate) fn empty() -> ProxyBody {
    Empty::<Bytes>::new().map_err(|never| match never {}).boxed()
}

pub(crate) fn boxed<B>(body: B) -> ProxyBody
where
    B: Body<Data = Bytes> + Send + Sync + 'static,
    B::Error: Into<BoxError>,
{
    body.map_err(Into::into).boxed()
}

/// Decodes a `Content-Encoding` chain (`gzip`, `x-gzip`, `deflate`, `br`, `zstd`, `identity`;
/// several comma-separated codings are undone last-first).
pub fn decode_body(raw: &[u8], encoding: &str) -> Result<Vec<u8>, String> {
    let mut data = raw.to_vec();
    let codings: Vec<String> = encoding.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
    for coding in codings.iter().rev() {
        data = match coding.as_str() {
            "identity" => data,
            "gzip" | "x-gzip" => read_all(flate2::read::MultiGzDecoder::new(&data[..]))?,
            "deflate" => match read_all(flate2::read::ZlibDecoder::new(&data[..])) {
                Ok(d) => d,
                // Some servers send raw deflate without the zlib wrapper.
                Err(_) => read_all(flate2::read::DeflateDecoder::new(&data[..]))?,
            },
            "br" => read_all(brotli::Decompressor::new(&data[..], 8192))?,
            "zstd" => read_all(zstd::stream::read::Decoder::new(&data[..]).map_err(|e| format!("zstd: {e}"))?)?,
            other => return Err(format!("unsupported content-encoding {other}")),
        };
    }
    Ok(data)
}

fn read_all(r: impl Read) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut limited = r.take(MAX_DECODED + 1);
    limited.read_to_end(&mut out).map_err(|e| format!("could not decode body: {e}"))?;
    if out.len() as u64 > MAX_DECODED {
        return Err("decoded body is larger than 256 MB".into());
    }
    Ok(out)
}

/// What a [`TeeBody`] saw once the stream ended (or was dropped).
#[derive(Debug, Clone)]
pub(crate) struct TeeOutcome {
    pub data: Bytes,
    pub size: u64,
    pub truncated: bool,
    /// False when the stream errored or was dropped before its end.
    pub complete: bool,
    pub error: Option<String>,
}

type Done = Box<dyn FnOnce(TeeOutcome) + Send + Sync>;

/// Passes frames through untouched while copying up to `cap` bytes; calls `done` exactly once.
pub(crate) struct TeeBody<B> {
    inner: B,
    buf: BytesMut,
    cap: usize,
    size: u64,
    truncated: bool,
    done: Option<Done>,
}

impl<B: Body> TeeBody<B> {
    pub fn new(inner: B, cap: usize, done: impl FnOnce(TeeOutcome) + Send + Sync + 'static) -> Self {
        let mut t = Self { inner, buf: BytesMut::new(), cap, size: 0, truncated: false, done: Some(Box::new(done)) };
        // Bodies known to be empty (HEAD, 204, `Content-Length: 0`) may never be polled.
        if t.inner.is_end_stream() {
            t.finish(true, None);
        }
        t
    }
}

impl<B> TeeBody<B> {
    fn finish(&mut self, complete: bool, error: Option<String>) {
        if let Some(f) = self.done.take() {
            f(TeeOutcome { data: self.buf.split().freeze(), size: self.size, truncated: self.truncated, complete, error });
        }
    }
}

impl<B> Body for TeeBody<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        match ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
            Some(Ok(frame)) => {
                if let Some(d) = frame.data_ref() {
                    this.size += d.len() as u64;
                    let room = this.cap.saturating_sub(this.buf.len());
                    if d.len() > room {
                        this.truncated = true;
                    }
                    this.buf.extend_from_slice(&d[..d.len().min(room)]);
                }
                // Servers stop polling once a body with a known length reports its end, so the
                // final `None` may never be observed.
                if this.inner.is_end_stream() {
                    this.finish(true, None);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(e)) => {
                let e: BoxError = e.into();
                this.finish(false, Some(crate::util::error_chain(&*e)));
                Poll::Ready(Some(Err(e)))
            }
            None => {
                this.finish(true, None);
                Poll::Ready(None)
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl<B> Drop for TeeBody<B> {
    fn drop(&mut self) {
        self.finish(false, Some("connection closed before the body was fully transferred".into()));
    }
}

/// Delivers data frames at most `bytes_per_sec` (in chunks of ~100 ms worth of data).
pub(crate) struct ThrottleBody<B> {
    inner: B,
    bps: u64,
    chunk: usize,
    pending: Option<Bytes>,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<B> ThrottleBody<B> {
    pub fn new(inner: B, bytes_per_sec: u64) -> Self {
        let bps = bytes_per_sec.max(1);
        let chunk = ((bps / 10) as usize).clamp(256, 64 * 1024);
        Self { inner, bps, chunk, pending: None, sleep: None }
    }
}

impl<B> Body for ThrottleBody<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        loop {
            if let Some(s) = this.sleep.as_mut() {
                ready!(s.as_mut().poll(cx));
                this.sleep = None;
            }
            if let Some(mut data) = this.pending.take() {
                let n = data.len().min(this.chunk);
                let out = data.split_to(n);
                if !data.is_empty() {
                    this.pending = Some(data);
                }
                this.sleep = Some(Box::pin(tokio::time::sleep(Duration::from_secs_f64(n as f64 / this.bps as f64))));
                return Poll::Ready(Some(Ok(Frame::data(out))));
            }
            match ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(d) if d.is_empty() => continue,
                    Ok(d) => this.pending = Some(d),
                    Err(frame) => return Poll::Ready(Some(Ok(frame))),
                },
                Some(Err(e)) => return Poll::Ready(Some(Err(e.into()))),
                None => return Poll::Ready(None),
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.pending.is_none() && self.inner.is_end_stream()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn decodes_common_encodings() {
        let text = b"hello hello hello hello";
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(text).unwrap();
        assert_eq!(decode_body(&gz.finish().unwrap(), "gzip").unwrap(), text);
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(text).unwrap();
        assert_eq!(decode_body(&z.finish().unwrap(), "deflate").unwrap(), text);
        let mut br = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
            w.write_all(text).unwrap();
        }
        assert_eq!(decode_body(&br, "br").unwrap(), text);
        let zs = zstd::stream::encode_all(&text[..], 3).unwrap();
        assert_eq!(decode_body(&zs, "zstd").unwrap(), text);
        assert!(decode_body(b"x", "compress").is_err());
        assert_eq!(decode_body(text, "identity").unwrap(), text);
    }

    #[tokio::test]
    async fn tee_captures_up_to_cap() {
        let seen = Arc::new(Mutex::new(None));
        let s2 = seen.clone();
        let body = TeeBody::new(Full::new(Bytes::from_static(b"0123456789")), 4, move |o| *s2.lock().unwrap() = Some(o));
        let out = body.collect().await.unwrap().to_bytes();
        assert_eq!(&out[..], b"0123456789");
        let o = seen.lock().unwrap().clone().unwrap();
        assert_eq!(&o.data[..], b"0123");
        assert_eq!(o.size, 10);
        assert!(o.truncated && o.complete);
    }

    #[tokio::test]
    async fn dropped_tee_is_incomplete_but_empty_is_complete() {
        let seen = Arc::new(Mutex::new(None));
        let s2 = seen.clone();
        drop(TeeBody::new(Full::new(Bytes::from_static(b"abc")), 10, move |o| *s2.lock().unwrap() = Some(o)));
        assert!(!seen.lock().unwrap().clone().unwrap().complete);
        let s3 = seen.clone();
        drop(TeeBody::new(Empty::<Bytes>::new(), 10, move |o| *s3.lock().unwrap() = Some(o)));
        assert!(seen.lock().unwrap().clone().unwrap().complete);
    }

    #[tokio::test]
    async fn throttle_paces_data() {
        let start = std::time::Instant::now();
        let body = ThrottleBody::new(Full::new(Bytes::from(vec![0u8; 2000])), 10_000);
        let out = body.collect().await.unwrap().to_bytes();
        assert_eq!(out.len(), 2000);
        // 2000 bytes at 10 kB/s: the last chunk's pause comes after it, so ≥ ~100 ms elapsed.
        assert!(start.elapsed() >= Duration::from_millis(100), "{:?}", start.elapsed());
    }
}
