//! Port-forward manager.
//!
//! A forward exposes a pod/service/workload port (Kubernetes) or a container's published port
//! (Docker) on `127.0.0.1`, and can **bind an environment variable** to the resulting URL: while
//! the forward runs, `{{baseUrl}}` (or any variable) resolves to e.g. `http://127.0.0.1:18080`
//! through an in-memory [`RuntimeOverrides`] layer. The environment file is only written when the
//! user asks ("Save to environment").
//!
//! Definitions are project files, `connections/<slug>.forward.yaml`:
//!
//! ```yaml
//! version: 1
//! name: Orders API
//! context: gke_shop_europe-west1_dev
//! namespace: orders
//! target:
//!   kind: service          # pod | service | deployment | statefulset | daemonset | replicaset | container
//!   name: orders-api
//! remotePort: 8080
//! localPort: 18080         # omit for any free port
//! bindEnvironment:
//!   environment: local     # omit to apply whichever environment is active
//!   variable: baseUrl
//!   template: http://127.0.0.1:{port}
//! ```
//!
//! Docker definitions use `source: docker` and `target: { kind: container, name: … }`;
//! `remotePort` is the container port and the bound URL uses its published host port.

mod overrides;
mod runtime;
mod template;

use std::path::{Path, PathBuf};

use devall_core::fsutil::{read_yaml, safe_join, slugify, unique_name, write_yaml};
use devall_core::{Extra, FORMAT_VERSION, Project};
use serde::{Deserialize, Serialize};

pub use overrides::{OverrideEntry, RuntimeOverrides, global_overrides};
pub use runtime::{BindingStatus, ForwardManager, ForwardState, ForwardStatus, pick_pod, selector_string};
pub use template::{DEFAULT_TEMPLATE, TemplateCtx, render_template};

use crate::error::{Error, Result};

pub const FORWARD_DIR: &str = "connections";
pub const FORWARD_EXT: &str = ".forward.yaml";

fn format_version() -> u32 {
    FORMAT_VERSION
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForwardSource {
    #[default]
    Kubernetes,
    Docker,
}

impl ForwardSource {
    fn is_kubernetes(&self) -> bool {
        *self == ForwardSource::Kubernetes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Pod,
    Service,
    Deployment,
    StatefulSet,
    DaemonSet,
    ReplicaSet,
    /// A Docker container (by name or id).
    Container,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardTarget {
    pub kind: TargetKind,
    pub name: String,
}

fn base_url() -> String {
    "baseUrl".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvBinding {
    /// Environment id (file stem under `environments/`). `None` = whichever is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default = "base_url")]
    pub variable: String,
    /// `{port}`, `{host}`, `{remotePort}`, `{name}`, `{namespace}`, `{target}`, `{context}`.
    /// Default: `http://127.0.0.1:{port}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardFile {
    #[serde(default = "format_version")]
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "ForwardSource::is_kubernetes")]
    pub source: ForwardSource,
    /// Kubeconfig context; `None` = current-context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// `None` = the context's default namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub target: ForwardTarget,
    pub remote_port: u16,
    /// Local port; `None`/0 = any free port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_environment: Option<EnvBinding>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl ForwardFile {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::Invalid("a forward needs a name".into()));
        }
        if self.target.name.trim().is_empty() {
            return Err(Error::Invalid("a forward needs a target name".into()));
        }
        if self.remote_port == 0 {
            return Err(Error::Invalid("remotePort must be 1–65535".into()));
        }
        let docker_target = self.target.kind == TargetKind::Container;
        match self.source {
            ForwardSource::Docker if !docker_target => return Err(Error::Invalid("Docker forwards target a container".into())),
            ForwardSource::Kubernetes if docker_target => return Err(Error::Invalid("`container` targets need `source: docker`".into())),
            _ => {}
        }
        if let Some(b) = &self.bind_environment
            && b.variable.trim().is_empty()
        {
            return Err(Error::Invalid("bindEnvironment.variable is empty".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardEntry {
    /// File stem: `orders-api` for `connections/orders-api.forward.yaml`.
    pub id: String,
    /// Relative to the project root.
    pub path: String,
    pub file: Option<ForwardFile>,
    /// Parse error, when the file is invalid.
    pub error: Option<String>,
}

fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Err(Error::Invalid(format!("invalid forward id {id:?}")));
    }
    Ok(())
}

pub fn forward_path(root: &Path, id: &str) -> Result<PathBuf> {
    check_id(id)?;
    Ok(safe_join(root, &format!("{FORWARD_DIR}/{id}{FORWARD_EXT}"))?)
}

/// Every `connections/*.forward.yaml` (other `connections/` files belong to other modules).
pub fn list(root: &Path) -> Vec<ForwardEntry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root.join(FORWARD_DIR)) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(FORWARD_EXT) else { continue };
        let (file, error) = match read_yaml::<ForwardFile>(&e.path()) {
            Ok(f) => (Some(f), None),
            Err(err) => (None, Some(err.to_string())),
        };
        out.push(ForwardEntry { id: id.to_string(), path: format!("{FORWARD_DIR}/{name}"), file, error });
    }
    out.sort_by_key(|e| e.file.as_ref().map(|f| f.name.to_lowercase()).unwrap_or_else(|| e.id.clone()));
    out
}

