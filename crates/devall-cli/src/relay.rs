//! Hosted-relay commands: `devall tunnel http|tcp`, `devall tcp connect`, `devall inbox listen`,
//! and the relay client used by `devall mock publish` / `devall load cloud`. Credentials: the
//! desktop's sign-in (keychain) for the hosted relay, or `--relay wss://relay.example.com` +
//! `--token` / `DEVALL_RELAY_TOKEN` for a self-hosted one.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use devall_core::{KeychainSecretStore, MemorySecretStore, SecretStore};
use devall_relay::tcp::generate_access_secret;
use devall_relay::{
    ConnState, InboxOptions, InboxUpdate, RelayClient, RelayConfig, TcpAccess, TcpConnectEvent, TcpConnectOptions, TcpTunnelEvent, TcpTunnelOptions, TunnelEvent, TunnelOptions, hosted,
};

#[derive(Args, Clone, Default)]
pub struct RelayArgs {
    /// Self-hosted relay address (`wss://relay.example.com`); default: the desktop app's relay
    /// setting, else the hosted relay. Env: DEVALL_RELAY_URL.
    #[arg(long)]
    pub relay: Option<String>,
    /// Self-hosted relay token (env: DEVALL_RELAY_TOKEN); default: the one stored by the app.
    #[arg(long)]
    pub token: Option<String>,
}

fn secrets() -> Arc<dyn SecretStore> {
    if KeychainSecretStore::available() { Arc::new(KeychainSecretStore::new()) } else { Arc::new(MemorySecretStore::default()) }
}

/// The relay client the desktop app would use (or the one given on the command line), signed in.
pub fn client(args: &RelayArgs) -> Result<RelayClient> {
    let c = client_unchecked(args)?;
    if !c.is_signed_in() {
        bail!("not signed in to the relay: sign in from the /dev/all app (Tunnels page), or pass --relay and --token for a self-hosted relay");
    }
    Ok(c)
}

