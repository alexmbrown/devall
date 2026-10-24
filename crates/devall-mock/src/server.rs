//! HTTP plumbing (hyper 1.x, HTTP/1.1) and the shared server state.
//!
//! Faults are injected below hyper: every connection's socket is wrapped in [`FaultIo`], and when
//! a response must fail, the handler arms the wrapper, which then resets the socket, closes it,
//! or writes garbage instead of the response hyper tries to send.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::future::Future as _;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::request::Parts;
use http::{HeaderName, HeaderValue, Request, Response, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Body, Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinSet;
use tokio::time::Sleep;
use uuid::Uuid;

use crate::chaos::Chaos;
use crate::error::MockError;
use crate::files::{LoadError, ProjectFiles};
use crate::journal::{Journal, LoggedResponse, ServeEvent, Timing};
use crate::recording::RecordingState;
use crate::request::MockRequest;
use crate::store::{Matched, StubStore};
use crate::stub::{Dribble, Fault, Stub};
use crate::template::TemplateEngine;
use crate::{JournalEvent, MockConfig, RequestSummary, admin, response};

pub(crate) type RespBody = UnsyncBoxBody<Bytes, Infallible>;

const MAX_BODY: usize = 64 * 1024 * 1024;

pub(crate) fn full(body: impl Into<Bytes>) -> RespBody {
    Full::new(body.into()).boxed_unsync()
}

pub(crate) struct ServerState {
    pub config: MockConfig,
    pub files: ProjectFiles,
    pub store: RwLock<StubStore>,
    pub load_errors: RwLock<Vec<LoadError>>,
    pub journal: Mutex<Journal>,
    pub settings: RwLock<Map<String, Value>>,
    pub chaos: RwLock<Chaos>,
    pub recording: Mutex<RecordingState>,
    pub templates: TemplateEngine,
    pub data: Arc<crate::data::DataSources>,
    pub http: reqwest::Client,
    pub events: broadcast::Sender<JournalEvent>,
    pub started: Instant,
    pub port: u16,
    pub shutdown: watch::Sender<bool>,
    pub running: AtomicBool,
    /// GraphQL schemas from `mocks/graphql/`.
    pub graphql: RwLock<Arc<crate::graphql::GraphqlSet>>,
    pub ws: crate::ws::WsState,
    pub grpc: crate::grpc::GrpcState,
    pub mqtt: crate::mqtt::MqttState,
    /// WebSocket / gRPC / MQTT message journal.
    pub messages: crate::messages::Messages,
}

/// Locks that recover from poisoning (a panicking request must not take the server down).
pub(crate) fn read<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}
pub(crate) fn write<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ServerState {
    pub fn notify(&self, event: JournalEvent) {
        let _ = self.events.send(event);
    }

    pub fn stub_count(&self) -> usize {
        read(&self.store).len()
    }

    fn mappings_changed(&self) {
        self.notify(JournalEvent::MappingsChanged { stub_count: self.stub_count() });
    }

    /// Replaces all stubs with what is on disk (non-persistent stubs are dropped), and reloads
    /// the GraphQL schemas and the WebSocket / gRPC / MQTT definitions.
    pub fn reload(self: &Arc<Self>) -> Vec<LoadError> {
        let (stubs, mut errors) = self.files.load();
        {
            let mut store = write(&self.store);
            store.clear();
            for s in stubs {
                store.insert(s, true);
            }
        }
        let gql = crate::graphql::GraphqlSet::load(&self.files.root);
        errors.extend(gql.errors.iter().cloned());
        *write(&self.graphql) = Arc::new(gql);
        errors.extend(self.ws.reload(&self.files.root));
        errors.extend(self.grpc.reload(&self.files.root));
        errors.extend(self.mqtt.reload(&self.files.root));
        crate::mqtt::restart_behaviours(self);
        *write(&self.load_errors) = errors.clone();
        self.mappings_changed();
        errors
    }

    /// Adds (or replaces by id) a stub, writing it to the project when it is persistent.
    pub fn add_stub(&self, mut stub: Stub) -> Result<Arc<Stub>, MockError> {
        let existing = read(&self.store).get(&stub.id);
        if let Some(old) = &existing {
            stub.source = old.source.clone();
            if !stub.persistent && old.source.is_some() {
                self.files.remove_stub(old)?;
                stub.source = None;
            }
        }
        if stub.persistent {
            self.files.save_stub(&mut stub)?;
        }
        let arc = write(&self.store).insert(stub, existing.is_none());
        self.mappings_changed();
        Ok(arc)
    }

    pub fn remove_stub(&self, id: &Uuid) -> Result<Option<Arc<Stub>>, MockError> {
        let removed = write(&self.store).remove(id);
        if let Some(s) = &removed {
            self.files.remove_stub(s)?;
            self.mappings_changed();
        }
        Ok(removed)
    }

    /// Persists every stub that is not yet in the project.
    pub fn save_all(&self) -> Result<usize, MockError> {
        let pending: Vec<Arc<Stub>> = read(&self.store).all().iter().filter(|s| !s.persistent || s.source.is_none()).cloned().collect();
        for s in &pending {
            let mut stub = (**s).clone();
            self.files.save_stub(&mut stub)?;
            write(&self.store).insert(stub, false);
        }
        if !pending.is_empty() {
            self.mappings_changed();
        }
        Ok(pending.len())
    }

    /// Finds the serving stub and applies its scenario transition atomically.
    pub fn match_request(&self, req: &MockRequest) -> Option<Matched> {
        for _ in 0..8 {
            let found = read(&self.store).find(req)?;
            if found.stub.scenario_name.is_none() {
                return Some(found);
            }
            if write(&self.store).transition(&found.stub) {
                return Some(found);
            }
        }
        None
    }

    pub fn clear_journal(&self) {
        lock(&self.journal).clear();
        self.notify(JournalEvent::JournalCleared);
    }
}

