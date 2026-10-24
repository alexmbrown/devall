//! Running forwards: a loopback listener per Kubernetes forward (one API port-forward stream per
//! accepted connection, like kubectl), re-resolution of the backing pod when it restarts or is
//! replaced, and published-port tracking for Docker containers.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::Client;
use kube::api::{Api, ListParams};
use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{AbortHandle, JoinSet};

use super::overrides::{OverrideEntry, RuntimeOverrides, norm_root};
use super::template::{DEFAULT_TEMPLATE, TemplateCtx, render_template};
use super::{ForwardFile, ForwardSource, ForwardTarget, TargetKind};
use crate::docker::DockerClient;
use crate::error::{Error, Result};
use crate::kube::KubeConn;
use crate::kube::resources::{KindRef, dyn_api, to_json};

const HEALTH_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ForwardState {
    Starting,
    Active,
    /// The backing pod/container went away; waiting for a replacement (the listener stays up).
    Reconnecting,
    Error,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingStatus {
    pub environment: Option<String>,
    pub variable: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardStatus {
    pub id: String,
    pub root: String,
    pub name: String,
    pub source: ForwardSource,
    pub state: ForwardState,
    /// Local port (Kubernetes) or the container's published host port (Docker).
    pub local_port: Option<u16>,
    pub url: Option<String>,
    /// Pod or container currently behind the forward.
    pub backend: Option<String>,
    pub namespace: Option<String>,
    pub context: Option<String>,
    pub connections: u64,
    pub active_connections: u64,
    /// Times the forward switched to a new pod / port after a restart.
    pub reconnects: u64,
    pub last_error: Option<String>,
    pub started_at: i64,
    pub binding: Option<BindingStatus>,
    /// Started from a saved `connections/*.forward.yaml`.
    pub saved: bool,
    pub spec: ForwardFile,
}

struct Running {
    status: Arc<Mutex<ForwardStatus>>,
    abort: AbortHandle,
}

/// State shared between a forward's tasks.
struct Shared {
    status: Arc<Mutex<ForwardStatus>>,
    overrides: RuntimeOverrides,
    root: String,
    spec: ForwardFile,
    namespace: String,
    context: String,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut ForwardStatus)) {
        f(&mut self.status.lock().unwrap());
    }

    fn error(&self, msg: impl Into<String>) {
        let msg = msg.into();
        self.update(|s| s.last_error = Some(msg));
    }

    /// Renders the bound variable for `port` and publishes it as a runtime override.
    fn publish(&self, port: u16) {
        let target = format!("{}/{}", kind_word(self.spec.target.kind), self.spec.target.name);
        let ctx = TemplateCtx {
            port,
            host: "127.0.0.1",
            remote_port: self.spec.remote_port,
            name: &self.spec.target.name,
            namespace: &self.namespace,
            target: &target,
            context: &self.context,
        };
        let url = render_template(DEFAULT_TEMPLATE, &ctx);
        // The id can change while running (an ad-hoc forward saved to a file is adopted).
        let id = self.status.lock().unwrap().id.clone();
        let binding = self.spec.bind_environment.as_ref().map(|b| {
            let value = render_template(b.template.as_deref().unwrap_or(DEFAULT_TEMPLATE), &ctx);
            self.overrides.set(OverrideEntry {
                root: self.root.clone(),
                environment: b.environment.clone().filter(|e| !e.is_empty()),
                variable: b.variable.clone(),
                value: value.clone(),
                forward_id: id.clone(),
                forward_name: self.spec.name.clone(),
                since: now_ms(),
            });
            BindingStatus { environment: b.environment.clone().filter(|e| !e.is_empty()), variable: b.variable.clone(), value }
        });
        self.update(|s| {
            s.local_port = Some(port);
            s.url = Some(url);
            s.binding = binding;
        });
    }
}

