//! `devall-mock`: a native, WireMock-compatible mock HTTP server.
//!
//! Stubs live in the project as WireMock JSON (`<project>/mocks/mappings/**/*.json`, single
//! mappings or `{"mappings": [...]}`) with body files in `<project>/mocks/__files/`. The admin API
//! under `/__admin` speaks WireMock 3.x JSON so existing WireMock tooling (including the
//! wiremock-ui admin UI) can drive it unmodified; `/__admin/devall/*` adds chaos controls and
//! project info.
//!
//! Stubs can read tabular data from `<project>/mocks/data/` (CSV, JSON, SQLite) with the
//! `dataSource` / `dataRows` / `dataCount` template helpers or a
//! `transformerParameters.dataSource` lookup — see [`data`]. [`load_project`] loads and checks
//! everything without starting a server (`devall mock check`).
//!
//! ```no_run
//! # async fn demo() -> Result<(), devall_mock::MockError> {
//! use devall_mock::{MockConfig, MockServer};
//! let handle = MockServer::start(MockConfig::new("/path/to/project")).await?;
//! println!("mock server on {} with {} stubs", handle.url(), handle.stub_count());
//! handle.stop().await;
//! # Ok(()) }
//! ```
//!
//! Beyond HTTP (see each module): GraphQL schemas with auto-mocking and validation
//! ([`graphql`], on the HTTP port), WebSocket message stubs ([`ws`], on the HTTP port), gRPC
//! ([`grpc`], a separate HTTP/2 listener with reflection) and an MQTT broker with scripted
//! behaviours ([`mqtt`], a separate listener). Their frames and messages go to one message journal
//! (`GET /__admin/devall/messages`).
//!
//! Known gaps vs WireMock 3.x:
//! - Matching: `matchesJsonSchema` and `customMatcher` never match. JSONPath is RFC 9535 plus
//!   Jayway shims (see `jsonpath.rs`); `subsetof`/`anyof`/regex flags are unsupported. XPath is
//!   XPath 1.0 (no DTDs). `equalToXml` supports only the `${xmlunit.ignore}` placeholder and
//!   ignores `exemptedComparisons` / `namespaceAwareness`.
//! - Serving: plain HTTP/1.1 only (no HTTPS, HTTP/2 or browser-proxy mode); proxied responses
//!   are buffered and decompressed; `postServeActions` / `serveEventListeners` (webhooks) are
//!   kept on the stub but not executed; SSE message stubs are absent (WebSocket message stubs
//!   are /dev/all's own format, see [`ws`]).
//! - Templating: `soapXPath` and `jsonSort` are missing, `systemValue` is disabled; variables set
//!   by `assign` / `regexExtract` / `formData` / `parseJson` resolve from the template root (not
//!   from inside `#each` scopes).
//! - Admin: no admin authentication (cross-origin calls are limited to loopback/Tauri origins
//!   instead); project files are not watched (use `POST /__admin/mappings/reset`).

mod admin;
mod chaos;
pub mod data;
mod dates;
mod error;
mod files;
pub mod graphql;
pub mod grpc;
mod journal;
mod jsonpath;
mod matchers;
pub mod messages;
pub mod mqtt;
mod pattern;
mod proxy;
mod recording;
mod request;
mod response;
mod server;
mod store;
mod stub;
mod template;
mod util;
pub mod ws;
mod xml;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use serde::Serialize;
use serde_json::Map;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

pub use chaos::{CHAOS_PRESETS, ChaosConfig};
pub use data::{DATA_DIR, DataSourceInfo, DataSources};
pub use error::{MockError, Result};
pub use files::{FILES_DIR, LoadError, MAPPINGS_DIR, MOCKS_DIR};
pub use messages::{Direction as MessageDirection, MessageEvent, Protocol};
pub use stub::Fault;
pub use template::{FAKER_KEYS, fake_value};

use crate::server::{ServerState, lock, read, write};

/// Version reported by `/__admin/health` and `/__admin/version`. UIs that gate features on the
/// WireMock major version see a 3.x server.
pub const VERSION: &str = "3.13.0-devall";

/// Default request-journal capacity.
pub const DEFAULT_JOURNAL_SIZE: usize = 1000;

