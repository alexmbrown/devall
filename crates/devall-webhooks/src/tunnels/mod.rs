//! Bring-your-own tunnels: expose the local receiver through the user's own `ngrok`,
//! `cloudflared` (quick tunnels, no account) or `tailscale funnel`, run as child processes.
//!
//! * Detection looks on `PATH` and in common install locations (winget/scoop/Chocolatey/Program
//!   Files on Windows; Homebrew, MacPorts and the Tailscale app bundle on macOS; `/usr/bin`,
//!   `/snap/bin`, `~/.local/bin` on Linux) and reports the version and whether the tool is ready
//!   (ngrok authtoken configured, Tailscale logged in) — without printing any token.
//! * A started tunnel streams [`TunnelStatus`] snapshots (public URL, errors with hints such as
//!   "ngrok authtoken missing — …"). Stopping kills the whole process tree (`taskkill /T` on
//!   Windows, the process group on Unix).
//!
//! Commands: `cloudflared tunnel --no-autoupdate --url http://127.0.0.1:PORT`,
//! `ngrok http 127.0.0.1:PORT --log stdout --log-format json`, `tailscale funnel PORT`.
//! `127.0.0.1` (not `localhost`) avoids an IPv6 `::1` lookup the receiver doesn't listen on.

pub mod config;
pub mod parse;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};

use crate::WebhookError;
use crate::event::now_ms;
use parse::Signal;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const LOG_LINES: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TunnelKind {
    Cloudflared,
    Ngrok,
    Tailscale,
}

impl TunnelKind {
    pub const ALL: [TunnelKind; 3] = [TunnelKind::Cloudflared, TunnelKind::Ngrok, TunnelKind::Tailscale];

    pub fn label(self) -> &'static str {
        match self {
            TunnelKind::Cloudflared => "Cloudflare quick tunnel",
            TunnelKind::Ngrok => "ngrok",
            TunnelKind::Tailscale => "Tailscale Funnel",
        }
    }

    /// The executable's base name (`ngrok`, `cloudflared`, `tailscale`).
    pub fn program_name(self) -> &'static str {
        self.names()[0]
    }

    fn names(self) -> &'static [&'static str] {
        match self {
            TunnelKind::Cloudflared => &["cloudflared"],
            TunnelKind::Ngrok => &["ngrok"],
            TunnelKind::Tailscale => &["tailscale"],
        }
    }

    pub fn args(self, port: u16) -> Vec<String> {
        match self {
            TunnelKind::Cloudflared => vec!["tunnel".into(), "--no-autoupdate".into(), "--url".into(), format!("http://127.0.0.1:{port}")],
            TunnelKind::Ngrok => vec!["http".into(), format!("127.0.0.1:{port}"), "--log".into(), "stdout".into(), "--log-format".into(), "json".into()],
            TunnelKind::Tailscale => vec!["funnel".into(), port.to_string()],
        }
    }

    fn install_hint(self) -> &'static str {
        match (self, std::env::consts::OS) {
            (TunnelKind::Cloudflared, "windows") => "winget install --id Cloudflare.cloudflared — quick tunnels need no Cloudflare account.",
            (TunnelKind::Cloudflared, "macos") => "brew install cloudflared — quick tunnels need no Cloudflare account.",
            (TunnelKind::Cloudflared, _) => "Install cloudflared from your package manager or github.com/cloudflare/cloudflared/releases — no account needed.",
            (TunnelKind::Ngrok, "windows") => "winget install ngrok.ngrok, then ngrok config add-authtoken <token> (free account at ngrok.com).",
            (TunnelKind::Ngrok, "macos") => "brew install ngrok, then ngrok config add-authtoken <token> (free account at ngrok.com).",
            (TunnelKind::Ngrok, _) => "Install ngrok (ngrok.com/download), then ngrok config add-authtoken <token>.",
            (TunnelKind::Tailscale, "windows") => "winget install tailscale.tailscale, log in, and enable Funnel + HTTPS for your tailnet in the admin console.",
            (TunnelKind::Tailscale, "macos") => "Install Tailscale (tailscale.com/download or brew install --cask tailscale), log in, and enable Funnel + HTTPS in the admin console.",
            (TunnelKind::Tailscale, _) => "Install Tailscale, log in, and enable Funnel + HTTPS for your tailnet in the admin console.",
        }
    }

    fn docs_url(self) -> &'static str {
        match self {
            TunnelKind::Cloudflared => "https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/",
            TunnelKind::Ngrok => "https://ngrok.com/docs/getting-started/",
            TunnelKind::Tailscale => "https://tailscale.com/kb/1223/funnel",
        }
    }

    /// Notes shown next to the tool (limits that matter for webhooks).
    fn notes(self) -> &'static str {
        match self {
            TunnelKind::Cloudflared => "Random URL that changes every start; no SSE; best effort.",
            TunnelKind::Ngrok => "Free plan: one assigned dev domain, request quotas, browser interstitial (webhooks unaffected).",
            TunnelKind::Tailscale => "Stable https://<device>.<tailnet>.ts.net URL; Funnel must be allowed for the device.",
        }
    }
}

