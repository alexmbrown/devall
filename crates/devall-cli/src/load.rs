//! `devall load run <file>` — load tests from `load/*.load.yaml`, with k6-style exit codes
//! (0 passed, 99 thresholds failed, 105 aborted, 2 confirmation required) — `devall load agent`,
//! the worker that runs a share of a distributed test, and `devall load cloud verify|run`, the
//! relay's cloud load-generator regions (verified targets only).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use devall_core::{MemorySecretStore, SecretStore};
use devall_load::distributed::{DistributedOptions, K8sOptions, TOKEN_ENV, k8s_manifests, parse_agents, random_token};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Subcommand)]
pub enum LoadCmd {
    /// Run a load test (path relative to `load/`, e.g. `users.load.yaml`).
    Run {
        file: String,
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Environment id (overrides the test's `environment`).
        #[arg(short, long)]
        env: Option<String>,
        /// Confirm high-rate runs against non-local hosts / protected environments.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Don't save the run summary (kept on this device, in the app data folder — never in
        /// the repository).
        #[arg(long)]
        no_save: bool,
        /// Also write the run summary JSON (secrets masked) to this file, e.g. a CI artifact.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Don't read secrets from the OS keychain.
        #[arg(long)]
        no_keychain: bool,
        /// Send the load to the project's mock server (started for the run) instead of the real
        /// hosts, and save the run as a mock baseline. Tests with `target: mock` always do.
        #[arg(long)]
        against_mock: bool,
        /// Split the run across load agents: `host1:7777,host2:7777` (`dns:<headless-service>:7777`
        /// expands to every pod). Start them with `devall load agent`.
        #[arg(long)]
        agents: Option<String>,
        /// Relative share per agent, e.g. `2,1` (default: equal).
        #[arg(long)]
        agent_weights: Option<String>,
        /// Environment variable holding the agents' bearer token.
        #[arg(long, default_value = TOKEN_ENV)]
        agents_token_env: String,
        /// Send resolved `{{secret.X}}` values to the agents (refused otherwise when a request
        /// uses secrets). Only with agents and networks you trust.
        #[arg(long)]
        allow_secrets_to_agents: bool,
        /// PEM CA for agents served over https:// with a private certificate.
        #[arg(long)]
        agents_ca: Option<PathBuf>,
    },
    /// List load tests in the project.
    List {
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
    /// Run a load agent: a worker that runs its share of a distributed test
    /// (`devall load run … --agents`). Token from DEVALL_AGENT_TOKEN.
    Agent(AgentArgs),
    /// Cloud load-generator regions of the relay (Supporter / self-hosted): verify a target host,
    /// then `devall load cloud run <test> --regions weur,enam`.
    #[command(subcommand)]
    Cloud(CloudCmd),
}

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct AgentArgs {
    #[command(subcommand)]
    cmd: Option<AgentCmd>,
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:7777")]
    listen: String,
    /// Bearer token. Prefer the environment variable (command lines are visible to other users).
    #[arg(long)]
    token: Option<String>,
    /// Environment variable holding the token.
    #[arg(long, default_value = TOKEN_ENV)]
    token_env: String,
    /// PEM certificate chain: serve HTTPS (with --tls-key).
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    /// PEM private key for --tls-cert.
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,
}

#[derive(Subcommand)]
enum AgentCmd {
    /// Print Kubernetes manifests: a Secret with the token, a Deployment of agents and a headless
    /// Service (`--agents dns:<name>.<namespace>.svc:7777` reaches every pod).
    K8s {
        #[arg(long, default_value_t = 2)]
        replicas: u32,
        /// Image with the devall CLI (the repo's Dockerfile: `docker build --target agent`).
        #[arg(long)]
        image: String,
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long, default_value = "devall-load-agent")]
        name: String,
        #[arg(long, default_value_t = 7777)]
        port: u16,
        /// Put this environment variable's value in the Secret (default: a new random token).
        #[arg(long, default_value = TOKEN_ENV)]
        token_env: String,
        #[arg(long, default_value = "1")]
        cpu: String,
        #[arg(long, default_value = "512Mi")]
        memory: String,
    },
    /// Print a new random token for agents and coordinator.
    Token,
    /// Exit 0 when the agent on this machine answers (container HEALTHCHECK).
    Health {
        #[arg(long, default_value_t = 7777)]
        port: u16,
    },
}

