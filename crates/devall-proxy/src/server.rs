//! The listener: plain HTTP proxying (absolute-form), `CONNECT` tunnels that are decrypted with
//! an on-the-fly leaf certificate (or passed through untouched for bypassed hosts and non-HTTP
//! protocols), and a tiny info page with the CA download for direct requests.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes};
use http::{Method, Request, Response, StatusCode, header};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::Inner;
use crate::body::{BoxError, ProxyBody, boxed, empty, full};
use crate::error::{ProxyError, Result};
use crate::model::{CapturedBody, CapturedResponse, Exchange, ExchangeState};
use crate::pipeline::{self, Origin};
use crate::rules::Target;
use crate::util::{error_chain, now_ms, split_host_port};

/// How long to wait for the client's first bytes inside a tunnel before assuming a
/// server-speaks-first protocol and passing the tunnel through.
const PEEK_TIMEOUT: Duration = Duration::from_millis(2000);

pub(crate) struct RunningServer {
    pub addr: SocketAddr,
    pub started_at: i64,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RunningServer {
    pub fn stop(self) {
        let _ = self.shutdown.send(true);
        self.task.abort();
    }
}

#[derive(Clone)]
pub(crate) struct Tunnel {
    pub scheme: &'static str,
    pub host: String,
    pub port: u16,
}

#[derive(Clone)]
pub(crate) struct ConnCtx {
    pub engine: Arc<Inner>,
    pub client_addr: SocketAddr,
    pub listen_addr: SocketAddr,
    /// Set inside CONNECT tunnels: where origin-form requests go.
    pub tunnel: Option<Tunnel>,
    /// Flips to `true` when the proxy stops; tunnels and connections close.
    pub shutdown: watch::Receiver<bool>,
}

pub(crate) async fn start(engine: Arc<Inner>, port: u16) -> Result<RunningServer> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await.map_err(|source| ProxyError::Bind { addr: addr.to_string(), source })?;
    let local = listener.local_addr()?;
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(accept_loop(engine, listener, local, rx));
    Ok(RunningServer { addr: local, started_at: now_ms(), shutdown: tx, task })
}

async fn wait_shutdown(mut rx: watch::Receiver<bool>) {
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            return;
        }
    }
}

async fn accept_loop(engine: Arc<Inner>, listener: TcpListener, local: SocketAddr, shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = wait_shutdown(shutdown.clone()) => break,
            res = listener.accept() => match res {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    let ctx = Arc::new(ConnCtx { engine: engine.clone(), client_addr: peer, listen_addr: local, tunnel: None, shutdown: shutdown.clone() });
                    let stop = shutdown.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            _ = wait_shutdown(stop) => {}
                            _ = serve_http(ctx, TokioIo::new(stream)) => {}
                        }
                    });
                }
                // Out of file descriptors and the like: back off briefly.
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }
}

async fn serve_http<I>(ctx: Arc<ConnCtx>, io: I)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder.http1().preserve_header_case(true).timer(TokioTimer::new());
    builder.http2().timer(TokioTimer::new());
    let svc = service_fn(move |req| -> HandlerFuture {
        let ctx = ctx.clone();
        Box::pin(handle(ctx, req))
    });
    // Boxed: proving `Send` for hyper's connection future inside generic async code hits
    // rustc's higher-ranked lifetime limitations; at this coercion the types are concrete.
    let conn: Pin<Box<dyn std::future::Future<Output = std::result::Result<(), BoxError>> + Send + '_>> =
        Box::pin(builder.serve_connection_with_upgrades(io, svc));
    let _ = conn.await;
}

type HandlerFuture = Pin<Box<dyn std::future::Future<Output = std::result::Result<Response<ProxyBody>, BoxError>> + Send>>;

async fn handle(ctx: Arc<ConnCtx>, req: Request<Incoming>) -> std::result::Result<Response<ProxyBody>, BoxError> {
    if req.method() == Method::CONNECT {
        return Ok(handle_connect(ctx, req));
    }
    let Some(target) = request_target(&ctx, &req) else {
        return Ok(info_page(&ctx, req.uri().path()));
    };
    if ctx.tunnel.is_none() && (is_self(&ctx, &target) || target.host.eq_ignore_ascii_case("devall.proxy")) {
        return Ok(info_page(&ctx, target.path()));
    }
    let (parts, body) = req.into_parts();
    let origin = Origin { client_addr: Some(ctx.client_addr.to_string()), replay: false };
    pipeline::handle(&ctx.engine, origin, parts, boxed(body), target).await.1
}

