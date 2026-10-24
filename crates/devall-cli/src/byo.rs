//! `devall tunnel ngrok|cloudflared|tailscale --port N`: bring-your-own tunnels from the terminal,
//! with the same settings (`<app data>/tunnels.json`) and keychain credentials as the app.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use devall_core::{MemorySecretStore, SecretStore};
use devall_webhooks::tunnels::config::TunnelSettings;
use devall_webhooks::tunnels::{TunnelKind, TunnelState, configured_launch, start_configured};

#[derive(clap::Args)]
pub struct ByoArgs {
    /// Local port to expose (forwards to 127.0.0.1:PORT).
    #[arg(long)]
    pub port: u16,
    /// ngrok: reserved/static domain; cloudflared (named/token tunnels): the public hostname.
    /// Overrides the app's Tunnel settings.
    #[arg(long)]
    pub domain: Option<String>,
    /// ngrok region (us, eu, ap, au, sa, jp, in).
    #[arg(long)]
    pub region: Option<String>,
    /// Path to the tool's binary (default: Tunnel settings, then PATH and common install folders).
    #[arg(long)]
    pub program: Option<PathBuf>,
    /// Don't read credentials (ngrok authtoken, Cloudflare tunnel token) from the OS keychain;
    /// ngrok still honours NGROK_AUTHTOKEN and its own config file.
    #[arg(long)]
    pub no_keychain: bool,
    /// Print the command that would run (credentials masked) and exit.
    #[arg(long)]
    pub dry_run: bool,
    /// JSON status lines on stdout instead of the URL.
    #[arg(long)]
    pub json: bool,
}

fn apply_overrides(kind: TunnelKind, settings: &mut TunnelSettings, a: &ByoArgs) {
    let program = a.program.as_ref().map(|p| p.display().to_string());
    match kind {
        TunnelKind::Ngrok => {
            if a.domain.is_some() {
                settings.ngrok.domain = a.domain.clone();
            }
            if a.region.is_some() {
                settings.ngrok.region = a.region.clone();
            }
            if program.is_some() {
                settings.ngrok.program = program;
            }
        }
        TunnelKind::Cloudflared => {
            if a.domain.is_some() {
                settings.cloudflared.hostname = a.domain.clone();
            }
            if program.is_some() {
                settings.cloudflared.program = program;
            }
        }
        TunnelKind::Tailscale => {
            if program.is_some() {
                settings.tailscale.program = program;
            }
        }
    }
}

pub async fn run(kind: TunnelKind, a: ByoArgs) -> Result<ExitCode> {
    let mut settings = crate::app_data_dir().map(|d| TunnelSettings::load(&d)).unwrap_or_default();
    apply_overrides(kind, &mut settings, &a);
    let memory = MemorySecretStore::default();
    let keychain = devall_core::KeychainSecretStore::new();
    let store: &dyn SecretStore = if a.no_keychain || !devall_core::KeychainSecretStore::available() { &memory } else { &keychain };

    if a.dry_run {
        println!("{}", configured_launch(kind, a.port, &settings, store).await?.display(kind));
        return Ok(ExitCode::SUCCESS);
    }
    let t = start_configured(kind, a.port, &settings, store).await?;
    if !a.json {
        eprintln!("Starting {} for http://127.0.0.1:{}…", kind.label(), a.port);
    }
    let mut rx = t.subscribe();
    let mut announced = false;
    loop {
        let s = rx.borrow_and_update().clone();
        if a.json {
            println!("{}", serde_json::to_string(&s)?);
        }
        match s.state {
            TunnelState::Online if !announced => {
                if let Some(u) = &s.public_url {
                    announced = true;
                    if !a.json {
                        println!("{u}");
                        eprintln!("Forwarding {u} → http://127.0.0.1:{} — Ctrl+C to stop", a.port);
                    }
                }
            }
            TunnelState::Error | TunnelState::Stopped => {
                if !a.json {
                    eprintln!("error: {}", s.message.as_deref().unwrap_or("the tunnel stopped"));
                    if let Some(h) = &s.hint {
                        eprintln!("hint: {h}");
                    }
                }
                t.stop().await;
                return Ok(ExitCode::from(1));
            }
            _ => {}
        }
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    return Ok(ExitCode::from(1));
                }
            }
            _ = tokio::signal::ctrl_c() => {
                t.stop().await;
                if !a.json {
                    eprintln!("Stopped.");
                }
                return Ok(ExitCode::SUCCESS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_override_settings() {
        let a = ByoArgs { port: 8080, domain: Some("me.ngrok-free.app".into()), region: Some("eu".into()), program: Some("/opt/ngrok".into()), no_keychain: true, dry_run: true, json: false };
        let mut s = TunnelSettings::default();
        apply_overrides(TunnelKind::Ngrok, &mut s, &a);
        assert_eq!((s.ngrok.domain.as_deref(), s.ngrok.region.as_deref(), s.ngrok.program.as_deref()), (Some("me.ngrok-free.app"), Some("eu"), Some("/opt/ngrok")));
        let mut s = TunnelSettings::default();
        apply_overrides(TunnelKind::Cloudflared, &mut s, &a);
        assert_eq!(s.cloudflared.hostname.as_deref(), Some("me.ngrok-free.app"));
        assert!(s.ngrok.domain.is_none());
    }

    #[test]
    fn parses_the_subcommands() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(["devall", "tunnel", "ngrok", "--port", "8080", "--domain", "x.ngrok-free.app"]).unwrap();
        assert!(matches!(cli.command, crate::Command::Tunnel(crate::relay::TunnelCmd::Ngrok(ByoArgs { port: 8080, .. }))));
        let cli = crate::Cli::try_parse_from(["devall", "tunnel", "cloudflared", "--port", "3000", "--dry-run"]).unwrap();
        assert!(matches!(cli.command, crate::Command::Tunnel(crate::relay::TunnelCmd::Cloudflared(ByoArgs { port: 3000, dry_run: true, .. }))));
        assert!(crate::Cli::try_parse_from(["devall", "tunnel", "tailscale"]).is_err(), "--port is required");
    }
}