fn env_token(var: &str) -> Option<String> {
    std::env::var(var).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

async fn health(port: u16) -> Result<ExitCode> {
    let fut = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        s.write_all(format!("GET /healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buf).to_string())
    };
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Ok(Ok(resp)) if resp.starts_with("HTTP/1.1 200") => {
            println!("healthy");
            Ok(ExitCode::SUCCESS)
        }
        Ok(Ok(resp)) => {
            eprintln!("unhealthy: {}", resp.lines().next().unwrap_or(""));
            Ok(ExitCode::from(1))
        }
        Ok(Err(e)) => {
            eprintln!("unreachable: {e}");
            Ok(ExitCode::from(1))
        }
        Err(_) => {
            eprintln!("timed out");
            Ok(ExitCode::from(1))
        }
    }
}

async fn agent(args: AgentArgs) -> Result<ExitCode> {
    match args.cmd {
        Some(AgentCmd::Token) => {
            println!("{}", random_token());
            return Ok(ExitCode::SUCCESS);
        }
        Some(AgentCmd::Health { port }) => return health(port).await,
        Some(AgentCmd::K8s { replicas, image, namespace, name, port, token_env, cpu, memory }) => {
            let token = match env_token(&token_env) {
                Some(t) => t,
                None => {
                    eprintln!("{token_env} is not set: generated a new token (it is in the Secret's stringData.token; give the coordinator the same value).");
                    random_token()
                }
            };
            print!("{}", k8s_manifests(&K8sOptions { name, namespace, replicas: replicas.max(1), image, port, token, cpu, memory }));
            return Ok(ExitCode::SUCCESS);
        }
        None => {}
    }
    let token = args.token.or_else(|| env_token(&args.token_env)).with_context(|| format!("set {} (or --token) to the shared agent token — `devall load agent token` makes one", args.token_env))?;
    let listen: std::net::SocketAddr = args.listen.parse().with_context(|| format!("--listen {}: expected host:port, e.g. 0.0.0.0:7777", args.listen))?;
    let tls = match (args.tls_cert, args.tls_key) {
        (Some(c), Some(k)) => Some((c, k)),
        _ => None,
    };
    let srv = devall_load::start_agent(devall_load::AgentOptions { listen, token, tls, version: env!("CARGO_PKG_VERSION").to_string() }).await?;
    eprintln!("devall load agent listening on {} — waiting for a coordinator (Ctrl-C to stop)", srv.url());
    if !srv.tls && !listen.ip().is_loopback() {
        eprintln!(
            "warning: plain HTTP: the token and the plans (including any secrets the coordinator was allowed to send) travel unencrypted. Use --tls-cert/--tls-key or a TLS terminator on untrusted networks."
        );
    }
    crate::mock::shutdown_signal().await;
    eprintln!("\nstopping…");
    srv.closed().await;
    Ok(ExitCode::SUCCESS)
}

pub async fn run(cmd: LoadCmd) -> Result<ExitCode> {
    match cmd {
        LoadCmd::Agent(args) => agent(args).await,
        LoadCmd::Cloud(c) => cloud(c).await,
        LoadCmd::List { project } => {
            let p = crate::find_project(project)?;
            for t in devall_load::store::list_tests(p.root())? {
                println!("{}", serde_json::to_string(&t)?);
            }
            Ok(ExitCode::SUCCESS)
        }
        LoadCmd::Run { file, project, env, yes, no_save, out, no_keychain, against_mock, agents, agent_weights, agents_token_env, allow_secrets_to_agents, agents_ca } => {
            let p = crate::find_project(project)?;
            let memory = MemorySecretStore::default();
            let keychain = devall_core::KeychainSecretStore::new();
            let store: &dyn SecretStore = if no_keychain || !devall_core::KeychainSecretStore::available() { &memory } else { &keychain };
            let path = file.trim_start_matches("load/").trim_start_matches("load\\").to_string();
            let distributed = match &agents {
                Some(list) => {
                    if against_mock {
                        bail!("--against-mock runs on this machine; it can't be combined with --agents");
                    }
                    let token = env_token(&agents_token_env).with_context(|| format!("set {agents_token_env} to the agents' token"))?;
                    Some(DistributedOptions { agents: parse_agents(list, agent_weights.as_deref())?, token, allow_secrets: allow_secrets_to_agents, ca_cert: agents_ca, ..Default::default() })
                }
                None => None,
            };
            let control = devall_load::RunControl::default();
            let stopper = control.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    eprintln!("\nstopping… (summary will be marked aborted)");
                    stopper.stop();
                }
            });
            let mock = if distributed.is_none() && (against_mock || devall_load::cli::wants_mock(p.root(), &path).unwrap_or(false)) {
                let h = devall_mock::MockServer::start(devall_mock::MockConfig::new(p.root())).await?;
                for e in h.load_errors() {
                    eprintln!("mock: {}: {}", e.file, e.error);
                }
                eprintln!("against the project's mock server {} ({} stubs)", h.url(), h.stub_count());
                Some(h)
            } else {
                None
            };
            if let Some(d) = &distributed {
                eprintln!("distributing across {} agent{}", d.agents.len(), if d.agents.len() == 1 { "" } else { "s" });
            }
            let opts = devall_load::cli::CliRunOptions {
                environment: env,
                confirmed: yes,
                write_summary: !no_save,
                summary_out: out,
                progress: None,
                control,
                mock_url: mock.as_ref().map(|h| h.url()),
                distributed,
            };
            let result = devall_load::cli::run_test_file(p.root(), &path, store, opts).await;
            if let Some(h) = mock {
                h.stop().await;
            }
            match result {
                Ok(outcome) => {
                    println!("{}", devall_load::cli::format_summary(&outcome.summary));
                    if let Some(f) = &outcome.file {
                        let path = devall_load::store::run_path(p.root(), f).map(|p| p.display().to_string()).unwrap_or_else(|_| f.clone());
                        println!("summary: {path}");
                    }
                    Ok(ExitCode::from(outcome.exit_code as u8))
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    Ok(ExitCode::from(devall_load::cli::error_exit_code(&e) as u8))
                }
            }
        }
    }
}

