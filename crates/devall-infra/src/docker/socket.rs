//! Finding the container engine socket. We never install or replace a runtime; we connect to
//! whichever one is running: Docker Desktop, Podman (machine or rootless), OrbStack, colima,
//! Rancher Desktop, Docker Engine on Linux, or whatever `DOCKER_HOST` / the current
//! `docker context` points at.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::sys::{Os, home_dir};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SocketCandidate {
    /// `unix:///…`, `npipe:////./pipe/…` or `tcp://host:port`.
    pub host: String,
    /// Where it came from: `DOCKER_HOST`, `docker context "x"`, `Docker Desktop`, `Podman`, …
    pub source: String,
    /// Whether the socket/pipe exists right now (`tcp://` is always reported as present).
    pub exists: bool,
}

/// Inputs for detection, so it can be tested without touching the real machine.
#[derive(Debug, Clone, Default)]
pub struct DetectEnv {
    pub os: Option<Os>,
    pub docker_host: Option<String>,
    pub home: Option<PathBuf>,
    pub xdg_runtime_dir: Option<String>,
    pub uid: Option<u32>,
    pub tmpdir: Option<String>,
    /// `(context name, host)` of the active `docker context`, when not `default`.
    pub docker_context: Option<(String, String)>,
    /// Named pipes present under `\\.\pipe\` (Windows only).
    pub pipes: Vec<String>,
}

impl DetectEnv {
    /// The real environment of this process.
    pub fn current() -> Self {
        let os = Os::current();
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        DetectEnv {
            os: Some(os),
            docker_host: var("DOCKER_HOST"),
            home: home_dir(),
            xdg_runtime_dir: var("XDG_RUNTIME_DIR"),
            uid: current_uid(),
            tmpdir: var("TMPDIR"),
            docker_context: current_docker_context(),
            pipes: if os == Os::Windows { list_pipes() } else { Vec::new() },
        }
    }
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    // The owner of our home directory is a good-enough proxy for getuid() without libc.
    home_dir().and_then(|h| std::fs::metadata(h).ok()).map(|m| m.uid())
}

#[cfg(not(unix))]
fn current_uid() -> Option<u32> {
    None
}

/// Names of the named pipes on this machine (Windows), e.g. `docker_engine`.
pub fn list_pipes() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    std::fs::read_dir(r"\\.\pipe\")
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
        .unwrap_or_default()
}

fn docker_config_dir() -> Option<PathBuf> {
    std::env::var_os("DOCKER_CONFIG").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| home_dir().map(|h| h.join(".docker")))
}

/// The active `docker context` (from `DOCKER_CONTEXT` or `~/.docker/config.json`) and its
/// endpoint, unless it is `default`.
pub fn current_docker_context() -> Option<(String, String)> {
    let dir = docker_config_dir()?;
    let name = match std::env::var("DOCKER_CONTEXT").ok().filter(|v| !v.is_empty()) {
        Some(n) => n,
        None => {
            let cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("config.json")).ok()?).ok()?;
            cfg.get("currentContext")?.as_str()?.to_string()
        }
    };
    if name.is_empty() || name == "default" {
        return None;
    }
    let host = context_host(&dir, &name)?;
    Some((name, host))
}

/// Reads `<config>/contexts/meta/<sha256(name)>/meta.json` → `Endpoints.docker.Host`.
pub fn context_host(config_dir: &Path, name: &str) -> Option<String> {
    let digest = Sha256::digest(name.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config_dir.join("contexts").join("meta").join(hex).join("meta.json")).ok()?).ok()?;
    meta.pointer("/Endpoints/docker/Host")?.as_str().map(str::to_string)
}

/// `unix://<base>/<rel>` with `/` separators whatever the host OS (unix sockets only exist on
/// macOS/Linux; building them with `Path::join` on Windows would mix in backslashes).
fn unix_in(base: &Path, rel: &str) -> String {
    format!("unix://{}/{}", base.to_string_lossy().trim_end_matches(['/', '\\']), rel.trim_start_matches('/'))
}

fn pipe_host(name: &str) -> String {
    format!("npipe:////./pipe/{name}")
}

/// The socket path or pipe name inside a host URL.
fn host_target(host: &str) -> Option<String> {
    if let Some(p) = host.strip_prefix("unix://") {
        return Some(p.to_string());
    }
    if let Some(p) = host.strip_prefix("npipe://") {
        let p = p.replace('\\', "/");
        return p.rsplit('/').next().map(str::to_string);
    }
    None
}