#[derive(Debug, Clone)]
pub struct MockConfig {
    /// Project folder; stubs are read from `<root>/mocks/mappings`, bodies from `<root>/mocks/__files`.
    pub project_root: PathBuf,
    /// TCP port; `0` picks a free one.
    pub port: u16,
    /// Listen on `0.0.0.0` instead of `127.0.0.1`.
    pub bind_all: bool,
    /// Number of serve events kept in the request journal.
    pub journal_size: usize,
    /// Apply response templating to every stub (WireMock's `--global-response-templating`).
    pub global_templating: bool,
    /// Add permissive CORS headers to stub responses and answer unmatched preflights
    /// (WireMock's `--enable-stub-cors`).
    pub stub_cors: bool,
    /// Move deleted mapping / body files to the OS trash instead of deleting them.
    pub trash_deleted_files: bool,
    /// Extra browser origins allowed to call the admin API (`*` allows all). Loopback and Tauri
    /// origins are always allowed.
    pub admin_allowed_origins: Vec<String>,
    /// Accept invalid TLS certificates from proxy / recording targets.
    pub proxy_trust_all_certs: bool,
    /// Initial chaos settings.
    pub chaos: ChaosConfig,
    /// Start the gRPC listener on this port (`0` = any free port; `None` = off until
    /// [`MockHandle::start_grpc`]).
    pub grpc_port: Option<u16>,
    /// Start the MQTT broker on this port (`0` = any free port; `None` = off until
    /// [`MockHandle::start_mqtt`]).
    pub mqtt_port: Option<u16>,
}

impl MockConfig {
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self { project_root: project_root.into(), ..Self::default() }
    }
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            project_root: PathBuf::new(),
            port: 0,
            bind_all: false,
            journal_size: DEFAULT_JOURNAL_SIZE,
            global_templating: false,
            stub_cors: false,
            trash_deleted_files: true,
            admin_allowed_origins: Vec::new(),
            proxy_trust_all_certs: false,
            chaos: ChaosConfig::default(),
            grpc_port: None,
            mqtt_port: None,
        }
    }
}

/// One served request, as broadcast to subscribers.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestSummary {
    pub id: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub was_matched: bool,
    pub stub_id: Option<String>,
    pub stub_name: Option<String>,
    /// Epoch milliseconds.
    pub logged_date: i64,
    pub total_time_ms: u64,
    pub fault: Option<String>,
    pub proxied: bool,
}

impl RequestSummary {
    pub(crate) fn from_event(e: &journal::ServeEvent) -> Self {
        Self {
            id: e.id.to_string(),
            method: e.request.method.clone(),
            url: e.request.url.clone(),
            status: e.response.status,
            was_matched: e.was_matched,
            stub_id: e.stub.as_ref().map(|s| s.id.to_string()),
            stub_name: e.stub.as_ref().and_then(|s| s.name.clone()),
            logged_date: e.request.logged_at.timestamp_millis(),
            total_time_ms: e.timing.total_time,
            fault: e.response.fault.map(|f| f.name().to_string()),
            proxied: e.proxied_from.is_some(),
        }
    }
}

/// Live events from a running server.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum JournalEvent {
    Request(RequestSummary),
    /// A WebSocket frame, gRPC message or MQTT message (or a connection event).
    Message(MessageEvent),
    JournalCleared,
    MappingsChanged { stub_count: usize },
    Stopped,
}

pub struct MockServer;

