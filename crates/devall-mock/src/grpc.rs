//! gRPC mocks: a separate HTTP/2 (h2c) listener serving the services described by `.proto` files
//! in `mocks/grpc/` (compiled with protox, messages handled dynamically with prost-reflect), with
//! server reflection (v1 and v1alpha) so grpcurl, Postman or /dev/all's own gRPC client discover
//! them.
//!
//! Stubs are `*.json` files anywhere under `mocks/grpc/` (one object, an array or
//! `{"stubs": [...]}`):
//!
//! ```json
//! {
//!   "name": "Say hello to Ada",
//!   "method": "test.v1.Greeter/SayHello",
//!   "request": { "body": { "name": "Ada" }, "metadata": { "x-tenant": { "equalTo": "a" } } },
//!   "response": {
//!     "message": { "message": "Hello {{jsonPath request.body '$.name'}}" },
//!     "headers": { "x-mock": "yes" }, "trailers": { "x-trace": "t-1" },
//!     "status": "OK", "fixedDelayMilliseconds": 0
//!   }
//! }
//! ```
//!
//! `request.body` is a WireMock value matcher on the request message as proto3 JSON (an object is
//! shorthand for `equalToJson` ignoring extra fields); client-streaming and bidi calls match on the
//! JSON array of every message the client sent (the mock answers once the client half-closes).
//! Server-streaming responses list `messages` with `streamIntervalMs` between them; `status` is a
//! code name or number with an optional `statusMessage`. Responses are response templates.
//! Unmatched calls get `UNIMPLEMENTED`; compressed request messages are refused.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use bytes::{BufMut, Bytes, BytesMut};
use http::{HeaderMap, HeaderName, HeaderValue, Request, Response};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper_util::rt::{TokioExecutor, TokioIo};
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MethodDescriptor};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tower::util::BoxCloneSyncService;
use tower::{Service, ServiceExt};

use crate::error::MockError;
use crate::files::{LoadError, MOCKS_DIR, load_json_defs};
use crate::matchers::{MultiPattern, ValuePattern};
use crate::messages::{Direction, Protocol};
use crate::request::MockRequest;
use crate::server::ServerState;
use crate::template;
use crate::util::stable_uuid;

pub const GRPC_DIR: &str = "grpc";
const MAX_REQUEST: usize = 16 * 1024 * 1024;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type GrpcBody = UnsyncBoxBody<Bytes, BoxError>;
type Reflection = BoxCloneSyncService<Request<Incoming>, Response<tonic::body::Body>, Infallible>;

const CODES: [&str; 17] = [
    "OK",
    "CANCELLED",
    "UNKNOWN",
    "INVALID_ARGUMENT",
    "DEADLINE_EXCEEDED",
    "NOT_FOUND",
    "ALREADY_EXISTS",
    "PERMISSION_DENIED",
    "RESOURCE_EXHAUSTED",
    "FAILED_PRECONDITION",
    "ABORTED",
    "OUT_OF_RANGE",
    "UNIMPLEMENTED",
    "INTERNAL",
    "UNAVAILABLE",
    "DATA_LOSS",
    "UNAUTHENTICATED",
];

fn parse_code(v: &Value) -> Result<u16, String> {
    match v {
        Value::Number(n) => n.as_u64().filter(|n| *n <= 16).map(|n| n as u16).ok_or_else(|| format!("status {n} is not a gRPC code (0–16)")),
        Value::String(s) => {
            let up = s.trim().to_ascii_uppercase().replace(['-', ' '], "_");
            CODES.iter().position(|c| *c == up).map(|i| i as u16).ok_or_else(|| format!("unknown gRPC status {s}"))
        }
        _ => Err("status must be a code name (NOT_FOUND) or number".into()),
    }
}

/// gRPC `grpc-message` percent-encoding.
fn encode_message(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if (0x20..0x7f).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(Debug)]
pub(crate) struct GrpcStub {
    pub id: String,
    pub name: Option<String>,
    pub file: String,
    /// `package.Service/Method`.
    pub method: String,
    priority: i64,
    body: Option<ValuePattern>,
    metadata: Vec<(String, MultiPattern)>,
    messages: Vec<Value>,
    stream_interval_ms: u64,
    delay_ms: u64,
    status: u16,
    status_message: String,
    headers: Vec<(String, String)>,
    trailers: Vec<(String, String)>,
    pub raw: Value,
}

