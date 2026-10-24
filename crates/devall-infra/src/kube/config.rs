//! Kubeconfig loading and exec credential plugins.
//!
//! Exec plugins (`gke-gcloud-auth-plugin`, `aws eks get-token`, `kubelogin`, `doctl`, …) are
//! spawned by kube-rs itself. A GUI-launched app usually can't find them because it lacks the
//! shell `PATH`, so before building a client we resolve the plugin to an absolute path on
//! [`crate::sys::search_path`], hand the plugin that same `PATH` (it often calls `gcloud`/`az`
//! itself), and force `interactiveMode: Never` (no TTY in a GUI). A missing plugin becomes a
//! readable error with the install hint. On Windows, a plugin installed only inside WSL can be
//! run through `wsl.exe` when [`KubeOptions::wsl_fallback`] is on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kube::config::{ExecConfig, ExecInteractiveMode, Kubeconfig};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::sys;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KubeOptions {
    /// Kubeconfig files to merge, in order. Empty = `KUBECONFIG`, else `~/.kube/config`.
    pub kubeconfig: Vec<String>,
    /// Windows: when an exec auth plugin isn't installed on Windows, run it inside WSL.
    pub wsl_fallback: bool,
}

/// The kubeconfig files to read for `opts`.
pub fn kubeconfig_paths(opts: &KubeOptions) -> Vec<PathBuf> {
    let explicit: Vec<PathBuf> = opts.kubeconfig.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    if !explicit.is_empty() {
        return explicit;
    }
    if let Some(v) = std::env::var_os("KUBECONFIG").filter(|v| !v.is_empty()) {
        let paths: Vec<PathBuf> = std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect();
        if !paths.is_empty() {
            return paths;
        }
    }
    sys::home_dir().map(|h| vec![h.join(".kube").join("config")]).unwrap_or_default()
}