impl MockServer {
    /// Loads the project's stubs and starts listening. Bad mapping files are reported through
    /// [`MockHandle::load_errors`] and do not prevent the server from starting.
    pub async fn start(config: MockConfig) -> Result<MockHandle> {
        let chaos = chaos::Chaos::new(config.chaos.clone()).map_err(MockError::Invalid)?;
        let ip = if config.bind_all { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
        let addr = SocketAddr::new(ip, config.port);
        let listener = TcpListener::bind(addr).await.map_err(|source| MockError::Bind { addr: addr.to_string(), source })?;
        let local = listener.local_addr().map_err(|e| MockError::io(&config.project_root, e))?;
        let (events, _) = broadcast::channel(1024);
        let messages = messages::Messages::new(config.journal_size, events.clone());
        let (shutdown, _) = watch::channel(false);
        let files = files::ProjectFiles::new(&config.project_root, config.trash_deleted_files);
        let data = Arc::new(data::DataSources::for_project(&config.project_root));
        let state = Arc::new(ServerState {
            http: proxy::client(config.proxy_trust_all_certs),
            journal: Mutex::new(journal::Journal::new(config.journal_size)),
            files,
            store: RwLock::new(store::StubStore::default()),
            load_errors: RwLock::new(Vec::new()),
            settings: RwLock::new(Map::new()),
            chaos: RwLock::new(chaos),
            recording: Mutex::new(recording::RecordingState::default()),
            data: data.clone(),
            templates: template::TemplateEngine::with_data(data),
            events,
            started: Instant::now(),
            port: local.port(),
            shutdown,
            running: AtomicBool::new(true),
            graphql: RwLock::new(Arc::new(graphql::GraphqlSet::default())),
            ws: ws::WsState::default(),
            grpc: grpc::GrpcState::default(),
            mqtt: mqtt::MqttState::default(),
            messages,
            config,
        });
        state.reload();
        let task = tokio::spawn(server::run(listener, state.clone()));
        let handle = MockHandle { inner: Arc::new(HandleInner { state: state.clone(), addr: local, task: Mutex::new(Some(task)) }) };
        if let Some(p) = state.config.grpc_port
            && let Err(e) = grpc::start(&state, p).await
        {
            handle.stop().await;
            return Err(e);
        }
        if let Some(p) = state.config.mqtt_port
            && let Err(e) = mqtt::start(&state, p).await
        {
            handle.stop().await;
            return Err(e);
        }
        Ok(handle)
    }
}

struct HandleInner {
    state: Arc<ServerState>,
    addr: SocketAddr,
    task: Mutex<Option<JoinHandle<()>>>,
}

/// Handle to a running server. Cheap to clone; the server keeps running until [`MockHandle::stop`]
/// or `POST /__admin/shutdown`.
#[derive(Clone)]
pub struct MockHandle {
    inner: Arc<HandleInner>,
}

impl std::fmt::Debug for MockHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockHandle").field("addr", &self.inner.addr).field("root", &self.project_root()).finish()
    }
}

impl MockHandle {
    pub fn port(&self) -> u16 {
        self.inner.addr.port()
    }

    /// Base URL for clients on this machine (`http://127.0.0.1:<port>`).
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    pub fn admin_url(&self) -> String {
        format!("{}/__admin", self.url())
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.inner.addr
    }

    pub fn project_root(&self) -> &Path {
        &self.inner.state.config.project_root
    }

    pub fn stub_count(&self) -> usize {
        self.inner.state.stub_count()
    }

    pub fn load_errors(&self) -> Vec<LoadError> {
        read(&self.inner.state.load_errors).clone()
    }