fn kind_word(k: TargetKind) -> &'static str {
    match k {
        TargetKind::Pod => "pod",
        TargetKind::Service => "svc",
        TargetKind::Deployment => "deploy",
        TargetKind::StatefulSet => "sts",
        TargetKind::DaemonSet => "ds",
        TargetKind::ReplicaSet => "rs",
        TargetKind::Container => "container",
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Running forwards, keyed by project root + forward id. Lives as long as the app, so forwards
/// keep running while the user navigates elsewhere.
pub struct ForwardManager {
    overrides: RuntimeOverrides,
    running: Mutex<HashMap<(String, String), Running>>,
}

impl Default for ForwardManager {
    fn default() -> Self {
        Self::new(RuntimeOverrides::new())
    }
}

impl Drop for ForwardManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

impl ForwardManager {
    pub fn new(overrides: RuntimeOverrides) -> Self {
        Self { overrides, running: Mutex::new(HashMap::new()) }
    }

    /// The override table the HTTP layer reads.
    pub fn overrides(&self) -> RuntimeOverrides {
        self.overrides.clone()
    }

    fn key(root: &str, id: &str) -> (String, String) {
        (norm_root(root), id.to_string())
    }

    fn base_status(root: &str, id: &str, spec: &ForwardFile, saved: bool, namespace: Option<String>, context: Option<String>) -> ForwardStatus {
        ForwardStatus {
            id: id.into(),
            root: root.into(),
            name: spec.name.clone(),
            source: spec.source,
            state: ForwardState::Starting,
            local_port: None,
            url: None,
            backend: None,
            namespace,
            context,
            connections: 0,
            active_connections: 0,
            reconnects: 0,
            last_error: None,
            started_at: now_ms(),
            binding: None,
            saved,
            spec: spec.clone(),
        }
    }

    fn register(&self, root: &str, id: &str, status: Arc<Mutex<ForwardStatus>>, abort: AbortHandle) {
        self.running.lock().unwrap().insert(Self::key(root, id), Running { status, abort });
    }

    /// Starts (or restarts) a Kubernetes forward. Fails fast when the target can't be resolved or
    /// the local port is taken.
    pub async fn start_kube(&self, root: &str, id: &str, spec: ForwardFile, conn: KubeConn, saved: bool) -> Result<ForwardStatus> {
        spec.validate()?;
        if spec.source != ForwardSource::Kubernetes {
            return Err(Error::Invalid("not a Kubernetes forward".into()));
        }
        self.stop(root, id);
        let ns = spec.namespace.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| conn.default_namespace.clone());
        let (pod, port) = resolve_backend(&conn.client, &ns, &spec.target, spec.remote_port).await?;
        let want = spec.local_port.unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", want)).await.map_err(|e| Error::Invalid(format!("cannot listen on 127.0.0.1:{want}: {e}")))?;
        let local_port = listener.local_addr()?.port();

        let mut st = Self::base_status(root, id, &spec, saved, Some(ns.clone()), Some(conn.context.clone()));
        st.state = ForwardState::Active;
        st.backend = Some(pod.clone());
        let status = Arc::new(Mutex::new(st));
        let shared = Arc::new(Shared {
            status: status.clone(),
            overrides: self.overrides.clone(),
            root: root.into(),
            spec,
            namespace: ns.clone(),
            context: conn.context.clone(),
        });
        shared.publish(local_port);
        let backend = Arc::new(Mutex::new(Some((pod, port))));
        let task = tokio::spawn(run_kube(shared, conn.client, ns, listener, backend));
        self.register(root, id, status.clone(), task.abort_handle());
        let snapshot = status.lock().unwrap().clone();
        Ok(snapshot)
    }

    /// Binds a Docker container's published port (no proxy needed) and keeps following it: a
    /// container restarted with a new ephemeral port updates the bound variable.
    pub async fn start_docker(&self, root: &str, id: &str, spec: ForwardFile, docker: DockerClient, saved: bool) -> Result<ForwardStatus> {
        spec.validate()?;
        if spec.source != ForwardSource::Docker {
            return Err(Error::Invalid("not a Docker forward".into()));
        }
        self.stop(root, id);
        let port = docker.published_port(&spec.target.name, spec.remote_port).await?;
        let mut st = Self::base_status(root, id, &spec, saved, None, Some(docker.endpoint.source.clone()));
        st.state = ForwardState::Active;
        st.backend = Some(spec.target.name.clone());
        let status = Arc::new(Mutex::new(st));
        let shared = Arc::new(Shared {
            status: status.clone(),
            overrides: self.overrides.clone(),
            root: root.into(),
            spec,
            namespace: String::new(),
            context: "docker".into(),
        });
        shared.publish(port);
        let task = tokio::spawn(run_docker(shared, docker, port));
        self.register(root, id, status.clone(), task.abort_handle());
        let snapshot = status.lock().unwrap().clone();
        Ok(snapshot)
    }

    /// Stops a forward and removes its runtime override. Returns whether it was running.
    pub fn stop(&self, root: &str, id: &str) -> bool {
        let removed = self.running.lock().unwrap().remove(&Self::key(root, id));
        self.overrides.clear(root, id);
        match removed {
            Some(r) => {
                r.abort.abort();
                true
            }
            None => false,
        }
    }

    /// Re-keys a running forward (an ad-hoc forward the user just saved as `new_id`), keeping its
    /// listener, port and bound variable.
    pub fn adopt(&self, root: &str, old_id: &str, new_id: &str, spec: Option<ForwardFile>) -> bool {
        let mut map = self.running.lock().unwrap();
        let Some(r) = map.remove(&Self::key(root, old_id)) else { return false };
        {
            let mut s = r.status.lock().unwrap();
            s.id = new_id.to_string();
            s.saved = true;
            if let Some(spec) = spec {
                s.name = spec.name.clone();
                s.spec = spec;
            }
        }
        if let Some(mut e) = self.overrides.get(root, old_id) {
            self.overrides.clear(root, old_id);
            e.forward_id = new_id.to_string();
            self.overrides.set(e);
        }
        map.insert(Self::key(root, new_id), r);
        true
    }

    pub fn stop_all(&self) {
        let all: Vec<((String, String), Running)> = self.running.lock().unwrap().drain().collect();
        for ((root, id), r) in all {
            r.abort.abort();
            self.overrides.clear(&root, &id);
        }
    }

    pub fn status(&self, root: &str, id: &str) -> Option<ForwardStatus> {
        self.running.lock().unwrap().get(&Self::key(root, id)).map(|r| r.status.lock().unwrap().clone())
    }

    /// Running forwards (of `root`, when given), oldest first.
    pub fn list(&self, root: Option<&str>) -> Vec<ForwardStatus> {
        let want = root.map(norm_root);
        let mut v: Vec<ForwardStatus> = self
            .running
            .lock()
            .unwrap()
            .iter()
            .filter(|((r, _), _)| want.as_ref().is_none_or(|w| w == r))
            .map(|(_, r)| r.status.lock().unwrap().clone())
            .collect();
        v.sort_by_key(|s| s.started_at);
        v
    }
}

