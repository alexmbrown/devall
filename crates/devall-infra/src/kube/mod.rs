//! Kubernetes: kubeconfig contexts (exec auth plugins included), clients per context, generic
//! resources, watches, logs, exec. Port-forwarding lives in [`crate::forward`].

pub mod config;
pub mod kubectl;
pub mod resources;
pub mod stream;

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use kube::config::KubeConfigOptions;
pub use kube::Client;
use kube::Config;
use serde::Serialize;

pub use config::{AuthSummary, ContextInfo, KubeContexts, KubeOptions};
pub use resources::{Describe, KindInfo, KindRef, ResourceList, ResourceRow, SecretEntry};
pub use stream::{PodLogOptions, WatchMsg};

use crate::error::{Error, Result};
use crate::sys;

/// A connected context.
#[derive(Clone)]
pub struct KubeConn {
    pub client: Client,
    pub context: String,
    pub default_namespace: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterInfo {
    pub context: String,
    pub default_namespace: String,
    /// e.g. `v1.34.1`.
    pub version: String,
    pub platform: String,
}

/// Clients per kubeconfig context, created on first use.
pub struct KubeManager {
    options: RwLock<KubeOptions>,
    conns: Mutex<HashMap<String, KubeConn>>,
}

impl Default for KubeManager {
    fn default() -> Self {
        Self::new(KubeOptions::default())
    }
}

impl KubeManager {
    pub fn new(options: KubeOptions) -> Self {
        Self { options: RwLock::new(options), conns: Mutex::new(HashMap::new()) }
    }

    pub fn options(&self) -> KubeOptions {
        self.options.read().unwrap().clone()
    }

    /// Replaces the options and drops every cached client.
    pub fn set_options(&self, options: KubeOptions) {
        *self.options.write().unwrap() = options;
        self.conns.lock().unwrap().clear();
    }

    /// Drops the cached client of `context` (after a kubeconfig change or an auth failure).
    pub fn forget(&self, context: &str) {
        self.conns.lock().unwrap().remove(context);
    }

    /// Every context in the merged kubeconfig, with auth plugin availability. No network.
    pub async fn contexts(&self) -> Result<KubeContexts> {
        let opts = self.options();
        tokio::task::spawn_blocking(move || {
            let paths = config::kubeconfig_paths(&opts);
            let kc = config::load_kubeconfig(&paths)?;
            Ok(KubeContexts {
                current: kc.current_context.clone(),
                contexts: config::list_contexts(&kc, &sys::which),
                files: paths.iter().filter(|p| p.is_file()).map(|p| p.display().to_string()).collect(),
            })
        })
        .await
        .map_err(|e| Error::Kube(e.to_string()))?
    }

    /// The client for `context` (empty = current-context), created and cached on first use.
    pub async fn conn(&self, context: &str) -> Result<KubeConn> {
        if let Some(c) = self.conns.lock().unwrap().get(context) {
            return Ok(c.clone());
        }
        crate::tls::ensure_crypto_provider();
        let opts = self.options();
        let want = context.to_string();
        // Reading files and probing PATH may block (login-shell PATH on macOS).
        let (kc, ctx) = tokio::task::spawn_blocking(move || -> Result<_> {
            let mut kc = config::load_kubeconfig(&config::kubeconfig_paths(&opts))?;
            let ctx = if want.is_empty() {
                kc.current_context.clone().ok_or_else(|| Error::Kubeconfig("no current-context set; pick a context".into()))?
            } else {
                want
            };
            let wsl = if cfg!(windows) && opts.wsl_fallback { sys::which("wsl") } else { None };
            let env = config::ExecEnv { resolve: &sys::which, path_var: sys::search_path_var().to_string_lossy().to_string(), wsl };
            config::prepare_context(&mut kc, &ctx, &env)?;
            Ok((kc, ctx))
        })
        .await
        .map_err(|e| Error::Kube(e.to_string()))??;
        let cfg = Config::from_custom_kubeconfig(kc, &KubeConfigOptions { context: Some(ctx.clone()), ..Default::default() }).await?;
        let default_namespace = cfg.default_namespace.clone();
        let client = Client::try_from(cfg)?;
        let conn = KubeConn { client, context: ctx.clone(), default_namespace };
        let mut map = self.conns.lock().unwrap();
        map.insert(context.to_string(), conn.clone());
        if context != ctx {
            map.insert(ctx, conn.clone());
        }
        Ok(conn)
    }

    pub async fn client(&self, context: &str) -> Result<Client> {
        Ok(self.conn(context).await?.client)
    }

    /// Connects and asks the API server for its version (runs the auth plugin, so it doubles as
    /// a connection test). A failure drops the cached client so the next try starts fresh.
    pub async fn test(&self, context: &str) -> Result<ClusterInfo> {
        let conn = self.conn(context).await?;
        match tokio::time::timeout(std::time::Duration::from_secs(20), conn.client.apiserver_version()).await {
            Ok(Ok(v)) => Ok(ClusterInfo { context: conn.context, default_namespace: conn.default_namespace, version: v.git_version, platform: v.platform }),
            Ok(Err(e)) => {
                self.forget(context);
                Err(e.into())
            }
            Err(_) => {
                self.forget(context);
                Err(Error::Kube(format!("context \"{}\": the API server did not answer within 20 s", conn.context)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a reachable cluster in the current kubeconfig; only with `DEVALL_TEST_KUBE=1`.
    /// Read-only: version, namespaces, pods.
    #[tokio::test]
    async fn live_cluster_reads() {
        if std::env::var("DEVALL_TEST_KUBE").as_deref() != Ok("1") {
            return;
        }
        let m = KubeManager::default();
        let ctxs = m.contexts().await.unwrap();
        assert!(!ctxs.contexts.is_empty());
        let info = m.test("").await.unwrap();
        assert!(info.version.starts_with('v'));
        let client = m.client("").await.unwrap();
        let ns = resources::namespaces(&client, &info.default_namespace).await.unwrap();
        assert!(!ns.is_empty());
        let pods = KindRef::new("", "v1", "Pod", "pods", true);
        resources::list(&client, &pods, Some(&info.default_namespace), None).await.unwrap();
    }
}