fn pairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_object).map(|o| o.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect()).unwrap_or_default()
}

impl GrpcStub {
    fn from_json(v: &Value, location: &str, file: &str) -> Result<Self, String> {
        let o = v.as_object().ok_or("a gRPC stub must be an object")?;
        let method = match (o.get("method").and_then(Value::as_str), o.get("service").and_then(Value::as_str)) {
            (Some(m), Some(s)) if !m.contains('/') => format!("{s}/{m}"),
            (Some(m), _) => m.trim_start_matches('/').to_string(),
            _ => return Err("method is required (\"package.Service/Method\")".into()),
        };
        if !method.contains('/') {
            return Err(format!("method {method} must look like package.Service/Method"));
        }
        let req = o.get("request").cloned().unwrap_or(Value::Null);
        let body = match req.get("body") {
            None | Some(Value::Null) => None,
            Some(b @ (Value::Object(_) | Value::Array(_))) if !b.as_object().is_some_and(|m| m.keys().any(|k| ValuePattern::is_operator(k))) => {
                Some(ValuePattern::from_json(&json!({ "equalToJson": b, "ignoreExtraElements": true })).map_err(|e| format!("request.body: {e}"))?)
            }
            Some(p) => Some(ValuePattern::from_json(p).map_err(|e| format!("request.body: {e}"))?),
        };
        let metadata = match req.get("metadata").and_then(Value::as_object) {
            Some(m) => m.iter().map(|(k, p)| MultiPattern::from_json(p).map(|mp| (k.to_ascii_lowercase(), mp)).map_err(|e| format!("request.metadata.{k}: {e}"))).collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let resp = o.get("response").cloned().unwrap_or_else(|| json!({}));
        let messages = match (resp.get("messages"), resp.get("message")) {
            (Some(Value::Array(a)), _) => a.clone(),
            (Some(_), _) => return Err("response.messages must be an array".into()),
            (None, Some(m)) => vec![m.clone()],
            (None, None) => Vec::new(),
        };
        let status = resp.get("status").map(parse_code).transpose().map_err(|e| format!("response.{e}"))?.unwrap_or(0);
        if status == 0 && messages.is_empty() {
            return Err("response needs a message, messages or a non-OK status".into());
        }
        Ok(Self {
            id: o.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| stable_uuid(location).to_string()),
            name: o.get("name").and_then(Value::as_str).map(str::to_string),
            file: file.to_string(),
            method,
            priority: o.get("priority").and_then(Value::as_i64).unwrap_or(crate::stub::DEFAULT_PRIORITY),
            body,
            metadata,
            messages,
            stream_interval_ms: resp.get("streamIntervalMs").and_then(Value::as_u64).unwrap_or(0),
            delay_ms: resp.get("fixedDelayMilliseconds").and_then(Value::as_u64).unwrap_or(0),
            status,
            status_message: resp.get("statusMessage").and_then(Value::as_str).unwrap_or_default().to_string(),
            headers: pairs(resp.get("headers")),
            trailers: pairs(resp.get("trailers")),
            raw: v.clone(),
        })
    }

    fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.id.clone())
    }

    fn matches(&self, path: &str, body_json: &str, md: &HeaderMap) -> bool {
        if self.method != path.trim_start_matches('/') {
            return false;
        }
        if let Some(p) = &self.body
            && !p.eval_bytes(body_json.as_bytes(), false).matched
        {
            return false;
        }
        self.metadata.iter().all(|(k, p)| {
            let vals: Vec<&str> = md.get_all(k.as_str()).iter().filter_map(|v| v.to_str().ok()).collect();
            p.eval(&vals, false).matched
        })
    }
}

