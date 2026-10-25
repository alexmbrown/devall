//! Dynamic gRPC client: descriptors come from server reflection or from `.proto` files compiled
//! at runtime with `protox` (no `protoc` binary); messages are `prost_reflect::DynamicMessage`s
//! converted from/to JSON. Unary, server-streaming, client-streaming and bidi calls all go through
//! one streaming code path; every message in either direction is reported as an event.

pub mod codec;
mod reflection;
pub mod template;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use devall_core::Resolver;
use hyper_util::rt::TokioIo;
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor, SerializeOptions};
use schemars::JsonSchema;
use serde::Serialize;
use tokio::sync::{Notify, mpsc};
use tonic::metadata::{AsciiMetadataValue, BinaryMetadataValue, MetadataKey, MetadataMap};
use tonic::transport::{Channel, Endpoint};
use tonic::Status;

use crate::error::{ProtoError, Result, chain};
use crate::event::{Direction, EventSink, ProtoEvent};
use crate::model::{GrpcFile, resolve_pairs};
use crate::net::{self, timeout_ms};
use crate::now_ms;

pub use codec::DynamicCodec;
pub use template::{message_template, message_template_text};

const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const REFLECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a server is and how to talk to it.
#[derive(Debug, Clone, PartialEq)]
pub struct GrpcTarget {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub verify_tls: bool,
    pub connect_timeout: Duration,
}

impl GrpcTarget {
    /// `grpc://` / `http://` / bare `host:port` = plaintext; `grpcs://` / `https://` = TLS.
    pub fn parse(raw: &str, verify_tls: bool, connect_timeout: Duration) -> Result<Self> {
        let raw = raw.trim().trim_end_matches('/');
        if raw.is_empty() {
            return Err(ProtoError::InvalidUrl("server URL is empty".into()));
        }
        let (tls, rest) = match raw.split_once("://") {
            Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
                "grpc" | "http" | "h2c" => (false, rest),
                "grpcs" | "https" => (true, rest),
                other => return Err(ProtoError::InvalidUrl(format!("{raw}: unsupported scheme {other}:// (use grpc://, grpcs://, http:// or https://)"))),
            },
            None => (false, raw),
        };
        let url = url::Url::parse(&format!("http://{rest}")).map_err(|e| ProtoError::InvalidUrl(format!("{raw} ({e})")))?;
        let host = net::host_of(&url)?;
        let port = url.port().unwrap_or(if tls { 443 } else { 80 });
        Ok(Self { host, port, tls, verify_tls, connect_timeout })
    }

    /// `scheme://authority` used as the `:scheme`/`:authority` of requests.
    pub fn origin(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        format!("{}://{host}:{}", if self.tls { "https" } else { "http" }, self.port)
    }
}

/// Where descriptors come from.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaSource {
    Reflection,
    ProtoFiles { files: Vec<PathBuf>, includes: Vec<PathBuf> },
}

/// Concrete call settings (every template resolved).
#[derive(Debug, Clone, PartialEq)]
pub struct GrpcOptions {
    pub target: GrpcTarget,
    pub service: String,
    pub method: String,
    /// Request JSON (an array = several messages for client-streaming methods).
    pub message: String,
    pub metadata: Vec<(String, String)>,
    pub deadline: Option<Duration>,
    pub source: SchemaSource,
}

