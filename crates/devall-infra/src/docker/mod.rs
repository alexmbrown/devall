//! Docker Engine API client (also Podman's compatible API) over the local socket / named pipe.

pub mod socket;
mod stream;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use bollard::Docker;
use bollard::query_parameters::{
    ListContainersOptions, ListImagesOptions, ListNetworksOptions, ListVolumesOptions, RemoveContainerOptions, RestartContainerOptions,
    StopContainerOptions,
};
use serde::{Deserialize, Serialize};

pub use socket::{SocketCandidate, candidates};
pub use stream::{DockerLogOptions, StatsSample, compute_stats};

use crate::error::{Error, Result};
use crate::sys;

const CONNECT_TIMEOUT_SECS: u64 = 120;
const PING_TIMEOUT: Duration = Duration::from_secs(3);

pub const COMPOSE_PROJECT: &str = "com.docker.compose.project";
pub const COMPOSE_SERVICE: &str = "com.docker.compose.service";
pub const COMPOSE_WORKING_DIR: &str = "com.docker.compose.project.working_dir";
pub const COMPOSE_CONFIG_FILES: &str = "com.docker.compose.project.config_files";

/// A connected engine.
#[derive(Clone)]
pub struct DockerClient {
    docker: Docker,
    pub endpoint: SocketCandidate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub host: String,
    pub source: String,
    /// `Docker`, `Podman`, … (from the version components).
    pub engine: String,
    pub version: Option<String>,
    pub api_version: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
}

fn connect_host(host: &str) -> Result<Docker> {
    let version = bollard::API_DEFAULT_VERSION;
    #[cfg(unix)]
    if host.starts_with("unix://") {
        return Ok(Docker::connect_with_unix(host, CONNECT_TIMEOUT_SECS, version)?);
    }
    #[cfg(windows)]
    if host.starts_with("npipe://") {
        return Ok(Docker::connect_with_named_pipe(host, CONNECT_TIMEOUT_SECS, version)?);
    }
    if host.starts_with("tcp://") || host.starts_with("http://") {
        return Ok(Docker::connect_with_http(host, CONNECT_TIMEOUT_SECS, version)?);
    }
    Err(Error::Docker(format!(
        "unsupported engine address {host} (supported: unix://, npipe://, tcp:// without TLS). For remote or TLS engines, forward the socket locally (e.g. `ssh -L`)."
    )))
}

impl DockerClient {
    /// Connects to `host` (any `DOCKER_HOST` form), or auto-detects the first reachable engine.
    pub async fn connect(host: Option<&str>) -> Result<Self> {
        if let Some(h) = host.map(str::trim).filter(|h| !h.is_empty()) {
            let cand = socket::candidates().into_iter().find(|c| c.host == h).unwrap_or(SocketCandidate {
                host: h.to_string(),
                source: "custom".into(),
                exists: true,
            });
            return Self::try_candidate(cand).await;
        }
        let all = socket::candidates();
        let mut tried = Vec::new();
        for cand in all.iter().filter(|c| c.exists) {
            match Self::try_candidate(cand.clone()).await {
                Ok(c) => return Ok(c),
                Err(e) => tried.push(format!("{} ({}): {e}", cand.host, cand.source)),
            }
        }
        let missing: Vec<String> = all.iter().filter(|c| !c.exists).map(|c| format!("{} ({})", c.host, c.source)).collect();
        let mut msg = String::new();
        if !tried.is_empty() {
            msg.push_str(&tried.join("; "));
            msg.push_str(". ");
        }
        msg.push_str(&format!(
            "Not present: {}. Start Docker Desktop, Podman, OrbStack, colima or Rancher Desktop, or set DOCKER_HOST.",
            missing.join(", ")
        ));
        Err(Error::NoEngine(msg))
    }