struct Compiled {
    pool: DescriptorPool,
    services: Vec<String>,
    reflection_v1: Reflection,
    reflection_v1alpha: Reflection,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcMethodInfo {
    pub path: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    pub input_type: String,
    pub output_type: String,
    /// Example request JSON with every field.
    pub template: String,
}

pub(crate) struct Listener {
    pub addr: SocketAddr,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

#[derive(Default)]
pub(crate) struct GrpcState {
    compiled: RwLock<Option<Arc<Compiled>>>,
    stubs: RwLock<Vec<Arc<GrpcStub>>>,
    pub errors: RwLock<Vec<LoadError>>,
    pub listener: Mutex<Option<Listener>>,
    calls: AtomicU64,
}

fn read<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}

fn proto_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            proto_files(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("proto")) {
            out.push(p);
        }
    }
}

fn reflection(fds: prost_types::FileDescriptorSet) -> Result<(Reflection, Reflection), String> {
    let v1 = tonic_reflection::server::Builder::configure().register_file_descriptor_set(fds.clone()).build_v1().map_err(|e| e.to_string())?;
    let v1alpha = tonic_reflection::server::Builder::configure().register_file_descriptor_set(fds).build_v1alpha().map_err(|e| e.to_string())?;
    Ok((BoxCloneSyncService::new(v1), BoxCloneSyncService::new(v1alpha)))
}

impl GrpcState {
    /// Compiles `mocks/grpc/**/*.proto` and loads the stubs.
    pub fn reload(&self, root: &std::path::Path) -> Vec<LoadError> {
        let dir = root.join(MOCKS_DIR).join(GRPC_DIR);
        let mut errors = Vec::new();
        let mut files = Vec::new();
        proto_files(&dir, &mut files);
        files.sort();
        let compiled = if files.is_empty() {
            None
        } else {
            match devall_proto::grpc::compile_protos(&files, std::slice::from_ref(&dir)) {
                Ok(pool) => {
                    let fds = prost_types::FileDescriptorSet { file: pool.file_descriptor_protos().cloned().collect() };
                    match reflection(fds) {
                        Ok((v1, v1alpha)) => {
                            let services = pool.services().map(|s| s.full_name().to_string()).collect();
                            Some(Arc::new(Compiled { pool, services, reflection_v1: v1, reflection_v1alpha: v1alpha }))
                        }
                        Err(e) => {
                            errors.push(LoadError { file: format!("{MOCKS_DIR}/{GRPC_DIR}"), error: format!("reflection: {e}") });
                            None
                        }
                    }
                }
                Err(e) => {
                    errors.push(LoadError { file: format!("{MOCKS_DIR}/{GRPC_DIR}"), error: e.to_string() });
                    None
                }
            }
        };
        let (defs, errs) = load_json_defs(root, GRPC_DIR, "stubs", &[]);
        errors.extend(errs);
        let mut stubs = Vec::new();
        for d in defs {
            match GrpcStub::from_json(&d.value, &d.location(), &d.file) {
                Ok(s) => {
                    if let Some(c) = &compiled
                        && find_method(&c.pool, &s.method).is_none()
                    {
                        errors.push(LoadError { file: d.file.clone(), error: format!("method {} is not in the .proto files", s.method) });
                    }
                    stubs.push(Arc::new(s));
                }
                Err(e) => errors.push(LoadError { file: d.file.clone(), error: if let Some(i) = d.index { format!("stub #{}: {e}", i + 1) } else { e } }),
            }
        }
        stubs.sort_by_key(|s| s.priority);
        *self.compiled.write().unwrap_or_else(|e| e.into_inner()) = compiled;
        *self.stubs.write().unwrap_or_else(|e| e.into_inner()) = stubs;
        *self.errors.write().unwrap_or_else(|e| e.into_inner()) = errors.clone();
        errors
    }

    pub fn has_protos(&self) -> bool {
        read(&self.compiled).is_some()
    }

    pub fn port(&self) -> Option<u16> {
        self.listener.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|l| l.addr.port())
    }

    pub fn methods(&self) -> Vec<GrpcMethodInfo> {
        let Some(c) = read(&self.compiled).clone() else { return Vec::new() };
        c.services
            .iter()
            .filter_map(|s| c.pool.get_service_by_name(s))
            .flat_map(|s| s.methods().collect::<Vec<_>>())
            .map(|m| {
                let info = devall_proto::grpc::method_info(&m);
                GrpcMethodInfo {
                    path: info.path,
                    client_streaming: info.client_streaming,
                    server_streaming: info.server_streaming,
                    input_type: info.input_type,
                    output_type: info.output_type,
                    template: info.template,
                }
            })
            .collect()
    }

    pub fn stubs_json(&self) -> Vec<Value> {
        read(&self.stubs).iter().map(|s| json!({ "id": s.id, "name": s.name, "file": s.file, "method": s.method, "definition": s.raw })).collect()
    }
}