impl GrpcOptions {
    /// Resolves `file`; relative `.proto` paths and import paths are relative to `project_root`,
    /// `repo:<name>/<path>` ones to one of the project's repositories (protos usually live with the
    /// backend's code).
    pub fn resolve(file: &GrpcFile, r: &mut Resolver, project_root: &Path) -> Result<Self> {
        let path = |p: &str, r: &mut Resolver| {
            let p = r.resolve(p.trim());
            if devall_core::repos::parse_link(&p).is_some()
                && let Ok(abs) = devall_core::repos::resolve_link_at(project_root, &p)
            {
                return abs;
            }
            let pb = PathBuf::from(&p);
            if pb.is_absolute() { pb } else { project_root.join(pb) }
        };
        let source = if file.use_reflection {
            SchemaSource::Reflection
        } else {
            let files: Vec<PathBuf> = file.proto_files.iter().filter(|p| !p.trim().is_empty()).map(|p| path(p, r)).collect();
            let mut includes: Vec<PathBuf> = file.import_paths.iter().filter(|p| !p.trim().is_empty()).map(|p| path(p, r)).collect();
            includes.push(project_root.to_path_buf());
            SchemaSource::ProtoFiles { files, includes }
        };
        Ok(Self {
            target: GrpcTarget::parse(&r.resolve(&file.url), !file.insecure, timeout_ms(crate::model::DEFAULT_CONNECT_TIMEOUT_MS))?,
            service: r.resolve(file.service.trim()),
            method: r.resolve(file.method.trim()),
            message: r.resolve(&file.message),
            metadata: resolve_pairs(&file.metadata, r),
            deadline: (file.deadline_ms > 0).then(|| Duration::from_millis(file.deadline_ms)),
            source,
        })
    }
}

/// Opens an HTTP/2 channel; TLS (ALPN h2) is done by our own connector so "insecure" works.
pub async fn connect(target: &GrpcTarget) -> Result<Channel> {
    let tls = if target.tls { Some(net::tls_config(target.verify_tls, &[b"h2"])?) } else { None };
    let (host, port, timeout) = (target.host.clone(), target.port, target.connect_timeout);
    let connector = tower::service_fn(move |_uri: http::Uri| {
        let (host, tls) = (host.clone(), tls.clone());
        async move { net::connect(&host, port, tls, timeout).await.map(TokioIo::new).map_err(|e| std::io::Error::other(e.to_string())) }
    });
    let origin = http::Uri::from_str(&target.origin()).map_err(|e| ProtoError::InvalidUrl(e.to_string()))?;
    // The endpoint URI is always http:// so tonic never tries its own TLS; `origin` sets the real scheme.
    let plain = format!("http://{}", target.origin().split_once("://").map(|(_, a)| a).unwrap_or_default());
    let endpoint = Endpoint::from_shared(plain).map_err(|e| ProtoError::InvalidUrl(chain(&e)))?.origin(origin);
    endpoint.connect_with_connector(connector).await.map_err(|e| {
        let msg = chain(&e);
        if msg.contains("timed out") { ProtoError::Timeout(msg) } else { ProtoError::Connect(msg) }
    })
}

/// Compiles `.proto` files (imports resolved against `includes`, the files' directories and
/// Google's bundled well-known types).
pub fn compile_protos(files: &[PathBuf], includes: &[PathBuf]) -> Result<DescriptorPool> {
    if files.is_empty() {
        return Err(ProtoError::Descriptor("no .proto files configured (add some, or turn on server reflection)".into()));
    }
    let mut incs: Vec<PathBuf> = includes.iter().filter(|p| p.is_dir()).cloned().collect();
    for f in files {
        if !f.is_file() {
            return Err(ProtoError::Descriptor(format!("{} not found", f.display())));
        }
        if let Some(dir) = f.parent()
            && !incs.iter().any(|i| i == dir)
        {
            // The file's own directory first, like `protoc -I<dir>`.
            incs.insert(0, dir.to_path_buf());
        }
    }
    let mut c = protox::Compiler::new(&incs).map_err(|e| ProtoError::Descriptor(e.to_string()))?;
    c.include_imports(true);
    c.open_files(files).map_err(|e| ProtoError::Descriptor(e.to_string()))?;
    Ok(c.descriptor_pool())
}

/// A descriptor pool plus what the server/files offer.
#[derive(Debug, Clone)]
pub struct Descriptors {
    pub pool: DescriptorPool,
    /// Callable services (reflection services excluded).
    pub services: Vec<String>,
    pub warnings: Vec<String>,
    /// `reflection` or `protoFiles`.
    pub source: &'static str,
}

