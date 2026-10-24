//! `devall load agent`: a load worker that runs plans sent by a coordinator
//! ([`crate::distributed`]).
//!
//! | Route | Auth | |
//! |---|---|---|
//! | `GET /healthz` | none | `ok` (container probes) |
//! | `GET /v1/info` | bearer | [`AgentInfo`] |
//! | `POST /v1/run` | bearer | [`AgentPlan`] → NDJSON stream of [`AgentEvent`]s |
//! | `POST /v1/stop` | bearer | stops the running plan (it still reports its results) |
//!
//! One run at a time (`409` while busy). A coordinator that disconnects stops its run. TLS is
//! optional (`--tls-cert/--tls-key`, rustls); otherwise run it on a trusted network or behind a
//! TLS terminator — the bearer token and any secrets in plans travel in the clear without it.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Frame, Incoming};
use hyper::{Request, Response, StatusCode};
use serde_json::json;
use tokio::sync::mpsc;

use crate::distributed::{AGENT_PROTOCOL, AgentEvent, AgentInfo, AgentPlan, MIN_TOKEN_LEN, agent_run_options};
use crate::engine::{ProgressFn, RunControl};
use crate::error::{LoadError, Result};
use crate::plan::Plan;
use crate::summary::Machine;

/// Largest plan accepted (requests with bodies; generous).
const MAX_PLAN_BYTES: usize = 64 * 1024 * 1024;

type Resp = Response<BoxBody<Bytes, Infallible>>;

#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub listen: SocketAddr,
    /// Shared bearer token (at least 16 characters).
    pub token: String,
    /// PEM certificate chain and private key: serve HTTPS.
    pub tls: Option<(PathBuf, PathBuf)>,
    /// Reported in `/v1/info` (the CLI passes its version).
    pub version: String,
}

struct AgentState {
    token: String,
    version: String,
    scratch: PathBuf,
    busy: Mutex<Option<(String, RunControl)>>,
}

impl AgentState {
    fn info(&self) -> AgentInfo {
        let busy = self.busy.lock().unwrap();
        AgentInfo { version: self.version.clone(), protocol: AGENT_PROTOCOL, machine: Machine::current(), busy: busy.is_some(), run_id: busy.as_ref().map(|b| b.0.clone()) }
    }
}

/// A running agent. Dropping it (or [`AgentServer::stop`]) closes the listener and stops the run.
pub struct AgentServer {
    pub addr: SocketAddr,
    pub tls: bool,
    shutdown: tokio::sync::watch::Sender<bool>,
    state: Arc<AgentState>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl AgentServer {
    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
        if let Some((_, c)) = self.state.busy.lock().unwrap().as_ref() {
            c.stop_with("the agent is shutting down");
        }
    }

    /// Waits until the accept loop has ended (after [`AgentServer::stop`]).
    pub async fn closed(mut self) {
        self.stop();
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }

    pub fn url(&self) -> String {
        format!("{}://{}", if self.tls { "https" } else { "http" }, self.addr)
    }
}

impl Drop for AgentServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

fn tls_acceptor(cert: &PathBuf, key: &PathBuf) -> Result<tokio_rustls::TlsAcceptor> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
        .map_err(|e| LoadError::Invalid(format!("TLS certificate {}: {e}", cert.display())))?
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| LoadError::Invalid(format!("TLS certificate {}: {e}", cert.display())))?;
    if certs.is_empty() {
        return Err(LoadError::Invalid(format!("no certificates in {}", cert.display())));
    }
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| LoadError::Invalid(format!("TLS key {}: {e}", key.display())))?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| LoadError::Invalid(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| LoadError::Invalid(format!("TLS: {e}")))?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(cfg)))
}

/// Binds and starts serving.
pub async fn start_agent(opts: AgentOptions) -> Result<AgentServer> {
    if opts.token.trim().len() < MIN_TOKEN_LEN {
        return Err(LoadError::Invalid(format!("the agent token must be at least {MIN_TOKEN_LEN} characters (generate one with `devall load agent token`)")));
    }
    let acceptor = match &opts.tls {
        Some((c, k)) => Some(tls_acceptor(c, k)?),
        None => None,
    };
    let scratch = std::env::temp_dir().join("devall-load-agent");
    let _ = std::fs::create_dir_all(&scratch);
    let listener = tokio::net::TcpListener::bind(opts.listen).await.map_err(|e| LoadError::Invalid(format!("cannot listen on {}: {e}", opts.listen)))?;
    let addr = listener.local_addr().map_err(|e| LoadError::Engine(e.to_string()))?;
    let state = Arc::new(AgentState { token: opts.token.trim().to_string(), version: opts.version, scratch, busy: Mutex::new(None) });
    let (shutdown, mut rx) = tokio::sync::watch::channel(false);
    let st = state.clone();
    let tls = acceptor.is_some();
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                a = listener.accept() => a,
                _ = rx.changed() => break,
            };
            let Ok((stream, _)) = accepted else { continue };
            let _ = stream.set_nodelay(true);
            let st = st.clone();
            let acceptor = acceptor.clone();
            let mut stop = rx.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |req| handle(st.clone(), req));
                let builder = hyper::server::conn::http1::Builder::new();
                match acceptor {
                    Some(a) => {
                        let Ok(Ok(tls)) = tokio::time::timeout(Duration::from_secs(10), a.accept(stream)).await else { return };
                        let conn = builder.serve_connection(hyper_util::rt::TokioIo::new(tls), svc);
                        tokio::select! {
                            _ = conn => {}
                            _ = stop.changed() => {}
                        }
                    }
                    None => {
                        let conn = builder.serve_connection(hyper_util::rt::TokioIo::new(stream), svc);
                        tokio::select! {
                            _ = conn => {}
                            _ = stop.changed() => {}
                        }
                    }
                }
            });
        }
    });
    Ok(AgentServer { addr, tls, shutdown, state, task: Some(task) })
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn json_resp(status: StatusCode, v: serde_json::Value) -> Resp {
    let mut r = Response::new(Full::new(Bytes::from(v.to_string())).boxed());
    *r.status_mut() = status;
    r.headers_mut().insert(hyper::header::CONTENT_TYPE, hyper::header::HeaderValue::from_static("application/json"));
    r
}