/// Like [`client`] without requiring credentials (a TCP tunnel's access secret is enough).
pub fn client_unchecked(args: &RelayArgs) -> Result<RelayClient> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let token = args.token.clone().or_else(|| env("DEVALL_RELAY_TOKEN"));
    let url = args.relay.clone().or_else(|| env("DEVALL_RELAY_URL"));
    let config = if let Some(url) = url {
        RelayConfig::self_hosted(url, token)
    } else {
        // The desktop app's choice (`<config dir>/dev.devall.app/relay.json`, no secrets in it).
        let settings: serde_json::Value = dirs::config_dir()
            .map(|d| d.join("dev.devall.app").join("relay.json"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let endpoint = settings["selfHostedEndpoint"].as_str().unwrap_or("").to_string();
        if settings["mode"] == "selfHosted" && !endpoint.is_empty() {
            RelayConfig::self_hosted(endpoint, token)
        } else {
            if !hosted::is_configured() {
                bail!("the hosted relay isn't available in this build; pass --relay wss://<your relay> and --token (self-hosting is free, see relay/README.md)");
            }
            RelayConfig::hosted()
        }
    };
    Ok(RelayClient::new(config, secrets())?)
}

#[derive(Subcommand)]
pub enum TunnelCmd {
    /// Expose a local HTTP server at a public HTTPS URL (WebSockets and streaming bodies pass
    /// through): `devall tunnel http --port 3000`. Prints the URL and a request log.
    Http {
        #[arg(long)]
        port: u16,
        /// Local host to forward to.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// The local server speaks HTTPS (self-signed certificates are accepted).
        #[arg(long)]
        https: bool,
        /// Reserved subdomain (Supporter) or any name (self-hosted); default: random.
        #[arg(long)]
        subdomain: Option<String>,
        /// Send `Host: <host>:<port>` to the local server instead of the public host (which stays
        /// in `X-Forwarded-Host`).
        #[arg(long)]
        rewrite_host: bool,
        /// JSON lines on stdout (`connected`, `request`, `status`) instead of text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        relay: RelayArgs,
    },
    /// Publish a local TCP port (Postgres, Redis, SSH…) as a raw TCP tunnel (Supporter or
    /// self-hosted). Teammates connect with `devall tcp connect <name> --secret …`.
    Tcp {
        #[arg(long)]
        port: u16,
        /// Local host to forward to.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Reserved subdomain (Supporter) or any name (self-hosted); default: random.
        #[arg(long)]
        name: Option<String>,
        /// Access secret for teammates (`dtcp_…`); default: a new random one. Env: DEVALL_TCP_SECRET.
        #[arg(long)]
        secret: Option<String>,
        /// Only your own account may connect (no access secret).
        #[arg(long, conflicts_with = "secret")]
        no_secret: bool,
        #[command(flatten)]
        relay: RelayArgs,
    },
    /// Your own ngrok (free account; authtoken, static domain, region… from the app's Tunnel
    /// settings and keychain): `devall tunnel ngrok --port 8080`. Prints the public URL.
    Ngrok(crate::byo::ByoArgs),
    /// Your own cloudflared: a quick tunnel (no account) or the named / token tunnel from Tunnel
    /// settings: `devall tunnel cloudflared --port 8080`.
    Cloudflared(crate::byo::ByoArgs),
    /// Tailscale Funnel on this device: `devall tunnel tailscale --port 8080`.
    Tailscale(crate::byo::ByoArgs),
}

#[derive(Subcommand)]
pub enum TcpCmd {
    /// Listen on a local port and forward each connection through a TCP tunnel.
    Connect {
        /// The TCP tunnel's name (what `devall tunnel tcp` printed).
        name: String,
        #[arg(long)]
        local_port: u16,
        /// Address to listen on (127.0.0.1 keeps it on this machine).
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// The tunnel's access secret (env: DEVALL_TCP_SECRET); default: your account (owner only).
        #[arg(long)]
        secret: Option<String>,
        #[command(flatten)]
        relay: RelayArgs,
    },
}

#[derive(Subcommand)]
pub enum InboxCmd {
    /// Receive the project's hosted webhook inbox here: the events buffered while nothing was
    /// connected, then live ones — stored in the project's webhook store like the app does (and
    /// acknowledged only after that). Takes over the inbox from the app while it runs.
    Listen {
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Inbox key (default: the project's id — the inbox the app uses).
        #[arg(long)]
        key: Option<String>,
        /// Only print the events; don't store them in the project.
        #[arg(long)]
        no_store: bool,
        /// JSON lines on stdout (`connected`, `event` with the raw body as base64, `status`).
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        relay: RelayArgs,
    },
}

pub(crate) fn json_line(v: serde_json::Value) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn human_bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

async fn tunnel_http(port: u16, host: String, https: bool, subdomain: Option<String>, rewrite_host: bool, json: bool, relay: RelayArgs) -> Result<ExitCode> {
    let c = client(&relay)?;
    let local = url::Url::parse(&format!("{}://{host}:{port}", if https { "https" } else { "http" })).with_context(|| format!("invalid --host {host}"))?;
    let t = c.start_tunnel(TunnelOptions { subdomain, rewrite_host, ..TunnelOptions::new(local.clone()) });
    let mut events = t.subscribe();
    // Like ngrok: if the very first connection is refused (plan limit, bad token, relay down), say
    // why and exit instead of retrying in silence. Later drops reconnect on their own.
    let first = {
        let mut rx = t.watch();
        tokio::time::timeout(Duration::from_secs(30), rx.wait_for(|s| !matches!(s.state, ConnState::Connecting))).await.ok().and_then(|r| r.ok()).map(|s| s.clone())
    };
    let st = match first {
        Some(s) if s.state == ConnState::Connected => s,
        other => {
            t.stop().await;
            let why = other.and_then(|s| s.last_error).unwrap_or_else(|| "no answer from the relay within 30 s".into());
            bail!("tunnel: {why}");
        }
    };
    let urls: Vec<String> = if st.public_urls.is_empty() { st.public_url.clone().into_iter().collect() } else { st.public_urls.clone() };
    if json {
        json_line(serde_json::json!({ "type": "connected", "publicUrl": st.public_url, "publicUrls": urls, "subdomain": st.subdomain, "plan": st.plan, "local": local.as_str() }));
    } else {
        for u in &urls {
            println!("Forwarding {u} -> {}", local.as_str().trim_end_matches('/'));
        }
        eprintln!("plan {} · Ctrl+C to stop", st.plan.as_deref().unwrap_or("?"));
    }
    let mut was_connected = true;
    let code = loop {
        tokio::select! {
            _ = crate::mock::shutdown_signal() => break ExitCode::SUCCESS,
            ev = events.recv() => match ev {
                Ok(TunnelEvent::Request(l)) => {
                    if json {
                        json_line(serde_json::json!({ "type": "request", "log": l }));
                    } else {
                        let status = l.status.map(|s| s.to_string()).unwrap_or_else(|| "---".into());
                        let kind = if l.websocket { " (websocket)" } else { "" };
                        println!(
                            "{} {} {status} {} ms  in {} / out {}{kind}{}",
                            l.method,
                            l.path,
                            l.duration_ms,
                            human_bytes(l.bytes_in),
                            human_bytes(l.bytes_out),
                            l.error.map(|e| format!("  — {e}")).unwrap_or_default()
                        );
                    }
                }
                Ok(TunnelEvent::Status(s)) => {
                    match s.state {
                        ConnState::Failed | ConnState::Stopped => {
                            let why = s.last_error.clone().unwrap_or_default();
                            if json {
                                json_line(serde_json::json!({ "type": "status", "state": s.state, "error": why }));
                            }
                            eprintln!("tunnel {}: {why}", if s.state == ConnState::Failed { "failed" } else { "stopped" });
                            break ExitCode::from(1);
                        }
                        ConnState::Reconnecting if was_connected => {
                            was_connected = false;
                            if json {
                                json_line(serde_json::json!({ "type": "status", "state": s.state, "error": s.last_error }));
                            }
                            eprintln!("reconnecting… ({})", s.last_error.clone().unwrap_or_default());
                        }
                        ConnState::Connected if !was_connected => {
                            was_connected = true;
                            if json {
                                json_line(serde_json::json!({ "type": "status", "state": s.state, "publicUrl": s.public_url }));
                            }
                            eprintln!("connected again: {}", s.public_url.clone().unwrap_or_default());
                        }
                        _ => {}
                    }
                    if let Some(q) = s.quota.as_ref().filter(|_| !json) {
                        eprintln!("quota: {}", q.message);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => eprintln!("(skipped {n} log lines)"),
                Err(_) => break ExitCode::SUCCESS,
            }
        }
    };
    t.stop().await;
    Ok(code)
}

pub async fn inbox(cmd: InboxCmd) -> Result<ExitCode> {
    let InboxCmd::Listen { project, key, no_store, json, relay } = cmd;
    let project = match crate::find_project(project) {
        Ok(p) => Some(p),
        Err(_) if no_store && key.is_some() => None,
        Err(e) => return Err(e.context("`devall inbox listen` stores events in a project (or pass --no-store --key <inbox key>)")),
    };
    let key = key.or_else(|| project.as_ref().map(|p| p.manifest.id.clone()));
    let hub = match (&project, no_store) {
        (Some(p), false) => Some(devall_webhooks::WebhookHub::open(p.root(), Arc::new(devall_webhooks::NoTemplates)).context("opening the project's webhook store")?),
        _ => None,
    };
    let c = client(&relay)?;
    let handler = devall_relay::handler(move |ev: devall_relay::InboxEvent| {
        let hub = hub.clone();
        async move {
            let body = ev.body().map_err(|e| e.to_string())?;
            let stored = match &hub {
                Some(hub) => {
                    let incoming = devall_webhooks::IncomingEvent {
                        method: ev.method.clone(),
                        url: ev.url.clone(),
                        path: ev.path.clone(),
                        headers: ev.headers.clone(),
                        body: body.clone(),
                        received_at: ev.received_at_ms().unwrap_or(0),
                        remote_addr: ev.source_ip.clone(),
                        public_url: Some(ev.url.clone()),
                        external_id: Some(ev.id.clone()),
                        source: devall_webhooks::EventSource::Relay,
                        ..Default::default()
                    };
                    Some(hub.ingest(incoming).await.map_err(|e| e.to_string())?.id)
                }
                None => None,
            };
            if json {
                json_line(serde_json::json!({
                    "type": "event", "id": ev.id, "seq": ev.seq, "method": ev.method, "path": ev.path, "url": ev.url,
                    "headers": ev.headers, "bodySize": ev.body_size, "bodyBase64": ev.body_base64, "receivedAt": ev.received_at, "stored": stored,
                }));
            } else {
                println!("{} {} {} ({}){}", ev.received_at, ev.method, ev.path, human_bytes(body.len() as u64), stored.map(|s| format!(" → stored {s}")).unwrap_or_default());
            }
            Ok(())
        }
    });
    let h = c.connect_inbox(InboxOptions { project_key: key, ..InboxOptions::default() }, handler);
    let mut updates = h.subscribe();
    let st = match h.wait_connected(Duration::from_secs(30)).await {
        Ok(s) => s,
        Err(e) => {
            h.stop().await;
            bail!("inbox: {e}");
        }
    };
    if json {
        json_line(serde_json::json!({ "type": "connected", "url": st.url, "inboxId": st.inbox_id, "plan": st.plan }));
    } else {
        println!("Webhook inbox {} — send webhooks there; Ctrl+C to stop.", st.url.clone().unwrap_or_default());
    }
    let mut last = (u64::MAX, u64::MAX, u64::MAX);
    let code = loop {
        tokio::select! {
            _ = crate::mock::shutdown_signal() => break ExitCode::SUCCESS,
            u = updates.recv() => match u {
                Ok(InboxUpdate::Status(s)) => {
                    if matches!(s.state, ConnState::Failed | ConnState::Stopped) {
                        eprintln!("inbox {}: {}", if s.state == ConnState::Failed { "failed" } else { "stopped" }, s.last_error.clone().unwrap_or_default());
                        break ExitCode::from(1);
                    }
                    let now = (s.buffered, s.delivered, s.duplicates);
                    if now != last && s.state == ConnState::Connected {
                        last = now;
                        if json {
                            json_line(serde_json::json!({ "type": "status", "buffered": s.buffered, "delivered": s.delivered, "duplicates": s.duplicates, "todayCount": s.today_count }));
                        }
                    }
                }
                Ok(InboxUpdate::Delivered(d)) if d.duplicate => {
                    if json {
                        json_line(serde_json::json!({ "type": "duplicate", "id": d.id }));
                    } else {
                        eprintln!("(already handled {}, acknowledged again)", d.id);
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break ExitCode::SUCCESS,
            }
        }
    };
    h.stop().await;
    Ok(code)
}

pub async fn tunnel(cmd: TunnelCmd) -> Result<ExitCode> {
    match cmd {
        TunnelCmd::Http { port, host, https, subdomain, rewrite_host, json, relay } => tunnel_http(port, host, https, subdomain, rewrite_host, json, relay).await,
        TunnelCmd::Ngrok(a) => crate::byo::run(devall_webhooks::TunnelKind::Ngrok, a).await,
        TunnelCmd::Cloudflared(a) => crate::byo::run(devall_webhooks::TunnelKind::Cloudflared, a).await,
        TunnelCmd::Tailscale(a) => crate::byo::run(devall_webhooks::TunnelKind::Tailscale, a).await,

        TunnelCmd::Tcp { port, host, name, secret, no_secret, relay } => {
            let c = client(&relay)?;
            let secret = if no_secret { None } else { Some(secret.or_else(|| std::env::var("DEVALL_TCP_SECRET").ok().filter(|s| !s.is_empty())).unwrap_or_else(generate_access_secret)) };
            if let Some(s) = &secret
                && !s.starts_with(devall_relay::tcp::SECRET_PREFIX)
            {
                bail!("access secrets start with {}", devall_relay::tcp::SECRET_PREFIX);
            }
            let opts = TcpTunnelOptions { local: format!("{host}:{port}"), subdomain: name, access_secret: secret.clone(), ..TcpTunnelOptions::for_port(port) };
            let t = c.start_tcp_tunnel(opts);
            let mut events = t.subscribe();
            let st = t.wait_connected(Duration::from_secs(30)).await.context("TCP tunnel")?;
            let name = st.name.clone().unwrap_or_default();
            eprintln!("TCP tunnel \"{name}\" → {host}:{port} ({})", st.plan.unwrap_or_default());
            eprintln!("Connect from another machine:");
            match &secret {
                Some(s) => eprintln!("  devall tcp connect {name} --local-port {port} --secret {s}"),
                None => eprintln!("  devall tcp connect {name} --local-port {port}   (signed in to the same account)"),
            }
            eprintln!("Ctrl+C to stop.");
            loop {
                tokio::select! {
                    _ = crate::mock::shutdown_signal() => break,
                    ev = events.recv() => match ev {
                        Ok(TcpTunnelEvent::Connection(l)) => eprintln!(
                            "connection from {} ({}) {} ms, {} B in / {} B out{}",
                            l.client_ip.as_deref().unwrap_or("?"), l.via, l.duration_ms, l.bytes_in, l.bytes_out,
                            l.error.map(|e| format!(" — {e}")).unwrap_or_default()
                        ),
                        Ok(TcpTunnelEvent::Status(s)) if matches!(s.state, ConnState::Failed | ConnState::Stopped) => {
                            eprintln!("tunnel {}: {}", if s.state == ConnState::Failed { "failed" } else { "stopped" }, s.last_error.unwrap_or_default());
                            return Ok(ExitCode::from(1));
                        }
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
            }
            t.stop().await;
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub async fn tcp(cmd: TcpCmd) -> Result<ExitCode> {
    match cmd {
        TcpCmd::Connect { name, local_port, bind, secret, relay } => {
            let secret = secret.or_else(|| std::env::var("DEVALL_TCP_SECRET").ok().filter(|s| !s.is_empty()));
            // With the tunnel's access secret no account is needed; otherwise the owner's account.
            let c = if secret.is_some() { client_unchecked(&relay)? } else { client(&relay)? };
            let access = match secret {
                Some(s) => TcpAccess::Secret(s),
                None => TcpAccess::Account,
            };
            let bind = format!("{bind}:{local_port}").parse().with_context(|| format!("invalid --bind {bind}"))?;
            let h = c.tcp_connect(TcpConnectOptions { name: name.clone(), bind, access }).await?;
            let mut events = h.subscribe();
            eprintln!("Forwarding {} → TCP tunnel \"{name}\". Ctrl+C to stop.", h.local_addr());
            loop {
                tokio::select! {
                    _ = crate::mock::shutdown_signal() => break,
                    ev = events.recv() => match ev {
                        Ok(TcpConnectEvent::Connection(l)) => eprintln!(
                            "connection {} closed after {} ms, {} B in / {} B out{}",
                            l.client_ip.as_deref().unwrap_or("?"), l.duration_ms, l.bytes_in, l.bytes_out,
                            l.error.map(|e| format!(" — {e}")).unwrap_or_default()
                        ),
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
            }
            h.stop().await;
            Ok(ExitCode::SUCCESS)
        }
    }
}