/// Loads descriptors; `channel` is required for reflection.
pub async fn load_descriptors(opts: &GrpcOptions, channel: Option<&Channel>) -> Result<Descriptors> {
    match &opts.source {
        SchemaSource::ProtoFiles { files, includes } => {
            let pool = compile_protos(files, includes)?;
            let services = pool.services().map(|s| s.full_name().to_string()).collect();
            Ok(Descriptors { pool, services, warnings: Vec::new(), source: "protoFiles" })
        }
        SchemaSource::Reflection => {
            let owned;
            let channel = match channel {
                Some(c) => c,
                None => {
                    owned = connect(&opts.target).await?;
                    &owned
                }
            };
            let md = metadata_map(&opts.metadata)?;
            let r = tokio::time::timeout(REFLECTION_TIMEOUT, reflection::fetch(channel.clone(), md))
                .await
                .map_err(|_| ProtoError::Timeout("server reflection".into()))?
                .map_err(|s| ProtoError::Descriptor(format!("server reflection failed: {}", status_text(&s))))?;
            let mut pool = DescriptorPool::new();
            pool.add_file_descriptor_protos(r.files).map_err(|e| ProtoError::Descriptor(e.to_string()))?;
            let services = r.services.into_iter().filter(|s| !s.starts_with("grpc.reflection.") && pool.get_service_by_name(s).is_some()).collect();
            Ok(Descriptors { pool, services, warnings: r.warnings, source: "reflection" })
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MethodInfo {
    pub name: String,
    /// `/package.Service/Method`
    pub path: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    pub input_type: String,
    pub output_type: String,
    /// Example request JSON with every field.
    pub template: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub name: String,
    pub methods: Vec<MethodInfo>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrpcDescription {
    /// `reflection` or `protoFiles`.
    pub source: String,
    pub services: Vec<ServiceInfo>,
    pub warnings: Vec<String>,
}

pub fn method_info(m: &MethodDescriptor) -> MethodInfo {
    MethodInfo {
        name: m.name().to_string(),
        path: format!("/{}/{}", m.parent_service().full_name(), m.name()),
        client_streaming: m.is_client_streaming(),
        server_streaming: m.is_server_streaming(),
        input_type: m.input().full_name().to_string(),
        output_type: m.output().full_name().to_string(),
        template: message_template_text(&m.input()),
    }
}

impl Descriptors {
    pub fn describe(&self) -> GrpcDescription {
        let services = self
            .services
            .iter()
            .filter_map(|n| self.pool.get_service_by_name(n))
            .map(|s| ServiceInfo { name: s.full_name().to_string(), methods: s.methods().map(|m| method_info(&m)).collect() })
            .collect();
        GrpcDescription { source: self.source.into(), services, warnings: self.warnings.clone() }
    }

    pub fn method(&self, service: &str, method: &str) -> Result<MethodDescriptor> {
        if service.is_empty() || method.is_empty() {
            return Err(ProtoError::Invalid("pick a service and method first".into()));
        }
        let svc = self.pool.get_service_by_name(service).ok_or_else(|| ProtoError::Invalid(format!("unknown service {service}")))?;
        svc.methods().find(|m| m.name() == method).ok_or_else(|| ProtoError::Invalid(format!("{service} has no method {method}")))
    }
}

/// Lists services and methods with request templates.
pub async fn describe(opts: &GrpcOptions) -> Result<GrpcDescription> {
    Ok(load_descriptors(opts, None).await?.describe())
}

/// Parses proto3 JSON into a message; empty text = empty message.
pub fn message_from_json(desc: &MessageDescriptor, json: &str) -> Result<DynamicMessage> {
    if json.trim().is_empty() {
        return Ok(DynamicMessage::new(desc.clone()));
    }
    let mut de = serde_json::Deserializer::from_str(json);
    let m = DynamicMessage::deserialize(desc.clone(), &mut de).map_err(|e| ProtoError::Invalid(format!("{}: {e}", desc.full_name())))?;
    de.end().map_err(|e| ProtoError::Invalid(format!("{}: {e}", desc.full_name())))?;
    Ok(m)
}

/// Compact proto3 JSON with default values included (like `grpcurl -emit-defaults`).
pub fn message_to_json(m: &DynamicMessage) -> String {
    let mut ser = serde_json::Serializer::new(Vec::new());
    let opts = SerializeOptions::new().skip_default_fields(false);
    match m.serialize_with_options(&mut ser, &opts) {
        Ok(()) => String::from_utf8(ser.into_inner()).unwrap_or_default(),
        Err(e) => format!("{{\"error\":{:?}}}", e.to_string()),
    }
}

/// Splits request JSON into messages: arrays are spread for client-streaming methods.
fn initial_messages(method: &MethodDescriptor, text: &str) -> Result<Vec<DynamicMessage>> {
    let input = method.input();
    if method.is_client_streaming() {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| ProtoError::Invalid(format!("request is not valid JSON: {e}")))?;
        return match v {
            serde_json::Value::Array(items) => items.iter().map(|i| message_from_json(&input, &i.to_string())).collect(),
            other => Ok(vec![message_from_json(&input, &other.to_string())?]),
        };
    }
    Ok(vec![message_from_json(&input, text)?])
}

/// Metadata pairs → tonic metadata. `-bin` keys take base64 values (raw text is used as-is when
/// it isn't base64).
pub fn metadata_map(pairs: &[(String, String)]) -> Result<MetadataMap> {
    let mut map = MetadataMap::new();
    for (k, v) in pairs {
        let k = k.trim().to_ascii_lowercase();
        if k.ends_with("-bin") {
            let key = MetadataKey::from_bytes(k.as_bytes()).map_err(|_| ProtoError::Invalid(format!("invalid metadata key {k}")))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(v.trim()).unwrap_or_else(|_| v.as_bytes().to_vec());
            map.append_bin(key, BinaryMetadataValue::from_bytes(&bytes));
        } else {
            let key = MetadataKey::from_bytes(k.as_bytes()).map_err(|_| ProtoError::Invalid(format!("invalid metadata key {k}")))?;
            let value = AsciiMetadataValue::try_from(v.as_str()).map_err(|_| ProtoError::Invalid(format!("metadata {k}: value must be ASCII (use a -bin key for binary)")))?;
            map.append(key, value);
        }
    }
    Ok(map)
}

fn entries(map: &MetadataMap) -> Vec<(String, String)> {
    map.clone().into_headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect()
}

/// Canonical status code names (`NOT_FOUND`, …).
pub fn code_name(code: i32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN",
    }
}

fn status_text(s: &Status) -> String {
    let code = s.code() as i32;
    if s.message().is_empty() { code_name(code).to_string() } else { format!("{} — {}", code_name(code), s.message()) }
}

/// `google.rpc.Status` (the `grpc-status-details-bin` trailer).
#[derive(Clone, PartialEq, prost::Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
    #[prost(message, repeated, tag = "3")]
    details: Vec<prost_types::Any>,
}

/// Rich error details as JSON; `Any` payloads are decoded when their type is in `pool`.
fn decode_details(bytes: &[u8], pool: &DescriptorPool) -> Option<serde_json::Value> {
    if bytes.is_empty() {
        return None;
    }
    let Ok(st) = RpcStatus::decode(bytes) else {
        return Some(serde_json::json!({ "raw": base64::engine::general_purpose::STANDARD.encode(bytes) }));
    };
    let details: Vec<serde_json::Value> = st
        .details
        .iter()
        .map(|any| {
            let type_name = any.type_url.rsplit('/').next().unwrap_or_default();
            match pool.get_message_by_name(type_name).and_then(|d| DynamicMessage::decode(d, any.value.as_slice()).ok()) {
                Some(m) => {
                    let mut v: serde_json::Value = serde_json::from_str(&message_to_json(&m)).unwrap_or_default();
                    if let Some(o) = v.as_object_mut() {
                        o.insert("@type".into(), any.type_url.clone().into());
                    }
                    v
                }
                None => serde_json::json!({ "@type": any.type_url, "value": base64::engine::general_purpose::STANDARD.encode(&any.value) }),
            }
        })
        .collect();
    Some(serde_json::json!({ "code": st.code, "message": st.message, "details": details }))
}

/// The outcome of a call.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrpcStatus {
    pub code: i32,
    pub code_name: String,
    pub message: String,
    /// Decoded `google.rpc.Status` details, when the server sent any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub headers: Vec<(String, String)>,
    pub trailers: Vec<(String, String)>,
    pub messages_sent: usize,
    pub messages_received: usize,
    pub duration_ms: f64,
}