    /// `false` once stopped (also after `POST /__admin/shutdown`).
    pub fn is_running(&self) -> bool {
        self.inner.state.running.load(Ordering::SeqCst)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JournalEvent> {
        self.inner.state.events.subscribe()
    }

    /// Reloads stubs from the project files (dropping non-persistent ones), like
    /// `POST /__admin/mappings/reset`.
    pub fn reload(&self) -> Vec<LoadError> {
        self.inner.state.reload()
    }

    pub fn chaos(&self) -> ChaosConfig {
        read(&self.inner.state.chaos).config.clone()
    }

    pub fn set_chaos(&self, config: ChaosConfig) -> Result<()> {
        let c = chaos::Chaos::new(config).map_err(MockError::Invalid)?;
        *write(&self.inner.state.chaos) = c;
        Ok(())
    }

    /// Starts proxy-and-record against `spec.targetBaseUrl` (WireMock's recording spec, as
    /// `POST /__admin/recordings/start` takes it).
    pub fn start_recording(&self, spec: serde_json::Value) -> std::result::Result<(), String> {
        recording::start(&self.inner.state, spec)
    }

    /// Stops recording and installs (and, unless `persist: false`, saves) the recorded stubs.
    /// Returns WireMock's `{"mappings": [...]}` result.
    pub fn stop_recording(&self) -> std::result::Result<serde_json::Value, String> {
        recording::stop(&self.inner.state)
    }

    /// Data files under `mocks/data/` with their columns and row counts.
    pub fn data_sources(&self) -> Vec<DataSourceInfo> {
        self.inner.state.data.list()
    }

    /// The newest journal entry for `method` + `url` (path and query, as WireMock records it) with
    /// the stub that answered — or, when nothing matched (or the request is not in the journal),
    /// the closest stubs.
    pub fn explain_request(&self, method: &str, url: &str) -> RequestMatch {
        let st = &self.inner.state;
        let event = lock(&st.journal).events().into_iter().rev().find(|e| e.request.method.eq_ignore_ascii_case(method) && e.request.url == url);
        let stub = event.as_ref().and_then(|e| e.stub.clone());
        let file = |s: &stub::Stub| s.source.as_ref().map(|src| format!("{MOCKS_DIR}/{MAPPINGS_DIR}/{}", src.rel_path));
        let mut out = RequestMatch {
            event: event.as_ref().map(|e| e.to_json()),
            matched: event.as_ref().is_some_and(|e| e.was_matched),
            stub: stub.as_ref().map(|s| s.to_json()),
            stub_id: stub.as_ref().map(|s| s.id.to_string()),
            stub_name: stub.as_ref().and_then(|s| s.name.clone()),
            stub_file: stub.as_deref().and_then(file),
            near_misses: Vec::new(),
        };
        if !out.matched {
            let req = match &event {
                Some(e) => Some(e.request.clone()),
                None => request::MockRequest::from_json(&serde_json::json!({ "method": method.to_ascii_uppercase(), "url": url })).ok(),
            };
            if let Some(r) = req {
                out.near_misses = admin::near_misses_for_request(st, &r, None);
                // Point near misses at their files too.
                let store = read(&st.store);
                for nm in &mut out.near_misses {
                    let id = nm.pointer("/stubMapping/id").and_then(serde_json::Value::as_str).and_then(|i| uuid::Uuid::parse_str(i).ok());
                    if let Some(f) = id.and_then(|id| store.get(&id)).and_then(|s| file(&s))
                        && let Some(o) = nm.as_object_mut()
                    {
                        o.insert("file".into(), serde_json::Value::String(f));
                    }
                }
            }
        }
        out
    }

    /// The gRPC listener's port, when running.
    pub fn grpc_port(&self) -> Option<u16> {
        self.inner.state.grpc.port()
    }

    /// The MQTT broker's port, when running.
    pub fn mqtt_port(&self) -> Option<u16> {
        self.inner.state.mqtt.port()
    }

    /// Starts (or moves) the gRPC listener; `0` = any free port. Returns the port.
    pub async fn start_grpc(&self, port: u16) -> Result<u16> {
        grpc::start(&self.inner.state, port).await
    }

    pub async fn stop_grpc(&self) {
        grpc::stop(&self.inner.state).await;
    }

    /// Starts (or moves) the MQTT broker; `0` = any free port. Returns the port.
    pub async fn start_mqtt(&self, port: u16) -> Result<u16> {
        mqtt::start(&self.inner.state, port).await
    }

    pub async fn stop_mqtt(&self) {
        mqtt::stop(&self.inner.state).await;
    }

    /// GraphQL endpoints, WebSocket stubs/connections, gRPC and MQTT listeners.
    pub fn protocols(&self) -> ProtocolsInfo {
        let st = &self.inner.state;
        ProtocolsInfo {
            graphql: server::read(&st.graphql).info().into_iter().map(|s| s.endpoint).collect(),
            websocket_stubs: st.ws.stubs_json().len(),
            websocket_connections: st.ws.connections().len(),
            grpc_port: st.grpc.port(),
            grpc_methods: st.grpc.methods().len(),
            mqtt_port: st.mqtt.port(),
            mqtt_clients: st.mqtt.clients().len(),
        }
    }

    /// Recent WebSocket / gRPC / MQTT messages, newest first.
    pub fn messages(&self, protocol: Option<Protocol>, limit: usize) -> Vec<MessageEvent> {
        self.inner.state.messages.list(protocol, limit)
    }

    /// Number of entries currently in the request journal.
    pub fn journal_len(&self) -> usize {
        lock(&self.inner.state.journal).len()
    }

    /// Stops accepting connections, lets in-flight requests finish (up to 2 s) and waits for
    /// the server task. Idempotent.
    pub async fn stop(&self) {
        grpc::stop(&self.inner.state).await;
        mqtt::stop(&self.inner.state).await;
        let _ = self.inner.state.shutdown.send_replace(true);
        let task = lock(&self.inner.task).take();
        if let Some(t) = task {
            let _ = t.await;
        }
    }
}

/// The non-HTTP side of a running server (for status bars and `devall mock serve`).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolsInfo {
    /// GraphQL endpoint paths on the HTTP port.
    pub graphql: Vec<String>,
    pub websocket_stubs: usize,
    pub websocket_connections: usize,
    pub grpc_port: Option<u16>,
    pub grpc_methods: usize,
    pub mqtt_port: Option<u16>,
    pub mqtt_clients: usize,
}