/// Hosts of public tunnel services (requests through them are HTTPS at the edge).
pub fn is_tunnel_host(host: &str) -> bool {
    let h = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    ["trycloudflare.com", "ngrok-free.app", "ngrok.app", "ngrok.io", "ngrok-free.dev", "ngrok.dev", "ngrok.pizza", "ts.net"].iter().any(|s| h.ends_with(&format!(".{s}")))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub kind: TunnelKind,
    pub label: &'static str,
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// ngrok: authtoken configured; Tailscale: logged in and running; cloudflared: `None` (no account needed).
    pub ready: Option<bool>,
    /// What to do before this tool can start a tunnel.
    pub hint: Option<String>,
    pub install_hint: &'static str,
    pub notes: &'static str,
    pub docs_url: &'static str,
    /// Tailscale: the Funnel URL this device gets.
    pub predicted_url: Option<String>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

/// Common install locations outside `PATH` (GUI apps on macOS don't inherit the shell's PATH).
fn extra_locations(kind: TunnelKind) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let name = kind.names()[0];
    if cfg!(windows) {
        let exe = format!("{name}.exe");
        if let Some(l) = env_dir("LOCALAPPDATA") {
            out.push(l.join("Microsoft").join("WinGet").join("Links").join(&exe));
            out.push(l.join("Microsoft").join("WindowsApps").join(&exe));
            out.push(l.join(name).join(&exe));
        }
        if let Some(h) = home() {
            out.push(h.join("scoop").join("shims").join(&exe));
            out.push(h.join(name).join(&exe));
            out.push(h.join("bin").join(&exe));
        }
        if let Some(p) = env_dir("ProgramData") {
            out.push(p.join("chocolatey").join("bin").join(&exe));
        }
        for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = env_dir(pf) {
                out.push(p.join(match kind {
                    TunnelKind::Tailscale => "Tailscale",
                    _ => name,
                }).join(&exe));
            }
        }
    } else {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin", "/snap/bin"] {
            out.push(Path::new(d).join(name));
        }
        if let Some(h) = home() {
            out.push(h.join(".local").join("bin").join(name));
            out.push(h.join("bin").join(name));
        }
        if kind == TunnelKind::Tailscale && cfg!(target_os = "macos") {
            // The Mac app's binary doubles as the CLI.
            out.push(PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale"));
        }
    }
    out
}