/// Candidate endpoints in preference order, deduplicated. `exists` checks a socket path.
pub fn candidates_for(env: &DetectEnv, exists: &dyn Fn(&Path) -> bool) -> Vec<SocketCandidate> {
    let os = env.os.unwrap_or_else(Os::current);
    let mut out: Vec<SocketCandidate> = Vec::new();
    let pipe_exists = |name: &str| env.pipes.iter().any(|p| p.eq_ignore_ascii_case(name));
    let host_exists = |host: &str| -> bool {
        if host.starts_with("tcp://") || host.starts_with("http://") || host.starts_with("https://") || host.starts_with("ssh://") {
            return true;
        }
        match (host.starts_with("npipe://"), host_target(host)) {
            (true, Some(name)) => pipe_exists(&name),
            (false, Some(path)) => exists(Path::new(&path)),
            _ => false,
        }
    };
    let push = |host: String, source: &str, out: &mut Vec<SocketCandidate>| {
        if out.iter().any(|c| c.host == host) {
            return;
        }
        let exists = host_exists(&host);
        out.push(SocketCandidate { host, source: source.to_string(), exists });
    };

    if let Some(h) = &env.docker_host {
        push(h.trim().to_string(), "DOCKER_HOST", &mut out);
    }
    if let Some((name, host)) = &env.docker_context {
        push(host.clone(), &format!("docker context \"{name}\""), &mut out);
    }
    let home = env.home.as_deref();
    match os {
        Os::Windows => {
            push(pipe_host("docker_engine"), "Docker Desktop / Rancher Desktop", &mut out);
            push(pipe_host("dockerDesktopLinuxEngine"), "Docker Desktop", &mut out);
            push(pipe_host("podman-machine-default"), "Podman", &mut out);
            let mut others: Vec<&String> = env.pipes.iter().filter(|p| p.starts_with("podman-") && !p.ends_with("-api")).collect();
            others.sort();
            for p in others {
                push(pipe_host(p), "Podman", &mut out);
            }
        }
        Os::MacOs => {
            if let Some(h) = home {
                push(unix_in(h, ".docker/run/docker.sock"), "Docker Desktop", &mut out);
                push(unix_in(h, ".orbstack/run/docker.sock"), "OrbStack", &mut out);
                push(unix_in(h, ".colima/default/docker.sock"), "colima", &mut out);
                push(unix_in(h, ".colima/docker.sock"), "colima", &mut out);
                push(unix_in(h, ".rd/docker.sock"), "Rancher Desktop", &mut out);
                for rel in [
                    ".local/share/containers/podman/machine/podman.sock",
                    ".local/share/containers/podman/machine/applehv/podman.sock",
                    ".local/share/containers/podman/machine/libkrun/podman.sock",
                    ".local/share/containers/podman/machine/qemu/podman.sock",
                ] {
                    push(unix_in(h, rel), "Podman", &mut out);
                }
            }
            if let Some(tmp) = &env.tmpdir {
                push(unix_in(Path::new(tmp), "podman/podman-machine-default-api.sock"), "Podman", &mut out);
            }
            push("unix:///var/run/docker.sock".into(), "default socket", &mut out);
        }
        Os::Linux => {
            push("unix:///var/run/docker.sock".into(), "Docker Engine", &mut out);
            if let Some(x) = &env.xdg_runtime_dir {
                push(unix_in(Path::new(x), "docker.sock"), "Docker (rootless)", &mut out);
                push(unix_in(Path::new(x), "podman/podman.sock"), "Podman (rootless)", &mut out);
            }
            if let Some(uid) = env.uid {
                push(format!("unix:///run/user/{uid}/podman/podman.sock"), "Podman (rootless)", &mut out);
            }
            push("unix:///run/podman/podman.sock".into(), "Podman", &mut out);
            if let Some(h) = home {
                push(unix_in(h, ".docker/desktop/docker.sock"), "Docker Desktop", &mut out);
                push(unix_in(h, ".colima/default/docker.sock"), "colima", &mut out);
                push(unix_in(h, ".rd/docker.sock"), "Rancher Desktop", &mut out);
            }
        }
    }
    out
}

