//! `devall mock serve | check | record | health` — the project's mock server outside the app
//! (CI, Docker, a teammate without the app).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use devall_mock::{CHAOS_PRESETS, ChaosConfig, JournalEvent, MockConfig, MockServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    /// One readable line per request.
    Text,
    /// One JSON object per request (method, url, status, stubName, totalTimeMs, …).
    Json,
    /// No request log.
    Off,
}

#[derive(Subcommand)]
pub enum MockCmd {
    /// Serve the project's mocks/ (WireMock-compatible, admin API under /__admin) until Ctrl+C.
    /// Later flags override earlier ones (the Docker image appends yours to its defaults).
    #[command(args_override_self = true)]
    Serve {
        /// Project folder (default: current directory or a parent).
        #[arg(short, long)]
        project: Option<PathBuf>,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        /// Address to listen on: 127.0.0.1 (default) or 0.0.0.0 (all interfaces, e.g. in Docker).
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Chaos preset (slow, flaky, broken, all) or a JSON/YAML file with a chaos config.
        #[arg(long)]
        chaos: Option<String>,
        /// Request log on stdout.
        #[arg(long, value_enum, default_value_t = LogFormat::Text)]
        log: LogFormat,
        /// Also list every stub and data source at startup and log mapping reloads.
        #[arg(short, long)]
        verbose: bool,
        /// Template every response (WireMock's --global-response-templating).
        #[arg(long)]
        global_templating: bool,
        /// Add permissive CORS headers to stub responses (WireMock's --enable-stub-cors).
        #[arg(long)]
        cors: bool,
        /// Exit with an error if any mapping fails to load (instead of serving the rest).
        #[arg(long)]
        strict: bool,
        /// gRPC listener port for mocks/grpc (0 = any free port). Default: 50051 when
        /// mocks/grpc has .proto files.
        #[arg(long)]
        grpc_port: Option<u16>,
        /// Don't start the gRPC listener.
        #[arg(long, conflicts_with = "grpc_port")]
        no_grpc: bool,
        /// MQTT broker port for mocks/mqtt (0 = any free port). Default: 1883 when mocks/mqtt has
        /// behaviour files.
        #[arg(long)]
        mqtt_port: Option<u16>,
        /// Don't start the MQTT broker.
        #[arg(long, conflicts_with = "mqtt_port")]
        no_mqtt: bool,
    },
    /// Load every mapping and data source; exit 1 if any fails (for CI).
    Check {
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Proxy to a real API and record its responses as stubs (saved to mocks/mappings/ on Ctrl+C).
    Record {
        /// The API to record, e.g. https://api.example.com
        #[arg(long)]
        target: String,
        #[arg(short, long)]
        project: Option<PathBuf>,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Record request headers to match on (repeatable), e.g. --match-header Accept
        #[arg(long = "match-header")]
        match_headers: Vec<String>,
    },
    /// Exit 0 when a mock server answers /__admin/health (for Docker / CI health checks).
    Health {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
    /// Publish the project's stubs to the hosted relay so they keep answering while this machine
    /// is off (https://<name>.mock.<domain>/…). Checks for features hosted mocks don't support first.
    Publish {
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Hosted mock name (3–40 chars a-z 0-9 -); default: the project name.
        #[arg(long)]
        name: Option<String>,
        /// Don't answer CORS preflights / add access-control-allow-origin.
        #[arg(long)]
        no_cors: bool,
        /// Template every response (WireMock's --global-response-templating).
        #[arg(long)]
        global_templating: bool,
        /// Only print what would be uploaded and the warnings.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        relay: crate::relay::RelayArgs,
    },
    /// Remove a published (hosted) mock.
    Unpublish {
        name: String,
        #[command(flatten)]
        relay: crate::relay::RelayArgs,
    },
    /// List your published (hosted) mocks.
    Published {
        #[command(flatten)]
        relay: crate::relay::RelayArgs,
    },
    /// Recent requests a published mock received (newest first).
    Journal {
        name: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        relay: crate::relay::RelayArgs,
    },
}

/// Default gRPC port when `mocks/grpc` has .proto files.
const DEFAULT_GRPC_PORT: u16 = 50051;
/// Default MQTT port when `mocks/mqtt` has behaviour files.
const DEFAULT_MQTT_PORT: u16 = 1883;

/// Ctrl+C everywhere, plus SIGTERM on Unix (`docker stop`; PID 1 ignores unhandled signals).
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn bind_all(bind: &str) -> Result<bool> {
    match bind.trim() {
        "0.0.0.0" | "::" | "[::]" | "*" | "all" => Ok(true),
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | "" => Ok(false),
        other => bail!("--bind supports 127.0.0.1 (this machine) or 0.0.0.0 (all interfaces), not {other}"),
    }
}

/// A preset name or a JSON/YAML file.
pub fn chaos_from(arg: &str) -> Result<ChaosConfig> {
    if let Some(c) = ChaosConfig::preset(arg) {
        return Ok(c);
    }
    let path = Path::new(arg);
    if !path.is_file() {
        let names: Vec<&str> = CHAOS_PRESETS.iter().map(|(n, _)| *n).collect();
        bail!("--chaos {arg}: not a preset ({}) or a file", names.join(", "));
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut c: ChaosConfig = if text.trim_start().starts_with('{') { serde_json::from_str(&text)? } else { serde_yaml_ng::from_str(&text)? };
    // A file that sets rates but forgets `enabled` means "on".
    if !text.contains("enabled") {
        c.enabled = true;
    }
    Ok(c)
}

/// The project root and name: a /dev/all project (devall.yaml here or above), or else any folder
/// with a `mocks/` directory — so a plain WireMock layout mounted at `<dir>/mocks` works too.
pub(crate) fn mock_root(dir: Option<PathBuf>) -> Result<(PathBuf, String)> {
    if let Ok(p) = crate::find_project(dir.clone()) {
        return Ok((p.root().to_path_buf(), p.manifest.name.clone()));
    }
    let d = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    if !d.join(devall_mock::MOCKS_DIR).is_dir() {
        bail!("{} has no devall.yaml and no mocks/ folder", d.display());
    }
    let name = d.canonicalize().ok().and_then(|c| c.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "mocks".into());
    Ok((d, name))
}

fn display_host(bind_all: bool) -> &'static str {
    if bind_all { "0.0.0.0" } else { "127.0.0.1" }
}

async fn publish(project: Option<PathBuf>, name: Option<String>, settings: devall_relay::MockSettings, dry_run: bool, relay: &crate::relay::RelayArgs) -> Result<ExitCode> {
    let (root, project_name) = mock_root(project)?;
    let name = name.unwrap_or_else(|| devall_relay::hosted_mock::suggest_name(&project_name));
    devall_relay::hosted_mock::check_name(&name)?;
    let mocks = devall_mock::load_project(&root);
    for e in mocks.problems() {
        eprintln!("✗ {e} (not published)");
    }
    let mappings: Vec<serde_json::Value> = mocks.mappings.into_iter().map(|m| m.mapping).collect();
    let client = if dry_run { None } else { Some(crate::relay::client(relay)?) };
    let limits = match &client {
        Some(c) => c.account().await.ok().map(|a| a.limits),
        None => None,
    };
    let files_dir = root.join(devall_mock::MOCKS_DIR).join(devall_mock::FILES_DIR);
    let (payload, plan) = devall_relay::hosted_mock::build_payload(mappings, &files_dir, settings, limits.as_ref())?;
    eprintln!("{} stubs, {} files, {} bytes", plan.stubs, plan.files.len(), plan.bytes);
    for w in &plan.warnings {
        eprintln!("⚠ {}: {} — {}", w.stub, w.feature, w.message);
    }
    if !plan.blocking.is_empty() {
        for b in &plan.blocking {
            eprintln!("✗ {b}");
        }
        bail!("can't publish on your plan (or self-host the relay for free)");
    }
    let Some(client) = client else {
        eprintln!("dry run: nothing uploaded");
        return Ok(ExitCode::SUCCESS);
    };
    let r = client.publish_mock(&name, &payload).await?;
    for w in r.warnings.iter().filter(|w| !plan.warnings.contains(w)) {
        eprintln!("⚠ {}: {} — {}", w.stub, w.feature, w.message);
    }
    println!("{}", r.url);
    if let Some(p) = r.path_url.as_ref().filter(|p| **p != r.url) {
        eprintln!("also at {p}");
    }
    eprintln!("Published \"{}\": {} stubs. Re-run to update; `devall mock unpublish {}` to remove.", r.name, r.stubs, r.name);
    Ok(ExitCode::SUCCESS)
}

pub async fn run(cmd: MockCmd) -> Result<ExitCode> {
    match cmd {
        MockCmd::Publish { project, name, no_cors, global_templating, dry_run, relay } => {
            publish(project, name, devall_relay::MockSettings { cors: !no_cors, global_templating }, dry_run, &relay).await
        }
        MockCmd::Unpublish { name, relay } => {
            let c = crate::relay::client(&relay)?;
            c.api(devall_relay::ApiMethod::DELETE, &format!("/v1/mocks/{name}"), None).await?;
            eprintln!("Unpublished {name}.");
            Ok(ExitCode::SUCCESS)
        }
        MockCmd::Published { relay } => {
            let c = crate::relay::client(&relay)?;
            let v = c.api(devall_relay::ApiMethod::GET, "/v1/mocks", None).await?;
            let list = v["mocks"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                eprintln!("No published mocks. `devall mock publish` publishes this project's.");
            }
            for m in list {
                println!("{:<24} {:>5} stubs  {:>6} today  {}", m["name"].as_str().unwrap_or(""), m["stubs"], m["requestsToday"], m["url"].as_str().unwrap_or(""));
            }
            Ok(ExitCode::SUCCESS)
        }
        MockCmd::Journal { name, limit, json, relay } => {
            let c = crate::relay::client(&relay)?;
            let v = c.api(devall_relay::ApiMethod::GET, &format!("/v1/mocks/{name}/requests?limit={limit}"), None).await?;
            let list = v["requests"].as_array().cloned().unwrap_or_default();
            for r in list {
                if json {
                    println!("{r}");
                } else {
                    println!(
                        "{}  {:<6} {:<40} {} {}",
                        r["at"].as_str().unwrap_or(""),
                        r["method"].as_str().unwrap_or(""),
                        r["url"].as_str().unwrap_or(""),
                        r["status"],
                        if r["matched"].as_bool() == Some(true) { r["stubName"].as_str().unwrap_or("matched").to_string() } else { "(no stub matched)".into() }
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        MockCmd::Check { project, json } => check(project, json),
        MockCmd::Health { host, port } => health(&host, port).await,
        MockCmd::Serve { project, port, bind, chaos, log, verbose, global_templating, cors, strict, grpc_port, no_grpc, mqtt_port, no_mqtt } => {
            let (root, name) = mock_root(project)?;
            let mut cfg = MockConfig::new(&root);
            cfg.port = port;
            cfg.bind_all = bind_all(&bind)?;
            cfg.global_templating = global_templating;
            cfg.stub_cors = cors;
            if let Some(c) = &chaos {
                cfg.chaos = chaos_from(c)?;
            }
            if strict {
                let mocks = devall_mock::load_project(&root);
                if !mocks.problems().is_empty() {
                    for e in mocks.problems() {
                        eprintln!("✗ {e}");
                    }
                    bail!("{} mapping/data problems (--strict)", mocks.problems().len());
                }
            }
            let all = cfg.bind_all;
            let handle = MockServer::start(cfg).await?;
            let errors = handle.load_errors();
            eprintln!(
                "/dev/all mock server for \"{}\" on http://{}:{} — {} stubs{}",
                name,
                display_host(all),
                handle.port(),
                handle.stub_count(),
                if errors.is_empty() { String::new() } else { format!(", {} failed to load", errors.len()) }
            );
            eprintln!("admin API: http://{}:{}/__admin  ·  Ctrl+C to stop", display_host(all), handle.port());
            for e in &errors {
                eprintln!("  ✗ {}: {}", e.file, e.error);
            }
            // Other protocols: explicit ports must bind; default ports fall back to a free one.
            let folders = devall_mock::protocol_folders(&root);
            let grpc = if no_grpc { None } else { grpc_port.map(|p| (p, true)).or(folders.grpc.then_some((DEFAULT_GRPC_PORT, false))) };
            if let Some((p, explicit)) = grpc {
                let port = match handle.start_grpc(p).await {
                    Ok(p) => p,
                    Err(e) if !explicit => {
                        eprintln!("gRPC: port {p} is busy ({e}); using a free port");
                        handle.start_grpc(0).await?
                    }
                    Err(e) => {
                        handle.stop().await;
                        return Err(e.into());
                    }
                };
                eprintln!("gRPC (h2c, reflection on): {}:{port}", display_host(all));
            }
            let mqtt = if no_mqtt { None } else { mqtt_port.map(|p| (p, true)).or(folders.mqtt.then_some((DEFAULT_MQTT_PORT, false))) };
            if let Some((p, explicit)) = mqtt {
                let port = match handle.start_mqtt(p).await {
                    Ok(p) => p,
                    Err(e) if !explicit => {
                        eprintln!("MQTT: port {p} is busy ({e}); using a free port");
                        handle.start_mqtt(0).await?
                    }
                    Err(e) => {
                        handle.stop().await;
                        return Err(e.into());
                    }
                };
                eprintln!("MQTT broker: mqtt://{}:{port}", display_host(all));
            }
            let protocols = handle.protocols();
            for g in &protocols.graphql {
                eprintln!("GraphQL: http://{}:{}{g}", display_host(all), handle.port());
            }
            if protocols.websocket_stubs > 0 {
                eprintln!("WebSocket: {} stub{} on ws://{}:{}", protocols.websocket_stubs, if protocols.websocket_stubs == 1 { "" } else { "s" }, display_host(all), handle.port());
            }
            if let Some(c) = &chaos {
                eprintln!("chaos: {c}");
            }
            if verbose {
                for m in devall_mock::load_project(&root).mappings {
                    eprintln!("  · {} ({})", m.name.unwrap_or_else(|| m.id.clone()), m.file);
                }
                for d in handle.data_sources() {
                    eprintln!("  ⊞ data source {} — {} rows ({})", d.name, d.rows, d.file);
                }
            }
            let mut events = handle.subscribe();
            let printer = tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(JournalEvent::Request(r)) => match log {
                            LogFormat::Off => {}
                            LogFormat::Json => println!("{}", serde_json::to_string(&r).unwrap_or_default()),
                            LogFormat::Text => {
                                let what = match (&r.fault, r.was_matched, &r.stub_name) {
                                    (Some(f), _, _) => format!("fault {f}"),
                                    (None, false, _) => "no stub matched".to_string(),
                                    (None, true, Some(n)) => format!("→ {n}"),
                                    (None, true, None) => format!("→ stub {}", r.stub_id.as_deref().unwrap_or("?").chars().take(8).collect::<String>()),
                                };
                                let when = chrono_like(r.logged_date);
                                println!("{when} {:<7} {} {} {}ms {what}{}", r.method, r.url, r.status, r.total_time_ms, if r.proxied { " (proxied)" } else { "" });
                            }
                        },
                        Ok(JournalEvent::Message(m)) => match log {
                            LogFormat::Off => {}
                            LogFormat::Json => println!("{}", serde_json::to_string(&m).unwrap_or_default()),
                            LogFormat::Text => {
                                let proto = match m.protocol {
                                    devall_mock::Protocol::Websocket => "WS",
                                    devall_mock::Protocol::Grpc => "gRPC",
                                    devall_mock::Protocol::Mqtt => "MQTT",
                                };
                                let dir = match m.direction {
                                    devall_mock::MessageDirection::In => "→",
                                    devall_mock::MessageDirection::Out => "←",
                                    devall_mock::MessageDirection::Event => "·",
                                };
                                let what = if m.body.is_empty() { m.meta.get("event").and_then(|e| e.as_str()).unwrap_or("").to_string() } else { m.body.chars().take(120).collect() };
                                println!("{} {proto:<5} {dir} {} {what}", chrono_like(m.at), m.channel);
                            }
                        },
                        Ok(JournalEvent::MappingsChanged { stub_count }) if verbose => eprintln!("mappings changed: {stub_count} stubs"),
                        Ok(JournalEvent::Stopped) => break,
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => eprintln!("(skipped {n} log lines)"),
                        Err(_) => break,
                    }
                }
            });
            // Stop on Ctrl+C / SIGTERM, or when stopped through POST /__admin/shutdown.
            let h = handle.clone();
            tokio::select! {
                _ = shutdown_signal() => eprintln!("\nstopping…"),
                _ = async move { while h.is_running() { tokio::time::sleep(Duration::from_millis(250)).await; } } => eprintln!("stopped via the admin API"),
            }
            handle.stop().await;
            printer.abort();
            Ok(ExitCode::SUCCESS)
        }
        MockCmd::Record { target, project, port, bind, match_headers } => {
            let (root, _) = mock_root(project)?;
            let mut cfg = MockConfig::new(&root);
            cfg.port = port;
            cfg.bind_all = bind_all(&bind)?;
            let all = cfg.bind_all;
            let handle = MockServer::start(cfg).await?;
            let mut spec = serde_json::json!({ "targetBaseUrl": target, "persist": true });
            if !match_headers.is_empty() {
                spec["captureHeaders"] = serde_json::Value::Object(match_headers.iter().map(|h| (h.clone(), serde_json::json!({}))).collect());
            }
            if let Err(e) = handle.start_recording(spec) {
                handle.stop().await;
                bail!("could not start recording: {e}");
            }
            eprintln!("Recording {target} through http://{}:{} — send requests there, Ctrl+C to save the stubs", display_host(all), handle.port());
            shutdown_signal().await;
            let result = handle.stop_recording();
            handle.stop().await;
            match result {
                Ok(v) => {
                    let n = v.get("mappings").and_then(|m| m.as_array()).map_or(0, Vec::len);
                    eprintln!("\nSaved {n} stub{} to mocks/mappings/", if n == 1 { "" } else { "s" });
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => bail!("could not save the recording: {e}"),
            }
        }
    }
}

/// `HH:MM:SS` (UTC) from epoch milliseconds, without pulling in a date crate.
fn chrono_like(ms: i64) -> String {
    let s = ms.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn check(project: Option<PathBuf>, json: bool) -> Result<ExitCode> {
    let (root, _) = mock_root(project)?;
    let mocks = devall_mock::load_project(&root);
    let problems = mocks.problems();
    if json {
        let out = serde_json::json!({
            "stubs": mocks.mappings.len(),
            "dataSources": mocks.data_sources,
            "loadErrors": mocks.load_errors,
            "ok": problems.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("{} stub{} in mocks/mappings/", mocks.mappings.len(), if mocks.mappings.len() == 1 { "" } else { "s" });
        for d in &mocks.data_sources {
            match &d.error {
                Some(e) => println!("✗ data source {} ({}): {e}", d.name, d.file),
                None => println!("  data source {} — {} rows, columns {}", d.name, d.rows, d.columns.join(", ")),
            }
        }
        for e in &mocks.load_errors {
            println!("✗ {}: {}", e.file, e.error);
        }
        println!("{}", if problems.is_empty() { "✓ all mappings load".to_string() } else { format!("{} problem{}", problems.len(), if problems.len() == 1 { "" } else { "s" }) });
    }
    Ok(if problems.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

async fn health(host: &str, port: u16) -> Result<ExitCode> {
    let fut = async {
        let mut s = tokio::net::TcpStream::connect((host, port)).await?;
        s.write_all(format!("GET /__admin/health HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buf).to_string())
    };
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Ok(Ok(resp)) if resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200") => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_and_chaos_arguments() {
        assert!(bind_all("0.0.0.0").unwrap());
        assert!(!bind_all("127.0.0.1").unwrap());
        assert!(bind_all("10.0.0.2").is_err());
        assert!(chaos_from("flaky").unwrap().error_rate > 0.0);
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("c.yaml");
        std::fs::write(&f, "latencyMs: 250\nerrorRate: 0.2\n").unwrap();
        let c = chaos_from(f.to_str().unwrap()).unwrap();
        assert!(c.enabled && c.latency_ms == 250 && c.error_rate == 0.2);
        assert!(chaos_from("nope").is_err());
        assert_eq!(chrono_like(3_723_000), "01:02:03");
    }

    #[tokio::test]
    async fn publish_dry_run_checks_without_uploading() {
        let dir = tempfile::tempdir().unwrap();
        devall_core::Project::init(dir.path(), "Pet Store").unwrap();
        let maps = dir.path().join("mocks/mappings");
        std::fs::create_dir_all(&maps).unwrap();
        std::fs::create_dir_all(dir.path().join("mocks/__files")).unwrap();
        std::fs::write(dir.path().join("mocks/__files/pets.json"), "[]").unwrap();
        std::fs::write(maps.join("ok.json"), r#"{"request":{"urlPath":"/pets"},"response":{"status":200,"bodyFileName":"pets.json"}}"#).unwrap();
        std::fs::write(maps.join("xml.json"), r#"{"request":{"bodyPatterns":[{"equalToXml":"<a/>"}]},"response":{"status":200}}"#).unwrap();
        let code = publish(Some(dir.path().to_path_buf()), None, devall_relay::MockSettings::default(), true, &crate::relay::RelayArgs::default()).await.unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(publish(Some(dir.path().to_path_buf()), Some("Bad Name".into()), devall_relay::MockSettings::default(), true, &crate::relay::RelayArgs::default()).await.is_err());
    }

    #[test]
    fn self_hosted_relay_from_flags() {
        let args = crate::relay::RelayArgs { relay: Some("ws://127.0.0.1:9".into()), token: Some("cli-token-0123456789".into()) };
        let c = crate::relay::client(&args).unwrap();
        assert_eq!(c.endpoint().as_str(), "ws://127.0.0.1:9/");
    }

    #[tokio::test]
    async fn check_and_health() {
        let dir = tempfile::tempdir().unwrap();
        devall_core::Project::init(dir.path(), "m").unwrap();
        let maps = dir.path().join("mocks/mappings");
        std::fs::create_dir_all(&maps).unwrap();
        std::fs::write(maps.join("ok.json"), r#"{"request":{"urlPath":"/a"},"response":{"status":200}}"#).unwrap();
        assert_eq!(check(Some(dir.path().to_path_buf()), true).unwrap(), ExitCode::SUCCESS);
        std::fs::write(maps.join("bad.json"), "{nope").unwrap();
        assert_eq!(check(Some(dir.path().to_path_buf()), false).unwrap(), ExitCode::from(1));
        // A bare folder with mocks/ (no devall.yaml) works too; one without is refused.
        let bare = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(bare.path().join("mocks/mappings")).unwrap();
        assert_eq!(check(Some(bare.path().to_path_buf()), true).unwrap(), ExitCode::SUCCESS);
        let empty = tempfile::tempdir().unwrap();
        assert!(mock_root(Some(empty.path().to_path_buf())).is_err());

        let h = MockServer::start(MockConfig::new(dir.path())).await.unwrap();
        assert_eq!(health("127.0.0.1", h.port()).await.unwrap(), ExitCode::SUCCESS);
        let port = h.port();
        h.stop().await;
        assert_eq!(health("127.0.0.1", port).await.unwrap(), ExitCode::from(1));
    }
}
