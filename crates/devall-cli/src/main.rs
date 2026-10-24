//! `devall` — the same project files the desktop app edits, runnable from a terminal or CI.

mod byo;
mod knowledge;
mod load;
mod mock;
mod openapi;
mod plugin;
mod relay;
mod scan;
mod run;

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use devall_core::Project;

#[derive(Parser)]
#[command(name = "devall", version, about = "/dev/all developer toolkit — command line")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new project (devall.yaml, http/, environments/, .gitignore entries).
    Init {
        /// Folder to initialize (default: current directory).
        dir: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Send requests in tree order with assertions and captures; exit 1 when anything fails.
    /// Also runs a `.http` / `.hurl` file directly: `devall run api.hurl --var host=localhost`.
    Run(run::RunArgs),
    /// Developer utilities: `devall util list`, `devall util jwt.decode <token>`; plugin ops are
    /// `<plugin>:<op>`.
    Util {
        /// Operation id, or `list`.
        op: String,
        /// Input text; omit or pass `-` to read stdin.
        input: Option<String>,
        /// Operation arguments, e.g. --arg key=secret --arg alg=HS256
        #[arg(short, long = "arg", value_parser = parse_kv)]
        args: Vec<(String, String)>,
    },
    /// Import a cURL command (from stdin or an argument) as a request file.
    ImportCurl {
        /// The cURL command; omit to read stdin.
        command: Option<String>,
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Folder under http/ to create the request in.
        #[arg(long, default_value = "")]
        folder: String,
    },
    /// Load tests (load/*.load.yaml): `devall load run users.load.yaml --env staging`.
    #[command(subcommand)]
    Load(load::LoadCmd),
    /// The project's mock server: `devall mock serve --port 8080`, `devall mock check`.
    #[command(subcommand)]
    Mock(mock::MockCmd),
    /// WebAssembly plugins: `devall plugin list`, `devall plugin install ./my-plugin.zip`,
    /// `devall plugin run hmac-signer:sign "text"`.
    #[command(subcommand)]
    Plugin(plugin::PluginCmd),
    /// OpenAPI checks: `devall openapi lint api.yaml`, `devall openapi drift --spec api.yaml`.
    #[command(subcommand)]
    Openapi(openapi::OpenApiCmd),
    /// Tunnels from the terminal: hosted relay (`devall tunnel http --port 3000`,
    /// `devall tunnel tcp --port 5432`) or your own, free (`devall tunnel ngrok --port 8080`,
    /// `devall tunnel cloudflared --port 8080`, `devall tunnel tailscale --port 8080`).
    #[command(subcommand)]
    Tunnel(relay::TunnelCmd),
    /// Connect to a raw TCP tunnel: `devall tcp connect <name> --local-port 15432`.
    #[command(subcommand)]
    Tcp(relay::TcpCmd),
    /// The project's hosted webhook inbox: `devall inbox listen` (buffered + live events into the
    /// project's webhook store).
    #[command(subcommand)]
    Inbox(relay::InboxCmd),
    /// Serve this project to coding agents over MCP (stdio by default):
    /// `claude mcp add devall -- devall mcp --project <path>`.
    Mcp {
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Allow send_request to send POST/PUT/PATCH/DELETE (never in protected environments).
        #[arg(long)]
        allow_writes: bool,
        /// Default environment id for send_request.
        #[arg(short, long)]
        env: Option<String>,
        /// Serve Streamable HTTP on 127.0.0.1:<port> (0 = random) instead of stdio.
        #[arg(long)]
        http: Option<u16>,
        /// Don't read secrets from the OS keychain.
        #[arg(long)]
        no_keychain: bool,
        /// Don't let agents write shared notes or this project's personal to-dos (allowed by default).
        #[arg(long)]
        read_only_notes: bool,
    },
    /// Shared project notes (notes/*.md).
    #[command(subcommand)]
    Notes(knowledge::NotesCmd),
    /// Find credentials in project files; `--staged` for git pre-commit, `--install-hook` to set it up.
    Scan(scan::ScanArgs),
    /// Your personal to-do list (stored in the app data folder, never in the repo).
    #[command(subcommand)]
    Todo(knowledge::TodoCmd),
}

pub fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).ok_or_else(|| format!("expected key=value, got `{s}`"))
}

pub fn read_input(arg: Option<String>) -> Result<String> {
    match arg.as_deref() {
        Some("-") | None => {
            if std::io::stdin().is_terminal() && arg.is_none() {
                return Ok(String::new());
            }
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            // Windows PowerShell prefixes piped text with a BOM.
            Ok(s.trim_start_matches('\u{feff}').trim_end_matches(['\r', '\n']).to_string())
        }
        Some(s) => Ok(s.to_string()),
    }
}

/// Finds the project containing `dir` (or the current directory), walking up like git does.
pub fn find_project(dir: Option<PathBuf>) -> Result<Project> {
    let start = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    let mut cur: Option<&Path> = Some(&start);
    while let Some(d) = cur {
        if d.join(devall_core::project::MANIFEST).is_file() {
            let p = Project::open(d)?;
            // Shares the desktop app's per-project data folder (history, load runs…); moves what
            // older versions kept in `<project>/.devall/` the first time.
            devall_core::local::touch_project(p.root(), &p.manifest.name);
            return Ok(p);
        }
        cur = d.parent();
    }
    bail!("no devall.yaml found in {} or its parents (run `devall init`)", start.display())
}