pub fn read(root: &Path, id: &str) -> Result<ForwardFile> {
    let path = forward_path(root, id)?;
    if !path.is_file() {
        return Err(Error::NotFound(format!("forward {id}")));
    }
    Ok(read_yaml(&path)?)
}

pub fn save(root: &Path, id: &str, file: &ForwardFile) -> Result<()> {
    file.validate()?;
    let mut f = file.clone();
    f.version = FORMAT_VERSION;
    Ok(write_yaml(&forward_path(root, id)?, &f)?)
}

/// Writes a new `connections/<slug>.forward.yaml` and returns its id.
pub fn create(root: &Path, file: &ForwardFile) -> Result<String> {
    file.validate()?;
    let dir = root.join(FORWARD_DIR);
    std::fs::create_dir_all(&dir)?;
    let name = unique_name(&dir, &slugify(&file.name), FORWARD_EXT);
    let id = name.trim_end_matches(FORWARD_EXT).to_string();
    save(root, &id, file)?;
    Ok(id)
}

/// Moves the definition to the OS trash.
pub fn delete(root: &Path, id: &str) -> Result<()> {
    let path = forward_path(root, id)?;
    if !path.exists() {
        return Err(Error::NotFound(format!("forward {id}")));
    }
    trash::delete(&path).map_err(|e| Error::Io(std::io::Error::other(format!("{}: {e}", path.display()))))
}

/// "Save to environment": writes `variable = value` into `environments/<env>.env.yaml`.
pub fn write_env_variable(root: &Path, env: &str, variable: &str, value: &str) -> Result<()> {
    let p = Project::open(root)?;
    let mut file = p.read_environment(env)?;
    file.variables.insert(variable.to_string(), value.to_string());
    p.save_environment(env, &file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ForwardFile {
        ForwardFile {
            version: 1,
            name: "Orders API".into(),
            source: ForwardSource::Kubernetes,
            context: Some("gke_shop_europe-west1_dev".into()),
            namespace: Some("orders".into()),
            target: ForwardTarget { kind: TargetKind::Service, name: "orders-api".into() },
            remote_port: 8080,
            local_port: Some(18080),
            bind_environment: Some(EnvBinding { environment: Some("local".into()), variable: "baseUrl".into(), template: Some("http://127.0.0.1:{port}".into()) }),
            extra: Extra::new(),
        }
    }

    #[test]
    fn file_round_trip_and_format() {
        let dir = tempfile::tempdir().unwrap();
        let id = create(dir.path(), &sample()).unwrap();
        assert_eq!(id, "orders-api");
        let text = std::fs::read_to_string(dir.path().join("connections/orders-api.forward.yaml")).unwrap();
        assert!(text.starts_with("version: 1\nname: Orders API\n"), "{text}");
        assert!(text.contains("kind: service"), "{text}");
        assert!(text.contains("remotePort: 8080"), "{text}");
        assert!(text.contains("bindEnvironment:"), "{text}");
        assert!(!text.contains("source:"), "kubernetes is the default: {text}");
        assert_eq!(read(dir.path(), &id).unwrap(), sample());
        // A second one with the same name gets a fresh slug.
        assert_eq!(create(dir.path(), &sample()).unwrap(), "orders-api-2");
        // Other connection files are ignored; broken forwards are listed with an error.
        std::fs::write(dir.path().join("connections/db.conn.yaml"), "name: db").unwrap();
        std::fs::write(dir.path().join("connections/broken.forward.yaml"), "name: [").unwrap();
        let entries = list(dir.path());
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|e| e.id == "broken" && e.error.is_some()));
        assert!(read(dir.path(), "../x").is_err());
    }

    #[test]
    fn minimal_yaml_and_unknown_keys() {
        let f: ForwardFile = serde_yaml_ng::from_str(
            "name: web\nsource: docker\ntarget:\n  kind: container\n  name: shop-web-1\nremotePort: 80\nbindEnvironment: {}\nfuture: kept\n",
        )
        .unwrap();
        assert_eq!(f.source, ForwardSource::Docker);
        assert_eq!(f.bind_environment.as_ref().unwrap().variable, "baseUrl");
        assert_eq!(f.extra.get("future").and_then(|v| v.as_str()), Some("kept"));
        f.validate().unwrap();
        let back = serde_yaml_ng::to_string(&f).unwrap();
        assert!(back.contains("future: kept"));
    }

    #[test]
    fn validation() {
        let mut f = sample();
        f.remote_port = 0;
        assert!(f.validate().is_err());
        let mut f = sample();
        f.target.kind = TargetKind::Container;
        assert!(f.validate().is_err());
        let mut f = sample();
        f.bind_environment.as_mut().unwrap().variable = " ".into();
        assert!(f.validate().is_err());
    }

    #[test]
    fn save_to_environment_writes_the_variable() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Shop").unwrap();
        write_env_variable(p.root(), "local", "baseUrl", "http://127.0.0.1:18080").unwrap();
        let env = p.read_environment("local").unwrap();
        assert_eq!(env.variables.get("baseUrl").map(String::as_str), Some("http://127.0.0.1:18080"));
        assert!(write_env_variable(p.root(), "missing", "baseUrl", "x").is_err());
    }
}
