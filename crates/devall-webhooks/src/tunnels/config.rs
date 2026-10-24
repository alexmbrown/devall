//! Bring-your-own tunnel settings and the command lines they produce.
//!
//! Settings are per user (`<app data>/tunnels.json`, never in a project) and contain no secrets:
//! the ngrok authtoken, the ngrok basic-auth password and the Cloudflare tunnel token live in the
//! OS keychain ([`secret_scope`]) and reach the child process through its environment
//! (`NGROK_AUTHTOKEN`, `TUNNEL_TOKEN`) — except basic auth, which ngrok only takes as a flag.

use std::path::{Path, PathBuf};

use devall_core::secrets::{ALL_ENVIRONMENTS, SecretScope, SecretStore};
use serde::{Deserialize, Serialize};

use super::TunnelKind;
use crate::WebhookError;

pub const SETTINGS_FILE: &str = "tunnels.json";

/// Keychain entry names (scope [`secret_scope`]).
pub const NGROK_AUTHTOKEN: &str = "ngrok-authtoken";
pub const NGROK_BASIC_AUTH_PASSWORD: &str = "ngrok-basic-auth-password";
pub const CLOUDFLARED_TOKEN: &str = "cloudflared-token";
pub const SECRET_NAMES: [&str; 3] = [NGROK_AUTHTOKEN, NGROK_BASIC_AUTH_PASSWORD, CLOUDFLARED_TOKEN];

/// App-level keychain scope for tunnel credentials (not tied to a project).
pub fn secret_scope() -> SecretScope {
    SecretScope::new("devall-tunnels", ALL_ENVIRONMENTS)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TunnelSettings {
    pub ngrok: NgrokSettings,
    pub cloudflared: CloudflaredSettings,
    pub tailscale: TailscaleSettings,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NgrokSettings {
    /// Path to the `ngrok` binary (default: detected).
    pub program: Option<String>,
    /// An ngrok agent config file (`--config`).
    pub config_file: Option<String>,
    /// Reserved / static domain (the free plan includes one), e.g. `my-app.ngrok-free.app`.
    pub domain: Option<String>,
    /// `us`, `eu`, `ap`, `au`, `sa`, `jp`, `in` (ngrok picks the closest when unset).
    pub region: Option<String>,
    /// Basic-auth user; the password is in the keychain.
    pub basic_auth_user: Option<String>,
    /// OAuth provider (`google`, `github`, `microsoft`, `gitlab`, …).
    pub oauth_provider: Option<String>,
    pub oauth_allow_emails: Vec<String>,
    pub oauth_allow_domains: Vec<String>,
    /// Extra arguments, shell-style (`--request-header-add "x: y"`).
    pub extra_args: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudflaredMode {
    /// `cloudflared tunnel --url …`: random trycloudflare.com URL, no account.
    #[default]
    Quick,
    /// A locally-managed named tunnel (`cloudflared tunnel login` + `create`), routed to the port.
    Named,
    /// A remotely-managed tunnel run with its token (routes configured in the Cloudflare dashboard).
    Token,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudflaredSettings {
    pub program: Option<String>,
    pub mode: CloudflaredMode,
    /// Named mode: tunnel name or UUID.
    pub tunnel_name: Option<String>,
    /// Named / token mode: the public hostname routed to this tunnel (shown as its URL).
    pub hostname: Option<String>,
    /// `--config` for named tunnels.
    pub config_file: Option<String>,
    pub extra_args: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TailscaleSettings {
    pub program: Option<String>,
    /// Public HTTPS port: 443 (default), 8443 or 10000 — the ports Funnel allows.
    pub https_port: Option<u16>,
    /// Mount path (`--set-path /hooks`).
    pub path: Option<String>,
    pub extra_args: String,
}

fn nonempty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl TunnelSettings {
    pub fn load(dir: &Path) -> TunnelSettings {
        std::fs::read(dir.join(SETTINGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), WebhookError> {
        std::fs::create_dir_all(dir).map_err(|e| WebhookError::Io(e.to_string()))?;
        let json = serde_json::to_vec_pretty(self).map_err(|e| WebhookError::Invalid(e.to_string()))?;
        devall_core::fsutil::atomic_write(&dir.join(SETTINGS_FILE), &json)?;
        Ok(())
    }

    /// The binary configured for `kind`, if any.
    pub fn program(&self, kind: TunnelKind) -> Option<PathBuf> {
        match kind {
            TunnelKind::Ngrok => nonempty(&self.ngrok.program),
            TunnelKind::Cloudflared => nonempty(&self.cloudflared.program),
            TunnelKind::Tailscale => nonempty(&self.tailscale.program),
        }
        .map(PathBuf::from)
    }

    /// Checks values that would make the tool fail in confusing ways.
    pub fn validate(&self) -> Result<(), WebhookError> {
        if let Some(p) = self.tailscale.https_port
            && ![443, 8443, 10000].contains(&p)
        {
            return Err(WebhookError::Invalid(format!("Tailscale Funnel only serves on ports 443, 8443 and 10000 (got {p})")));
        }
        if let Some(path) = nonempty(&self.tailscale.path)
            && !path.starts_with('/')
        {
            return Err(WebhookError::Invalid("the Funnel path must start with /".into()));
        }
        if self.cloudflared.mode == CloudflaredMode::Named && nonempty(&self.cloudflared.tunnel_name).is_none() {
            return Err(WebhookError::Invalid("named Cloudflare tunnels need the tunnel name (cloudflared tunnel create <name>)".into()));
        }
        if let Some(d) = nonempty(&self.ngrok.domain)
            && (d.contains(char::is_whitespace) || d.contains('/') && !d.starts_with("https://"))
        {
            return Err(WebhookError::Invalid(format!("not a domain: {d}")));
        }
        for a in [&self.ngrok.extra_args, &self.cloudflared.extra_args, &self.tailscale.extra_args] {
            split_args(a)?;
        }
        Ok(())
    }
}

/// Splits a command-line string into arguments (double/single quotes, backslash escapes in
/// double quotes); the same on every platform.
pub fn split_args(s: &str) -> Result<Vec<String>, WebhookError> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err(WebhookError::Invalid("unterminated ' in extra arguments".into())),
                    }
                }
            }
            '"' => {
                has = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if matches!(chars.peek(), Some('"' | '\\')) => cur.push(chars.next().unwrap_or('\\')),
                        Some(x) => cur.push(x),
                        None => return Err(WebhookError::Invalid("unterminated \" in extra arguments".into())),
                    }
                }
            }
            c if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                has = true;
                cur.push(c);
            }
        }
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

