//! WebSocket, gRPC, MQTT and Socket.IO sessions plus GraphQL schema introspection — a thin layer
//! over `crates/devall-proto`. Files live next to HTTP requests under `http/` (`*.ws.yaml`,
//! `*.grpc.yaml`, `*.mqtt.yaml`, `*.sio.yaml`).
//!
//! # Integration (done by whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    `devall-proto = { path = "../../../crates/devall-proto" }`
//!    (`tauri-plugin-opener`, already a dependency, is used for OAuth 2.0 browser sign-in during
//!    GraphQL introspection.)
//! 2. `src/modules/mod.rs`: `pub mod proto;`
//! 3. `src/lib.rs`: on the builder `.manage(modules::proto::ProtoState::default())`, and in
//!    `tauri::generate_handler![...]`:
//!    ```text
//!    modules::proto::proto_read,
//!    modules::proto::proto_save,
//!    modules::proto::proto_create,
//!    modules::proto::proto_sessions,
//!    modules::proto::proto_close,
//!    modules::proto::ws_connect,
//!    modules::proto::ws_send,
//!    modules::proto::ws_close,
//!    modules::proto::grpc_describe,
//!    modules::proto::grpc_call,
//!    modules::proto::grpc_send,
//!    modules::proto::grpc_end,
//!    modules::proto::grpc_cancel,
//!    modules::proto::mqtt_connect,
//!    modules::proto::mqtt_publish,
//!    modules::proto::mqtt_subscribe,
//!    modules::proto::mqtt_unsubscribe,
//!    modules::proto::mqtt_disconnect,
//!    modules::proto::sio_connect,
//!    modules::proto::sio_emit,
//!    modules::proto::sio_close,
//!    modules::proto::graphql_introspect,
//!    ```
//!
//! Variables and secrets resolve exactly like `send.rs`: runtime (captured) values first, then the
//! active environment, folders (nearest first) and the project, via `commands::resolver_for`, with
//! `{{secret.X}}` looked up in the keychain store of `state::AppState`. Every secret value used by a
//! session is masked in the events streamed to the UI and in returned errors. Outgoing messages are
//! resolved when they are sent, so they see the current runtime variables.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use devall_core::secrets::SecretResolver;
use devall_core::vars::redact_values;
use devall_core::{HttpRequestFile, Project, Resolver};
use devall_proto::grpc::{self, GrpcCallHandle, GrpcDescription, GrpcOptions, GrpcStatus};
use devall_proto::graphql::{self, GraphqlSchema};
use devall_proto::mqtt::{MqttOptions, MqttSession};
use devall_proto::sio::{self, SioOptions, SioSession};
use devall_proto::ws::{WsOptions, WsOutgoing, WsSession};
use devall_proto::{EventSink, GrpcFile, MessageFormat, MqttFile, ProtoError, ProtoEvent, ProtoFile, ProtoKind, SioFile, WsFile};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::commands::resolver_for;
use crate::state::{AppState, KeychainTokens};

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn open(root: &str) -> R<Project> {
    Project::open(root).map_err(err)
}

/// Where a session's templates are resolved from.
#[derive(Debug, Clone)]
struct Ctx {
    root: String,
    path: String,
    env: Option<String>,
}

enum SessionKind {
    Ws(WsSession),
    Mqtt(MqttSession),
    Sio(SioSession),
    Grpc(Arc<GrpcCallHandle>),
}

impl SessionKind {
    fn name(&self) -> &'static str {
        match self {
            SessionKind::Ws(_) => "ws",
            SessionKind::Mqtt(_) => "mqtt",
            SessionKind::Sio(_) => "socketio",
            SessionKind::Grpc(_) => "grpc",
        }
    }

    fn is_open(&self) -> bool {
        match self {
            SessionKind::Ws(s) => s.is_open(),
            SessionKind::Mqtt(s) => s.is_open(),
            SessionKind::Sio(s) => s.is_open(),
            SessionKind::Grpc(h) => h.is_open(),
        }
    }
}

struct Live {
    kind: SessionKind,
    ctx: Ctx,
    /// Secret values used so far (connect + every send), masked in events and errors.
    secrets: Arc<Mutex<BTreeSet<String>>>,
    sink: EventSink,
}

impl Live {
    fn redact(&self, text: &str) -> String {
        redact_values(text, &self.secrets.lock().unwrap())
    }
}

type Sessions = Arc<Mutex<HashMap<String, Arc<Live>>>>;

/// Live sessions by id (sessions are app-wide; ids are returned to the UI on connect).
#[derive(Default)]
pub struct ProtoState {
    sessions: Sessions,
}

