//! Optional Streamable HTTP transport, for clients that can't spawn a stdio process.
//!
//! * Binds `127.0.0.1` only (random port by default), endpoint `/mcp`.
//! * Every request needs `Authorization: Bearer <token>` (random 256-bit token per start).
//! * `Origin`, when present, must be this server's own loopback origin — browsers always send it,
//!   so a DNS-rebinding page is rejected with 403 even before the token check. rmcp additionally
//!   validates `Host` against loopback names.

use std::convert::Infallible;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::{Confirmer, DevallMcp, McpError, McpOptions};

type Resp = http::Response<BoxBody<Bytes, Infallible>>;

/// A running HTTP server; stops when [`HttpServer::stop`] is called or it is dropped.
pub struct HttpServer {
    pub url: String,
    pub token: String,
    pub port: u16,
    cancel: CancellationToken,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpInfo {
    pub url: String,
    pub port: u16,
    /// Bearer token clients must send (`Authorization: Bearer …`).
    pub token: String,
}

impl HttpServer {
    pub fn info(&self) -> HttpInfo {
        HttpInfo { url: self.url.clone(), port: self.port, token: self.token.clone() }
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn random_token() -> String {
    (0..4).map(|_| format!("{:016x}", fastrand::u64(..))).collect()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn plain(status: u16, msg: &'static str) -> Resp {
    let mut r = http::Response::new(Full::new(Bytes::from_static(msg.as_bytes())).boxed());
    *r.status_mut() = http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::BAD_REQUEST);
    r.headers_mut().insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("text/plain; charset=utf-8"));
    if status == 401 {
        r.headers_mut().insert(http::header::WWW_AUTHENTICATE, http::HeaderValue::from_static("Bearer realm=\"devall-mcp\""));
    }
    r
}

/// Origin + bearer-token gate in front of rmcp's service.
#[derive(Clone)]
struct Gate {
    inner: StreamableHttpService<DevallMcp, LocalSessionManager>,
    token: String,
    origins: [String; 2],
}

impl Gate {
    fn check<B>(&self, req: &http::Request<B>) -> Option<Resp> {
        if req.uri().path() != "/mcp" {
            return Some(plain(404, "not found — the MCP endpoint is /mcp"));
        }
        if let Some(origin) = req.headers().get(http::header::ORIGIN) {
            let o = origin.to_str().unwrap_or("");
            if !self.origins.iter().any(|a| a == o) {
                return Some(plain(403, "forbidden origin"));
            }
        }
        let auth = req.headers().get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
        let presented = auth.strip_prefix("Bearer ").or_else(|| auth.strip_prefix("bearer ")).unwrap_or("");
        if !ct_eq(presented.as_bytes(), self.token.as_bytes()) {
            return Some(plain(401, "missing or invalid bearer token"));
        }
        None
    }
}

impl tower_service::Service<http::Request<hyper::body::Incoming>> for Gate {
    type Response = Resp;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Resp, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<hyper::body::Incoming>) -> Self::Future {
        if let Some(denied) = self.check(&req) {
            return Box::pin(async move { Ok(denied) });
        }
        let fut = tower_service::Service::call(&mut self.inner, req);
        Box::pin(fut)
    }
}

/// Starts the Streamable HTTP server on `127.0.0.1:<port>` (`0` = random).
pub async fn serve_http(project_root: impl AsRef<Path>, options: McpOptions, port: u16) -> Result<HttpServer, McpError> {
    serve_http_with(project_root, options, port, None).await
}

/// [`serve_http`] with a [`Confirmer`] that asks the user to approve writes and requests in
/// protected environments (the desktop app).
pub async fn serve_http_with(project_root: impl AsRef<Path>, options: McpOptions, port: u16, confirmer: Option<Arc<dyn Confirmer>>) -> Result<HttpServer, McpError> {
    serve_server(DevallMcp::new_with_confirmer(project_root, options, confirmer)?, port).await
}

/// Serves an already configured server (e.g. with [`DevallMcp::set_embedder_source`]).
pub async fn serve_server(server: DevallMcp, port: u16) -> Result<HttpServer, McpError> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let port = listener.local_addr()?.port();
    let token = random_token();
    let origins = [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")];
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(["127.0.0.1", "localhost", "::1"])
        .with_allowed_origins(origins.clone())
        .enforce_origin_validation()
        .with_json_response(true);
    let cancel = config.cancellation_token.clone();
    let factory = server.clone();
    let service = StreamableHttpService::new(move || Ok(factory.clone()), std::sync::Arc::new(LocalSessionManager::default()), config);
    let gate = Gate { inner: service, token: token.clone(), origins };
    let stop = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let svc = hyper_util::service::TowerToHyperService::new(gate.clone());
                    let stop = stop.clone();
                    tokio::spawn(async move {
                        let conn = hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(stream), svc);
                        tokio::select! {
                            _ = conn => {}
                            _ = stop.cancelled() => {}
                        }
                    });
                }
            }
        }
    });
    Ok(HttpServer { url: format!("http://127.0.0.1:{port}/mcp"), token, port, cancel })
}