// ---------------------------------------------------------------- fault-injecting socket

const FAULT_NONE: u8 = 0;
const FAULT_DONE: u8 = 255;

fn fault_code(f: Fault) -> u8 {
    match f {
        Fault::ConnectionResetByPeer => 1,
        Fault::EmptyResponse => 2,
        Fault::MalformedResponseChunk => 3,
        Fault::RandomDataThenClose => 4,
    }
}

struct FaultIo {
    inner: TcpStream,
    fault: Arc<AtomicU8>,
}

impl FaultIo {
    fn fire(&mut self, code: u8) {
        match code {
            1 => {
                let _ = self.inner.set_zero_linger();
            }
            3 => {
                let _ = self.inner.try_write(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nlskdu018973t09sylgasjkfg1][]'./.sdlv");
            }
            4 => {
                let garbage: Vec<u8> = (0..256).map(|_| fastrand::u8(..)).collect();
                let _ = self.inner.try_write(&garbage);
            }
            _ => {}
        }
        self.fault.store(FAULT_DONE, Ordering::SeqCst);
    }
}

impl AsyncRead for FaultIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for FaultIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.fault.load(Ordering::SeqCst) {
            FAULT_NONE => Pin::new(&mut self.inner).poll_write(cx, buf),
            code => {
                if code != FAULT_DONE {
                    self.fire(code);
                }
                Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionAborted, "injected fault")))
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

// ---------------------------------------------------------------- dribbled bodies

struct DribbleBody {
    chunks: VecDeque<Bytes>,
    interval: Duration,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl Body for DribbleBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let this = self.get_mut();
        if let Some(s) = this.sleep.as_mut() {
            if s.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            this.sleep = None;
        }
        match this.chunks.pop_front() {
            None => Poll::Ready(None),
            Some(chunk) => {
                if !this.chunks.is_empty() {
                    this.sleep = Some(Box::pin(tokio::time::sleep(this.interval)));
                }
                Poll::Ready(Some(Ok(Frame::data(chunk))))
            }
        }
    }
}

fn dribble(body: Bytes, d: Dribble) -> RespBody {
    let n = (d.chunks.max(1) as usize).min(body.len().max(1));
    let size = body.len().div_ceil(n).max(1);
    let mut chunks = VecDeque::new();
    let mut start = 0;
    while start < body.len() {
        let end = (start + size).min(body.len());
        chunks.push_back(body.slice(start..end));
        start = end;
    }
    let gaps = chunks.len().saturating_sub(1).max(1) as u64;
    DribbleBody { chunks, interval: Duration::from_millis(d.total_ms / gaps), sleep: None }.boxed_unsync()
}

// ---------------------------------------------------------------- serving

async fn read_body(body: Incoming) -> Result<Bytes, ()> {
    match Limited::new(body, MAX_BODY).collect().await {
        Ok(c) => Ok(c.to_bytes()),
        Err(_) => Err(()),
    }
}

fn mock_request(parts: &Parts, body: Bytes, peer: SocketAddr, port: u16) -> MockRequest {
    let url = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let headers: Vec<(String, String)> = parts.headers.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let has_host = parts.headers.contains_key(http::header::HOST);
    let mut r = MockRequest::new(parts.method.as_str(), url, headers, body);
    r.client_ip = peer.ip().to_string();
    if !has_host {
        r.host = "127.0.0.1".into();
        r.port = port;
    }
    r.protocol = match parts.version {
        http::Version::HTTP_10 => "HTTP/1.0".into(),
        http::Version::HTTP_2 => "HTTP/2".into(),
        _ => "HTTP/1.1".into(),
    };
    r
}

