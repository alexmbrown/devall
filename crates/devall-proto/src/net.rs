//! TCP + TLS plumbing shared by the WebSocket, Socket.IO and gRPC clients.
//!
//! TLS is rustls with the aws-lc-rs provider (the one the rest of the app already uses). With
//! verification on, certificates are checked against the operating system's trust store via
//! `rustls-platform-verifier` (so corporate CAs work, like the HTTP client); with it off, any
//! certificate is accepted but handshake signatures are still checked.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::error::{ProtoError, Result, chain};

/// The crypto provider used for every client config (explicit, so it doesn't matter which
/// rustls provider features other crates enable).
pub fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// A rustls client config. `verify = false` accepts any certificate. `alpn` e.g. `[b"h2"]`.
pub fn tls_config(verify: bool, alpn: &[&[u8]]) -> Result<Arc<ClientConfig>> {
    let provider = crypto_provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| ProtoError::Tls(e.to_string()))?;
    let verifier: Arc<dyn ServerCertVerifier> = if verify {
        Arc::new(rustls_platform_verifier::Verifier::new(provider).map_err(|e| ProtoError::Tls(format!("OS certificate store: {e}")))?)
    } else {
        Arc::new(AcceptAnyCert(provider))
    };
    let mut cfg = builder.dangerous().with_custom_certificate_verifier(verifier).with_no_client_auth();
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(Arc::new(cfg))
}

#[derive(Debug)]
struct AcceptAnyCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// A plain or TLS TCP stream.
#[derive(Debug)]
pub enum NetStream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for NetStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for NetStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[std::io::IoSlice<'_>]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            NetStream::Plain(s) => s.is_write_vectored(),
            NetStream::Tls(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_flush(cx),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            NetStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            NetStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Converts `0` to "no timeout".
pub fn timeout_ms(ms: u64) -> Duration {
    if ms == 0 { Duration::from_secs(60 * 60 * 24 * 365) } else { Duration::from_millis(ms) }
}

/// Opens a TCP connection to `host:port`, then TLS when `tls` is set (SNI = `host`).
pub async fn connect(host: &str, port: u16, tls: Option<Arc<ClientConfig>>, timeout: Duration) -> Result<NetStream> {
    let fut = async {
        let tcp = TcpStream::connect((host, port)).await.map_err(|e| ProtoError::Connect(format!("{host}:{port}: {e}")))?;
        let _ = tcp.set_nodelay(true);
        match tls {
            None => Ok(NetStream::Plain(tcp)),
            Some(cfg) => {
                let name = ServerName::try_from(host.to_string()).map_err(|e| ProtoError::Tls(format!("invalid server name {host}: {e}")))?;
                let s = TlsConnector::from(cfg).connect(name, tcp).await.map_err(|e| ProtoError::Tls(chain(&e)))?;
                Ok(NetStream::Tls(Box::new(s)))
            }
        }
    };
    tokio::time::timeout(timeout, fut).await.map_err(|_| ProtoError::Timeout(format!("connecting to {host}:{port}")))?
}

/// Host of a URL suitable for connecting and SNI (IPv6 brackets removed).
pub fn host_of(url: &url::Url) -> Result<String> {
    match url.host() {
        Some(url::Host::Domain(d)) => Ok(d.to_string()),
        Some(url::Host::Ipv4(a)) => Ok(a.to_string()),
        Some(url::Host::Ipv6(a)) => Ok(a.to_string()),
        None => Err(ProtoError::InvalidUrl(format!("{url} has no host"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_both_tls_modes() {
        let strict = tls_config(true, &[b"h2"]).unwrap();
        assert_eq!(strict.alpn_protocols, vec![b"h2".to_vec()]);
        let lax = tls_config(false, &[]).unwrap();
        assert!(lax.alpn_protocols.is_empty());
    }

    #[test]
    fn hosts_without_brackets() {
        assert_eq!(host_of(&url::Url::parse("ws://[::1]:80/x").unwrap()).unwrap(), "::1");
        assert_eq!(host_of(&url::Url::parse("ws://example.com/x").unwrap()).unwrap(), "example.com");
    }

    #[tokio::test]
    async fn connect_refused_is_a_connect_error() {
        // Bind then drop to get a port nobody listens on.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let e = connect("127.0.0.1", port, None, Duration::from_secs(5)).await.unwrap_err();
        assert!(matches!(e, ProtoError::Connect(_)), "{e}");
    }
}