/// Finds the tool on `PATH`, then in common install locations.
pub fn find_program(kind: TunnelKind) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in kind.names() {
                for ext in exts {
                    let p = dir.join(format!("{name}{ext}"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    extra_locations(kind).into_iter().find(|p| p.is_file())
}

/// Runs a short command with a timeout; returns (stdout, stderr) when it exits in time.
fn run_quick(program: &Path, args: &[&str], timeout: Duration) -> Option<(String, String)> {
    use std::io::Read;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let (mut out, mut err) = (String::new(), String::new());
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut err);
    }
    Some((out, err))
}

/// The tool's version (`ngrok version`, `cloudflared --version`, `tailscale version`); blocking, ≤ 4 s.
pub fn version(kind: TunnelKind, program: &Path) -> Option<String> {
    version_of(kind, program)
}

fn version_of(kind: TunnelKind, program: &Path) -> Option<String> {
    let args: &[&str] = match kind {
        TunnelKind::Ngrok | TunnelKind::Tailscale => &["version"],
        TunnelKind::Cloudflared => &["--version"],
    };
    let (out, err) = run_quick(program, args, Duration::from_secs(4))?;
    let text = if out.trim().is_empty() { err } else { out };
    let first = text.lines().map(str::trim).find(|l| !l.is_empty())?.to_string();
    Some(first.trim_start_matches("ngrok version ").trim_start_matches("cloudflared version ").split(" (").next().unwrap_or(&first).trim().to_string())
}

/// Locations of the ngrok agent config (v3 and legacy v2).
fn ngrok_configs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let h = home();
    if cfg!(windows)
        && let Some(l) = env_dir("LOCALAPPDATA")
    {
        out.push(l.join("ngrok").join("ngrok.yml"));
    }
    if cfg!(target_os = "macos")
        && let Some(h) = &h
    {
        out.push(h.join("Library").join("Application Support").join("ngrok").join("ngrok.yml"));
    }
    if let Some(x) = env_dir("XDG_CONFIG_HOME") {
        out.push(x.join("ngrok").join("ngrok.yml"));
    }
    if let Some(h) = &h {
        out.push(h.join(".config").join("ngrok").join("ngrok.yml"));
        out.push(h.join(".ngrok2").join("ngrok.yml"));
    }
    out
}

/// Whether an ngrok config text sets a non-empty `authtoken` (v2 top level or v3 `agent:`).
pub fn config_has_authtoken(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim();
        !t.starts_with('#')
            && t.strip_prefix("authtoken:").is_some_and(|v| {
                let v = v.split(" #").next().unwrap_or("").trim().trim_matches(['"', '\'']);
                !v.is_empty()
            })
    })
}

fn ngrok_authtoken_configured() -> bool {
    if std::env::var("NGROK_AUTHTOKEN").is_ok_and(|v| !v.trim().is_empty()) {
        return true;
    }
    ngrok_configs().iter().any(|p| std::fs::read_to_string(p).is_ok_and(|t| config_has_authtoken(&t)))
}

/// `tailscale status --json` → (backend state, DNS name without the trailing dot).
pub fn parse_tailscale_status(json: &str) -> Option<(String, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let state = v.get("BackendState")?.as_str()?.to_string();
    let dns = v.pointer("/Self/DNSName").and_then(|d| d.as_str()).map(|d| d.trim_end_matches('.').to_string()).filter(|d| !d.is_empty());
    Some((state, dns))
}

fn tailscale_status(program: &Path) -> Option<(String, Option<String>)> {
    let (out, _) = run_quick(program, &["status", "--json"], Duration::from_secs(4))?;
    parse_tailscale_status(&out)
}

/// Inspects one tool (runs `--version`/`status`; blocking, a few seconds at most).
pub fn detect(kind: TunnelKind) -> ToolInfo {
    detect_with(kind, None, false)
}

/// Like [`detect`], with a configured binary (used when it exists) and whether an ngrok authtoken
/// is stored in the keychain (it is passed as `NGROK_AUTHTOKEN` when starting).
pub fn detect_with(kind: TunnelKind, program: Option<PathBuf>, keychain_token: bool) -> ToolInfo {
    let path = program.filter(|p| p.is_file()).or_else(|| find_program(kind));
    let mut info = ToolInfo {
        kind,
        label: kind.label(),
        installed: path.is_some(),
        path: path.as_ref().map(|p| p.display().to_string()),
        version: None,
        ready: None,
        hint: None,
        install_hint: kind.install_hint(),
        notes: kind.notes(),
        docs_url: kind.docs_url(),
        predicted_url: None,
    };
    let Some(program) = path else {
        info.hint = Some(format!("{} not found. {}", kind.names()[0], kind.install_hint()));
        return info;
    };
    info.version = version_of(kind, &program);
    match kind {
        TunnelKind::Cloudflared => {}
        TunnelKind::Ngrok => {
            let ok = keychain_token || ngrok_authtoken_configured();
            info.ready = Some(ok);
            if !ok {
                info.hint = Some("ngrok authtoken missing — copy it from dashboard.ngrok.com (free account) and paste it in the ngrok settings, or run `ngrok config add-authtoken <token>`.".into());
            }
        }
        TunnelKind::Tailscale => match tailscale_status(&program) {
            Some((state, dns)) => {
                info.ready = Some(state == "Running");
                info.predicted_url = dns.map(|d| format!("https://{d}"));
                info.hint = match state.as_str() {
                    "Running" => Some("Funnel must be allowed for this device (admin console → Access controls → `funnel` node attribute) with HTTPS certificates enabled.".into()),
                    "NeedsLogin" | "NoState" => Some("Log in to Tailscale first (open the app or run `tailscale up`).".into()),
                    "Stopped" => Some("Tailscale is stopped — connect it in the app or run `tailscale up`.".into()),
                    s => Some(format!("Tailscale is {s}.")),
                };
            }
            None => {
                info.ready = Some(false);
                info.hint = Some("Couldn't reach the Tailscale service — is Tailscale running?".into());
            }
        },
    }
    info
}