impl ProtoState {
    fn get(&self, id: &str) -> R<Arc<Live>> {
        self.sessions.lock().unwrap().get(id).cloned().ok_or_else(|| "this session has ended — connect again".to_string())
    }

    fn insert(&self, id: String, live: Live) {
        let open = live.kind.is_open();
        if open {
            self.sessions.lock().unwrap().insert(id, Arc::new(live));
        }
    }

    fn remove(&self, id: &str) -> Option<Arc<Live>> {
        self.sessions.lock().unwrap().remove(id)
    }

    /// Streams events to the UI (secrets masked); forgets the session once it reports `closed`.
    fn sink(&self, id: &str, on_event: Channel<ProtoEvent>, secrets: Arc<Mutex<BTreeSet<String>>>, forget_on_close: bool) -> EventSink {
        let sessions = self.sessions.clone();
        let id = id.to_string();
        EventSink::new(move |mut e| {
            let closed = e.is_closed();
            e.redact(&secrets.lock().unwrap());
            let _ = on_event.send(e);
            if closed && forget_on_close {
                // Take it out before dropping so no lock is held while the session shuts down.
                let gone = sessions.lock().unwrap().remove(&id);
                drop(gone);
            }
        })
    }
}

struct Resolved<T> {
    value: T,
    secrets: BTreeSet<String>,
    unresolved: Vec<String>,
    secret_errors: Vec<String>,
}

/// Builds the same resolver as `send.rs` and runs `f` with it.
fn resolve_with<T>(state: &AppState, ctx: &Ctx, f: impl FnOnce(&mut Resolver, &Project) -> Result<T, ProtoError>) -> R<Resolved<T>> {
    let p = open(&ctx.root)?;
    let env = ctx.env.as_deref().filter(|e| !e.is_empty());
    let env_file = env.map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let folder_vars = p.effective_request(&ctx.path, &HttpRequestFile::new("")).map_err(err)?.folder_variables;
    let secrets = SecretResolver::new(state.secrets.as_ref(), p.id(), env, p.root());
    let runtime = state.runtime_vars(&ctx.root);
    let mut r = resolver_for(&p, &runtime, env, env_file.as_ref(), &folder_vars, &secrets);
    let out = f(&mut r, &p);
    let secret_values = r.secret_values.clone();
    let value = out.map_err(|e| e.redacted(&secret_values))?;
    let secret_errors = secrets.errors.lock().unwrap().clone();
    Ok(Resolved { value, secrets: secret_values, unresolved: r.unresolved.iter().cloned().collect(), secret_errors })
}

/// Tells the UI about unresolved `{{references}}` and keychain problems.
fn report<T>(sink: &EventSink, res: &Resolved<T>) {
    if !res.unresolved.is_empty() {
        sink.info(format!(
            "unresolved: {} — sent literally; check the active environment",
            res.unresolved.iter().map(|u| format!("{{{{{u}}}}}")).collect::<Vec<_>>().join(", ")
        ));
    }
    for e in &res.secret_errors {
        sink.error(format!("keychain: {e}"));
    }
}

fn ctx(root: String, path: String, env: Option<String>) -> Ctx {
    Ctx { root, path, env }
}

// ---------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------

/// Reads a `*.ws.yaml` / `*.grpc.yaml` / `*.mqtt.yaml` / `*.sio.yaml` file (kind from the extension).
#[tauri::command]
pub fn proto_read(root: String, path: String) -> R<serde_json::Value> {
    Ok(ProtoFile::read(&open(&root)?, &path).map_err(err)?.to_value())
}

#[tauri::command]
pub fn proto_save(root: String, path: String, item: serde_json::Value) -> R<()> {
    let kind = ProtoKind::from_path(&path).ok_or_else(|| format!("{path} is not a WebSocket, gRPC, MQTT or Socket.IO file"))?;
    ProtoFile::from_value(kind, item).map_err(err)?.save(&open(&root)?, &path).map_err(err)
}

/// Creates a starter file; `kind` is `ws`, `grpc`, `mqtt` or `socketio`. Returns its path.
#[tauri::command]
pub fn proto_create(root: String, folder: String, kind: String, name: String) -> R<String> {
    let kind = ProtoKind::parse(&kind).ok_or_else(|| format!("unknown kind {kind}"))?;
    ProtoFile::create(&open(&root)?, &folder, kind, &name).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    id: String,
    kind: &'static str,
    root: String,
    path: String,
    open: bool,
}