// ---------- cloud regions ----------

#[derive(Subcommand)]
pub enum CloudCmd {
    /// Prove you control a host before load-testing it from cloud regions (like loader.io): prints
    /// the token and where to serve it (`<origin>/.well-known/devall-load-verification.txt`, or a
    /// DNS TXT record `_devall-load.<host>`), then checks it.
    Verify {
        /// `api.example.com`, `https://api.example.com:8443`, `http://127.0.0.1:3000` (self-host).
        target: String,
        /// Keep checking for up to this many seconds (while you deploy the file).
        #[arg(long, default_value_t = 0)]
        wait: u64,
        /// Print the target (token, file URL, status) as JSON on stdout.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        relay: crate::relay::RelayArgs,
    },
    /// Run a load test from cloud regions of the relay (Supporter, or any self-hosted relay):
    /// `devall load cloud run users.load.yaml --regions weur,enam`. Open-model tests only; every
    /// target host must be verified first.
    Run {
        file: String,
        /// Region ids: wnam, enam, sam, weur, eeur, apac, oc, afr, me.
        #[arg(long)]
        regions: String,
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Environment id (overrides the test's `environment`).
        #[arg(short, long)]
        env: Option<String>,
        /// Send resolved `{{secret.X}}` values to the relay's edge (refused otherwise when a
        /// request uses secrets).
        #[arg(long)]
        allow_secrets_to_cloud: bool,
        /// Don't save the run summary (kept on this device, never in the repository).
        #[arg(long)]
        no_save: bool,
        /// Also write the run summary JSON (secrets masked) to this file, e.g. a CI artifact.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Don't read secrets from the OS keychain.
        #[arg(long)]
        no_keychain: bool,
        /// Print the summary as JSON on stdout instead of text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        relay: crate::relay::RelayArgs,
    },
    /// Regions, limits and verified targets of your account.
    Status {
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        relay: crate::relay::RelayArgs,
    },
    /// Forget a verified target.
    Remove {
        target: String,
        #[command(flatten)]
        relay: crate::relay::RelayArgs,
    },
}

fn print_target(v: &serde_json::Value) {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    println!("{}  {}", s("origin"), s("status").to_uppercase());
    match s("status").as_str() {
        "verified" => println!("  verified by {} until {}", s("method"), s("expiresAt")),
        _ => {
            println!("  token: {}", s("token"));
            println!("  serve it (the file holds just the token, HTTP 200, no redirect) at:");
            println!("    {}", s("fileUrl"));
            println!("  or add a DNS TXT record (covers every port of the host):");
            println!("    {}  TXT  \"{}\"", s("dnsName"), s("token"));
        }
    }
}

async fn cloud(cmd: CloudCmd) -> Result<ExitCode> {
    use devall_relay::ApiMethod;
    match cmd {
        CloudCmd::Verify { target, wait, json, relay } => {
            let c = crate::relay::client(&relay)?;
            let body = serde_json::json!({ "target": target });
            let t = c.api(ApiMethod::POST, "/v1/load/targets", Some(&body)).await?;
            if t["status"] == "verified" && wait == 0 {
                if json {
                    crate::relay::json_line(t);
                } else {
                    print_target(&t);
                }
                return Ok(ExitCode::SUCCESS);
            }
            if !json {
                print_target(&t);
                eprintln!("checking…");
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(wait);
            loop {
                match c.api(ApiMethod::POST, "/v1/load/targets/verify", Some(&body)).await {
                    Ok(v) => {
                        if json {
                            crate::relay::json_line(v);
                        } else {
                            print_target(&v);
                        }
                        return Ok(ExitCode::SUCCESS);
                    }
                    Err(devall_relay::RelayError::Rejected { status: 422, message, .. }) => {
                        if std::time::Instant::now() >= deadline {
                            if json {
                                crate::relay::json_line(serde_json::json!({ "status": "pending", "token": t["token"], "fileUrl": t["fileUrl"], "dnsName": t["dnsName"], "origin": t["origin"], "error": message }));
                            }
                            eprintln!("not verified: {message}");
                            if !json {
                                eprintln!("Serve the token, then run this again (or pass --wait 120 to keep checking).");
                            }
                            return Ok(ExitCode::from(1));
                        }
                    }
                    Err(devall_relay::RelayError::Rejected { status: 429, .. }) => {}
                    Err(e) => return Err(e.into()),
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
        CloudCmd::Status { json, relay } => {
            let c = crate::relay::client(&relay)?;
            let v = c.api(ApiMethod::GET, "/v1/load", None).await?;
            if json {
                crate::relay::json_line(v);
                return Ok(ExitCode::SUCCESS);
            }
            let l = &v["limits"];
            println!(
                "cloud load {} · up to {} regions · {} s · {} req/s · {} runs/day ({} today){}",
                if v["enabled"] == true { "enabled" } else { "DISABLED by the relay operator" },
                l["loadRegions"],
                l["loadMaxDurationSeconds"],
                if l["loadMaxRps"].is_null() { "unlimited".to_string() } else { l["loadMaxRps"].to_string() },
                if l["loadRunsPerDay"].is_null() { "unlimited".to_string() } else { l["loadRunsPerDay"].to_string() },
                v["runsToday"],
                if l["loadPrivateTargets"] == true { " · private targets allowed" } else { "" }
            );
            let regions: Vec<String> = v["regions"].as_array().into_iter().flatten().map(|r| format!("{} ({})", r["id"].as_str().unwrap_or(""), r["name"].as_str().unwrap_or(""))).collect();
            println!("regions: {}", regions.join(", "));
            for t in v["targets"].as_array().into_iter().flatten() {
                print_target(t);
            }
            Ok(ExitCode::SUCCESS)
        }
        CloudCmd::Remove { target, relay } => {
            let c = crate::relay::client(&relay)?;
            let q = url::form_urlencoded::Serializer::new(String::new()).append_pair("target", &target).finish();
            c.api(ApiMethod::DELETE, &format!("/v1/load/targets?{q}"), None).await?;
            eprintln!("Removed {target}.");
            Ok(ExitCode::SUCCESS)
        }
        CloudCmd::Run { file, regions, project, env, allow_secrets_to_cloud, no_save, out, no_keychain, json, relay } => {
            let p = crate::find_project(project)?;
            let memory = MemorySecretStore::default();
            let keychain = devall_core::KeychainSecretStore::new();
            let store: &dyn SecretStore = if no_keychain || !devall_core::KeychainSecretStore::available() { &memory } else { &keychain };
            let path = file.trim_start_matches("load/").trim_start_matches("load\\").to_string();
            let test = devall_load::store::read_test(p.root(), &path)?;
            let environment = devall_load::effective_environment(&test, env.as_deref(), None);
            let plan = devall_load::build_plan(&p, &path, &test, environment.as_deref(), store)?;
            let regions = devall_load::cloud::parse_regions(&regions)?;
            let prepared = match plan.cloud_plan(&devall_load::cloud::CloudOptions { regions: regions.clone(), allow_secrets: allow_secrets_to_cloud, run_id: String::new() }) {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("error: {e}");
                    return Ok(ExitCode::from(devall_load::cli::error_exit_code(&e) as u8));
                }
            };
            let c = std::sync::Arc::new(crate::relay::client(&relay)?);
            for w in &prepared.warnings {
                eprintln!("! {w}");
            }
            let planned = plan.test.executor.planned_duration();
            eprintln!(
                "running \"{}\" from {} for {:.0} s against {}",
                plan.test.name,
                regions.join(", "),
                planned.as_secs_f64(),
                prepared.origins.join(", ")
            );
            let run_id = prepared.plan.run_id.clone();
            let body = serde_json::to_value(&prepared.plan)?;
            // Ctrl+C asks the relay to stop the run at its next segment (≤ 10 s) and keeps waiting
            // for the partial result.
            let stopper = c.clone();
            let stop_path = format!("/v1/load/runs/{run_id}/stop");
            let ctrlc = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    eprintln!("\nstopping… (the relay ends the run at its next segment)");
                    let _ = stopper.api(ApiMethod::POST, &stop_path, None).await;
                }
            });
            let result = c.api_with_timeout(ApiMethod::POST, "/v1/load/runs", Some(&body), planned + Duration::from_secs(120)).await;
            ctrlc.abort();
            let v = match result {
                Ok(v) => v,
                Err(devall_relay::RelayError::Rejected { status, code, message, .. }) => {
                    eprintln!("error: {message} ({code}, HTTP {status})");
                    return Ok(ExitCode::from(1));
                }
                Err(e) => return Err(e.into()),
            };
            let result: devall_load::cloud::CloudResult = serde_json::from_value(v).context("unexpected answer from the relay")?;
            let secrets = plan.secret_values.clone();
            let summary = plan.cloud_summary(result, &prepared.warnings);
            let file = if no_save { None } else { Some(devall_load::store::write_run(p.root(), &summary, &secrets)?) };
            if let Some(out) = &out {
                devall_core::fsutil::atomic_write(out, devall_load::store::summary_json(&summary, &secrets).as_bytes())?;
            }
            if json {
                crate::relay::json_line(serde_json::from_str(&devall_load::store::summary_json(&summary, &secrets))?);
            } else {
                println!("{}", devall_load::cli::format_summary(&summary));
            }
            if let Some(f) = &file {
                let f = devall_load::store::run_path(p.root(), f).map(|p| p.display().to_string()).unwrap_or_else(|_| f.clone());
                eprintln!("summary: {f}");
            }
            Ok(ExitCode::from(devall_load::cli::exit_code(&summary) as u8))
        }
    }
}