/// Reads and merges the existing files among `paths` (kubectl's merge rules).
pub fn load_kubeconfig(paths: &[PathBuf]) -> Result<Kubeconfig> {
    let existing: Vec<&PathBuf> = paths.iter().filter(|p| p.is_file()).collect();
    if existing.is_empty() {
        let list = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
        return Err(Error::Kubeconfig(format!(
            "no kubeconfig found ({}). Create one with your cloud CLI (e.g. `gcloud container clusters get-credentials`, `aws eks update-kubeconfig`, `az aks get-credentials`) or a local cluster (kind, minikube, k3d, Docker Desktop, Rancher Desktop, OrbStack).",
            if list.is_empty() { "no home directory".to_string() } else { list }
        )));
    }
    let mut merged: Option<Kubeconfig> = None;
    for p in existing {
        let kc = Kubeconfig::read_from(p).map_err(|e| Error::Kubeconfig(format!("{}: {}", p.display(), crate::error::chain(&e))))?;
        merged = Some(match merged {
            Some(m) => m.merge(kc)?,
            None => kc,
        });
    }
    Ok(merged.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AuthSummary {
    None,
    Token,
    ClientCertificate,
    Basic,
    /// An exec credential plugin.
    Exec {
        command: String,
        args: Vec<String>,
        /// Absolute path it resolves to on this machine, when found.
        resolved: Option<String>,
        found: bool,
        install_hint: Option<String>,
    },
    /// Legacy in-tree auth provider (`gcp`, `azure`, `oidc`).
    AuthProvider { name: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextInfo {
    pub name: String,
    pub cluster: String,
    pub user: Option<String>,
    pub namespace: Option<String>,
    pub server: Option<String>,
    pub auth: AuthSummary,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeContexts {
    pub current: Option<String>,
    pub contexts: Vec<ContextInfo>,
    /// Kubeconfig files that were read.
    pub files: Vec<String>,
}

/// Summarizes every context without touching the network. `resolve` finds plugin binaries.
pub fn list_contexts(kc: &Kubeconfig, resolve: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<ContextInfo> {
    let mut out = Vec::new();
    for named in &kc.contexts {
        let Some(ctx) = &named.context else { continue };
        let server = kc.clusters.iter().find(|c| c.name == ctx.cluster).and_then(|c| c.cluster.as_ref()).and_then(|c| c.server.clone());
        let auth_info = ctx.user.as_ref().and_then(|u| kc.auth_infos.iter().find(|a| &a.name == u)).and_then(|a| a.auth_info.as_ref());
        let auth = match auth_info {
            None => AuthSummary::None,
            Some(a) => {
                if let Some(exec) = &a.exec {
                    let command = exec.command.clone().unwrap_or_default();
                    let resolved = if command.is_empty() { None } else { resolve(&command) };
                    AuthSummary::Exec {
                        args: exec.args.clone().unwrap_or_default(),
                        found: resolved.is_some(),
                        resolved: resolved.map(|p| p.display().to_string()),
                        install_hint: exec.install_hint.clone().or_else(|| plugin_hint(&command).map(str::to_string)),
                        command,
                    }
                } else if let Some(p) = &a.auth_provider {
                    AuthSummary::AuthProvider { name: p.name.clone() }
                } else if a.token.is_some() || a.token_file.is_some() {
                    AuthSummary::Token
                } else if a.client_certificate.is_some() || a.client_certificate_data.is_some() {
                    AuthSummary::ClientCertificate
                } else if a.username.is_some() {
                    AuthSummary::Basic
                } else {
                    AuthSummary::None
                }
            }
        };
        out.push(ContextInfo {
            name: named.name.clone(),
            cluster: ctx.cluster.clone(),
            user: ctx.user.clone(),
            namespace: ctx.namespace.clone(),
            server,
            auth,
            current: kc.current_context.as_deref() == Some(named.name.as_str()),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Install instructions for the common exec plugins.
pub fn plugin_hint(command: &str) -> Option<&'static str> {
    let base = Path::new(command).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    Some(match base.as_str() {
        "gke-gcloud-auth-plugin" => "Install it with `gcloud components install gke-gcloud-auth-plugin` (or `apt/dnf install google-cloud-cli-gke-gcloud-auth-plugin`, or Homebrew's google-cloud-sdk), then reopen /dev/all.",
        "aws" => "Install the AWS CLI v2 (https://aws.amazon.com/cli/) and run `aws configure sso` or `aws configure`.",
        "aws-iam-authenticator" => "Install aws-iam-authenticator (https://github.com/kubernetes-sigs/aws-iam-authenticator).",
        "kubelogin" => "Install kubelogin: `az aks install-cli`, `winget install Microsoft.Azure.Kubelogin`, or `brew install Azure/kubelogin/kubelogin`.",
        "kubectl-oidc_login" | "kubectl" => "Install kubectl and the oidc-login plugin (`kubectl krew install oidc-login`).",
        "doctl" => "Install doctl (https://docs.digitalocean.com/reference/doctl/how-to/install/) and run `doctl auth init`.",
        "oci" => "Install the OCI CLI (https://docs.oracle.com/iaas/Content/API/SDKDocs/cliinstall.htm).",
        "ibmcloud" => "Install the IBM Cloud CLI.",
        "gcloud" => "Install the Google Cloud CLI (https://cloud.google.com/sdk/docs/install).",
        _ => return None,
    })
}

/// How [`prepare_exec`] may run a plugin.
pub struct ExecEnv<'a> {
    /// Finds a command on this machine.
    pub resolve: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// `PATH` handed to the plugin.
    pub path_var: String,
    /// `wsl.exe`, when WSL fallback is enabled and available (Windows).
    pub wsl: Option<PathBuf>,
}

fn has_env(exec: &ExecConfig, name: &str) -> bool {
    exec.env.as_ref().is_some_and(|v| v.iter().any(|e| e.get("name").is_some_and(|n| n.eq_ignore_ascii_case(name))))
}

fn push_env(exec: &mut ExecConfig, name: &str, value: String) {
    let mut m = HashMap::new();
    m.insert("name".to_string(), name.to_string());
    m.insert("value".to_string(), value);
    exec.env.get_or_insert_with(Vec::new).push(m);
}

/// Makes an exec plugin runnable from a GUI app (see module docs). Errors when it can't be found.
pub fn prepare_exec(exec: &mut ExecConfig, context: &str, env: &ExecEnv<'_>) -> Result<()> {
    let command = exec.command.clone().filter(|c| !c.trim().is_empty()).ok_or_else(|| {
        Error::AuthPlugin(format!("context \"{context}\": the kubeconfig user has an exec section without a command"))
    })?;
    // A GUI has no terminal for the plugin to prompt on; tell it so it fails fast instead of hanging.
    exec.interactive_mode = Some(ExecInteractiveMode::Never);
    if let Some(path) = (env.resolve)(&command) {
        exec.command = Some(path.to_string_lossy().to_string());
        if !has_env(exec, "PATH") {
            push_env(exec, "PATH", env.path_var.clone());
        }
        return Ok(());
    }
    if let Some(wsl) = &env.wsl {
        // `wsl.exe -e <cmd> <args…>`; WSLENV forwards the variables kube-rs and the kubeconfig set.
        let mut args = vec!["-e".to_string(), command.clone()];
        args.extend(exec.args.clone().unwrap_or_default());
        exec.args = Some(args);
        exec.command = Some(wsl.to_string_lossy().to_string());
        let mut names: Vec<String> = vec!["KUBERNETES_EXEC_INFO/u".into()];
        if let Some(vars) = &exec.env {
            names.extend(vars.iter().filter_map(|e| e.get("name")).filter(|n| !n.eq_ignore_ascii_case("WSLENV")).map(|n| format!("{n}/u")));
        }
        let existing = std::env::var("WSLENV").unwrap_or_default();
        let value = if existing.is_empty() { names.join(":") } else { format!("{existing}:{}", names.join(":")) };
        push_env(exec, "WSLENV", value);
        return Ok(());
    }
    let hint = exec.install_hint.clone().or_else(|| plugin_hint(&command).map(str::to_string)).unwrap_or_else(|| format!("Install `{command}` or put it on PATH."));
    let wsl_note = if cfg!(windows) {
        "\nIf it is installed only inside WSL, turn on \"Run auth plugins through WSL\" in the Kubernetes settings."
    } else {
        ""
    };
    Err(Error::AuthPlugin(format!(
        "context \"{context}\" authenticates with `{command}`, which was not found on PATH or in the usual install folders.\n{}{wsl_note}",
        hint.trim()
    )))
}

/// Prepares every exec plugin used by `context` (only that context's user is touched).
pub fn prepare_context(kc: &mut Kubeconfig, context: &str, env: &ExecEnv<'_>) -> Result<()> {
    let ctx = kc
        .contexts
        .iter()
        .find(|c| c.name == context)
        .and_then(|c| c.context.clone())
        .ok_or_else(|| Error::Kubeconfig(format!("context \"{context}\" not found")))?;
    let Some(user) = ctx.user else { return Ok(()) };
    if let Some(info) = kc.auth_infos.iter_mut().find(|a| a.name == user).and_then(|a| a.auth_info.as_mut())
        && let Some(exec) = info.exec.as_mut()
    {
        prepare_exec(exec, context, env)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: gke_shop_europe-west1_dev
clusters:
- name: gke_shop_europe-west1_dev
  cluster:
    server: https://34.1.2.3
    certificate-authority-data: Zm9v
- name: eks-prod
  cluster:
    server: https://ABC.gr7.eu-west-1.eks.amazonaws.com
- name: kind-local
  cluster:
    server: https://127.0.0.1:6443
contexts:
- name: gke_shop_europe-west1_dev
  context:
    cluster: gke_shop_europe-west1_dev
    user: gke-user
    namespace: orders
- name: eks-prod
  context:
    cluster: eks-prod
    user: eks-user
- name: kind-local
  context:
    cluster: kind-local
    user: kind-user
users:
- name: gke-user
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: gke-gcloud-auth-plugin
      installHint: Install gke-gcloud-auth-plugin for use with kubectl
      provideClusterInfo: true
      interactiveMode: IfAvailable
- name: eks-user
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args: [eks, get-token, --cluster-name, prod, --output, json]
      env:
      - name: AWS_PROFILE
        value: prod
- name: kind-user
  user:
    client-certificate-data: Zm9v
    client-key-data: YmFy
"#;

    fn kc() -> Kubeconfig {
        Kubeconfig::from_yaml(KUBECONFIG).unwrap()
    }

    #[test]
    fn lists_contexts_with_auth_kinds() {
        let resolve = |c: &str| (c == "aws").then(|| PathBuf::from("/usr/local/bin/aws"));
        let list = list_contexts(&kc(), &resolve);
        assert_eq!(list.len(), 3);
        let gke = list.iter().find(|c| c.name.starts_with("gke_")).unwrap();
        assert!(gke.current);
        assert_eq!(gke.namespace.as_deref(), Some("orders"));
        assert_eq!(gke.server.as_deref(), Some("https://34.1.2.3"));
        match &gke.auth {
            AuthSummary::Exec { command, found, install_hint, .. } => {
                assert_eq!(command, "gke-gcloud-auth-plugin");
                assert!(!found);
                assert_eq!(install_hint.as_deref(), Some("Install gke-gcloud-auth-plugin for use with kubectl"));
            }
            other => panic!("unexpected {other:?}"),
        }
        let eks = list.iter().find(|c| c.name == "eks-prod").unwrap();
        assert!(matches!(&eks.auth, AuthSummary::Exec { found: true, resolved: Some(p), args, .. } if p.contains("aws") && args[0] == "eks"));
        let kind = list.iter().find(|c| c.name == "kind-local").unwrap();
        assert_eq!(kind.auth, AuthSummary::ClientCertificate);
    }

    #[test]
    fn exec_plugin_resolved_to_absolute_path_with_path_env() {
        let mut k = kc();
        let resolve = |c: &str| (c == "aws").then(|| PathBuf::from("/opt/homebrew/bin/aws"));
        let env = ExecEnv { resolve: &resolve, path_var: "/opt/homebrew/bin:/usr/bin".into(), wsl: None };
        prepare_context(&mut k, "eks-prod", &env).unwrap();
        let exec = k.auth_infos.iter().find(|a| a.name == "eks-user").unwrap().auth_info.as_ref().unwrap().exec.as_ref().unwrap();
        assert_eq!(exec.command.as_deref(), Some("/opt/homebrew/bin/aws"));
        assert_eq!(exec.interactive_mode, Some(ExecInteractiveMode::Never));
        let envs = exec.env.as_ref().unwrap();
        assert!(envs.iter().any(|e| e["name"] == "AWS_PROFILE" && e["value"] == "prod"), "kubeconfig env kept");
        assert!(envs.iter().any(|e| e["name"] == "PATH" && e["value"] == "/opt/homebrew/bin:/usr/bin"));
        // Other users are untouched.
        let gke = k.auth_infos.iter().find(|a| a.name == "gke-user").unwrap().auth_info.as_ref().unwrap().exec.as_ref().unwrap();
        assert_eq!(gke.command.as_deref(), Some("gke-gcloud-auth-plugin"));
    }

    #[test]
    fn missing_plugin_is_a_clear_error() {
        let mut k = kc();
        let env = ExecEnv { resolve: &|_| None, path_var: String::new(), wsl: None };
        let err = prepare_context(&mut k, "gke_shop_europe-west1_dev", &env).unwrap_err().to_string();
        assert!(err.contains("gke-gcloud-auth-plugin"), "{err}");
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("Install gke-gcloud-auth-plugin for use with kubectl"), "{err}");
        // Client-cert contexts need nothing.
        prepare_context(&mut k, "kind-local", &env).unwrap();
        assert!(prepare_context(&mut k, "nope", &env).is_err());
    }

    #[test]
    fn wsl_fallback_wraps_the_command() {
        let mut k = kc();
        let env = ExecEnv { resolve: &|_| None, path_var: String::new(), wsl: Some(PathBuf::from("C:\\Windows\\System32\\wsl.exe")) };
        prepare_context(&mut k, "eks-prod", &env).unwrap();
        let exec = k.auth_infos.iter().find(|a| a.name == "eks-user").unwrap().auth_info.as_ref().unwrap().exec.as_ref().unwrap();
        assert_eq!(exec.command.as_deref(), Some("C:\\Windows\\System32\\wsl.exe"));
        let args = exec.args.as_ref().unwrap();
        assert_eq!(&args[..3], &["-e", "aws", "eks"]);
        let wslenv = exec.env.as_ref().unwrap().iter().find(|e| e["name"] == "WSLENV").unwrap()["value"].clone();
        assert!(wslenv.contains("KUBERNETES_EXEC_INFO/u") && wslenv.contains("AWS_PROFILE/u"), "{wslenv}");
    }

    #[test]
    fn hints_for_known_plugins() {
        assert!(plugin_hint("gke-gcloud-auth-plugin").unwrap().contains("gcloud components install"));
        assert!(plugin_hint("C:\\tools\\kubelogin.exe").unwrap().contains("kubelogin"));
        assert!(plugin_hint("/usr/local/bin/aws").unwrap().contains("AWS CLI"));
        assert!(plugin_hint("my-custom-thing").is_none());
    }

    #[test]
    fn merges_multiple_files_and_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.yaml");
        let b = dir.path().join("b.yaml");
        std::fs::write(&a, KUBECONFIG).unwrap();
        std::fs::write(
            &b,
            "apiVersion: v1\nkind: Config\ncurrent-context: other\nclusters:\n- name: c2\n  cluster:\n    server: https://10.0.0.1\ncontexts:\n- name: other\n  context:\n    cluster: c2\n    user: u2\nusers:\n- name: u2\n  user:\n    token: abc\n",
        )
        .unwrap();
        let merged = load_kubeconfig(&[a.clone(), b, dir.path().join("missing.yaml")]).unwrap();
        assert_eq!(merged.contexts.len(), 4);
        // First file wins current-context.
        assert_eq!(merged.current_context.as_deref(), Some("gke_shop_europe-west1_dev"));
        let err = load_kubeconfig(&[dir.path().join("nope")]).unwrap_err().to_string();
        assert!(err.contains("no kubeconfig found"), "{err}");
        let paths = kubeconfig_paths(&KubeOptions { kubeconfig: vec![a.display().to_string(), " ".into()], wsl_fallback: false });
        assert_eq!(paths, vec![a]);
    }
}