    async fn try_candidate(cand: SocketCandidate) -> Result<Self> {
        let docker = connect_host(&cand.host)?;
        match tokio::time::timeout(PING_TIMEOUT, docker.ping()).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err(Error::Docker("no answer within 3 s".into())),
        }
        // Older Podman/Docker engines speak older API versions.
        let docker = docker.negotiate_version().await.map_err(Error::from)?;
        Ok(Self { docker, endpoint: cand })
    }

    pub fn raw(&self) -> &Docker {
        &self.docker
    }

    pub async fn info(&self) -> Result<EngineInfo> {
        let v = self.docker.version().await?;
        let engine = v
            .components
            .as_ref()
            .and_then(|c| c.iter().find(|c| c.name.to_lowercase().contains("engine")).map(|c| c.name.clone()))
            .or_else(|| v.platform.as_ref().map(|p| p.name.clone()))
            .map(|n| if n.to_lowercase().contains("podman") { "Podman".to_string() } else { "Docker".to_string() })
            .unwrap_or_else(|| "Docker".into());
        Ok(EngineInfo {
            host: self.endpoint.host.clone(),
            source: self.endpoint.source.clone(),
            engine,
            version: v.version,
            api_version: v.api_version,
            os: v.os,
            arch: v.arch,
        })
    }

    // ---------- lists ----------

    pub async fn containers(&self, all: bool) -> Result<Vec<ContainerRow>> {
        let list = self.docker.list_containers(Some(ListContainersOptions { all, ..Default::default() })).await?;
        let mut rows: Vec<ContainerRow> = list.into_iter().map(ContainerRow::from_summary).collect();
        rows.sort_by(|a, b| (a.compose_project.is_none(), &a.compose_project, &a.name).cmp(&(b.compose_project.is_none(), &b.compose_project, &b.name)));
        Ok(rows)
    }

    pub async fn images(&self) -> Result<Vec<ImageRow>> {
        let list = self.docker.list_images(Some(ListImagesOptions { all: false, ..Default::default() })).await?;
        let mut rows: Vec<ImageRow> = list
            .into_iter()
            .map(|i| ImageRow {
                short_id: short_id(&i.id),
                id: i.id,
                tags: i.repo_tags.into_iter().filter(|t| t != "<none>:<none>").collect(),
                created: i.created,
                size: i.size,
                containers: i.containers,
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.created));
        Ok(rows)
    }

    pub async fn volumes(&self) -> Result<Vec<VolumeRow>> {
        let resp = self.docker.list_volumes(None::<ListVolumesOptions>).await?;
        let mut rows: Vec<VolumeRow> = resp
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| VolumeRow {
                compose_project: v.labels.get(COMPOSE_PROJECT).cloned(),
                name: v.name,
                driver: v.driver,
                mountpoint: v.mountpoint,
                created: v.created_at,
            })
            .collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rows)
    }

    pub async fn networks(&self) -> Result<Vec<NetworkRow>> {
        let list = self.docker.list_networks(None::<ListNetworksOptions>).await?;
        let mut rows: Vec<NetworkRow> = list
            .into_iter()
            .map(|n| NetworkRow {
                id: n.id.unwrap_or_default(),
                name: n.name.unwrap_or_default(),
                driver: n.driver,
                scope: n.scope,
                internal: n.internal.unwrap_or(false),
                compose_project: n.labels.as_ref().and_then(|l| l.get(COMPOSE_PROJECT).cloned()),
            })
            .collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rows)
    }

    /// Compose projects reconstructed from container labels.
    pub async fn compose_projects(&self) -> Result<Vec<ComposeProject>> {
        let list = self.docker.list_containers(Some(ListContainersOptions { all: true, ..Default::default() })).await?;
        Ok(group_compose(&list))
    }

    // ---------- container actions ----------

    pub async fn inspect(&self, id: &str) -> Result<serde_json::Value> {
        let c = self.docker.inspect_container(id, None).await?;
        Ok(serde_json::to_value(c)?)
    }

    pub async fn start(&self, id: &str) -> Result<()> {
        self.docker.start_container(id, None).await?;
        Ok(())
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        self.docker.stop_container(id, Some(StopContainerOptions { t: Some(10), signal: None })).await?;
        Ok(())
    }

    pub async fn restart(&self, id: &str) -> Result<()> {
        self.docker.restart_container(id, Some(RestartContainerOptions { t: Some(10), signal: None })).await?;
        Ok(())
    }

    /// Removes a container (the UI confirms first). `force` also removes a running one.
    pub async fn remove(&self, id: &str, force: bool) -> Result<()> {
        self.docker.remove_container(id, Some(RemoveContainerOptions { force, v: false, link: false })).await?;
        Ok(())
    }

    /// Host port published for `container_port/tcp` of a container (by id or name).
    pub async fn published_port(&self, container: &str, container_port: u16) -> Result<u16> {
        let c = self.docker.inspect_container(container, None).await?;
        let running = c.state.as_ref().and_then(|s| s.running).unwrap_or(false);
        if !running {
            return Err(Error::Invalid(format!("container {container} is not running")));
        }
        let ports = c.network_settings.and_then(|n| n.ports).unwrap_or_default();
        for key in [format!("{container_port}/tcp"), format!("{container_port}")] {
            if let Some(Some(bindings)) = ports.get(&key)
                && let Some(port) = bindings.iter().filter_map(|b| b.host_port.as_deref()).find_map(|p| p.parse::<u16>().ok())
            {
                return Ok(port);
            }
        }
        Err(Error::NotFound(format!("a published host port for {container}:{container_port} (publish it with -p or `ports:`)")))
    }

    // ---------- compose (client-side tool: shell out) ----------

    /// Runs `docker compose -p <project> [-f …] <args>` in the project's working directory,
    /// against the same engine we're connected to. Returns combined output.
    pub async fn compose(&self, project: &ComposeProject, args: &[&str]) -> Result<String> {
        let (program, prefix) = compose_cli()?;
        let mut full: Vec<String> = prefix;
        full.extend(["-p".to_string(), project.name.clone()]);
        for f in &project.config_files {
            full.extend(["-f".to_string(), f.clone()]);
        }
        full.extend(args.iter().map(|a| a.to_string()));
        let cwd = project.working_dir.as_ref().map(PathBuf::from).filter(|p| p.is_dir());
        let envs = vec![("DOCKER_HOST".to_string(), self.endpoint.host.clone())];
        let out = sys::run_capture(&program, &full, cwd.as_deref(), &envs, Duration::from_secs(600)).await?;
        let text = format!("{}{}", out.stdout, out.stderr);
        if out.success {
            Ok(text)
        } else {
            Err(Error::Command { command: format!("{} {}", program.display(), full.join(" ")), message: text.trim().to_string() })
        }
    }
}