impl GrpcStatus {
    pub fn is_ok(&self) -> bool {
        self.code == 0
    }
}

/// Controls a running call: send more messages (client/bidi streaming), half-close, cancel.
#[derive(Debug)]
pub struct GrpcCallHandle {
    tx: Mutex<Option<mpsc::UnboundedSender<DynamicMessage>>>,
    input: MessageDescriptor,
    label: String,
    client_streaming: bool,
    sent: Arc<AtomicUsize>,
    cancel: Arc<Notify>,
    sink: EventSink,
}

impl GrpcCallHandle {
    /// Whether more messages can be sent.
    pub fn is_open(&self) -> bool {
        self.tx.lock().unwrap().as_ref().is_some_and(|t| !t.is_closed())
    }

    pub fn client_streaming(&self) -> bool {
        self.client_streaming
    }

    /// Sends one more request message (JSON).
    pub fn send_json(&self, json: &str) -> Result<()> {
        let m = message_from_json(&self.input, json)?;
        self.send(m)
    }

    fn send(&self, m: DynamicMessage) -> Result<()> {
        let guard = self.tx.lock().unwrap();
        let tx = guard.as_ref().ok_or_else(|| ProtoError::Invalid("the request stream is already closed".into()))?;
        let body = message_to_json(&m);
        let size = m.encoded_len();
        tx.send(m).map_err(|_| ProtoError::Closed)?;
        self.sent.fetch_add(1, Ordering::SeqCst);
        self.sink.emit(ProtoEvent::Message { at: now_ms(), direction: Direction::Out, label: Some(self.label.clone()), body, binary: false, size, meta: None });
        Ok(())
    }