/// `ngrok version` → whether it understands `--url` (agent 3.16+; `--domain` works on every v3).
pub fn ngrok_supports_url_flag(version: Option<&str>) -> bool {
    let Some(v) = version else { return false };
    let v = v.trim().trim_start_matches("ngrok version ").trim_start_matches('v');
    let mut it = v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse::<u32>().unwrap_or(0));
    let (major, minor) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    major > 3 || (major == 3 && minor >= 16)
}

/// A ready-to-run tunnel command.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
    /// Extra environment for the child (credentials from the keychain).
    pub env: Vec<(String, String)>,
    /// URL known before the tool prints one (static domain, named-tunnel hostname).
    pub public_url: Option<String>,
    /// Tailscale: appended to the device URL derived from `tailscale status` (`:8443/hooks`).
    pub url_suffix: String,
}

impl Launch {
    /// The command line with credentials masked, for display.
    pub fn display(&self, kind: TunnelKind) -> String {
        let program = self.program.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| kind.program_name().to_string());
        let mut parts = vec![quote(&program)];
        let mut mask_next = false;
        for a in &self.args {
            if mask_next {
                parts.push(quote(&a.split_once(':').map(|(u, _)| format!("{u}:••••")).unwrap_or_else(|| "••••".into())));
                mask_next = false;
                continue;
            }
            mask_next = a == "--basic-auth";
            parts.push(quote(a));
        }
        let env: Vec<String> = self.env.iter().map(|(k, _)| format!("{k}=••••")).collect();
        if env.is_empty() { parts.join(" ") } else { format!("{} {}", env.join(" "), parts.join(" ")) }
    }
}

fn quote(s: &str) -> String {
    if !s.is_empty() && !s.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'') { s.to_string() } else { format!("\"{}\"", s.replace('"', "\\\"")) }
}

fn https(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    if h.starts_with("https://") || h.starts_with("http://") { h.to_string() } else { format!("https://{h}") }
}