/// `docker compose` (plugin), else `docker-compose`, else `podman compose`.
fn compose_cli() -> Result<(PathBuf, Vec<String>)> {
    if let Some(d) = sys::which("docker") {
        return Ok((d, vec!["compose".into()]));
    }
    if let Some(d) = sys::which("docker-compose") {
        return Ok((d, vec![]));
    }
    if let Some(p) = sys::which("podman") {
        return Ok((p, vec!["compose".into()]));
    }
    Err(Error::ToolMissing("the docker CLI was not found (needed for compose up/down). Install Docker CLI or Podman and restart /dev/all.".into()))
}

fn short_id(id: &str) -> String {
    id.trim_start_matches("sha256:").chars().take(12).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortMapping {
    pub private_port: u16,
    pub public_port: Option<u16>,
    pub ip: Option<String>,
    pub protocol: String,
    /// `http://localhost:<public>` for published TCP ports.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRow {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub image: String,
    pub command: Option<String>,
    /// Unix seconds.
    pub created: Option<i64>,
    /// `running`, `exited`, `paused`, `restarting`, `created`, `dead`, …
    pub state: String,
    /// Human status, e.g. `Up 3 minutes (healthy)`.
    pub status: String,
    pub health: Option<String>,
    pub ports: Vec<PortMapping>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    pub networks: Vec<String>,
}

impl ContainerRow {
    pub fn from_summary(c: bollard::models::ContainerSummary) -> Self {
        let id = c.id.unwrap_or_default();
        let name = c.names.as_ref().and_then(|n| n.first()).map(|n| n.trim_start_matches('/').to_string()).unwrap_or_else(|| short_id(&id));
        let labels = c.labels.unwrap_or_default();
        Self {
            short_id: short_id(&id),
            id,
            name,
            image: c.image.unwrap_or_default(),
            command: c.command,
            created: c.created,
            state: c.state.map(|s| s.to_string()).unwrap_or_default(),
            status: c.status.unwrap_or_default(),
            health: c.health.and_then(|h| h.status).map(|s| s.to_string()).filter(|s| !s.is_empty() && s != "none"),
            ports: port_mappings(c.ports.unwrap_or_default()),
            compose_project: labels.get(COMPOSE_PROJECT).cloned(),
            compose_service: labels.get(COMPOSE_SERVICE).cloned(),
            networks: c.network_settings.and_then(|n| n.networks).map(|n| n.into_keys().collect()).unwrap_or_default(),
        }
    }
}

/// Normalizes Docker's port list: drops IPv6 duplicates of IPv4 bindings, adds URLs.
pub fn port_mappings(ports: Vec<bollard::models::PortSummary>) -> Vec<PortMapping> {
    let mut out: Vec<PortMapping> = Vec::new();
    for p in ports {
        let protocol = p.typ.map(|t| t.to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| "tcp".into());
        let url = match (p.public_port, protocol.as_str()) {
            (Some(public), "tcp") => {
                let host = match p.ip.as_deref() {
                    None | Some("") | Some("0.0.0.0") | Some("::") => "localhost".to_string(),
                    Some(ip) if ip.contains(':') => format!("[{ip}]"),
                    Some(ip) => ip.to_string(),
                };
                Some(format!("http://{host}:{public}"))
            }
            _ => None,
        };
        let m = PortMapping { private_port: p.private_port, public_port: p.public_port, ip: p.ip, protocol, url };
        let dup = out.iter().any(|o| o.private_port == m.private_port && o.public_port == m.public_port && o.protocol == m.protocol && o.url == m.url);
        if !dup {
            out.push(m);
        }
    }
    out.sort_by_key(|m| (m.public_port.is_none(), m.private_port));
    out
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRow {
    pub id: String,
    pub short_id: String,
    pub tags: Vec<String>,
    pub created: i64,
    pub size: i64,
    /// Containers using the image (-1 when the engine didn't compute it).
    pub containers: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeRow {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub created: Option<String>,
    pub compose_project: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkRow {
    pub id: String,
    pub name: String,
    pub driver: Option<String>,
    pub scope: Option<String>,
    pub internal: bool,
    pub compose_project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeProject {
    pub name: String,
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
    pub services: Vec<String>,
    pub containers: usize,
    pub running: usize,
}

pub fn group_compose(list: &[bollard::models::ContainerSummary]) -> Vec<ComposeProject> {
    let mut map: BTreeMap<String, ComposeProject> = BTreeMap::new();
    let empty = HashMap::new();
    for c in list {
        let labels = c.labels.as_ref().unwrap_or(&empty);
        let Some(project) = labels.get(COMPOSE_PROJECT) else { continue };
        let entry = map.entry(project.clone()).or_insert_with(|| ComposeProject {
            name: project.clone(),
            working_dir: None,
            config_files: Vec::new(),
            services: Vec::new(),
            containers: 0,
            running: 0,
        });
        entry.containers += 1;
        if c.state == Some(bollard::models::ContainerSummaryStateEnum::RUNNING) {
            entry.running += 1;
        }
        if entry.working_dir.is_none() {
            entry.working_dir = labels.get(COMPOSE_WORKING_DIR).cloned();
        }
        if entry.config_files.is_empty()
            && let Some(files) = labels.get(COMPOSE_CONFIG_FILES)
        {
            entry.config_files = files.split(',').map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect();
        }
        if let Some(s) = labels.get(COMPOSE_SERVICE)
            && !entry.services.contains(s)
        {
            entry.services.push(s.clone());
        }
    }
    let mut out: Vec<ComposeProject> = map.into_values().collect();
    for p in &mut out {
        p.services.sort();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{ContainerSummary, ContainerSummaryStateEnum, PortSummary, PortSummaryTypeEnum};

    fn summary(name: &str, project: Option<&str>, service: &str, running: bool) -> ContainerSummary {
        let mut labels = HashMap::new();
        if let Some(p) = project {
            labels.insert(COMPOSE_PROJECT.to_string(), p.to_string());
            labels.insert(COMPOSE_SERVICE.to_string(), service.to_string());
            labels.insert(COMPOSE_WORKING_DIR.to_string(), format!("/src/{p}"));
            labels.insert(COMPOSE_CONFIG_FILES.to_string(), format!("/src/{p}/compose.yaml, /src/{p}/compose.override.yaml"));
        }
        ContainerSummary {
            id: Some(format!("{name}0123456789abcdef")),
            names: Some(vec![format!("/{name}")]),
            image: Some("nginx:1.27".into()),
            labels: Some(labels),
            state: Some(if running { ContainerSummaryStateEnum::RUNNING } else { ContainerSummaryStateEnum::EXITED }),
            status: Some("Up 2 minutes".into()),
            ..Default::default()
        }
    }

    #[test]
    fn groups_compose_projects() {
        let list = vec![
            summary("shop-api-1", Some("shop"), "api", true),
            summary("shop-db-1", Some("shop"), "db", false),
            summary("loose", None, "", true),
        ];
        let g = group_compose(&list);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].name, "shop");
        assert_eq!(g[0].services, vec!["api", "db"]);
        assert_eq!((g[0].containers, g[0].running), (2, 1));
        assert_eq!(g[0].working_dir.as_deref(), Some("/src/shop"));
        assert_eq!(g[0].config_files, vec!["/src/shop/compose.yaml", "/src/shop/compose.override.yaml"]);
    }

    #[test]
    fn container_row_from_summary() {
        let mut s = summary("shop-api-1", Some("shop"), "api", true);
        s.ports = Some(vec![
            PortSummary { ip: Some("0.0.0.0".into()), private_port: 8080, public_port: Some(18080), typ: Some(PortSummaryTypeEnum::TCP) },
            PortSummary { ip: Some("::".into()), private_port: 8080, public_port: Some(18080), typ: Some(PortSummaryTypeEnum::TCP) },
            PortSummary { ip: Some("127.0.0.1".into()), private_port: 9229, public_port: Some(9229), typ: Some(PortSummaryTypeEnum::TCP) },
            PortSummary { ip: None, private_port: 5353, public_port: None, typ: Some(PortSummaryTypeEnum::UDP) },
        ]);
        let row = ContainerRow::from_summary(s);
        assert_eq!(row.name, "shop-api-1");
        assert_eq!(row.short_id.len(), 12);
        assert_eq!(row.state, "running");
        assert_eq!(row.compose_service.as_deref(), Some("api"));
        assert_eq!(row.ports.len(), 3, "IPv6 duplicate dropped: {:?}", row.ports);
        assert_eq!(row.ports[0].url.as_deref(), Some("http://localhost:18080"));
        assert_eq!(row.ports[1].url.as_deref(), Some("http://127.0.0.1:9229"));
        assert_eq!(row.ports[2].url, None);
        assert_eq!(row.ports[2].protocol, "udp");
    }

    /// Talks to a real engine; only with `DEVALL_TEST_DOCKER=1`. Read-only.
    #[tokio::test]
    async fn live_engine_lists() {
        if std::env::var("DEVALL_TEST_DOCKER").as_deref() != Ok("1") {
            return;
        }
        let client = DockerClient::connect(None).await.expect("connect to an engine");
        let info = client.info().await.unwrap();
        assert!(!info.engine.is_empty());
        client.containers(true).await.unwrap();
        client.images().await.unwrap();
        client.volumes().await.unwrap();
        client.networks().await.unwrap();
        client.compose_projects().await.unwrap();
    }
}
