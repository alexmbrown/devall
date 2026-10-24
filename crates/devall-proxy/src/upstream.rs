//! Connections to the real servers: a pooled hyper client (HTTP/1.1 + HTTP/2 via ALPN) over a
//! connector that measures DNS / TCP / TLS time, verifies certificates (OS roots by default,
//! skipped per host on request) and reports it all through `Connected::extra`.

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use http::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::body::ProxyBody;
use crate::config::ProxyConfig;
use crate::error::{ProxyError, Result};
use crate::util::HostMatcher;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Facts about the upstream connection a response arrived on.
#[derive(Debug, Clone)]
pub(crate) struct ConnMeta {
    pub dns_ms: Option<f64>,
    pub connect_ms: f64,
    pub tls_ms: Option<f64>,
    pub remote_addr: Option<SocketAddr>,
    first_use: Arc<AtomicBool>,
}

impl ConnMeta {
    /// True only for the first response on this connection (which paid for DNS/TCP/TLS).
    pub fn first_use(&self) -> bool {
        !self.first_use.swap(true, Ordering::AcqRel)
    }
}

pub(crate) enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            Stream::Plain(s) => s.is_write_vectored(),
            Stream::Tls(s) => s.is_write_vectored(),
        }
    }
}

pub(crate) struct UpstreamIo {
    pub stream: Stream,
    pub meta: ConnMeta,
    pub h2: bool,
}

impl AsyncRead for UpstreamIo {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for UpstreamIo {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

impl Connection for UpstreamIo {
    fn connected(&self) -> Connected {
        let c = Connected::new().extra(self.meta.clone());
        if self.h2 { c.negotiated_h2() } else { c }
    }
}

/// Accepts any server certificate (hosts on the "ignore TLS errors" list). Signatures are still
/// checked so the handshake itself is sound.
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub(crate) struct ConnectorShared {
    verify: Arc<ClientConfig>,
    verify_h1: Arc<ClientConfig>,
    insecure: Arc<ClientConfig>,
    insecure_h1: Arc<ClientConfig>,
    ignore_tls: HostMatcher,
    http2: bool,
}

fn io_err(msg: String) -> io::Error {
    io::Error::other(msg)
}

fn tls_err(e: impl std::fmt::Display) -> ProxyError {
    ProxyError::Tls(e.to_string())
}

impl ConnectorShared {
    fn new(cfg: &ProxyConfig) -> Result<Self> {
        let provider = crate::ca::provider();
        let extra: Vec<CertificateDer<'static>> = cfg
            .extra_roots_pem
            .iter()
            .flat_map(|pem| CertificateDer::pem_slice_iter(pem.as_bytes()).filter_map(std::result::Result::ok).collect::<Vec<_>>())
            .collect();
        let verifier: Arc<dyn ServerCertVerifier> = if cfg.system_roots {
            if extra.is_empty() {
                Arc::new(rustls_platform_verifier::Verifier::new(provider.clone()).map_err(tls_err)?)
            } else {
                Arc::new(rustls_platform_verifier::Verifier::new_with_extra_roots(extra, provider.clone()).map_err(tls_err)?)
            }
        } else {
            let mut roots = RootCertStore::empty();
            for c in extra {
                roots.add(c).map_err(tls_err)?;
            }
            if roots.is_empty() {
                return Err(ProxyError::Tls("system roots are disabled and no extra roots are configured".into()));
            }
            rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().map_err(tls_err)?
        };
        let insecure: Arc<dyn ServerCertVerifier> = Arc::new(NoVerify(provider.clone()));
        let make = |v: Arc<dyn ServerCertVerifier>, h2: bool| -> Result<Arc<ClientConfig>> {
            let mut c = ClientConfig::builder_with_provider(provider.clone())
                .with_safe_default_protocol_versions()
                .map_err(tls_err)?
                .dangerous()
                .with_custom_certificate_verifier(v)
                .with_no_client_auth();
            c.alpn_protocols = if h2 { vec![b"h2".to_vec(), b"http/1.1".to_vec()] } else { vec![b"http/1.1".to_vec()] };
            Ok(Arc::new(c))
        };
        Ok(Self {
            verify: make(verifier.clone(), true)?,
            verify_h1: make(verifier, false)?,
            insecure: make(insecure.clone(), true)?,
            insecure_h1: make(insecure, false)?,
            ignore_tls: HostMatcher::new(&cfg.ignore_tls_errors),
            http2: cfg.upstream_http2,
        })
    }