// ---------- Kubernetes ----------

type Backend = Arc<Mutex<Option<(String, u16)>>>;

async fn run_kube(shared: Arc<Shared>, client: Client, ns: String, listener: TcpListener, backend: Backend) {
    let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
    let accept = async {
        let mut conns: JoinSet<()> = JoinSet::new();
        loop {
            tokio::select! {
                res = listener.accept() => match res {
                    Ok((tcp, _)) => {
                        let _ = tcp.set_nodelay(true);
                        shared.update(|s| { s.connections += 1; s.active_connections += 1; });
                        let (shared, client, pods, ns, backend) = (shared.clone(), client.clone(), pods.clone(), ns.clone(), backend.clone());
                        conns.spawn(async move {
                            if let Err(e) = serve_conn(&shared, &client, &pods, &ns, &backend, tcp).await {
                                shared.error(e.to_string());
                            }
                            shared.update(|s| s.active_connections = s.active_connections.saturating_sub(1));
                        });
                    }
                    Err(e) => {
                        shared.error(format!("accept failed: {e}"));
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                },
                Some(_) = conns.join_next(), if !conns.is_empty() => {}
            }
        }
    };
    let health = async {
        loop {
            tokio::time::sleep(HEALTH_EVERY).await;
            check_backend(&shared, &client, &pods, &ns, &backend).await;
        }
    };
    tokio::select! {
        _ = accept => {}
        _ = health => {}
    }
}

/// The backend to use now, re-resolving when the previous one was dropped.
async fn current_backend(shared: &Shared, client: &Client, ns: &str, backend: &Backend) -> Result<(String, u16)> {
    if let Some(b) = backend.lock().unwrap().clone() {
        return Ok(b);
    }
    let found = resolve_backend(client, ns, &shared.spec.target, shared.spec.remote_port).await;
    match &found {
        Ok((pod, port)) => {
            *backend.lock().unwrap() = Some((pod.clone(), *port));
            shared.update(|s| {
                if s.backend.as_deref() != Some(pod.as_str()) {
                    s.reconnects += 1;
                }
                s.backend = Some(pod.clone());
                s.state = ForwardState::Active;
            });
        }
        Err(e) => {
            let msg = e.to_string();
            shared.update(|s| {
                s.state = ForwardState::Reconnecting;
                s.last_error = Some(msg);
            });
        }
    }
    found
}

async fn serve_conn(shared: &Shared, client: &Client, pods: &Api<Pod>, ns: &str, backend: &Backend, mut tcp: TcpStream) -> Result<()> {
    let mut last_err: Option<Error> = None;
    // Second attempt after re-resolving, so a request that arrives just after a pod restart
    // reaches the replacement instead of failing.
    for _ in 0..2 {
        let (pod, port) = current_backend(shared, client, ns, backend).await?;
        let mut pf = match pods.portforward(&pod, &[port]).await {
            Ok(pf) => pf,
            Err(e) => {
                *backend.lock().unwrap() = None;
                last_err = Some(e.into());
                continue;
            }
        };
        let mut upstream = pf.take_stream(port).ok_or_else(|| Error::Kube("port-forward stream unavailable".into()))?;
        let port_error = pf.take_error(port);
        let copied = tokio::io::copy_bidirectional(&mut tcp, &mut upstream).await;
        drop(upstream);
        let _ = pf.join().await;
        if let Some(f) = port_error
            && let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(100), f).await
        {
            shared.error(format!("{pod}:{port}: {msg}"));
        }
        return copied.map(|_| ()).or_else(|e| match e.kind() {
            // Clients hanging up mid-response are normal.
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionAborted => Ok(()),
            _ => Err(Error::Io(e)),
        });
    }
    Err(last_err.unwrap_or_else(|| Error::Kube("port-forward failed".into())))
}