fn secret(store: &dyn SecretStore, name: &str) -> Result<Option<String>, WebhookError> {
    store.get(&secret_scope(), name).map(|v| v.filter(|s| !s.trim().is_empty())).map_err(|e| WebhookError::Secret(e.to_string()))
}

/// Builds the command for exposing `127.0.0.1:port` with `kind` and `settings`. `version` is the
/// tool's `version` output (ngrok: picks `--url` or `--domain`).
pub fn build(kind: TunnelKind, port: u16, settings: &TunnelSettings, version: Option<&str>, store: &dyn SecretStore) -> Result<Launch, WebhookError> {
    settings.validate()?;
    let local = format!("127.0.0.1:{port}");
    let mut l = Launch { program: settings.program(kind), args: Vec::new(), env: Vec::new(), public_url: None, url_suffix: String::new() };
    match kind {
        TunnelKind::Ngrok => {
            let s = &settings.ngrok;
            l.args = vec!["http".into(), local, "--log".into(), "stdout".into(), "--log-format".into(), "json".into()];
            if let Some(c) = nonempty(&s.config_file) {
                l.args.extend(["--config".into(), c.into()]);
            }
            if let Some(d) = nonempty(&s.domain) {
                if ngrok_supports_url_flag(version) {
                    l.args.extend(["--url".into(), https(d)]);
                } else {
                    l.args.extend(["--domain".into(), d.trim_start_matches("https://").trim_end_matches('/').into()]);
                }
                l.public_url = Some(https(d));
            }
            if let Some(r) = nonempty(&s.region) {
                l.args.extend(["--region".into(), r.into()]);
            }
            if let Some(user) = nonempty(&s.basic_auth_user) {
                let pass = secret(store, NGROK_BASIC_AUTH_PASSWORD)?.ok_or_else(|| WebhookError::Invalid("set the basic-auth password in Tunnel settings (stored in your keychain)".into()))?;
                if pass.len() < 8 {
                    return Err(WebhookError::Invalid("ngrok requires a basic-auth password of at least 8 characters".into()));
                }
                l.args.extend(["--basic-auth".into(), format!("{user}:{pass}")]);
            }
            if let Some(p) = nonempty(&s.oauth_provider) {
                l.args.extend(["--oauth".into(), p.into()]);
                for e in s.oauth_allow_emails.iter().map(|e| e.trim()).filter(|e| !e.is_empty()) {
                    l.args.extend(["--oauth-allow-email".into(), e.into()]);
                }
                for d in s.oauth_allow_domains.iter().map(|d| d.trim()).filter(|d| !d.is_empty()) {
                    l.args.extend(["--oauth-allow-domain".into(), d.into()]);
                }
            }
            l.args.extend(split_args(&s.extra_args)?);
            if let Some(token) = secret(store, NGROK_AUTHTOKEN)? {
                l.env.push(("NGROK_AUTHTOKEN".into(), token));
            }
        }
        TunnelKind::Cloudflared => {
            let s = &settings.cloudflared;
            let extra = split_args(&s.extra_args)?;
            l.args = vec!["tunnel".into(), "--no-autoupdate".into()];
            match s.mode {
                CloudflaredMode::Quick => {
                    l.args.extend(["--url".into(), format!("http://{local}")]);
                    l.args.extend(extra);
                }
                CloudflaredMode::Named => {
                    if let Some(c) = nonempty(&s.config_file) {
                        l.args.extend(["--config".into(), c.into()]);
                    }
                    l.args.extend(["--url".into(), format!("http://{local}")]);
                    l.args.extend(extra);
                    l.args.push("run".into());
                    l.args.push(nonempty(&s.tunnel_name).unwrap_or_default().into());
                    l.public_url = nonempty(&s.hostname).map(https);
                }
                CloudflaredMode::Token => {
                    let token = secret(store, CLOUDFLARED_TOKEN)?
                        .ok_or_else(|| WebhookError::Invalid("paste the tunnel token (Cloudflare dashboard → Networks → Tunnels → your tunnel) in Tunnel settings".into()))?;
                    l.args.extend(extra);
                    l.args.push("run".into());
                    l.env.push(("TUNNEL_TOKEN".into(), token));
                    l.public_url = nonempty(&s.hostname).map(https);
                }
            }
        }
        TunnelKind::Tailscale => {
            let s = &settings.tailscale;
            l.args = vec!["funnel".into()];
            let https_port = s.https_port.unwrap_or(443);
            if https_port != 443 {
                l.args.push(format!("--https={https_port}"));
                l.url_suffix.push_str(&format!(":{https_port}"));
            }
            if let Some(p) = nonempty(&s.path).filter(|p| *p != "/") {
                l.args.extend(["--set-path".into(), p.into()]);
                l.url_suffix.push_str(p.trim_end_matches('/'));
            }
            l.args.extend(split_args(&s.extra_args)?);
            l.args.push(port.to_string());
        }
    }
    Ok(l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devall_core::MemorySecretStore;

    fn store(entries: &[(&str, &str)]) -> MemorySecretStore {
        let s = MemorySecretStore::default();
        for (k, v) in entries {
            s.set(&secret_scope(), k, v).unwrap();
        }
        s
    }

    #[test]
    fn defaults_match_the_plain_commands() {
        let none = store(&[]);
        let d = TunnelSettings::default();
        for kind in TunnelKind::ALL {
            let l = build(kind, 4590, &d, None, &none).unwrap();
            assert_eq!(l.args, kind.args(4590), "{kind:?}");
            assert!(l.env.is_empty() && l.public_url.is_none() && l.program.is_none());
        }
    }

    #[test]
    fn ngrok_flags_follow_the_agent_version() {
        let mut s = TunnelSettings::default();
        s.ngrok.domain = Some("my-app.ngrok-free.app".into());
        s.ngrok.region = Some("eu".into());
        s.ngrok.config_file = Some("C:/Users/me/ngrok.yml".into());
        let none = store(&[]);
        let new = build(TunnelKind::Ngrok, 8080, &s, Some("3.22.1"), &none).unwrap();
        assert!(new.args.windows(2).any(|w| w == ["--url", "https://my-app.ngrok-free.app"]), "{:?}", new.args);
        assert!(new.args.windows(2).any(|w| w == ["--config", "C:/Users/me/ngrok.yml"]));
        assert!(new.args.windows(2).any(|w| w == ["--region", "eu"]));
        assert_eq!(new.public_url.as_deref(), Some("https://my-app.ngrok-free.app"));
        let old = build(TunnelKind::Ngrok, 8080, &s, Some("3.5.0"), &none).unwrap();
        assert!(old.args.windows(2).any(|w| w == ["--domain", "my-app.ngrok-free.app"]), "{:?}", old.args);
        assert!(!old.args.contains(&"--url".to_string()));
        // Unknown version: the flag every v3 agent accepts.
        assert!(build(TunnelKind::Ngrok, 8080, &s, None, &none).unwrap().args.contains(&"--domain".to_string()));

        assert!(ngrok_supports_url_flag(Some("ngrok version 3.16.0")));
        assert!(ngrok_supports_url_flag(Some("4.0.0")));
        assert!(!ngrok_supports_url_flag(Some("3.15.9")));
        assert!(!ngrok_supports_url_flag(Some("2.3.41")));
    }

    #[test]
    fn credentials_come_from_the_keychain_and_are_masked() {
        let mut s = TunnelSettings::default();
        s.ngrok.basic_auth_user = Some("dev".into());
        s.ngrok.oauth_provider = Some("google".into());
        s.ngrok.oauth_allow_emails = vec!["a@example.com".into(), " ".into()];
        s.ngrok.oauth_allow_domains = vec!["example.com".into()];
        s.ngrok.extra_args = r#"--request-header-add "x-env: dev" --host-header=rewrite"#.into();
        // Basic auth without a stored password is an error, not a silent unauthenticated tunnel.
        assert!(build(TunnelKind::Ngrok, 1, &s, None, &store(&[])).is_err());
        assert!(build(TunnelKind::Ngrok, 1, &s, None, &store(&[(NGROK_BASIC_AUTH_PASSWORD, "short")])).is_err());

        let keys = store(&[(NGROK_AUTHTOKEN, "2abcTOKEN_xyz"), (NGROK_BASIC_AUTH_PASSWORD, "hunter2-long")]);
        let l = build(TunnelKind::Ngrok, 3000, &s, Some("3.20.0"), &keys).unwrap();
        assert_eq!(l.env, [("NGROK_AUTHTOKEN".to_string(), "2abcTOKEN_xyz".to_string())]);
        assert!(!l.args.iter().any(|a| a.contains("2abcTOKEN")), "token never on the command line");
        assert!(l.args.windows(2).any(|w| w == ["--basic-auth", "dev:hunter2-long"]));
        assert!(l.args.windows(2).any(|w| w == ["--oauth", "google"]));
        assert_eq!(l.args.iter().filter(|a| *a == "--oauth-allow-email").count(), 1);
        assert!(l.args.windows(2).any(|w| w == ["--request-header-add", "x-env: dev"]));
        assert_eq!(l.args.last().map(String::as_str), Some("--host-header=rewrite"));

        let shown = l.display(TunnelKind::Ngrok);
        assert!(shown.starts_with("NGROK_AUTHTOKEN=•••• ngrok http 127.0.0.1:3000"), "{shown}");
        assert!(shown.contains("--basic-auth dev:••••") && !shown.contains("hunter2") && !shown.contains("2abcTOKEN"), "{shown}");
        assert!(shown.contains("\"x-env: dev\""));
    }

    #[test]
    fn cloudflared_modes() {
        let mut s = TunnelSettings::default();
        s.cloudflared.mode = CloudflaredMode::Named;
        assert!(build(TunnelKind::Cloudflared, 1, &s, None, &store(&[])).is_err(), "named needs a tunnel name");
        s.cloudflared.tunnel_name = Some("devbox".into());
        s.cloudflared.hostname = Some("hooks.example.com".into());
        s.cloudflared.config_file = Some("/home/me/.cloudflared/config.yml".into());
        let l = build(TunnelKind::Cloudflared, 4590, &s, None, &store(&[])).unwrap();
        assert_eq!(l.args, ["tunnel", "--no-autoupdate", "--config", "/home/me/.cloudflared/config.yml", "--url", "http://127.0.0.1:4590", "run", "devbox"]);
        assert_eq!(l.public_url.as_deref(), Some("https://hooks.example.com"));

        s.cloudflared.mode = CloudflaredMode::Token;
        assert!(build(TunnelKind::Cloudflared, 4590, &s, None, &store(&[])).is_err(), "token mode needs the token");
        let l = build(TunnelKind::Cloudflared, 4590, &s, None, &store(&[(CLOUDFLARED_TOKEN, "eyJhIjoi")])).unwrap();
        assert_eq!(l.args, ["tunnel", "--no-autoupdate", "run"]);
        assert_eq!(l.env, [("TUNNEL_TOKEN".to_string(), "eyJhIjoi".to_string())]);
        assert_eq!(l.display(TunnelKind::Cloudflared), "TUNNEL_TOKEN=•••• cloudflared tunnel --no-autoupdate run");
    }

    #[test]
    fn tailscale_port_mapping() {
        let mut s = TunnelSettings::default();
        s.tailscale.https_port = Some(8443);
        s.tailscale.path = Some("/hooks".into());
        s.tailscale.program = Some("/Applications/Tailscale.app/Contents/MacOS/Tailscale".into());
        let l = build(TunnelKind::Tailscale, 4590, &s, None, &store(&[])).unwrap();
        assert_eq!(l.args, ["funnel", "--https=8443", "--set-path", "/hooks", "4590"]);
        assert_eq!(l.url_suffix, ":8443/hooks");
        assert_eq!(l.program, Some(PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale")));
        s.tailscale.https_port = Some(8080);
        assert!(build(TunnelKind::Tailscale, 4590, &s, None, &store(&[])).is_err());
    }

    #[test]
    fn splits_extra_args() {
        assert_eq!(split_args(r#"a "b c" 'd e' f\g "q\"x""#).unwrap(), ["a", "b c", "d e", "f\\g", "q\"x"]);
        assert_eq!(split_args("  ").unwrap(), Vec::<String>::new());
        assert_eq!(split_args("--x ''").unwrap(), ["--x", ""]);
        assert!(split_args("\"open").is_err());
    }

    #[test]
    fn settings_round_trip_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = TunnelSettings::default();
        s.ngrok.domain = Some("x.ngrok-free.app".into());
        s.cloudflared.mode = CloudflaredMode::Token;
        s.save(dir.path()).unwrap();
        assert_eq!(TunnelSettings::load(dir.path()), s);
        let text = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(!text.to_lowercase().contains("token\":"), "no token fields: {text}");
        // Missing / corrupt files fall back to defaults.
        assert_eq!(TunnelSettings::load(&dir.path().join("nope")), TunnelSettings::default());
    }
}