    /// Half-closes the request stream (the server then finishes the call).
    pub fn end(&self) {
        if self.tx.lock().unwrap().take().is_some() && self.client_streaming {
            self.sink.info("request stream closed");
        }
    }

    /// Cancels the call (RST_STREAM); it ends with `CANCELLED`.
    pub fn cancel(&self) {
        self.cancel.notify_one();
    }
}

pub type CallFuture = Pin<Box<dyn std::future::Future<Output = GrpcStatus> + Send>>;

/// Connects, loads descriptors and starts the call. Returns a handle and a future that drives
/// the call to completion. With `keep_open`, client-streaming calls keep the request stream open
/// for [`GrpcCallHandle::send_json`] until [`GrpcCallHandle::end`].
pub async fn start_call(opts: &GrpcOptions, sink: EventSink, keep_open: bool) -> Result<(Arc<GrpcCallHandle>, CallFuture)> {
    sink.emit(ProtoEvent::Connecting { at: now_ms(), target: opts.target.origin() });
    let channel = connect(&opts.target).await?;
    let d = load_descriptors(opts, Some(&channel)).await?;
    for w in &d.warnings {
        sink.info(w.clone());
    }
    let method = d.method(&opts.service, &opts.method)?;
    let initial = initial_messages(&method, &opts.message)?;
    let metadata = metadata_map(&opts.metadata)?;

    let (tx, rx) = mpsc::unbounded_channel::<DynamicMessage>();
    let label = format!("{}/{}", method.parent_service().name(), method.name());
    let handle = Arc::new(GrpcCallHandle {
        tx: Mutex::new(Some(tx)),
        input: method.input(),
        label: label.clone(),
        client_streaming: method.is_client_streaming(),
        sent: Arc::new(AtomicUsize::new(0)),
        cancel: Arc::new(Notify::new()),
        sink: sink.clone(),
    });
    let kind = match (method.is_client_streaming(), method.is_server_streaming()) {
        (false, false) => "unary",
        (false, true) => "server streaming",
        (true, false) => "client streaming",
        (true, true) => "bidirectional streaming",
    };
    sink.emit(ProtoEvent::Opened { at: now_ms(), detail: Some(format!("{} · {kind} · descriptors from {}", method.full_name(), d.source)), headers: Vec::new() });
    for m in initial {
        handle.send(m)?;
    }
    if !(keep_open && method.is_client_streaming()) {
        handle.tx.lock().unwrap().take();
    }

    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|m| (m, rx)) });
    let mut request = tonic::Request::new(stream);
    *request.metadata_mut() = metadata;
    if let Some(d) = opts.deadline {
        request.set_timeout(d);
    }
    let path = http::uri::PathAndQuery::from_str(&format!("/{}/{}", method.parent_service().full_name(), method.name()))
        .map_err(|e| ProtoError::Invalid(format!("invalid method path: {e}")))?;
    let output = method.output();
    let deadline = opts.deadline;
    let pool = d.pool.clone();
    let (h, label2) = (handle.clone(), label);

    let fut = async move {
        let started = Instant::now();
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut trailers: Vec<(String, String)> = Vec::new();
        let mut received = 0usize;
        let call = async {
            let mut grpc = tonic::client::Grpc::new(channel).max_decoding_message_size(MAX_MESSAGE_BYTES).max_encoding_message_size(MAX_MESSAGE_BYTES);
            grpc.ready().await.map_err(|e| Status::unavailable(chain(&e)))?;
            let resp = grpc.streaming(request, path, DynamicCodec::new(output)).await?;
            let (meta, mut body, _) = resp.into_parts();
            headers = entries(&meta);
            sink.emit(ProtoEvent::Metadata { at: now_ms(), trailers: false, entries: headers.clone() });
            while let Some(m) = body.message().await? {
                received += 1;
                sink.emit(ProtoEvent::Message {
                    at: now_ms(),
                    direction: Direction::In,
                    label: Some(label2.clone()),
                    body: message_to_json(&m),
                    binary: false,
                    size: m.encoded_len(),
                    meta: None,
                });
            }
            if let Some(t) = body.trailers().await? {
                trailers = entries(&t);
            }
            Ok::<(), Status>(())
        };
        let result = tokio::select! {
            r = async {
                match deadline {
                    Some(d) => tokio::time::timeout(d, call).await.unwrap_or_else(|_| Err(Status::deadline_exceeded("deadline exceeded (client side)"))),
                    None => call.await,
                }
            } => r,
            _ = h.cancel.notified() => Err(Status::cancelled("cancelled by the user")),
        };
        h.tx.lock().unwrap().take();
        let (code, message, details) = match &result {
            Ok(()) => (0, String::new(), None),
            Err(s) => {
                let t = entries(s.metadata());
                if !t.is_empty() {
                    trailers = t;
                }
                (s.code() as i32, s.message().to_string(), decode_details(s.details(), &pool))
            }
        };
        if !trailers.is_empty() {
            sink.emit(ProtoEvent::Metadata { at: now_ms(), trailers: true, entries: trailers.clone() });
        }
        let name = code_name(code);
        sink.emit(ProtoEvent::closed(Some(code as i64), Some(if message.is_empty() { name.to_string() } else { format!("{name}: {message}") })));
        GrpcStatus {
            code,
            code_name: name.into(),
            message,
            details,
            headers,
            trailers,
            messages_sent: h.sent.load(Ordering::SeqCst),
            messages_received: received,
            duration_ms: (started.elapsed().as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
        }
    };
    Ok((handle, Box::pin(fut)))
}

#[cfg(test)]
mod tests;