async fn check_backend(shared: &Shared, client: &Client, pods: &Api<Pod>, ns: &str, backend: &Backend) {
    let current = backend.lock().unwrap().clone();
    if let Some((pod, _)) = &current {
        match pods.get_opt(pod).await {
            Ok(Some(p)) if pod_usable(&p) => {
                shared.update(|s| {
                    if s.state != ForwardState::Active {
                        s.state = ForwardState::Active;
                    }
                });
                return;
            }
            Ok(_) => {
                *backend.lock().unwrap() = None;
                shared.update(|s| s.state = ForwardState::Reconnecting);
            }
            Err(e) => {
                // API unreachable (network blip, expired token): keep the pod, report it.
                let msg = Error::from(e).to_string();
                shared.update(|s| {
                    s.state = ForwardState::Reconnecting;
                    s.last_error = Some(msg);
                });
                return;
            }
        }
    }
    let _ = current_backend(shared, client, ns, backend).await;
}

// ---------- Docker ----------

async fn run_docker(shared: Arc<Shared>, docker: DockerClient, mut port: u16) {
    loop {
        tokio::time::sleep(HEALTH_EVERY).await;
        match docker.published_port(&shared.spec.target.name, shared.spec.remote_port).await {
            Ok(p) => {
                if p != port {
                    port = p;
                    shared.publish(p);
                    shared.update(|s| s.reconnects += 1);
                }
                shared.update(|s| s.state = ForwardState::Active);
            }
            Err(e) => {
                let msg = e.to_string();
                shared.update(|s| {
                    s.state = ForwardState::Reconnecting;
                    s.last_error = Some(msg);
                });
            }
        }
    }
}

// ---------- target resolution ----------

fn pod_deleting(p: &Pod) -> bool {
    p.metadata.deletion_timestamp.is_some()
}

fn pod_running(p: &Pod) -> bool {
    p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running")
}

fn pod_ready(p: &Pod) -> bool {
    p.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|c| c.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
}