/// Which protocol folders a project has (callers decide which listeners to start).
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolFolders {
    pub graphql: bool,
    pub websocket: bool,
    /// `mocks/grpc` contains `.proto` files.
    pub grpc: bool,
    /// `mocks/mqtt` contains behaviour files.
    pub mqtt: bool,
}

/// Looks for `mocks/graphql`, `mocks/websocket`, `.proto` files under `mocks/grpc` and JSON files
/// under `mocks/mqtt`.
pub fn protocol_folders(root: &Path) -> ProtocolFolders {
    let dir = root.join(MOCKS_DIR);
    fn has(dir: &Path, ext: &str) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else { return false };
        rd.flatten().any(|e| {
            let p = e.path();
            if p.is_dir() { has(&p, ext) } else { p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) }
        })
    }
    ProtocolFolders {
        graphql: dir.join(graphql::GRAPHQL_DIR).is_dir(),
        websocket: has(&dir.join(ws::WS_DIR), "json"),
        grpc: has(&dir.join(grpc::GRPC_DIR), "proto"),
        mqtt: has(&dir.join(mqtt::MQTT_DIR), "json"),
    }
}

/// How the mock server handled (or would handle) a request — for "explain this failure".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMatch {
    /// The newest journal entry for the request (WireMock JSON), when it reached this server.
    pub event: Option<serde_json::Value>,
    pub matched: bool,
    /// The stub that answered (WireMock JSON).
    pub stub: Option<serde_json::Value>,
    pub stub_id: Option<String>,
    pub stub_name: Option<String>,
    /// Mapping file of that stub, relative to the project root.
    pub stub_file: Option<String>,
    /// Up to three closest stubs when nothing matched (WireMock near-miss JSON).
    pub near_misses: Vec<serde_json::Value>,
}

/// One stub mapping as stored in the project.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingInfo {
    pub id: String,
    pub name: Option<String>,
    /// File relative to the project root, `/`-separated.
    pub file: String,
    /// The mapping JSON (WireMock format, as it would be written).
    pub mapping: serde_json::Value,
}

/// Everything under `mocks/`, loaded without starting a server.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMocks {
    pub mappings: Vec<MappingInfo>,
    pub load_errors: Vec<LoadError>,
    pub data_sources: Vec<DataSourceInfo>,
}

impl ProjectMocks {
    /// Mapping files that failed to load plus unreadable data files, as `file: error` lines.
    pub fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self.load_errors.iter().map(|e| format!("{}: {}", e.file, e.error)).collect();
        out.extend(self.data_sources.iter().filter_map(|d| d.error.as_ref().map(|e| format!("{}: {e}", d.file))));
        out
    }
}

/// Problems in the GraphQL schemas and the WebSocket / gRPC / MQTT definitions (nothing is started).
pub fn check_protocols(root: &Path) -> Vec<LoadError> {
    let mut out = graphql::GraphqlSet::load(root).errors;
    out.extend(ws::WsState::default().reload(root));
    out.extend(grpc::GrpcState::default().reload(root));
    out.extend(mqtt::MqttState::default().reload(root));
    out
}

/// Loads (and validates) every mapping in `<root>/mocks/mappings` (plus the other protocols'
/// files) and lists the data sources —
/// what `devall mock check` reports and drift checks read.
pub fn load_project(root: &Path) -> ProjectMocks {
    let files = files::ProjectFiles::new(root, false);
    let (stubs, mut load_errors) = files.load();
    load_errors.extend(check_protocols(root));
    let mappings = stubs
        .iter()
        .map(|s| MappingInfo {
            id: s.id.to_string(),
            name: s.name.clone(),
            file: s.source.as_ref().map(|src| format!("{MOCKS_DIR}/{MAPPINGS_DIR}/{}", src.rel_path)).unwrap_or_default(),
            mapping: s.to_file_json(),
        })
        .collect();
    ProjectMocks { mappings, load_errors, data_sources: DataSources::for_project(root).list() }
}