pub(crate) fn build_response(status: u16, reason: Option<&str>, headers: &[(String, String)], body: RespBody) -> Response<RespBody> {
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if let Some(r) = reason.filter(|r| !r.is_empty())
        && let Ok(rp) = hyper::ext::ReasonPhrase::try_from(r.as_bytes().to_vec())
    {
        resp.extensions_mut().insert(rp);
    }
    for (k, v) in headers {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_bytes(v.as_bytes())) {
            resp.headers_mut().append(name, value);
        }
    }
    resp
}

fn stub_cors(req: &MockRequest, headers: &mut Vec<(String, String)>) {
    let origin = req.header("origin").unwrap_or("*").to_string();
    headers.retain(|(k, _)| !k.to_ascii_lowercase().starts_with("access-control-"));
    headers.push(("Access-Control-Allow-Origin".into(), origin));
    headers.push(("Access-Control-Allow-Methods".into(), "OPTIONS, GET, POST, PUT, PATCH, DELETE, HEAD".into()));
    let allow = req.header("access-control-request-headers").unwrap_or("*").to_string();
    headers.push(("Access-Control-Allow-Headers".into(), allow));
    headers.push(("Access-Control-Expose-Headers".into(), "*".into()));
}

async fn serve_mock(st: Arc<ServerState>, req: MockRequest, fault_cell: Arc<AtomicU8>) -> Response<RespBody> {
    let t0 = Instant::now();
    let decision = read(&st.chaos).decide(&req);
    let injected = decision.as_ref().is_some_and(|d| d.error_status.is_some() || d.fault.is_some());

    // Chaos-injected failures don't advance scenarios: the stub is looked up read-only.
    let matched = if injected { read(&st.store).find(&req) } else { st.match_request(&req) };
    let preflight = st.config.stub_cors && matched.is_none() && req.method == "OPTIONS" && req.header("access-control-request-method").is_some();
    let gql = read(&st.graphql).clone();
    let gql_endpoint = if matched.is_none() && !injected && !preflight { gql.for_path(&req.path) } else { None };
    let mut prepared = match (&matched, injected) {
        (_, true) => response::chaos_error(decision.as_ref().and_then(|d| d.error_status).unwrap_or(500)),
        (Some(m), false) => {
            let mut p = response::prepare(&st, m, &req).await;
            if m.stub.response.transformers.iter().any(|t| t == crate::graphql::AUTOMOCK_TRANSFORMER) {
                graphql_overlay(&gql, m, &req, &mut p);
            }
            p
        }
        (None, false) if preflight => response::Prepared::simple(200, "text/plain", ""),
        (None, false) => match gql_endpoint {
            Some(entry) => {
                let (status, body) = crate::graphql::respond(entry, &req, None);
                let mut p = response::Prepared::simple(status, "application/json", serde_json::to_vec(&body).unwrap_or_default());
                p.response_definition = serde_json::json!({ "status": status, "fromConfiguredStub": false, "devallGraphql": entry.name });
                p
            }
            None => response::not_found(&st, &req),
        },
    };
    if let Some(d) = &decision {
        prepared.delay_ms += d.latency_ms;
        if let Some(f) = d.fault {
            prepared.fault = Some(f);
        }
    }
    if st.config.stub_cors {
        stub_cors(&req, &mut prepared.headers);
    }
    let process_ms = t0.elapsed().as_millis() as u64;
    if prepared.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(prepared.delay_ms)).await;
    }
    let total = t0.elapsed().as_millis() as u64;
    let was_matched = matched.is_some() || preflight || gql_endpoint.is_some();
    let event = ServeEvent {
        seq: 0,
        id: Uuid::new_v4(),
        request: req,
        stub: matched.as_ref().map(|m| m.stub.clone()),
        response_definition: prepared.response_definition.clone(),
        response: LoggedResponse { status: prepared.status, headers: prepared.headers.clone(), body: prepared.body.clone(), fault: prepared.fault },
        was_matched,
        timing: Timing { added_delay: prepared.delay_ms, process_time: process_ms, response_send_time: 0, serve_time: total, total_time: total },
        proxied_from: prepared.proxied_from.clone(),
        path_params: matched.as_ref().map(|m| m.path_params.clone()).unwrap_or_default(),
        chaos: decision.as_ref().map(|d| d.to_json()),
    };
    let summary = RequestSummary::from_event(&event);
    lock(&st.journal).push(event);
    st.notify(JournalEvent::Request(summary));

    if let Some(f) = prepared.fault {
        fault_cell.store(fault_code(f), Ordering::SeqCst);
        return Response::new(full(Bytes::new()));
    }
    let body = match prepared.dribble {
        Some(d) if !prepared.body.is_empty() => dribble(prepared.body, d),
        _ => full(prepared.body),
    };
    build_response(prepared.status, prepared.reason.as_deref(), &prepared.headers, body)
}

