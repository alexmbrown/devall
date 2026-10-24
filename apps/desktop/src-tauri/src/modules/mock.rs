//! Mock server commands: one `devall-mock` server per project root.
//!
//! Integration (done by whoever wires the module in):
//!
//! `src/modules/mod.rs`:
//! ```ignore
//! pub mod mock;
//! ```
//!
//! `src/lib.rs`:
//! ```ignore
//! mod modules;                                   // if not declared yet
//! // in the builder chain (before .invoke_handler):
//! .manage(modules::mock::MockState::default())
//! // inside tauri::generate_handler![ … ]:
//! modules::mock::mock_start,
//! modules::mock::mock_stop,
//! modules::mock::mock_status,
//! modules::mock::mock_list,
//! modules::mock::mock_data_sources,
//! ```
//!
//! `src-tauri/Cargo.toml` `[dependencies]`:
//! ```toml
//! devall-mock = { path = "../../../crates/devall-mock" }
//! ```
//! (`serde` and `tauri` are already dependencies; nothing else is needed.)
//!
//! Servers keep running until `mock_stop`, `POST /__admin/shutdown` or app exit. A server stopped
//! through the admin API reports `running: false` and is restarted by the next `mock_start`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use devall_mock::{DataSourceInfo, DataSources, LoadError, MockConfig, MockHandle, MockServer, ProtocolFolders, ProtocolsInfo};
use serde::Serialize;
use tauri::State;

#[derive(Default)]
pub struct MockState {
    servers: Mutex<HashMap<String, MockHandle>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MockStatus {
    pub root: String,
    pub running: bool,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub stub_count: usize,
    pub load_errors: Vec<LoadError>,
    /// gRPC listener / MQTT broker ports, GraphQL endpoints, WebSocket stubs.
    pub protocols: Option<ProtocolsInfo>,
}

/// Map key for a project root: separators unified, trailing separators dropped, and (on Windows)
/// case-folded, so `C:\p\` and `c:/p` share one server.
fn key(root: &str) -> String {
    let unified = root.trim().replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    let k = if trimmed.is_empty() { unified.as_str() } else { trimmed };
    if cfg!(windows) { k.to_lowercase() } else { k.to_string() }
}

fn status_of(root: &str, handle: Option<&MockHandle>) -> MockStatus {
    match handle {
        Some(h) if h.is_running() => MockStatus {
            root: root.to_string(),
            running: true,
            port: Some(h.port()),
            url: Some(h.url()),
            stub_count: h.stub_count(),
            load_errors: h.load_errors(),
            protocols: Some(h.protocols()),
        },
        Some(h) => MockStatus { root: root.to_string(), running: false, port: None, url: None, stub_count: 0, load_errors: h.load_errors(), protocols: None },
        None => MockStatus { root: root.to_string(), running: false, port: None, url: None, stub_count: 0, load_errors: Vec::new(), protocols: None },
    }
}

/// The project's running mock server, if any.
pub(crate) fn running(state: &MockState, root: &str) -> Option<MockHandle> {
    get(state, root).filter(MockHandle::is_running)
}

fn get(state: &MockState, root: &str) -> Option<MockHandle> {
    state.servers.lock().unwrap_or_else(|e| e.into_inner()).get(&key(root)).cloned()
}

/// The project's running server, or a new one on a free local port (load tests against the mock).
pub async fn ensure_running(state: &MockState, root: &str) -> Result<MockStatus, String> {
    if let Some(h) = get(state, root)
        && h.is_running()
    {
        return Ok(status_of(root, Some(&h)));
    }
    start(state, root.to_string(), None, None).await
}

/// Starts the project's mock server (or returns the running one). `port` defaults to a free
/// port; asking for a different port or bind mode restarts the server.
#[tauri::command]
pub async fn mock_start(state: State<'_, MockState>, root: String, port: Option<u16>, bind_all: Option<bool>) -> Result<MockStatus, String> {
    start(&state, root, port, bind_all).await
}

/// CSV / JSON / SQLite data sources under `mocks/data/` (for the stub editor's helper picker).
#[tauri::command]
pub async fn mock_data_sources(root: String) -> Vec<DataSourceInfo> {
    tauri::async_runtime::spawn_blocking(move || DataSources::for_project(Path::new(&root)).list()).await.unwrap_or_default()
}

async fn start(state: &MockState, root: String, port: Option<u16>, bind_all: Option<bool>) -> Result<MockStatus, String> {
    if !Path::new(&root).is_dir() {
        return Err(format!("{root} is not a folder"));
    }
    let bind_all = bind_all.unwrap_or(false);
    if let Some(existing) = get(state, &root) {
        let same_port = port.is_none_or(|p| p == 0 || p == existing.port());
        let same_bind = existing.local_addr().ip().is_unspecified() == bind_all;
        if existing.is_running() && same_port && same_bind {
            return Ok(status_of(&root, Some(&existing)));
        }
        existing.stop().await;
        state.servers.lock().unwrap_or_else(|e| e.into_inner()).remove(&key(&root));
    }
    let mut config = MockConfig::new(&root);
    config.port = port.unwrap_or(0);
    config.bind_all = bind_all;
    // gRPC / MQTT listeners start on free ports when the project has them (the page can move
    // them to fixed ports).
    let folders = devall_mock::protocol_folders(Path::new(&root));
    config.grpc_port = folders.grpc.then_some(0);
    config.mqtt_port = folders.mqtt.then_some(0);
    let handle = MockServer::start(config).await.map_err(|e| e.to_string())?;
    crate::modules::logs::forward_mock_journal(&root, handle.subscribe());
    let status = status_of(&root, Some(&handle));
    state.servers.lock().unwrap_or_else(|e| e.into_inner()).insert(key(&root), handle);
    Ok(status)
}

#[tauri::command]
pub async fn mock_stop(state: State<'_, MockState>, root: String) -> Result<(), String> {
    let handle = state.servers.lock().unwrap_or_else(|e| e.into_inner()).remove(&key(&root));
    if let Some(h) = handle {
        h.stop().await;
    }
    Ok(())
}

#[tauri::command]
pub fn mock_status(state: State<'_, MockState>, root: String) -> MockStatus {
    status_of(&root, get(&state, &root).as_ref())
}

#[tauri::command]
pub fn mock_list(state: State<'_, MockState>) -> Vec<MockStatus> {
    let servers = state.servers.lock().unwrap_or_else(|e| e.into_inner());
    let mut list: Vec<MockStatus> = servers.values().map(|h| status_of(&h.project_root().display().to_string(), Some(h))).collect();
    list.sort_by(|a, b| a.root.cmp(&b.root));
    list
}

/// Starts (`port`: 0 = any free port) or moves the running server's gRPC listener.
#[tauri::command]
pub async fn mock_grpc_start(state: State<'_, MockState>, root: String, port: Option<u16>) -> Result<MockStatus, String> {
    let h = running(&state, &root).ok_or("the mock server is not running")?;
    h.start_grpc(port.unwrap_or(0)).await.map_err(|e| e.to_string())?;
    Ok(status_of(&root, Some(&h)))
}

#[tauri::command]
pub async fn mock_grpc_stop(state: State<'_, MockState>, root: String) -> Result<MockStatus, String> {
    let h = running(&state, &root).ok_or("the mock server is not running")?;
    h.stop_grpc().await;
    Ok(status_of(&root, Some(&h)))
}

/// Starts (`port`: 0 = any free port) or moves the running server's MQTT broker.
#[tauri::command]
pub async fn mock_mqtt_start(state: State<'_, MockState>, root: String, port: Option<u16>) -> Result<MockStatus, String> {
    let h = running(&state, &root).ok_or("the mock server is not running")?;
    h.start_mqtt(port.unwrap_or(0)).await.map_err(|e| e.to_string())?;
    Ok(status_of(&root, Some(&h)))
}

#[tauri::command]
pub async fn mock_mqtt_stop(state: State<'_, MockState>, root: String) -> Result<MockStatus, String> {
    let h = running(&state, &root).ok_or("the mock server is not running")?;
    h.stop_mqtt().await;
    Ok(status_of(&root, Some(&h)))
}

/// A definition file under `mocks/graphql|websocket|grpc|mqtt`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolFile {
    /// `graphql`, `websocket`, `grpc` or `mqtt`.
    pub protocol: String,
    /// Relative to the project root, `/`-separated.
    pub rel: String,
    /// Absolute path (for the editor).
    pub path: String,
    pub size: u64,
}