/// Candidate endpoints on this machine, in preference order.
pub fn candidates() -> Vec<SocketCandidate> {
    candidates_for(&DetectEnv::current(), &|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(c: &[SocketCandidate]) -> Vec<&str> {
        c.iter().map(|c| c.host.as_str()).collect()
    }

    #[test]
    fn docker_host_and_context_come_first() {
        let env = DetectEnv {
            os: Some(Os::Linux),
            docker_host: Some("tcp://10.0.0.5:2375".into()),
            docker_context: Some(("colima".into(), "unix:///home/me/.colima/default/docker.sock".into())),
            home: Some(PathBuf::from("/home/me")),
            ..Default::default()
        };
        let c = candidates_for(&env, &|_| false);
        assert_eq!(c[0].host, "tcp://10.0.0.5:2375");
        assert_eq!(c[0].source, "DOCKER_HOST");
        assert!(c[0].exists);
        assert_eq!(c[1].source, "docker context \"colima\"");
        // The context's socket is not listed twice.
        assert_eq!(hosts(&c).iter().filter(|h| h.contains(".colima/default")).count(), 1);
    }

    #[test]
    fn macos_sockets() {
        let env = DetectEnv {
            os: Some(Os::MacOs),
            home: Some(PathBuf::from("/Users/me")),
            tmpdir: Some("/var/folders/xy/T".into()),
            ..Default::default()
        };
        let present = |p: &Path| p.to_string_lossy().contains(".orbstack");
        let c = candidates_for(&env, &present);
        let h = hosts(&c);
        assert_eq!(h[0], "unix:///Users/me/.docker/run/docker.sock");
        assert!(h.contains(&"unix:///Users/me/.orbstack/run/docker.sock"));
        assert!(h.contains(&"unix:///Users/me/.colima/default/docker.sock"));
        assert!(h.iter().any(|x| x.ends_with("podman-machine-default-api.sock")));
        assert_eq!(h.last(), Some(&"unix:///var/run/docker.sock"));
        let orb = c.iter().find(|c| c.source == "OrbStack").unwrap();
        assert!(orb.exists);
        assert!(!c.iter().find(|c| c.source == "Docker Desktop").unwrap().exists);
    }

    #[test]
    fn linux_rootless() {
        let env = DetectEnv { os: Some(Os::Linux), xdg_runtime_dir: Some("/run/user/1000".into()), uid: Some(1000), ..Default::default() };
        let c = candidates_for(&env, &|p| p == Path::new("/run/user/1000/podman/podman.sock"));
        let podman = c.iter().find(|c| c.host == "unix:///run/user/1000/podman/podman.sock").unwrap();
        assert!(podman.exists);
        assert_eq!(hosts(&c).iter().filter(|h| h.contains("/run/user/1000/podman")).count(), 1);
    }

    #[test]
    fn windows_pipes() {
        let env = DetectEnv {
            os: Some(Os::Windows),
            pipes: vec!["podman-machine-default".into(), "podman-dev".into(), "podman-dev-api".into(), "other".into()],
            ..Default::default()
        };
        let c = candidates_for(&env, &|_| false);
        assert_eq!(c[0].host, "npipe:////./pipe/docker_engine");
        assert!(!c[0].exists);
        let pm = c.iter().find(|c| c.host == "npipe:////./pipe/podman-machine-default").unwrap();
        assert!(pm.exists);
        assert!(c.iter().any(|c| c.host == "npipe:////./pipe/podman-dev" && c.exists));
        assert!(!c.iter().any(|c| c.host.ends_with("-api")));
    }

    #[test]
    fn docker_context_meta() {
        let dir = tempfile::tempdir().unwrap();
        let digest = Sha256::digest(b"orbstack");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let meta_dir = dir.path().join("contexts").join("meta").join(hex);
        std::fs::create_dir_all(&meta_dir).unwrap();
        std::fs::write(
            meta_dir.join("meta.json"),
            r#"{"Name":"orbstack","Metadata":{},"Endpoints":{"docker":{"Host":"unix:///Users/me/.orbstack/run/docker.sock","SkipTLSVerify":false}}}"#,
        )
        .unwrap();
        assert_eq!(context_host(dir.path(), "orbstack").as_deref(), Some("unix:///Users/me/.orbstack/run/docker.sock"));
        assert_eq!(context_host(dir.path(), "missing"), None);
    }

    #[test]
    fn pipe_listing_does_not_panic() {
        // On Windows this enumerates \\.\pipe\; elsewhere it is empty.
        let pipes = list_pipes();
        if !cfg!(windows) {
            assert!(pipes.is_empty());
        }
    }
}