fn find_method(pool: &DescriptorPool, path: &str) -> Option<MethodDescriptor> {
    let (svc, m) = path.trim_start_matches('/').split_once('/')?;
    pool.get_service_by_name(svc)?.methods().find(|x| x.name() == m)
}

/// Starts the listener (`port` 0 = any free port). Restarts it on a different port.
pub(crate) async fn start(st: &Arc<ServerState>, port: u16) -> Result<u16, MockError> {
    if let Some(p) = st.grpc.port()
        && (port == 0 || port == p)
    {
        return Ok(p);
    }
    stop(st).await;
    let ip = if st.config.bind_all { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let addr = SocketAddr::new(ip, port);
    let listener = TcpListener::bind(addr).await.map_err(|source| MockError::Bind { addr: addr.to_string(), source })?;
    let local = listener.local_addr().map_err(|e| MockError::Invalid(e.to_string()))?;
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(serve(listener, st.clone(), rx));
    *st.grpc.listener.lock().unwrap_or_else(|e| e.into_inner()) = Some(Listener { addr: local, shutdown: tx, task });
    st.messages.record(Protocol::Grpc, "server", Direction::Event, "", String::new(), false, None, json!({ "event": "listening", "port": local.port() }));
    Ok(local.port())
}

pub(crate) async fn stop(st: &ServerState) {
    let l = st.grpc.listener.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(l) = l {
        let _ = l.shutdown.send(true);
        let _ = l.task.await;
    }
}

async fn serve(listener: TcpListener, st: Arc<ServerState>, mut shutdown: watch::Receiver<bool>) {
    let mut conns = JoinSet::new();
    loop {
        tokio::select! {
            res = listener.accept() => match res {
                Ok((stream, _)) => {
                    let _ = stream.set_nodelay(true);
                    let st = st.clone();
                    let mut sd = shutdown.clone();
                    conns.spawn(async move {
                        let svc = hyper::service::service_fn(move |req| {
                            let st = st.clone();
                            async move { Ok::<_, Infallible>(handle(st, req).await) }
                        });
                        let conn = hyper::server::conn::http2::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(stream), svc);
                        tokio::pin!(conn);
                        tokio::select! {
                            _ = conn.as_mut() => {}
                            _ = sd.changed() => {
                                conn.as_mut().graceful_shutdown();
                                let _ = tokio::time::timeout(Duration::from_secs(2), conn.as_mut()).await;
                            }
                        }
                    });
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            },
            _ = shutdown.changed() => break,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
    drop(listener);
    let drain = async { while conns.join_next().await.is_some() {} };
    if tokio::time::timeout(Duration::from_secs(3), drain).await.is_err() {
        conns.abort_all();
    }
}

fn trailers_only(code: u16, message: &str) -> Response<GrpcBody> {
    let mut r = Response::new(Full::new(Bytes::new()).map_err(|e: Infallible| -> BoxError { match e {} }).boxed_unsync());
    let h = r.headers_mut();
    h.insert("content-type", HeaderValue::from_static("application/grpc"));
    h.insert("grpc-status", HeaderValue::from(code));
    if let Ok(v) = HeaderValue::from_str(&encode_message(message)) {
        h.insert("grpc-message", v);
    }
    r
}

/// Length-prefixed messages of a request body.
fn split_frames(body: &[u8]) -> Result<Vec<&[u8]>, (u16, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body.len() - i < 5 {
            return Err((13, "truncated gRPC message".into()));
        }
        if body[i] != 0 {
            return Err((12, "compressed messages are not supported by the mock (disable grpc-encoding)".into()));
        }
        let len = u32::from_be_bytes([body[i + 1], body[i + 2], body[i + 3], body[i + 4]]) as usize;
        let start = i + 5;
        let end = start.checked_add(len).filter(|e| *e <= body.len()).ok_or((13, "truncated gRPC message".to_string()))?;
        out.push(&body[start..end]);
        i = end;
    }
    Ok(out)
}

fn frame(msg: &DynamicMessage) -> Bytes {
    let payload = msg.encode_to_vec();
    let mut b = BytesMut::with_capacity(payload.len() + 5);
    b.put_u8(0);
    b.put_u32(payload.len() as u32);
    b.put_slice(&payload);
    b.freeze()
}

async fn handle(st: Arc<ServerState>, req: Request<Incoming>) -> Response<GrpcBody> {
    let path = req.uri().path().to_string();
    let compiled = read(&st.grpc.compiled).clone();
    if let Some(c) = &compiled {
        let svc = if path.starts_with("/grpc.reflection.v1.ServerReflection/") {
            Some(c.reflection_v1.clone())
        } else if path.starts_with("/grpc.reflection.v1alpha.ServerReflection/") {
            Some(c.reflection_v1alpha.clone())
        } else {
            None
        };
        if let Some(mut svc) = svc {
            return match svc.ready().await {
                Ok(s) => match s.call(req).await {
                    Ok(resp) => resp.map(|b| b.map_err(|e| -> BoxError { Box::new(e) }).boxed_unsync()),
                    Err(e) => match e {},
                },
                Err(e) => match e {},
            };
        }
    }
    let call_id = format!("call-{}", st.grpc.calls.fetch_add(1, Ordering::SeqCst) + 1);
    let channel = path.clone();
    let journal = |dir: Direction, body: String, stub: Option<&str>, meta: Value| st.messages.record(Protocol::Grpc, &call_id, dir, &channel, body, false, stub, meta);
    let finish = |code: u16, message: &str, stub: Option<&str>| {
        journal(Direction::Event, String::new(), stub, json!({ "event": "status", "status": code, "statusName": CODES.get(code as usize).copied().unwrap_or("UNKNOWN"), "message": message }));
    };
    let Some(c) = compiled else {
        finish(12, "no .proto files in mocks/grpc", None);
        return trailers_only(12, "the mock has no .proto files (add them to mocks/grpc/)");
    };
    let Some(method) = find_method(&c.pool, &path) else {
        let msg = format!("unknown method {path}");
        finish(12, &msg, None);
        return trailers_only(12, &msg);
    };
    let (parts, body) = req.into_parts();
    let bytes = match Limited::new(body, MAX_REQUEST).collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            finish(8, "request too large", None);
            return trailers_only(8, &format!("request: {e}"));
        }
    };
    let frames = match split_frames(&bytes) {
        Ok(f) => f,
        Err((code, msg)) => {
            finish(code, &msg, None);
            return trailers_only(code, &msg);
        }
    };
    let input = method.input();
    let mut decoded = Vec::new();
    for f in &frames {
        match DynamicMessage::decode(input.clone(), *f) {
            Ok(m) => decoded.push(devall_proto::grpc::message_to_json(&m)),
            Err(e) => {
                let msg = format!("decoding {}: {e}", input.full_name());
                finish(13, &msg, None);
                return trailers_only(13, &msg);
            }
        }
    }
    let streaming_in = method.is_client_streaming();
    let body_json = if streaming_in { format!("[{}]", decoded.join(",")) } else { decoded.first().cloned().unwrap_or_else(|| "{}".into()) };
    for d in &decoded {
        journal(Direction::In, d.clone(), None, Value::Null);
    }
    let stub = read(&st.grpc.stubs).iter().find(|s| s.matches(&path, &body_json, &parts.headers)).cloned();
    let Some(stub) = stub else {
        let msg = format!("no stub matched {path} (add one in mocks/grpc/)");
        finish(12, &msg, None);
        return trailers_only(12, &msg);
    };
    let label = stub.label();
    // Template model: the request message as `request.body`, metadata as `request.headers`.
    let headers: Vec<(String, String)> = parts.headers.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let mreq = MockRequest::new("POST", &path, headers, body_json.clone());
    let mut model = template::model(&mreq, &[], &Value::Null);
    if streaming_in && let Value::Object(o) = &mut model {
        o.insert("messages".into(), serde_json::from_str(&body_json).unwrap_or(Value::Null));
    }
    let output = method.output();
    let mut out_msgs = Vec::new();
    for (i, m) in stub.messages.iter().enumerate() {
        let text = m.to_string();
        let rendered = if text.contains("{{") { st.templates.render(&text, &model).unwrap_or_else(|e| e) } else { text };
        match devall_proto::grpc::message_from_json(&output, &rendered) {
            Ok(dm) => out_msgs.push((dm, rendered)),
            Err(e) => {
                let msg = format!("stub {label} response message #{}: {e}", i + 1);
                finish(13, &msg, Some(&label));
                return trailers_only(13, &msg);
            }
        }
    }
    if !method.is_server_streaming() && out_msgs.len() > 1 {
        out_msgs.truncate(1);
    }
    if stub.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(stub.delay_ms)).await;
    }
    let status_msg = st.templates.render(&stub.status_message, &model).unwrap_or_else(|e| e);
    if out_msgs.is_empty() {
        finish(stub.status, &status_msg, Some(&label));
        let mut r = trailers_only(stub.status, &status_msg);
        add_metadata(r.headers_mut(), &stub.headers);
        add_metadata(r.headers_mut(), &stub.trailers);
        return r;
    }
    // Messages (optionally spaced out), then trailers.
    let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, BoxError>>(8);
    let interval = stub.stream_interval_ms;
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from(stub.status));
    if !status_msg.is_empty()
        && let Ok(v) = HeaderValue::from_str(&encode_message(&status_msg))
    {
        trailers.insert("grpc-message", v);
    }
    add_metadata(&mut trailers, &stub.trailers);
    let st2 = st.clone();
    let (cid, ch, lbl, code) = (call_id.clone(), channel.clone(), label.clone(), stub.status);
    tokio::spawn(async move {
        for (i, (m, text)) in out_msgs.iter().enumerate() {
            if i > 0 && interval > 0 {
                tokio::time::sleep(Duration::from_millis(interval)).await;
            }
            if tx.send(Ok(Frame::data(frame(m)))).await.is_err() {
                return;
            }
            st2.messages.record(Protocol::Grpc, &cid, Direction::Out, &ch, text.clone(), false, Some(&lbl), Value::Null);
        }
        let _ = tx.send(Ok(Frame::trailers(trailers))).await;
        st2.messages.record(
            Protocol::Grpc,
            &cid,
            Direction::Event,
            &ch,
            String::new(),
            false,
            Some(&lbl),
            json!({ "event": "status", "status": code, "statusName": CODES.get(code as usize).copied().unwrap_or("UNKNOWN") }),
        );
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|f| (f, rx)) });
    let mut r = Response::new(StreamBody::new(stream).boxed_unsync());
    r.headers_mut().insert("content-type", HeaderValue::from_static("application/grpc"));
    add_metadata(r.headers_mut(), &stub.headers);
    r
}