fn error(status: StatusCode, msg: impl Into<String>) -> Resp {
    json_resp(status, json!({ "error": msg.into() }))
}

/// The NDJSON event stream of a run.
struct ChannelBody(mpsc::Receiver<Bytes>);

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<std::result::Result<Frame<Bytes>, Infallible>>> {
        self.0.poll_recv(cx).map(|o| o.map(|b| Ok(Frame::data(b))))
    }
}

fn line(ev: &AgentEvent) -> Bytes {
    let mut v = serde_json::to_vec(ev).unwrap_or_default();
    v.push(b'\n');
    Bytes::from(v)
}

async fn handle(st: Arc<AgentState>, req: Request<Incoming>) -> std::result::Result<Resp, Infallible> {
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    if method == hyper::Method::GET && path == "/healthz" {
        return Ok(Response::new(Full::new(Bytes::from_static(b"ok")).boxed()));
    }
    let auth = req.headers().get(hyper::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
    let presented = auth.strip_prefix("Bearer ").or_else(|| auth.strip_prefix("bearer ")).unwrap_or("");
    if !ct_eq(presented.as_bytes(), st.token.as_bytes()) {
        // Slows down guessing.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let mut r = error(StatusCode::UNAUTHORIZED, "missing or invalid bearer token");
        r.headers_mut().insert(hyper::header::WWW_AUTHENTICATE, hyper::header::HeaderValue::from_static("Bearer realm=\"devall-load-agent\""));
        return Ok(r);
    }
    Ok(match (method.as_str(), path.as_str()) {
        ("GET", "/v1/info") => json_resp(StatusCode::OK, serde_json::to_value(st.info()).unwrap_or_default()),
        ("POST", "/v1/stop") => {
            let stopped = match st.busy.lock().unwrap().as_ref() {
                Some((_, c)) => {
                    c.stop_with("stopped by the coordinator");
                    true
                }
                None => false,
            };
            json_resp(StatusCode::OK, json!({ "stopped": stopped }))
        }
        ("POST", "/v1/run") => start_run(st, req).await,
        _ => error(StatusCode::NOT_FOUND, "not found"),
    })
}

async fn start_run(st: Arc<AgentState>, req: Request<Incoming>) -> Resp {
    let body = match Limited::new(req.into_body(), MAX_PLAN_BYTES).collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return error(StatusCode::PAYLOAD_TOO_LARGE, format!("could not read the plan: {e}")),
    };
    let ap: AgentPlan = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => return error(StatusCode::BAD_REQUEST, format!("invalid plan: {e}")),
    };
    if ap.protocol != AGENT_PROTOCOL {
        return error(StatusCode::BAD_REQUEST, format!("this agent speaks protocol {AGENT_PROTOCOL}, the plan is protocol {}", ap.protocol));
    }
    let control = RunControl::new();
    {
        let mut busy = st.busy.lock().unwrap();
        if let Some((id, _)) = busy.as_ref() {
            return error(StatusCode::CONFLICT, format!("already running {id}"));
        }
        *busy = Some((ap.run_id.clone(), control.clone()));
    }
    let (tx, rx) = mpsc::channel::<Bytes>(256);
    tokio::spawn(run_task(st, ap, control, tx));
    let mut r = Response::new(ChannelBody(rx).boxed());
    r.headers_mut().insert(hyper::header::CONTENT_TYPE, hyper::header::HeaderValue::from_static("application/x-ndjson"));
    r.headers_mut().insert(hyper::header::CACHE_CONTROL, hyper::header::HeaderValue::from_static("no-store"));
    r
}

async fn run_task(st: Arc<AgentState>, ap: AgentPlan, control: RunControl, tx: mpsc::Sender<Bytes>) {
    let finish = |st: &AgentState| *st.busy.lock().unwrap() = None;
    if tx.send(line(&AgentEvent::Started { run_id: ap.run_id.clone(), info: st.info() })).await.is_err() {
        finish(&st);
        return;
    }
    let opts = agent_run_options(&ap);
    let delay = Duration::from_millis(ap.start_delay_ms.min(10_000));
    let plan = match Plan::from_agent_plan(ap, &st.scratch) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(line(&AgentEvent::Error { message: e.to_string() })).await;
            finish(&st);
            return;
        }
    };
    tokio::time::sleep(delay).await;
    let secrets = plan.secret_values.clone();
    let (tx2, c2) = (tx.clone(), control.clone());
    let progress: ProgressFn = Arc::new(move |snapshot| {
        if let Err(mpsc::error::TrySendError::Closed(_)) = tx2.try_send(line(&AgentEvent::Snapshot { snapshot })) {
            c2.stop_with("the coordinator disconnected");
        }
    });
    let ev = match crate::engine::run_detailed(plan, opts, Some(progress), control).await {
        Ok((summary, measurements)) => AgentEvent::Finished { summary, measurements },
        Err(e) => AgentEvent::Error { message: devall_core::vars::redact_values(&e.to_string(), &secrets) },
    };
    let _ = tx.send(line(&ev)).await;
    finish(&st);
}