pub fn detect_all() -> Vec<ToolInfo> {
    TunnelKind::ALL.iter().map(|k| detect(*k)).collect()
}

// ---------- running tunnels ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TunnelState {
    Starting,
    Online,
    Error,
    Stopped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub id: String,
    pub kind: TunnelKind,
    /// Local port being exposed.
    pub port: u16,
    pub state: TunnelState,
    pub public_url: Option<String>,
    /// Connected to the provider's edge.
    pub connected: bool,
    pub message: Option<String>,
    pub hint: Option<String>,
    pub pid: Option<u32>,
    pub started_at: i64,
    pub exit_code: Option<i32>,
    /// Last output lines (stdout and stderr).
    pub log: Vec<String>,
}

pub struct Tunnel {
    id: String,
    status: watch::Receiver<TunnelStatus>,
    stop: watch::Sender<bool>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Tunnel {
    /// Starts `kind` exposing `127.0.0.1:port` with default settings. `program` overrides detection.
    pub async fn start(kind: TunnelKind, port: u16, program: Option<PathBuf>) -> Result<Arc<Tunnel>, WebhookError> {
        let launch = config::Launch { program, args: kind.args(port), env: Vec::new(), public_url: None, url_suffix: String::new() };
        Self::launch(kind, port, launch).await
    }

    /// Starts a tunnel from a command built by [`config::build`] (settings + keychain credentials).
    pub async fn launch(kind: TunnelKind, port: u16, launch: config::Launch) -> Result<Arc<Tunnel>, WebhookError> {
        let program = match launch.program {
            Some(p) if p.is_file() || p.components().count() == 1 => p,
            Some(p) => return Err(WebhookError::Invalid(format!("{} not found at {} — fix the path in Tunnel settings", kind.names()[0], p.display()))),
            None => find_program(kind).ok_or_else(|| WebhookError::Invalid(format!("{} is not installed. {}", kind.names()[0], kind.install_hint())))?,
        };
        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&launch.args).envs(launch.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd.spawn().map_err(|e| WebhookError::Io(format!("could not start {}: {e}", program.display())))?;
        let id = ulid::Ulid::generate().to_string();
        let initial = TunnelStatus {
            id: id.clone(),
            kind,
            port,
            state: TunnelState::Starting,
            public_url: launch.public_url.clone(),
            connected: false,
            message: Some(format!("Starting {}…", kind.label())),
            hint: None,
            pid: child.id(),
            started_at: now_ms(),
            exit_code: None,
            log: Vec::new(),
        };
        let (tx, rx) = watch::channel(initial);
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(supervise(child, kind, program, launch.url_suffix, tx, stop_rx));
        Ok(Arc::new(Tunnel { id, status: rx, stop, task: Mutex::new(Some(task)) }))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn status(&self) -> TunnelStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<TunnelStatus> {
        self.status.clone()
    }

    /// Waits until the tunnel is online, failed or stopped (or `timeout` passes).
    pub async fn wait_ready(&self, timeout: Duration) -> TunnelStatus {
        let mut rx = self.status.clone();
        let _ = tokio::time::timeout(timeout, rx.wait_for(|s| s.state != TunnelState::Starting)).await;
        self.status()
    }

    /// Kills the process tree and waits for it to exit.
    pub async fn stop(&self) {
        let _ = self.stop.send(true);
        let task = self.task.lock().unwrap().take();
        if let Some(t) = task {
            let _ = t.await;
        }
    }
}

/// Starts `kind` for `127.0.0.1:port` with the user's [`config::TunnelSettings`] and keychain
/// credentials (ngrok's version picks `--url` vs `--domain`).
pub async fn start_configured(kind: TunnelKind, port: u16, settings: &config::TunnelSettings, store: &dyn devall_core::SecretStore) -> Result<Arc<Tunnel>, WebhookError> {
    let launch = configured_launch(kind, port, settings, store).await?;
    Tunnel::launch(kind, port, launch).await
}

/// The command [`start_configured`] would run (credentials from `store`).
pub async fn configured_launch(kind: TunnelKind, port: u16, settings: &config::TunnelSettings, store: &dyn devall_core::SecretStore) -> Result<config::Launch, WebhookError> {
    let version = match (kind, settings.program(kind).filter(|p| p.is_file()).or_else(|| find_program(kind))) {
        (TunnelKind::Ngrok, Some(p)) => tokio::task::spawn_blocking(move || version_of(kind, &p)).await.ok().flatten(),
        _ => None,
    };
    config::build(kind, port, settings, version.as_deref(), store)
}

/// Result of [`test_tool`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelTest {
    pub ok: bool,
    pub public_url: Option<String>,
    pub message: String,
    pub hint: Option<String>,
}

/// Starts `kind` briefly with the user's settings (is the authtoken valid, the domain reserved,
/// Funnel allowed?) and stops it again. Nothing needs to listen on the exposed port.
pub async fn test_tool(kind: TunnelKind, settings: &config::TunnelSettings, store: &dyn devall_core::SecretStore, timeout: Duration) -> TunnelTest {
    let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(9);
    match start_configured(kind, port, settings, store).await {
        Err(e) => TunnelTest { ok: false, public_url: None, message: e.to_string(), hint: None },
        Ok(t) => {
            let s = t.wait_ready(timeout).await;
            t.stop().await;
            let ok = s.state == TunnelState::Online;
            let message = if ok {
                format!("{} works — it came online{}", kind.label(), s.public_url.as_deref().map(|u| format!(" at {u}")).unwrap_or_default())
            } else if s.state == TunnelState::Starting {
                format!("{} did not come online within {} s", kind.label(), timeout.as_secs())
            } else {
                s.message.clone().unwrap_or_else(|| format!("{} failed", kind.label()))
            };
            TunnelTest { ok, public_url: s.public_url, message, hint: s.hint }
        }
    }
}

fn push_log(log: &mut VecDeque<String>, line: &str) {
    if log.len() == LOG_LINES {
        log.pop_front();
    }
    log.push_back(line.chars().take(400).collect());
}

async fn supervise(mut child: tokio::process::Child, kind: TunnelKind, program: PathBuf, url_suffix: String, tx: watch::Sender<TunnelStatus>, mut stop: watch::Receiver<bool>) {
    let (line_tx, mut lines) = mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        let t = line_tx.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(out).lines();
            while let Ok(Some(l)) = r.next_line().await {
                let _ = t.send(l);
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let t = line_tx.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(err).lines();
            while let Ok(Some(l)) = r.next_line().await {
                let _ = t.send(l);
            }
        });
    }
    drop(line_tx);
    let pid = child.id();
    let mut log = VecDeque::new();
    let mut stopped_by_user = false;
    // Tailscale may not print the URL; derive it from `tailscale status` after a moment.
    let predict = tokio::time::sleep(Duration::from_secs(if kind == TunnelKind::Tailscale { 4 } else { 3600 }));
    tokio::pin!(predict);
    let mut predicted = false;

    let exit = loop {
        tokio::select! {
            Some(line) = lines.recv() => apply_line(&tx, &mut log, kind, &line),
            status = child.wait() => break status.ok().and_then(|s| s.code()),
            _ = stop.changed() => {
                stopped_by_user = true;
                kill_tree(pid, &mut child).await;
                break child.wait().await.ok().and_then(|s| s.code());
            }
            _ = &mut predict, if !predicted => {
                predicted = true;
                if tx.borrow().public_url.is_none() && kind == TunnelKind::Tailscale {
                    let p = program.clone();
                    if let Ok(Some((state, Some(dns)))) = tokio::task::spawn_blocking(move || tailscale_status(&p)).await
                        && state == "Running"
                    {
                        tx.send_modify(|s| {
                            s.public_url = Some(format!("https://{dns}{url_suffix}"));
                            s.state = TunnelState::Online;
                            s.message = Some("Funnel is serving (URL from tailscale status)".into());
                        });
                    }
                }
            }
        }
    };
    // Late output (the error that made it exit) is still useful.
    while let Ok(Some(line)) = tokio::time::timeout(Duration::from_millis(300), lines.recv()).await {
        apply_line(&tx, &mut log, kind, &line);
    }
    tx.send_modify(|s| {
        s.exit_code = exit;
        s.connected = false;
        if stopped_by_user {
            s.state = TunnelState::Stopped;
            s.message = Some("Stopped".into());
        } else {
            let was_error = s.state == TunnelState::Error;
            s.state = TunnelState::Error;
            if !was_error {
                let tail: Vec<&String> = log.iter().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
                let detail = if tail.is_empty() { String::new() } else { format!(": {}", tail.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" / ")) };
                s.message = Some(format!("{} exited{}{detail}", kind.names()[0], exit.map(|c| format!(" with code {c}")).unwrap_or_default()));
            }
        }
    });
}