fn is_self(ctx: &ConnCtx, t: &Target) -> bool {
    let local = matches!(t.host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1" | "[::1]");
    local && t.port == ctx.listen_addr.port()
}

fn request_target(ctx: &ConnCtx, req: &Request<Incoming>) -> Option<Target> {
    let uri = req.uri();
    let pq = uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .filter(|p| p.starts_with('/'))
        .unwrap_or_else(|| format!("/{}", uri.path_and_query().map_or("", |p| p.as_str())));
    if let (Some(scheme), Some(auth)) = (uri.scheme_str(), uri.authority()) {
        let scheme = scheme.to_ascii_lowercase();
        let default = if scheme == "https" { 443 } else { 80 };
        let (host, port) = split_host_port(auth.as_str(), default);
        return Some(Target { scheme, host, port, path_and_query: pq });
    }
    let tunnel = ctx.tunnel.as_ref()?;
    let (host, port) = match req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).filter(|h| !h.is_empty()) {
        Some(h) => split_host_port(h, tunnel.port),
        None => (tunnel.host.clone(), tunnel.port),
    };
    Some(Target { scheme: tunnel.scheme.to_string(), host, port, path_and_query: pq })
}

fn info_page(ctx: &ConnCtx, path: &str) -> Response<ProxyBody> {
    let ca = &ctx.engine.ca;
    let (ct, body, attach): (&str, Bytes, Option<&str>) = match path {
        "/ca.pem" | "/ca.crt" => ("application/x-pem-file", Bytes::from(ca.cert_pem().to_string()), Some("devall-ca.pem")),
        "/ca.der" | "/ca.cer" => ("application/x-x509-ca-cert", Bytes::copy_from_slice(ca.cert_der().as_ref()), Some("devall-ca.cer")),
        _ => {
            let html = format!(
                "<!doctype html><meta charset=utf-8><title>/dev/all traffic inspector</title>\
                 <body style=\"font:14px system-ui;margin:40px;max-width:640px\"><h1>/dev/all traffic inspector</h1>\
                 <p>This is the /dev/all debugging proxy on <code>{}</code>.</p>\
                 <p>To inspect HTTPS, trust its certificate authority <b>{}</b>: \
                 <a href=\"/ca.pem\">download PEM</a> · <a href=\"/ca.cer\">download DER</a>.</p>\
                 <p>SHA-256 fingerprint: <code>{}</code></p></body>",
                ctx.listen_addr,
                html_escape(ca.common_name()),
                ca.info().sha256
            );
            ("text/html; charset=utf-8", Bytes::from(html), None)
        }
    };
    let mut b = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(header::CONTENT_LENGTH, body.len())
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(name) = attach {
        b = b.header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""));
    }
    b.body(full(body)).unwrap_or_else(|_| Response::new(empty()))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn handle_connect(ctx: Arc<ConnCtx>, mut req: Request<Incoming>) -> Response<ProxyBody> {
    let Some(authority) = req.uri().authority().map(|a| a.as_str().to_string()) else {
        return Response::builder().status(StatusCode::BAD_REQUEST).body(full("CONNECT needs host:port\n")).unwrap_or_else(|_| Response::new(empty()));
    };
    let (host, port) = split_host_port(&authority, 443);
    let bypass = ctx.engine.runtime().bypass.matches(&host, port);
    let on_upgrade = hyper::upgrade::on(&mut req);
    let stop = ctx.shutdown.clone();
    let work: Pin<Box<dyn std::future::Future<Output = ()> + Send>> = Box::pin(async move {
        let Ok(upgraded) = on_upgrade.await else {
            return;
        };
        let io = TokioIo::new(upgraded);
        if bypass {
            tunnel(ctx, io, host, port, "bypassed").await;
        } else {
            intercept(ctx, io, host, port).await;
        }
    });
    tokio::spawn(async move {
        tokio::select! {
            _ = wait_shutdown(stop) => {}
            _ = work => {}
        }
    });
    Response::new(empty())
}

/// Looks at the first bytes of a tunnel: TLS is decrypted, plain HTTP is served directly,
/// anything else is passed through.
async fn intercept<I>(ctx: Arc<ConnCtx>, mut io: I, host: String, port: u16)
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut first = [0u8; 16];
    let n = match tokio::time::timeout(PEEK_TIMEOUT, io.read(&mut first)).await {
        Ok(Ok(0)) | Ok(Err(_)) => return,
        Ok(Ok(n)) => n,
        Err(_) => {
            tunnel(ctx, io, host, port, "rawTcp").await;
            return;
        }
    };
    let stream = Prefixed { prefix: Bytes::copy_from_slice(&first[..n]), inner: io };
    if first[0] == 0x16 {
        tls_intercept(ctx, stream, host, port).await;
    } else if looks_like_http(&first[..n]) {
        let ctx = Arc::new(ConnCtx { tunnel: Some(Tunnel { scheme: "http", host, port }), ..(*ctx).clone() });
        serve_http(ctx, TokioIo::new(stream)).await;
    } else {
        tunnel(ctx, stream, host, port, "rawTcp").await;
    }
}

fn looks_like_http(b: &[u8]) -> bool {
    let token: Vec<u8> = b.iter().copied().take_while(|c| *c != b' ').collect();
    !token.is_empty() && token.len() <= 10 && token.iter().all(u8::is_ascii_uppercase)
}