/// Lists the protocol folders' files (schemas, protos, stub JSON).
#[tauri::command]
pub async fn mock_protocol_files(root: String) -> Result<(ProtocolFolders, Vec<ProtocolFile>), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let base = Path::new(&root);
        let mut out = Vec::new();
        for proto in ["graphql", "websocket", "grpc", "mqtt"] {
            let dir = base.join(devall_mock::MOCKS_DIR).join(proto);
            let mut stack = vec![dir];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if e.file_name().to_string_lossy().starts_with('.') {
                        continue;
                    }
                    if p.is_dir() {
                        stack.push(p);
                    } else {
                        let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                        out.push(ProtocolFile { protocol: proto.into(), rel, path: p.display().to_string(), size: e.metadata().map(|m| m.len()).unwrap_or(0) });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.rel.cmp(&b.rel));
        Ok((devall_mock::protocol_folders(base), out))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Writes a protocol file (`rel` must stay inside `mocks/graphql|websocket|grpc|mqtt`).
#[tauri::command]
pub async fn mock_protocol_write(root: String, rel: String, content: String) -> Result<String, String> {
    let path = protocol_path(&root, &rel)?;
    devall_core::fsutil::atomic_write(&path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Moves a protocol file to the OS trash.
#[tauri::command]
pub async fn mock_protocol_delete(root: String, rel: String) -> Result<(), String> {
    let path = protocol_path(&root, &rel)?;
    if path.exists() {
        trash::delete(&path).map_err(|e| format!("cannot move {} to the trash: {e}", path.display()))?;
    }
    Ok(())
}

fn protocol_path(root: &str, rel: &str) -> Result<std::path::PathBuf, String> {
    let norm = devall_core::fsutil::normalize_rel(rel);
    let ok = ["graphql", "websocket", "grpc", "mqtt"].iter().any(|p| norm.starts_with(&format!("mocks/{p}/")));
    if !ok {
        return Err(format!("{rel} is not inside mocks/graphql, mocks/websocket, mocks/grpc or mocks/mqtt"));
    }
    devall_core::fsutil::safe_join(Path::new(root), &norm).map_err(|e| e.to_string())
}