fn apply_line(tx: &watch::Sender<TunnelStatus>, log: &mut VecDeque<String>, kind: TunnelKind, line: &str) {
    if line.trim().is_empty() {
        return;
    }
    push_log(log, line);
    let signals = parse::parse_line(kind, line);
    tx.send_modify(|s| {
        s.log = log.iter().cloned().collect();
        for sig in signals {
            match sig {
                Signal::Url(u) => {
                    if s.public_url.is_none() {
                        s.public_url = Some(u);
                    }
                    if s.state != TunnelState::Error {
                        s.state = TunnelState::Online;
                        s.message = Some("Tunnel is up".into());
                        s.hint = None;
                    }
                }
                Signal::Connected => {
                    s.connected = true;
                    if s.public_url.is_some() && s.state == TunnelState::Starting {
                        s.state = TunnelState::Online;
                    }
                }
                Signal::Error { message, hint, fatal } => {
                    if fatal || s.state == TunnelState::Starting {
                        s.message = Some(message);
                        if hint.is_some() {
                            s.hint = hint;
                        }
                    }
                    if fatal {
                        s.state = TunnelState::Error;
                    }
                }
            }
        }
    });
}

/// Kills the process and its children (scoop/Chocolatey shims start the real binary as a child).
async fn kill_tree(pid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = pid {
        let mut cmd = tokio::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
        let _ = tokio::time::timeout(Duration::from_secs(5), cmd.status()).await;
    }
    #[cfg(unix)]
    if let Some(pid) = pid {
        // The child leads its own process group (see `process_group(0)`).
        // SAFETY: plain syscall; a stale group id at worst yields ESRCH.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(3), child.wait()).await.is_ok() {
            return;
        }
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        assert_eq!(TunnelKind::Cloudflared.args(4590), ["tunnel", "--no-autoupdate", "--url", "http://127.0.0.1:4590"]);
        assert_eq!(TunnelKind::Ngrok.args(4590), ["http", "127.0.0.1:4590", "--log", "stdout", "--log-format", "json"]);
        assert_eq!(TunnelKind::Tailscale.args(4590), ["funnel", "4590"]);
    }

    #[test]
    fn ngrok_config_detection() {
        assert!(config_has_authtoken("version: \"3\"\nagent:\n  authtoken: 2abcDEF_123\n"));
        assert!(config_has_authtoken("authtoken: 'x'\nregion: us\n"));
        assert!(!config_has_authtoken("version: \"3\"\nagent:\n  authtoken: \"\"\n"));
        assert!(!config_has_authtoken("# authtoken: commented\nversion: 2\n"));
    }

    #[test]
    fn tailscale_status_json() {
        let json = r#"{"Version":"1.76.1","BackendState":"Running","Self":{"DNSName":"devbox.tail1234.ts.net.","HostName":"devbox"}}"#;
        assert_eq!(parse_tailscale_status(json), Some(("Running".into(), Some("devbox.tail1234.ts.net".into()))));
        assert_eq!(parse_tailscale_status(r#"{"BackendState":"NeedsLogin","Self":{"DNSName":""}}"#), Some(("NeedsLogin".into(), None)));
        assert_eq!(parse_tailscale_status("nope"), None);
    }

    #[test]
    fn tunnel_hosts() {
        assert!(is_tunnel_host("abc-def.trycloudflare.com"));
        assert!(is_tunnel_host("1a2b.ngrok-free.app:443"));
        assert!(is_tunnel_host("devbox.tail1234.ts.net"));
        assert!(!is_tunnel_host("trycloudflare.com.evil.example"));
        assert!(!is_tunnel_host("127.0.0.1:4590"));
    }

    #[test]
    fn status_from_lines() {
        let (tx, rx) = watch::channel(TunnelStatus {
            id: "t".into(),
            kind: TunnelKind::Ngrok,
            port: 1,
            state: TunnelState::Starting,
            public_url: None,
            connected: false,
            message: None,
            hint: None,
            pid: None,
            started_at: 0,
            exit_code: None,
            log: vec![],
        });
        let mut log = VecDeque::new();
        apply_line(&tx, &mut log, TunnelKind::Ngrok, r#"{"lvl":"eror","msg":"session closing","err":"authentication failed: Usage of ngrok requires a verified account and authtoken.\n\nERR_NGROK_4018\n"}"#);
        let s = rx.borrow().clone();
        assert_eq!(s.state, TunnelState::Error);
        assert!(s.hint.unwrap().contains("authtoken missing"));
        assert_eq!(s.log.len(), 1);

        let (tx, rx) = watch::channel(TunnelStatus { state: TunnelState::Starting, kind: TunnelKind::Cloudflared, ..rx.borrow().clone() });
        apply_line(&tx, &mut VecDeque::new(), TunnelKind::Cloudflared, "2024-10-21T10:00:01Z INF |  https://a-b.trycloudflare.com   |");
        assert_eq!(rx.borrow().state, TunnelState::Online);
        assert_eq!(rx.borrow().public_url.as_deref(), Some("https://a-b.trycloudflare.com"));
    }

    /// Exercises the supervisor with a harmless local command instead of a real tunnel.
    #[tokio::test]
    async fn supervisor_reports_exit_of_a_failing_program() {
        let (program, args): (&str, Vec<&str>) = if cfg!(windows) { ("cmd", vec!["/C", "echo ERROR:  authentication failed ERR_NGROK_4018 1>&2 && exit 3"]) } else { ("sh", vec!["-c", "echo 'ERROR:  authentication failed ERR_NGROK_4018' >&2; exit 3"]) };
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let child = cmd.spawn().unwrap();
        let (tx, mut rx) = watch::channel(TunnelStatus {
            id: "t".into(),
            kind: TunnelKind::Ngrok,
            port: 1,
            state: TunnelState::Starting,
            public_url: None,
            connected: false,
            message: None,
            hint: None,
            pid: child.id(),
            started_at: 0,
            exit_code: None,
            log: vec![],
        });
        let (_stop_tx, stop_rx) = watch::channel(false);
        supervise(child, TunnelKind::Ngrok, PathBuf::from(program), String::new(), tx, stop_rx).await;
        let s = rx.borrow_and_update().clone();
        assert_eq!(s.state, TunnelState::Error);
        assert_eq!(s.exit_code, Some(3));
        assert!(s.hint.as_deref().unwrap_or("").contains("authtoken missing"), "{s:?}");
    }

    #[tokio::test]
    async fn stop_kills_a_long_running_program() {
        let (program, args): (&str, Vec<&str>) = if cfg!(windows) { ("cmd", vec!["/C", "ping -n 30 127.0.0.1 >NUL"]) } else { ("sh", vec!["-c", "sleep 30"]) };
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd.spawn().unwrap();
        let initial = TunnelStatus {
            id: "t".into(),
            kind: TunnelKind::Cloudflared,
            port: 1,
            state: TunnelState::Starting,
            public_url: None,
            connected: false,
            message: None,
            hint: None,
            pid: child.id(),
            started_at: 0,
            exit_code: None,
            log: vec![],
        };
        let (tx, rx) = watch::channel(initial);
        let (stop, stop_rx) = watch::channel(false);
        let t = Tunnel { id: "t".into(), status: rx, stop, task: Mutex::new(Some(tokio::spawn(supervise(child, TunnelKind::Cloudflared, PathBuf::from(program), String::new(), tx, stop_rx)))) };
        let started = std::time::Instant::now();
        t.stop().await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(t.status().state, TunnelState::Stopped);
    }
}