/// Same location the desktop app uses for user data (Tauri `app_data_dir`), so to-dos, history
/// and settings are shared.
pub fn app_data_dir() -> Result<PathBuf> {
    devall_core::local::app_data_dir().context("no user data directory on this system")
}

#[tokio::main]
async fn main() -> ExitCode {
    match dispatch(Cli::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

async fn dispatch(cli: Cli) -> Result<ExitCode> {
    // Requests sent by `devall run`, load tests, … may use plugin auth providers and hooks.
    if matches!(cli.command, Command::Run(_) | Command::Load(_) | Command::Mcp { .. }) {
        plugin::install_request_bridge();
    }
    match cli.command {
        Command::Init { dir, name } => {
            let dir = match dir {
                Some(d) => d,
                None => std::env::current_dir()?,
            };
            std::fs::create_dir_all(&dir)?;
            let name = name.unwrap_or_else(|| {
                dir.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "My project".into())
            });
            let p = Project::init(&dir, &name)?;
            println!("Created project \"{}\" in {}", p.manifest.name, p.root().display());
            Ok(ExitCode::SUCCESS)
        }
        Command::Run(args) => run::run(args).await,
        Command::Util { op, input, args } => {
            if op == "list" {
                for o in devall_utils::list() {
                    println!("{:<18} {:<11} {}", o.id, o.category, o.description);
                }
                for o in plugin::utilities() {
                    println!("{:<18} {:<11} {}", o.id, o.category, if o.description.is_empty() { &o.name } else { &o.description });
                }
                return Ok(ExitCode::SUCCESS);
            }
            if op.contains(':') {
                println!("{}", plugin::run_utility(&op, input, args)?);
                return Ok(ExitCode::SUCCESS);
            }
            let info = devall_utils::info(&op).with_context(|| format!("unknown operation `{op}` (see `devall util list`)"))?;
            let input = if info.no_input { String::new() } else { read_input(input)? };
            let args = serde_json::Value::Object(args.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect());
            println!("{}", devall_utils::run(&op, &input, &args)?);
            Ok(ExitCode::SUCCESS)
        }
        Command::ImportCurl { command, project, folder } => {
            let p = find_project(project)?;
            let cmd = read_input(command)?;
            let req = devall_core::curl::parse_curl(&cmd)?;
            let rel = p.create_request(&folder, req)?;
            println!("Created http/{rel}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Notes(cmd) => knowledge::notes(cmd),
        Command::Todo(cmd) => knowledge::todo(cmd),
        Command::Scan(args) => scan::run(args),
        Command::Load(cmd) => load::run(cmd).await,
        Command::Mock(cmd) => mock::run(cmd).await,
        Command::Plugin(cmd) => plugin::run(cmd).await,
        Command::Openapi(cmd) => openapi::run(cmd).await,
        Command::Tunnel(cmd) => relay::tunnel(cmd).await,
        Command::Tcp(cmd) => relay::tcp(cmd).await,
        Command::Inbox(cmd) => relay::inbox(cmd).await,
        Command::Mcp { project, allow_writes, env, http, no_keychain, read_only_notes } => {
            let p = find_project(project)?;
            // No UI to ask from the CLI: writes follow --allow-writes and protected environments are
            // never used (the desktop app's MCP endpoint can ask per call instead).
            let opts = devall_mcp::McpOptions { allow_writes, confirm_writes: false, environment: env, read_only_local: read_only_notes, app_data_dir: app_data_dir().ok(), no_keychain };
            match http {
                Some(port) => {
                    let srv = devall_mcp::serve_http(p.root(), opts, port).await?;
                    eprintln!("MCP endpoint {}  (Authorization: Bearer {})", srv.url, srv.token);
                    mock::shutdown_signal().await;
                    drop(srv);
                }
                // stdout carries the protocol: nothing else may print to it.
                None => devall_mcp::serve_stdio(p.root(), opts).await?,
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_the_relay_commands() {
        let c = Cli::try_parse_from(["devall", "tunnel", "http", "--port", "3000", "--subdomain", "my-app", "--json", "--relay", "ws://127.0.0.1:8787"]).unwrap();
        match c.command {
            Command::Tunnel(relay::TunnelCmd::Http { port, subdomain, json, rewrite_host, https, relay, .. }) => {
                assert_eq!((port, subdomain.as_deref(), json, rewrite_host, https), (3000, Some("my-app"), true, false, false));
                assert_eq!(relay.relay.as_deref(), Some("ws://127.0.0.1:8787"));
            }
            _ => panic!("expected tunnel http"),
        }
        assert!(Cli::try_parse_from(["devall", "tunnel", "http"]).is_err(), "--port is required");
        let c = Cli::try_parse_from(["devall", "inbox", "listen", "--key", "k", "--no-store"]).unwrap();
        assert!(matches!(c.command, Command::Inbox(relay::InboxCmd::Listen { no_store: true, ref key, .. }) if key.as_deref() == Some("k")));
        let c = Cli::try_parse_from(["devall", "load", "cloud", "run", "smoke.load.yaml", "--regions", "weur,enam", "--allow-secrets-to-cloud"]).unwrap();
        assert!(matches!(c.command, Command::Load(load::LoadCmd::Cloud(load::CloudCmd::Run { ref regions, allow_secrets_to_cloud: true, .. })) if regions == "weur,enam"));
        let c = Cli::try_parse_from(["devall", "load", "cloud", "verify", "https://api.example.com", "--wait", "60"]).unwrap();
        assert!(matches!(c.command, Command::Load(load::LoadCmd::Cloud(load::CloudCmd::Verify { wait: 60, .. }))));
    }
}
