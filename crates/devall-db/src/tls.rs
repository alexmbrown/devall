//! rustls client configs (aws-lc provider, chosen explicitly) and the TLS connector for
//! tokio-postgres. `verify-full` checks certificates against the OS trust store (or a CA file);
//! `prefer`/`require` encrypt without verifying, like libpq.

use std::future::Future;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::{DbError, Result};

pub fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// Reads every certificate from a PEM file.
pub fn load_pem_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
        .map_err(|e| DbError::Tls(format!("cannot read CA file {}: {e}", path.display())))?
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| DbError::Tls(format!("invalid PEM in {}: {e}", path.display())))?;
    if certs.is_empty() {
        return Err(DbError::Tls(format!("no certificates in {}", path.display())));
    }
    Ok(certs)
}

/// A client config. `verify` = check the chain (OS store or `ca_cert`) and the host name.
pub fn client_config(verify: bool, ca_cert: Option<&Path>) -> Result<Arc<ClientConfig>> {
    let provider = crypto_provider();
    let builder = ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().map_err(|e| DbError::Tls(e.to_string()))?;
    let verifier: Arc<dyn ServerCertVerifier> = if !verify {
        Arc::new(AcceptAnyCert(provider))
    } else if let Some(ca) = ca_cert {
        let mut store = rustls::RootCertStore::empty();
        for c in load_pem_certs(ca)? {
            store.add(c).map_err(|e| DbError::Tls(format!("{}: {e}", ca.display())))?;
        }
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(store), provider).build().map_err(|e| DbError::Tls(e.to_string()))?
    } else {
        Arc::new(rustls_platform_verifier::Verifier::new(provider).map_err(|e| DbError::Tls(format!("OS certificate store: {e}")))?)
    };
    Ok(Arc::new(builder.dangerous().with_custom_certificate_verifier(verifier).with_no_client_auth()))
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

// ---------- tokio-postgres ----------

/// TLS for tokio-postgres. `server_name` overrides the host used for SNI and verification (the
/// real database host when connecting through an SSH tunnel on 127.0.0.1).
#[derive(Clone)]
pub struct PgTls {
    config: Arc<ClientConfig>,
    server_name: Option<String>,
}

impl PgTls {
    pub fn new(config: Arc<ClientConfig>, server_name: Option<String>) -> Self {
        Self { config, server_name }
    }
}

impl<S> tokio_postgres::tls::MakeTlsConnect<S> for PgTls
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = PgTlsStream<S>;
    type TlsConnect = PgTlsConnect;
    type Error = io::Error;

    fn make_tls_connect(&mut self, domain: &str) -> io::Result<PgTlsConnect> {
        let name = self.server_name.clone().unwrap_or_else(|| domain.to_string());
        let server_name = ServerName::try_from(name.clone()).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid TLS server name {name}: {e}")))?;
        Ok(PgTlsConnect { connector: tokio_rustls::TlsConnector::from(self.config.clone()), server_name })
    }
}

pub struct PgTlsConnect {
    connector: tokio_rustls::TlsConnector,
    server_name: ServerName<'static>,
}

impl<S> tokio_postgres::tls::TlsConnect<S> for PgTlsConnect
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = PgTlsStream<S>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<PgTlsStream<S>>> + Send>>;

    fn connect(self, stream: S) -> Self::Future {
        Box::pin(async move {
            let s = self.connector.connect(self.server_name, stream).await?;
            Ok(PgTlsStream(Box::new(s)))
        })
    }
}

pub struct PgTlsStream<S>(Box<tokio_rustls::client::TlsStream<S>>);

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for PgTlsStream<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_read(cx, buf)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for PgTlsStream<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> tokio_postgres::tls::TlsStream for PgTlsStream<S> {
    fn channel_binding(&self) -> tokio_postgres::tls::ChannelBinding {
        tokio_postgres::tls::ChannelBinding::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configs_build() {
        assert!(client_config(false, None).is_ok());
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("empty.pem");
        std::fs::write(&p, "not a cert").unwrap();
        assert!(client_config(true, Some(&p)).is_err());
        assert!(client_config(true, Some(&dir.path().join("missing.pem"))).is_err());
    }
}