    /// Resolves and connects (trying each address), measuring each phase. (Boxed so callers in
    /// generic async contexts don't trip over higher-ranked `Send` inference.)
    pub fn connect_tcp<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, io::Result<(TcpStream, ConnMeta)>> {
        Box::pin(self.connect_tcp_inner(host, port))
    }

    async fn connect_tcp_inner(&self, host: &str, port: u16) -> io::Result<(TcpStream, ConnMeta)> {
        let t0 = Instant::now();
        let (addrs, dns_ms) = match host.parse::<IpAddr>() {
            Ok(ip) => (vec![SocketAddr::new(ip, port)], None),
            Err(_) => {
                let found: Vec<SocketAddr> =
                    tokio::net::lookup_host((host, port)).await.map_err(|e| io_err(format!("DNS lookup for {host} failed: {e}")))?.collect();
                (found, Some(ms(t0)))
            }
        };
        if addrs.is_empty() {
            return Err(io_err(format!("DNS lookup for {host} returned no addresses")));
        }
        let t1 = Instant::now();
        let mut last: Option<io::Error> = None;
        for addr in addrs {
            match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
                Ok(Ok(s)) => {
                    let _ = s.set_nodelay(true);
                    let meta =
                        ConnMeta { dns_ms, connect_ms: ms(t1), tls_ms: None, remote_addr: s.peer_addr().ok(), first_use: Arc::new(AtomicBool::new(false)) };
                    return Ok((s, meta));
                }
                Ok(Err(e)) => last = Some(e),
                Err(_) => last = Some(io::Error::new(io::ErrorKind::TimedOut, "timed out")),
            }
        }
        let e = last.unwrap_or_else(|| io_err("no address".into()));
        Err(io::Error::new(e.kind(), format!("could not connect to {host}:{port}: {e}")))
    }

    pub fn connect<'a>(&'a self, https: bool, host: &'a str, port: u16, allow_h2: bool) -> BoxFuture<'a, io::Result<UpstreamIo>> {
        Box::pin(self.connect_inner(https, host, port, allow_h2))
    }

    async fn connect_inner(&self, https: bool, host: &str, port: u16, allow_h2: bool) -> io::Result<UpstreamIo> {
        let (tcp, mut meta) = self.connect_tcp_inner(host, port).await?;
        if !https {
            return Ok(UpstreamIo { stream: Stream::Plain(tcp), meta, h2: false });
        }
        let insecure = self.ignore_tls.matches(host, port);
        let h2 = allow_h2 && self.http2;
        let cfg = match (insecure, h2) {
            (false, true) => self.verify.clone(),
            (false, false) => self.verify_h1.clone(),
            (true, true) => self.insecure.clone(),
            (true, false) => self.insecure_h1.clone(),
        };
        let name = ServerName::try_from(host.to_string()).map_err(|e| io_err(format!("invalid TLS server name {host}: {e}")))?;
        let t = Instant::now();
        let tls = TlsConnector::from(cfg).connect(name, tcp).await.map_err(|e| {
            let msg = e.to_string();
            let hint = if msg.contains("certificate") {
                " — the server's certificate is not trusted; add the host to \"Ignore TLS errors\" in the proxy settings if that is expected"
            } else {
                ""
            };
            io::Error::new(e.kind(), format!("TLS handshake with {host}:{port} failed: {msg}{hint}"))
        })?;
        meta.tls_ms = Some(ms(t));
        let negotiated_h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
        Ok(UpstreamIo { stream: Stream::Tls(Box::new(tls)), meta, h2: negotiated_h2 })
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[derive(Clone)]
pub(crate) struct Connector(Arc<ConnectorShared>);

impl tower_service::Service<Uri> for Connector {
    type Response = TokioIo<UpstreamIo>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<TokioIo<UpstreamIo>>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<std::result::Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let shared = self.0.clone();
        Box::pin(async move {
            let https = uri.scheme_str() == Some("https");
            let host = uri.host().ok_or_else(|| io_err(format!("no host in {uri}")))?.trim_start_matches('[').trim_end_matches(']').to_string();
            let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
            shared.connect(https, &host, port, true).await.map(TokioIo::new)
        })
    }
}

/// The pooled upstream client plus raw connection access (WebSockets, tunnels).
pub(crate) struct Upstream {
    pub client: Client<Connector, ProxyBody>,
    pub shared: Arc<ConnectorShared>,
}

impl Upstream {
    pub fn new(cfg: &ProxyConfig) -> Result<Self> {
        let shared = Arc::new(ConnectorShared::new(cfg)?);
        let client = Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .timer(TokioTimer::new())
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(16)
            .http1_preserve_header_case(true)
            .set_host(true)
            .build(Connector(shared.clone()));
        Ok(Self { client, shared })
    }
}