async fn tls_intercept<I>(ctx: Arc<ConnCtx>, stream: I, host: String, port: u16)
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok(start) = tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream).await else {
        return;
    };
    let sni = start.client_hello().server_name().map(str::to_string);
    let cert_host = sni.clone().unwrap_or_else(|| host.clone());
    let config = match ctx.engine.ca.server_config(&cert_host) {
        Ok(c) => c,
        Err(e) => {
            record_tls_failure(&ctx, &cert_host, port, format!("could not issue a certificate for {cert_host}: {e}"));
            return;
        }
    };
    match start.into_stream(config).await {
        Ok(tls) => {
            let ctx = Arc::new(ConnCtx { tunnel: Some(Tunnel { scheme: "https", host: sni.unwrap_or(host), port }), ..(*ctx).clone() });
            serve_http(ctx, TokioIo::new(tls)).await;
        }
        Err(e) => {
            // Only report the client rejecting our certificate; plain disconnects are noise.
            let rejected = e.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()).is_some_and(|r| matches!(r, rustls::Error::AlertReceived(_)));
            if rejected {
                record_tls_failure(
                    &ctx,
                    &cert_host,
                    port,
                    format!(
                        "the client rejected the /dev/all certificate for {cert_host} ({}). Trust the CA (Trust CA…), or add the host to the bypass list if the app pins certificates.",
                        error_chain(&e)
                    ),
                );
            }
        }
    }
}

fn record_tls_failure(ctx: &ConnCtx, host: &str, port: u16, message: String) {
    let key = format!("{host}:{port}");
    {
        let mut seen = ctx.engine.tls_failures.lock().unwrap_or_else(|e| e.into_inner());
        if seen.get(&key).is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            return;
        }
        if seen.len() > 4096 {
            seen.clear();
        }
        seen.insert(key.clone(), Instant::now());
    }
    let mut ex = Exchange::new("CONNECT", format!("https://{}", crate::util::authority_for("https", host, port)), "HTTP/1.1", Vec::new());
    ex.client_addr = Some(ctx.client_addr.to_string());
    ex.state = ExchangeState::Failed;
    ex.error = Some(message);
    ex.tag("tlsRejected");
    ex.timing.total_ms = Some(0.0);
    ctx.engine.store.insert(ex);
}

/// Passes bytes through untouched, recording a single tunnel exchange with the byte counts.
async fn tunnel<I>(ctx: Arc<ConnCtx>, mut client: I, host: String, port: u16, tag: &'static str)
where
    I: AsyncRead + AsyncWrite + Unpin,
{
    let host = host.as_str();
    let started = Instant::now();
    let authority = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut ex = Exchange::new("CONNECT", authority, "HTTP/1.1", Vec::new());
    ex.client_addr = Some(ctx.client_addr.to_string());
    ex.state = ExchangeState::Tunnel;
    ex.tag(tag);
    let store = &ctx.engine.store;
    let id = store.insert(ex);
    let upstream = ctx.engine.runtime().upstream.clone();
    match upstream.shared.connect_tcp(host, port).await {
        Ok((mut server, meta)) => {
            store.update(id, |e| {
                e.timing.dns_ms = meta.dns_ms;
                e.timing.connect_ms = Some(meta.connect_ms);
                e.server_addr = meta.remote_addr.map(|a| a.to_string());
                e.response = Some(CapturedResponse {
                    status: 200,
                    reason: "Connection Established".into(),
                    http_version: "HTTP/1.1".into(),
                    headers: Vec::new(),
                    body: CapturedBody::default(),
                });
            });
            let (up, down) = tokio::io::copy_bidirectional(&mut client, &mut server).await.unwrap_or((0, 0));
            let total = started.elapsed().as_secs_f64() * 1000.0;
            store.update(id, |e| {
                e.request.body = CapturedBody { size: up, ..Default::default() };
                if let Some(r) = e.response.as_mut() {
                    r.body = CapturedBody { size: down, ..Default::default() };
                }
                e.timing.total_ms = Some(total);
                e.tag("closed");
            });
        }
        Err(e) => {
            let msg = e.to_string();
            store.update(id, |ex| {
                ex.state = ExchangeState::Failed;
                ex.error = Some(msg);
                ex.timing.total_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
            });
        }
    }
}

/// A stream that replays already-read bytes before reading from `inner`.
pub(crate) struct Prefixed<I> {
    prefix: Bytes,
    inner: I,
}

impl<I: AsyncRead + Unpin> AsyncRead for Prefixed<I> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.prefix.is_empty() {
            let n = this.prefix.len().min(buf.remaining());
            buf.put_slice(&this.prefix[..n]);
            this.prefix.advance(n);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<I: AsyncWrite + Unpin> AsyncWrite for Prefixed<I> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_sniffing() {
        assert!(looks_like_http(b"GET / HTTP/1.1"));
        assert!(looks_like_http(b"OPTIO"));
        assert!(!looks_like_http(&[0x16, 0x03, 0x01]));
        assert!(!looks_like_http(b"SSH-2.0-OpenSSH"));
    }
}