fn add_metadata(h: &mut HeaderMap, pairs: &[(String, String)]) {
    for (k, v) in pairs {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            h.append(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_frames() {
        assert_eq!(parse_code(&json!("not_found")).unwrap(), 5);
        assert_eq!(parse_code(&json!("UNAUTHENTICATED")).unwrap(), 16);
        assert_eq!(parse_code(&json!(3)).unwrap(), 3);
        assert!(parse_code(&json!("NOPE")).is_err());
        assert_eq!(encode_message("héllo 100%"), "h%C3%A9llo 100%25");
        let body = [0u8, 0, 0, 0, 2, 8, 1, 0, 0, 0, 0, 0];
        let frames = split_frames(&body).unwrap();
        assert_eq!(frames, vec![&[8u8, 1][..], &[][..]]);
        assert_eq!(split_frames(&[1, 0, 0, 0, 0]).unwrap_err().0, 12);
        assert_eq!(split_frames(&[0, 0, 0, 0, 9, 1]).unwrap_err().0, 13);
        let s = GrpcStub::from_json(&json!({ "service": "a.B", "method": "C", "response": { "status": "NOT_FOUND" } }), "x", "f").unwrap();
        assert_eq!((s.method.as_str(), s.status), ("a.B/C", 5));
        assert!(GrpcStub::from_json(&json!({ "method": "a.B/C", "response": {} }), "x", "f").is_err());
    }
}
