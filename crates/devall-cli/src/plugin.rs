//! `devall plugin list | install | run | trust | uninstall` — WebAssembly plugins from the terminal.
//! Plugins are shared with the desktop app (`<app data>/plugins`, approvals in `plugins.json`).

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use devall_plugins::{DefaultServices, PluginManager, ProjectRef, Scope, Status};

#[derive(Subcommand)]
pub enum PluginCmd {
    /// Installed plugins and the current project's plugins, with their status.
    List {
        #[arg(short, long)]
        project: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Install (or replace) a plugin from a folder or a .zip after reviewing its capabilities.
    Install {
        /// Plugin folder (with plugin.yaml + plugin.wasm) or .zip.
        source: PathBuf,
        /// Allow the plugin to read this declared secret (repeatable).
        #[arg(long = "grant-secret")]
        grant_secrets: Vec<String>,
        /// Don't ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// Run a plugin utility op: `devall plugin run hmac-signer:sign "payload" --arg key=…`.
    Run {
        /// `<plugin>:<op>` (see `devall plugin list` / `devall util list`).
        op: String,
        /// Input text; omit or pass `-` to read stdin.
        input: Option<String>,
        #[arg(short, long = "arg", value_parser = crate::parse_kv)]
        args: Vec<(String, String)>,
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
    /// Trust a project plugin on this machine (pinned to its current SHA-256).
    Trust {
        id: String,
        #[arg(short, long)]
        project: Option<PathBuf>,
        #[arg(long = "grant-secret")]
        grant_secrets: Vec<String>,
        #[arg(short, long)]
        yes: bool,
    },
    /// Move an installed plugin to the OS trash.
    Uninstall { id: String },
}

/// The plugin manager the CLI uses (keychain-backed plugin secrets when available).
pub fn manager() -> Result<Arc<PluginManager>> {
    let secrets: Option<Box<dyn devall_core::SecretStore>> =
        if devall_core::KeychainSecretStore::available() { Some(Box::new(devall_core::KeychainSecretStore::new())) } else { None };
    Ok(Arc::new(PluginManager::new(crate::app_data_dir()?, Arc::new(DefaultServices::new(secrets)))))
}

/// Installs the plugin bridge for request sends (`auth: { type: plugin }`, hooks). Best effort.
pub fn install_request_bridge() {
    if let Ok(m) = manager() {
        devall_http::plugins::install(Arc::new(devall_plugins::HttpBridge::new(m)));
    }
}

fn project_ref(dir: Option<PathBuf>) -> Option<ProjectRef> {
    crate::find_project(dir).ok().map(|p| ProjectRef::new(p.root(), p.id()))
}

fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("{question} — pass --yes to confirm when not running interactively");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn status_label(s: Status) -> &'static str {
    match s {
        Status::Ready => "ready",
        Status::Disabled => "disabled",
        Status::Untrusted => "untrusted",
        Status::Changed => "changed (re-approve)",
        Status::Broken => "broken",
        Status::Shadowed => "shadowed",
    }
}

pub async fn run(cmd: PluginCmd) -> Result<ExitCode> {
    let mgr = manager()?;
    // Plugin calls block (wasm + host HTTP): keep them off the async workers.
    tokio::task::spawn_blocking(move || run_blocking(&mgr, cmd)).await.context("plugin command panicked")?
}

fn run_blocking(mgr: &PluginManager, cmd: PluginCmd) -> Result<ExitCode> {
    match cmd {
        PluginCmd::List { project, json } => {
            let proj = project_ref(project);
            let list = mgr.list(proj.as_ref());
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(ExitCode::SUCCESS);
            }
            if list.is_empty() {
                println!("No plugins. Install one with `devall plugin install <folder|zip>` (plugins folder: {}).", mgr.user_dir().display());
            }
            for p in &list {
                let scope = if p.scope == Scope::User { "installed" } else { "project" };
                let version = p.manifest.as_ref().map(|m| m.version.as_str()).unwrap_or("?");
                println!("{:<24} {:<8} {:<9} {}", p.id, version, scope, status_label(p.status));
                if let Some(e) = &p.error {
                    println!("    {e}");
                }
                for c in &p.capabilities {
                    println!("    may: {c}");
                }
            }
            let cat = mgr.catalog(proj.as_ref());
            for u in &cat.utilities {
                println!("  util    {:<28} {}", u.id, u.name);
            }
            for i in &cat.importers {
                println!("  import  {:<28} {}", i.id, i.name);
            }
            for g in &cat.codegen {
                println!("  codegen {:<28} {}", g.id, g.name);
            }
            for a in &cat.auth {
                println!("  auth    {:<28} {}", format!("{}/{}", a.plugin, a.id), a.name);
            }
            for h in &cat.hooks {
                println!("  hook    {:<28} {}{}", format!("{}/{}", h.plugin, h.id), h.name, if h.enabled { "" } else { " (disabled)" });
            }
            Ok(ExitCode::SUCCESS)
        }
        PluginCmd::Install { source, grant_secrets, yes } => {
            let preview = mgr.preview(&source)?;
            let m = &preview.manifest;
            println!("{} {} ({})", m.name, m.version, m.id);
            if !m.description.is_empty() {
                println!("  {}", m.description);
            }
            println!("  SHA-256 {}", preview.wasm_sha256);
            if preview.capabilities.is_empty() {
                println!("  Capabilities: none (pure computation)");
            }
            for c in &preview.capabilities {
                println!("  May: {c}");
            }
            if let Some(v) = &preview.replaces {
                println!("  Replaces the installed version {v}");
            }
            if let Some(s) = grant_secrets.iter().find(|s| !m.capabilities.secrets_read.contains(s)) {
                bail!("--grant-secret {s}: the plugin does not declare that secret");
            }
            if !yes && !confirm("Install this plugin?")? {
                println!("Cancelled.");
                return Ok(ExitCode::from(1));
            }
            let info = mgr.install(&source, Some(&preview.wasm_sha256), &grant_secrets)?;
            println!("Installed {} in {}", info.id, info.dir);
            Ok(ExitCode::SUCCESS)
        }
        PluginCmd::Run { op, input, args, project } => {
            let proj = project_ref(project);
            let info = mgr.catalog(proj.as_ref()).utilities.into_iter().find(|u| u.id == op);
            let input = if info.as_ref().is_some_and(|u| u.no_input) { String::new() } else { crate::read_input(input)? };
            let args: BTreeMap<String, String> = args.into_iter().collect();
            let (out, logs) = mgr.run_utility(proj.as_ref(), &op, &input, &args)?;
            for l in logs {
                eprintln!("[{}] {}", l.level, l.message);
            }
            println!("{out}");
            Ok(ExitCode::SUCCESS)
        }
        PluginCmd::Trust { id, project, grant_secrets, yes } => {
            let proj = project_ref(project).context("not inside a /dev/all project")?;
            let info = mgr.list(Some(&proj)).into_iter().find(|p| p.id == id && p.scope == Scope::Project).with_context(|| format!("no project plugin {id}"))?;
            if let Some(e) = info.error.as_ref().filter(|_| info.status == Status::Broken) {
                bail!("{id}: {e}");
            }
            let sha = info.wasm_sha256.clone().unwrap_or_default();
            println!("{id} (project plugin) SHA-256 {sha}");
            for c in &info.capabilities {
                println!("  May: {c}");
            }
            if !yes && !confirm("Trust and run this project plugin on this machine?")? {
                println!("Cancelled.");
                return Ok(ExitCode::from(1));
            }
            mgr.trust(Some(&proj), Scope::Project, &id, &sha, &grant_secrets)?;
            println!("Trusted {id}.");
            Ok(ExitCode::SUCCESS)
        }
        PluginCmd::Uninstall { id } => {
            mgr.uninstall(&id)?;
            println!("Moved {id} to the trash.");
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// `devall util <plugin>:<op>` and `devall util list` additions.
pub fn utilities() -> Vec<devall_plugins::UtilityInfo> {
    let proj = project_ref(None);
    manager().map(|m| m.catalog(proj.as_ref()).utilities).unwrap_or_default()
}

pub fn run_utility(op: &str, input: Option<String>, args: Vec<(String, String)>) -> Result<String> {
    let mgr = manager()?;
    let proj = project_ref(None);
    let info = mgr.catalog(proj.as_ref()).utilities.into_iter().find(|u| u.id == op).with_context(|| format!("unknown operation `{op}` (see `devall util list`)"))?;
    let input = if info.no_input { String::new() } else { crate::read_input(input)? };
    let (out, logs) = mgr.run_utility(proj.as_ref(), op, &input, &args.into_iter().collect())?;
    for l in logs {
        eprintln!("[{}] {}", l.level, l.message);
    }
    Ok(out)
}