/// Live sessions (so a reloaded UI can reattach or close them).
#[tauri::command]
pub fn proto_sessions(proto: State<'_, ProtoState>) -> Vec<SessionInfo> {
    proto
        .sessions
        .lock()
        .unwrap()
        .iter()
        .map(|(id, l)| SessionInfo { id: id.clone(), kind: l.kind.name(), root: l.ctx.root.clone(), path: l.ctx.path.clone(), open: l.kind.is_open() })
        .collect()
}

/// Closes any kind of session. Returns false when it no longer exists.
#[tauri::command]
pub async fn proto_close(proto: State<'_, ProtoState>, session: String) -> R<bool> {
    let Some(live) = proto.remove(&session) else { return Ok(false) };
    match &live.kind {
        SessionKind::Ws(s) => s.close(None, None),
        SessionKind::Mqtt(s) => s.disconnect().await,
        SessionKind::Sio(s) => s.close(),
        SessionKind::Grpc(h) => h.cancel(),
    }
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------------------------

/// Connects; events stream to `on_event`. `item` is the editor's current (possibly unsaved) state
/// and `path` its location (for folder variables). Returns the session id.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ws_connect(
    state: State<'_, AppState>,
    proto: State<'_, ProtoState>,
    root: String,
    path: String,
    item: WsFile,
    env: Option<String>,
    on_event: Channel<ProtoEvent>,
) -> R<String> {
    let ctx = ctx(root, path, env);
    let res = resolve_with(&state, &ctx, |r, _| WsOptions::resolve(&item, r))?;
    let id = devall_core::new_id();
    let secrets = Arc::new(Mutex::new(res.secrets.clone()));
    let sink = proto.sink(&id, on_event, secrets.clone(), true);
    report(&sink, &res);
    let session = WsSession::connect(res.value, sink.clone()).await.map_err(|e| e.redacted(&res.secrets))?;
    proto.insert(id.clone(), Live { kind: SessionKind::Ws(session), ctx, secrets, sink });
    Ok(id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutMessage {
    body: String,
    #[serde(default)]
    format: MessageFormat,
}

/// Sends a message (its `{{templates}}` are resolved now).
#[tauri::command]
pub async fn ws_send(state: State<'_, AppState>, proto: State<'_, ProtoState>, session: String, message: OutMessage) -> R<()> {
    let live = proto.get(&session)?;
    let SessionKind::Ws(ws) = &live.kind else { return Err("not a WebSocket session".into()) };
    let res = resolve_with(&state, &live.ctx, |r, _| WsOutgoing::from_body(&r.resolve(&message.body), message.format))?;
    live.secrets.lock().unwrap().extend(res.secrets.iter().cloned());
    report(&live.sink, &res);
    ws.send(res.value).map_err(|e| live.redact(&e.to_string()))
}

#[tauri::command]
pub fn ws_close(proto: State<'_, ProtoState>, session: String, code: Option<u16>, reason: Option<String>) -> R<()> {
    let live = proto.get(&session)?;
    let SessionKind::Ws(ws) = &live.kind else { return Err("not a WebSocket session".into()) };
    ws.close(code, reason);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// gRPC
// ---------------------------------------------------------------------------------------------

/// Services, methods and request templates, from server reflection or the configured `.proto`
/// files. `path` (optional) is the file's location, for folder variables.
#[tauri::command]
pub async fn grpc_describe(state: State<'_, AppState>, root: String, path: Option<String>, item: GrpcFile, env: Option<String>) -> R<GrpcDescription> {
    let ctx = ctx(root, path.unwrap_or_default(), env);
    let res = resolve_with(&state, &ctx, |r, p| GrpcOptions::resolve(&item, r, p.root()))?;
    grpc::describe(&res.value).await.map_err(|e| e.redacted(&res.secrets))
}

/// Runs a call; messages and metadata stream to `on_event`; resolves with the final status.
/// `callId` is chosen by the UI (for `grpc_send`/`grpc_end`/`grpc_cancel`). With `keepOpen`,
/// client/bidi-streaming calls keep the request stream open until `grpc_end`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn grpc_call(
    state: State<'_, AppState>,
    proto: State<'_, ProtoState>,
    call_id: String,
    root: String,
    path: String,
    item: GrpcFile,
    env: Option<String>,
    keep_open: Option<bool>,
    on_event: Channel<ProtoEvent>,
) -> R<GrpcStatus> {
    let ctx = ctx(root, path, env);
    let res = resolve_with(&state, &ctx, |r, p| GrpcOptions::resolve(&item, r, p.root()))?;
    let secrets = Arc::new(Mutex::new(res.secrets.clone()));
    let sink = proto.sink(&call_id, on_event, secrets.clone(), false);
    report(&sink, &res);
    let (handle, fut) = grpc::start_call(&res.value, sink.clone(), keep_open.unwrap_or(false)).await.map_err(|e| e.redacted(&res.secrets))?;
    proto.sessions.lock().unwrap().insert(call_id.clone(), Arc::new(Live { kind: SessionKind::Grpc(handle), ctx, secrets: secrets.clone(), sink }));
    let mut status = fut.await;
    proto.remove(&call_id);
    let s = secrets.lock().unwrap();
    status.message = redact_values(&status.message, &s);
    for (_, v) in status.headers.iter_mut().chain(status.trailers.iter_mut()) {
        *v = redact_values(v, &s);
    }
    if let Some(d) = status.details.take() {
        status.details = serde_json::from_str(&redact_values(&d.to_string(), &s)).ok();
    }
    Ok(status)
}

/// Sends one more request message (JSON; `{{templates}}` resolved now) on a streaming call.
#[tauri::command]
pub async fn grpc_send(state: State<'_, AppState>, proto: State<'_, ProtoState>, call_id: String, message: String) -> R<()> {
    let live = proto.get(&call_id)?;
    let SessionKind::Grpc(h) = &live.kind else { return Err("not a gRPC call".into()) };
    let res = resolve_with(&state, &live.ctx, |r, _| Ok(r.resolve(&message)))?;
    live.secrets.lock().unwrap().extend(res.secrets.iter().cloned());
    report(&live.sink, &res);
    h.send_json(&res.value).map_err(|e| live.redact(&e.to_string()))
}

/// Half-closes the request stream.
#[tauri::command]
pub fn grpc_end(proto: State<'_, ProtoState>, call_id: String) -> R<()> {
    let live = proto.get(&call_id)?;
    let SessionKind::Grpc(h) = &live.kind else { return Err("not a gRPC call".into()) };
    h.end();
    Ok(())
}

#[tauri::command]
pub fn grpc_cancel(proto: State<'_, ProtoState>, call_id: String) -> bool {
    match proto.get(&call_id) {
        Ok(live) => match &live.kind {
            SessionKind::Grpc(h) => {
                h.cancel();
                true
            }
            _ => false,
        },
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------------------------
// MQTT
// ---------------------------------------------------------------------------------------------

/// Connects (and subscribes to the file's enabled subscriptions). Returns the session id.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn mqtt_connect(
    state: State<'_, AppState>,
    proto: State<'_, ProtoState>,
    root: String,
    path: String,
    item: MqttFile,
    env: Option<String>,
    on_event: Channel<ProtoEvent>,
) -> R<String> {
    let ctx = ctx(root, path, env);
    let res = resolve_with(&state, &ctx, |r, _| MqttOptions::resolve(&item, r))?;
    let id = devall_core::new_id();
    let secrets = Arc::new(Mutex::new(res.secrets.clone()));
    let sink = proto.sink(&id, on_event, secrets.clone(), true);
    report(&sink, &res);
    let session = MqttSession::connect(res.value, sink.clone()).await.map_err(|e| e.redacted(&res.secrets))?;
    proto.insert(id.clone(), Live { kind: SessionKind::Mqtt(session), ctx, secrets, sink });
    Ok(id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn mqtt_publish(
    state: State<'_, AppState>,
    proto: State<'_, ProtoState>,
    session: String,
    topic: String,
    payload: String,
    qos: Option<u8>,
    retain: Option<bool>,
    format: Option<MessageFormat>,
) -> R<()> {
    let live = proto.get(&session)?;
    let SessionKind::Mqtt(m) = &live.kind else { return Err("not an MQTT session".into()) };
    let format = format.unwrap_or_default();
    let res = resolve_with(&state, &live.ctx, |r, _| Ok((r.resolve(topic.trim()), format.encode(&r.resolve(&payload))?)))?;
    live.secrets.lock().unwrap().extend(res.secrets.iter().cloned());
    report(&live.sink, &res);
    let (topic, bytes) = res.value;
    m.publish(&topic, bytes, qos.unwrap_or(0), retain.unwrap_or(false)).await.map_err(|e| live.redact(&e.to_string()))
}

#[tauri::command]
pub async fn mqtt_subscribe(state: State<'_, AppState>, proto: State<'_, ProtoState>, session: String, topic: String, qos: Option<u8>) -> R<()> {
    let live = proto.get(&session)?;
    let SessionKind::Mqtt(m) = &live.kind else { return Err("not an MQTT session".into()) };
    let res = resolve_with(&state, &live.ctx, |r, _| Ok(r.resolve(topic.trim())))?;
    report(&live.sink, &res);
    m.subscribe(&res.value, qos.unwrap_or(0)).await.map_err(err)
}

#[tauri::command]
pub async fn mqtt_unsubscribe(state: State<'_, AppState>, proto: State<'_, ProtoState>, session: String, topic: String) -> R<()> {
    let live = proto.get(&session)?;
    let SessionKind::Mqtt(m) = &live.kind else { return Err("not an MQTT session".into()) };
    let res = resolve_with(&state, &live.ctx, |r, _| Ok(r.resolve(topic.trim())))?;
    m.unsubscribe(&res.value).await.map_err(err)
}

#[tauri::command]
pub async fn mqtt_disconnect(proto: State<'_, ProtoState>, session: String) -> R<()> {
    let Some(live) = proto.remove(&session) else { return Ok(()) };
    if let SessionKind::Mqtt(m) = &live.kind {
        m.disconnect().await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Socket.IO
// ---------------------------------------------------------------------------------------------

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn sio_connect(
    state: State<'_, AppState>,
    proto: State<'_, ProtoState>,
    root: String,
    path: String,
    item: SioFile,
    env: Option<String>,
    on_event: Channel<ProtoEvent>,
) -> R<String> {
    let ctx = ctx(root, path, env);
    let res = resolve_with(&state, &ctx, |r, _| SioOptions::resolve(&item, r))?;
    let id = devall_core::new_id();
    let secrets = Arc::new(Mutex::new(res.secrets.clone()));
    let sink = proto.sink(&id, on_event, secrets.clone(), true);
    report(&sink, &res);
    let session = SioSession::connect(res.value, sink.clone()).await.map_err(|e| e.redacted(&res.secrets))?;
    proto.insert(id.clone(), Live { kind: SessionKind::Sio(session), ctx, secrets, sink });
    Ok(id)
}

/// Emits `event`; `args` is JSON (an array is spread into arguments). Returns the ack id when
/// `ack` is set; the server's acknowledgement arrives as an `ack #<id>` message event.
#[tauri::command]
pub async fn sio_emit(state: State<'_, AppState>, proto: State<'_, ProtoState>, session: String, event: String, args: String, ack: Option<bool>) -> R<Option<u64>> {
    let live = proto.get(&session)?;
    let SessionKind::Sio(s) = &live.kind else { return Err("not a Socket.IO session".into()) };
    let res = resolve_with(&state, &live.ctx, |r, _| Ok((r.resolve(event.trim()), sio::parse_args(&r.resolve(&args))?)))?;
    live.secrets.lock().unwrap().extend(res.secrets.iter().cloned());
    report(&live.sink, &res);
    let (event, args) = res.value;
    s.emit(&event, args, ack.unwrap_or(false)).map_err(|e| live.redact(&e.to_string()))
}

#[tauri::command]
pub fn sio_close(proto: State<'_, ProtoState>, session: String) -> R<()> {
    let live = proto.get(&session)?;
    let SessionKind::Sio(s) = &live.kind else { return Err("not a Socket.IO session".into()) };
    s.close();
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// GraphQL
// ---------------------------------------------------------------------------------------------

/// Introspects the GraphQL endpoint of an HTTP request (its URL, params, headers, auth and
/// settings, with folder inheritance) and returns the schema with a generated operation per root
/// field.
#[tauri::command]
pub async fn graphql_introspect(app: AppHandle, state: State<'_, AppState>, root: String, path: String, request: HttpRequestFile, env: Option<String>) -> R<GraphqlSchema> {
    let p = open(&root)?;
    let env = env.filter(|e| !e.is_empty());
    let env_file = env.as_deref().map(|id| p.read_environment(id)).transpose().map_err(err)?;
    let eff = p.effective_request(&path, &request).map_err(err)?;
    let secrets = SecretResolver::new(state.secrets.as_ref(), p.id(), env.as_deref(), p.root());
    let runtime = state.runtime_vars(&root);
    let mut r = resolver_for(&p, &runtime, env.as_deref(), env_file.as_ref(), &eff.folder_variables, &secrets);
    let tokens = KeychainTokens { store: state.secrets.as_ref(), project_id: p.id().to_string() };
    let opener = {
        let app = app.clone();
        move |url: &str| app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
    };
    let req = graphql::introspection_request(&eff.request);
    let prepared = devall_http::prepare_authorized(&req, &mut r, p.root(), &tokens, Some(&opener)).await.map_err(|e| e.redacted(&r.secret_values).to_string())?;
    let secret_values = r.secret_values.clone();
    let pool = state.pool(&root);
    graphql::execute_introspection(&pool, &prepared).await.map_err(|e| e.redacted(&secret_values))
}