/// A stub with the `graphql-automock` transformer: data generated from the schema, with the stub
/// body's `data` as per-field overrides and its `errors` passed through.
fn graphql_overlay(gql: &crate::graphql::GraphqlSet, m: &crate::store::Matched, req: &MockRequest, p: &mut response::Prepared) {
    let params = &m.stub.response.transformer_parameters;
    let entry = params.get("schema").and_then(Value::as_str).and_then(|n| gql.by_name(n)).or_else(|| gql.for_path(&req.path)).or_else(|| (gql.schemas.len() == 1).then(|| &gql.schemas[0]));
    let Some(entry) = entry else {
        p.status = 500;
        p.body = Bytes::from("graphql-automock: no GraphQL schema for this stub (add one to mocks/graphql/ or set transformerParameters.schema)");
        return;
    };
    let stub_body: Option<Value> = if p.body.is_empty() { None } else { serde_json::from_slice(&p.body).ok() };
    let (status, body) = crate::graphql::respond(entry, req, stub_body.as_ref());
    if p.status == 200 {
        p.status = status;
    }
    p.body = Bytes::from(serde_json::to_vec(&body).unwrap_or_default());
    p.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type") && !k.eq_ignore_ascii_case("content-length"));
    p.headers.push(("Content-Type".into(), "application/json".into()));
}

async fn handle(st: Arc<ServerState>, mut req: Request<Incoming>, peer: SocketAddr, fault: Arc<AtomicU8>) -> Response<RespBody> {
    // WebSocket upgrades are answered by a matching WebSocket stub (else served like any request).
    if crate::ws::is_upgrade(req.headers()) && !req.uri().path().starts_with("/__admin") {
        let on_upgrade = hyper::upgrade::on(&mut req);
        let (parts, _) = req.into_parts();
        let mreq = mock_request(&parts, Bytes::new(), peer, st.port);
        return match crate::ws::accept(&st, &parts.headers, mreq, on_upgrade) {
            Ok(resp) => resp,
            Err(mreq) => serve_mock(st, *mreq, fault).await,
        };
    }
    let (parts, body) = req.into_parts();
    let Ok(body) = read_body(body).await else {
        return build_response(413, None, &[("Content-Type".into(), "text/plain".into())], full("request body too large"));
    };
    let path = parts.uri.path();
    if path == "/__admin" || path.starts_with("/__admin/") {
        return admin::handle(&st, &parts, body, peer).await;
    }
    let req = mock_request(&parts, body, peer, st.port);
    serve_mock(st, req, fault).await
}

async fn serve_connection(stream: TcpStream, peer: SocketAddr, st: Arc<ServerState>, mut shutdown: watch::Receiver<bool>) {
    let _ = stream.set_nodelay(true);
    let fault = Arc::new(AtomicU8::new(FAULT_NONE));
    let io = TokioIo::new(FaultIo { inner: stream, fault: fault.clone() });
    let svc = service_fn(move |req| {
        let st = st.clone();
        let fault = fault.clone();
        async move { Ok::<_, Infallible>(handle(st, req, peer, fault).await) }
    });
    let mut builder = http1::Builder::new();
    builder.timer(TokioTimer::new()).keep_alive(true).title_case_headers(true).header_read_timeout(Duration::from_secs(30));
    let conn = builder.serve_connection(io, svc).with_upgrades();
    tokio::pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => {}
        _ = shutdown.changed() => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.as_mut().await;
        }
    }
}

pub(crate) async fn run(listener: TcpListener, st: Arc<ServerState>) {
    let mut shutdown = st.shutdown.subscribe();
    let mut conns = JoinSet::new();
    if !*shutdown.borrow_and_update() {
        loop {
            tokio::select! {
                res = listener.accept() => match res {
                    Ok((stream, peer)) => {
                        conns.spawn(serve_connection(stream, peer, st.clone(), st.shutdown.subscribe()));
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                },
                _ = shutdown.changed() => break,
                Some(_) = conns.join_next(), if !conns.is_empty() => {}
            }
        }
    }
    drop(listener);
    let drain = async { while conns.join_next().await.is_some() {} };
    if tokio::time::timeout(Duration::from_secs(2), drain).await.is_err() {
        conns.abort_all();
        while conns.join_next().await.is_some() {}
    }
    crate::grpc::stop(&st).await;
    crate::mqtt::stop(&st).await;
    st.running.store(false, Ordering::SeqCst);
    st.notify(JournalEvent::Stopped);
}