/// Whether a pod can serve a port-forward.
pub fn pod_usable(p: &Pod) -> bool {
    pod_running(p) && !pod_deleting(p)
}

/// The pod a forward should use: running and not terminating, ready ones first, then by name
/// (stable choice between replicas).
pub fn pick_pod(pods: &[Pod]) -> Option<&Pod> {
    let mut usable: Vec<&Pod> = pods.iter().filter(|p| pod_usable(p)).collect();
    usable.sort_by_key(|p| (!pod_ready(p), p.metadata.name.clone().unwrap_or_default()));
    usable.into_iter().next()
}

/// `a=b,c=d` for a label selector map.
pub fn selector_string(labels: &BTreeMap<String, String>) -> String {
    labels.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TargetPort {
    Number(u16),
    Named(String),
}

fn service_target_port(svc: &Service, port: u16) -> Result<TargetPort> {
    let name = svc.metadata.name.clone().unwrap_or_default();
    let ports = svc.spec.as_ref().and_then(|s| s.ports.clone()).unwrap_or_default();
    let p = ports.iter().find(|p| p.port == i32::from(port)).ok_or_else(|| {
        let have: Vec<String> = ports.iter().map(|p| p.port.to_string()).collect();
        Error::Invalid(format!("service {name} has no port {port} (ports: {})", if have.is_empty() { "none".into() } else { have.join(", ") }))
    })?;
    Ok(match &p.target_port {
        None => TargetPort::Number(port),
        Some(IntOrString::Int(n)) => TargetPort::Number(u16::try_from(*n).map_err(|_| Error::Invalid(format!("bad targetPort {n}")))?),
        Some(IntOrString::String(s)) => match s.parse::<u16>() {
            Ok(n) => TargetPort::Number(n),
            Err(_) => TargetPort::Named(s.clone()),
        },
    })
}

fn container_port_by_name(pod: &Pod, name: &str) -> Option<u16> {
    pod.spec
        .as_ref()?
        .containers
        .iter()
        .flat_map(|c| c.ports.clone().unwrap_or_default())
        .find(|p| p.name.as_deref() == Some(name))
        .and_then(|p| u16::try_from(p.container_port).ok())
}

async fn pick_by_selector(pods: &Api<Pod>, selector: &BTreeMap<String, String>, what: &str) -> Result<Pod> {
    if selector.is_empty() {
        return Err(Error::Invalid(format!("{what} has no pod selector; forward to a pod instead")));
    }
    let sel = selector_string(selector);
    let list = pods.list(&ListParams::default().labels(&sel)).await?;
    pick_pod(&list.items).cloned().ok_or_else(|| Error::NotFound(format!("a running pod for {what} (selector {sel})")))
}

/// Resolves a forward target to `(pod, pod port)`.
pub(crate) async fn resolve_backend(client: &Client, ns: &str, target: &ForwardTarget, remote_port: u16) -> Result<(String, u16)> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    match target.kind {
        TargetKind::Pod => {
            let p = pods.get(&target.name).await?;
            if !pod_usable(&p) {
                return Err(Error::Invalid(format!("pod {} is not running", target.name)));
            }
            Ok((target.name.clone(), remote_port))
        }
        TargetKind::Service => {
            let svc: Service = Api::namespaced(client.clone(), ns).get(&target.name).await?;
            let tp = service_target_port(&svc, remote_port)?;
            let selector = svc.spec.as_ref().and_then(|s| s.selector.clone()).unwrap_or_default();
            let pod = pick_by_selector(&pods, &selector, &format!("service {}", target.name)).await?;
            let port = match tp {
                TargetPort::Number(n) => n,
                TargetPort::Named(name) => container_port_by_name(&pod, &name)
                    .ok_or_else(|| Error::Invalid(format!("pod {} has no container port named {name}", pod.metadata.name.clone().unwrap_or_default())))?,
            };
            Ok((pod.metadata.name.clone().unwrap_or_default(), port))
        }
        TargetKind::Deployment | TargetKind::StatefulSet | TargetKind::DaemonSet | TargetKind::ReplicaSet => {
            let (kind, plural) = match target.kind {
                TargetKind::Deployment => ("Deployment", "deployments"),
                TargetKind::StatefulSet => ("StatefulSet", "statefulsets"),
                TargetKind::DaemonSet => ("DaemonSet", "daemonsets"),
                _ => ("ReplicaSet", "replicasets"),
            };
            let kref = KindRef::new("apps", "v1", kind, plural, true);
            let obj = to_json(&dyn_api(client.clone(), &kref, Some(ns)).get(&target.name).await?);
            let selector: BTreeMap<String, String> = obj
                .pointer("/spec/selector/matchLabels")
                .and_then(|v| v.as_object())
                .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect())
                .unwrap_or_default();
            let pod = pick_by_selector(&pods, &selector, &format!("{} {}", kind.to_lowercase(), target.name)).await?;
            Ok((pod.metadata.name.clone().unwrap_or_default(), remote_port))
        }
        TargetKind::Container => Err(Error::Invalid("container targets are Docker forwards".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(name: &str, phase: &str, ready: bool, deleting: bool) -> Pod {
        let mut meta = json!({ "name": name });
        if deleting {
            meta["deletionTimestamp"] = json!("2026-10-21T10:00:00Z");
        }
        serde_json::from_value(json!({
            "metadata": meta,
            "spec": { "containers": [ { "name": "app", "ports": [ { "name": "http", "containerPort": 8080 }, { "name": "metrics", "containerPort": 9090 } ] } ] },
            "status": { "phase": phase, "conditions": [ { "type": "Ready", "status": if ready { "True" } else { "False" } } ] }
        }))
        .unwrap()
    }

    #[test]
    fn picks_ready_running_pods() {
        let pods = vec![
            pod("api-c", "Running", false, false),
            pod("api-a", "Running", true, true),
            pod("api-b", "Pending", false, false),
            pod("api-d", "Running", true, false),
            pod("api-e", "Running", true, false),
        ];
        assert_eq!(pick_pod(&pods).unwrap().metadata.name.as_deref(), Some("api-d"));
        // Not ready but running beats nothing.
        let only = vec![pod("api-c", "Running", false, false)];
        assert_eq!(pick_pod(&only).unwrap().metadata.name.as_deref(), Some("api-c"));
        assert!(pick_pod(&[pod("x", "Succeeded", false, false)]).is_none());
    }

    #[test]
    fn service_ports() {
        let svc: Service = serde_json::from_value(json!({
            "metadata": { "name": "api" },
            "spec": { "selector": { "app": "api", "tier": "web" }, "ports": [
                { "port": 80, "targetPort": "http" },
                { "port": 9090, "targetPort": 9090 },
                { "port": 5000 },
                { "port": 81, "targetPort": "8081" }
            ] }
        }))
        .unwrap();
        assert_eq!(service_target_port(&svc, 80).unwrap(), TargetPort::Named("http".into()));
        assert_eq!(service_target_port(&svc, 9090).unwrap(), TargetPort::Number(9090));
        assert_eq!(service_target_port(&svc, 5000).unwrap(), TargetPort::Number(5000));
        assert_eq!(service_target_port(&svc, 81).unwrap(), TargetPort::Number(8081));
        let err = service_target_port(&svc, 443).unwrap_err().to_string();
        assert!(err.contains("no port 443") && err.contains("80, 9090"), "{err}");
        let p = pod("api-1", "Running", true, false);
        assert_eq!(container_port_by_name(&p, "http"), Some(8080));
        assert_eq!(container_port_by_name(&p, "nope"), None);
        let sel = svc.spec.unwrap().selector.unwrap();
        assert_eq!(selector_string(&sel), "app=api,tier=web");
    }

    #[tokio::test]
    async fn adopt_rekeys_status_and_override() {
        let m = ForwardManager::default();
        let spec: ForwardFile = serde_yaml_ng::from_str("name: x
target: { kind: pod, name: p }
remotePort: 80
bindEnvironment: {}
").unwrap();
        let status = Arc::new(Mutex::new(ForwardManager::base_status("/p", "adhoc-1", &spec, false, None, None)));
        let task = tokio::spawn(std::future::pending::<()>());
        m.register("/p", "adhoc-1", status.clone(), task.abort_handle());
        let shared = Shared { status: status.clone(), overrides: m.overrides(), root: "/p".into(), spec, namespace: "default".into(), context: "c".into() };
        shared.publish(1234);
        assert!(m.adopt("/p", "adhoc-1", "x", None));
        assert!(m.status("/p", "adhoc-1").is_none());
        let st = m.status("/p", "x").unwrap();
        assert!(st.saved);
        assert_eq!(m.overrides().get("/p", "x").unwrap().value, "http://127.0.0.1:1234");
        assert!(m.overrides().get("/p", "adhoc-1").is_none());
        // Later re-publishes use the new id.
        shared.publish(1235);
        assert_eq!(m.overrides().list(None).len(), 1);
        assert!(m.stop("/p", "x"));
        assert!(m.overrides().list(None).is_empty());
    }

    #[tokio::test]
    async fn stop_clears_overrides_and_unknown_stop_is_false() {
        let m = ForwardManager::default();
        m.overrides().set(OverrideEntry {
            root: "/p".into(),
            environment: None,
            variable: "baseUrl".into(),
            value: "x".into(),
            forward_id: "api".into(),
            forward_name: "api".into(),
            since: 1,
        });
        assert!(!m.stop("/p", "api"));
        assert!(m.overrides().list(None).is_empty());
        assert!(m.list(None).is_empty());
    }

    #[test]
    fn publish_renders_binding_into_overrides() {
        let overrides = RuntimeOverrides::new();
        let spec: ForwardFile = serde_yaml_ng::from_str(
            "name: Orders\nnamespace: orders\ntarget: { kind: service, name: orders-api }\nremotePort: 8080\nbindEnvironment: { environment: local, variable: baseUrl, template: 'http://{host}:{port}/v1' }\n",
        )
        .unwrap();
        let status = Arc::new(Mutex::new(ForwardManager::base_status("/p", "orders", &spec, true, Some("orders".into()), Some("dev".into()))));
        let shared = Shared { status: status.clone(), overrides: overrides.clone(), root: "/p".into(), spec, namespace: "orders".into(), context: "dev".into() };
        shared.publish(18080);
        let vars = overrides.variables("/p", Some("local"));
        assert_eq!(vars.get("baseUrl").unwrap(), "http://127.0.0.1:18080/v1");
        assert!(overrides.variables("/p", Some("staging")).is_empty());
        let st = status.lock().unwrap().clone();
        assert_eq!(st.url.as_deref(), Some("http://127.0.0.1:18080"));
        assert_eq!(st.binding.unwrap().value, "http://127.0.0.1:18080/v1");
    }

    /// Forwards to a pod in the current context; only with `DEVALL_TEST_KUBE=1` and
    /// `DEVALL_TEST_KUBE_POD=<namespace>/<pod>:<port>`. Read-only.
    #[tokio::test]
    async fn live_pod_forward() {
        if std::env::var("DEVALL_TEST_KUBE").as_deref() != Ok("1") {
            return;
        }
        let Ok(spec) = std::env::var("DEVALL_TEST_KUBE_POD") else { return };
        let (ns, rest) = spec.split_once('/').expect("ns/pod:port");
        let (pod, port) = rest.split_once(':').expect("pod:port");
        let kube = crate::kube::KubeManager::default();
        let conn = kube.conn("").await.unwrap();
        let file = ForwardFile {
            version: 1,
            name: "live".into(),
            source: ForwardSource::Kubernetes,
            context: None,
            namespace: Some(ns.into()),
            target: ForwardTarget { kind: TargetKind::Pod, name: pod.into() },
            remote_port: port.parse().unwrap(),
            local_port: None,
            bind_environment: Some(super::super::EnvBinding { environment: None, variable: "baseUrl".into(), template: None }),
            extra: Default::default(),
        };
        let m = ForwardManager::default();
        let st = m.start_kube("/p", "live", file, conn, false).await.unwrap();
        let local = st.local_port.unwrap();
        assert_eq!(m.overrides().variables("/p", None).get("baseUrl").unwrap(), &format!("http://127.0.0.1:{local}"));
        let _ = TcpStream::connect(("127.0.0.1", local)).await.unwrap();
        assert!(m.stop("/p", "live"));
    }
}
