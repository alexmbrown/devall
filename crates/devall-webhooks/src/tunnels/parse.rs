//! Turns tunnel CLI output lines into status signals. Pure functions, tested with captured output.

use regex::Regex;
use std::sync::LazyLock;

use super::TunnelKind;

/// What one line of output means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// The public URL is known.
    Url(String),
    /// The tunnel is connected to the edge.
    Connected,
    /// A problem; `fatal` ones end the tunnel (the process usually exits too).
    Error { message: String, hint: Option<String>, fatal: bool },
}

static TRYCLOUDFLARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https://([a-z0-9-]+)\.trycloudflare\.com").unwrap());
static TS_NET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https://[a-zA-Z0-9.-]+\.ts\.net(?::\d+)?").unwrap());
static NGROK_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"ERR_NGROK_(\d+)").unwrap());
static ANY_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https://[^\s'\x22)]+").unwrap());

pub fn parse_line(kind: TunnelKind, line: &str) -> Vec<Signal> {
    match kind {
        TunnelKind::Cloudflared => cloudflared(line),
        TunnelKind::Ngrok => ngrok(line),
        TunnelKind::Tailscale => tailscale(line),
    }
}

/// `cloudflared tunnel --url …` logs to stderr, e.g.
/// `2024-10-21T10:00:01Z INF |  https://seasonal-deck-organisms-sf.trycloudflare.com  |`.
fn cloudflared(line: &str) -> Vec<Signal> {
    let mut out = Vec::new();
    if let Some(m) = TRYCLOUDFLARE.captures(line)
        && &m[1] != "api"
    {
        out.push(Signal::Url(m[0].to_string()));
    }
    if line.contains("Registered tunnel connection") {
        out.push(Signal::Connected);
    }
    let is_err = line.contains(" ERR ") || line.starts_with("ERR ") || line.contains("level=error");
    if is_err || line.to_ascii_lowercase().contains("failed to request quick tunnel") {
        let message = strip_log_prefix(line);
        let lower = message.to_ascii_lowercase();
        let (hint, fatal) = if lower.contains("failed to request quick tunnel") || lower.contains("api.trycloudflare.com") {
            (Some("cloudflared couldn't reach Cloudflare to create a quick tunnel — check your connection, proxy or firewall, then try again.".to_string()), true)
        } else if lower.contains("config") && (lower.contains("credentials") || lower.contains("ingress")) {
            (Some("A ~/.cloudflared/config.yml (named-tunnel config) interferes with quick tunnels — rename it while using quick tunnels.".to_string()), true)
        } else if lower.contains("quic") || lower.contains("udp") {
            (Some("QUIC (UDP) looks blocked; cloudflared falls back to HTTP/2 automatically.".to_string()), false)
        } else {
            (None, false)
        };
        out.push(Signal::Error { message, hint, fatal });
    }
    out
}

/// `ngrok http … --log stdout --log-format json` prints one JSON object per line, e.g.
/// `{"lvl":"info","msg":"started tunnel","obj":"tunnels","url":"https://….ngrok-free.app"}`.
/// Plain-text errors (`ERROR:  authentication failed: …`) are understood too.
fn ngrok(line: &str) -> Vec<Signal> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        if let Some(url) = v.get("url").and_then(|u| u.as_str()).filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            && (s("msg") == "started tunnel" || s("obj") == "tunnels")
        {
            out.push(Signal::Url(url.to_string()));
        }
        if s("msg") == "client session established" || s("msg") == "tunnel session started" {
            out.push(Signal::Connected);
        }
        let lvl = s("lvl");
        let err = s("err");
        if (lvl == "eror" || lvl == "crit" || lvl == "error") && !err.is_empty() && err != "<nil>" {
            out.push(ngrok_error(err, lvl == "crit" || s("msg") == "session closing" || s("msg") == "command failed"));
        }
        return out;
    }
    let t = line.trim();
    if t.starts_with("ERROR:") || t.starts_with("ERR_NGROK") || NGROK_CODE.is_match(t) || t.contains("authentication failed") {
        out.push(ngrok_error(t.trim_start_matches("ERROR:").trim(), true));
    }
    out
}

fn ngrok_error(err: &str, fatal: bool) -> Signal {
    let first = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(err).to_string();
    let code = NGROK_CODE.captures(err).map(|c| c[1].to_string());
    let lower = err.to_ascii_lowercase();
    let hint = match code.as_deref() {
        Some("4018") => Some("ngrok authtoken missing — sign up at dashboard.ngrok.com, then run `ngrok config add-authtoken <token>` (Your Authtoken page).".to_string()),
        Some("105") | Some("107") => Some("The ngrok authtoken is invalid or was reset — copy it again from dashboard.ngrok.com/get-started/your-authtoken and run `ngrok config add-authtoken <token>`.".to_string()),
        Some("108") => Some("Your ngrok plan allows a limited number of agent sessions and one is already running — stop the other ngrok (or close it in the dashboard).".to_string()),
        Some("334") => Some("This ngrok endpoint is already online from another agent — stop it there first.".to_string()),
        Some("120") | Some("121") => Some("This ngrok agent is too old for your account — update ngrok (`ngrok update`).".to_string()),
        Some(c) => Some(format!("See https://ngrok.com/docs/errors/err_ngrok_{c}")),
        None if lower.contains("authtoken") || lower.contains("authentication failed") => {
            Some("ngrok needs an authtoken: run `ngrok config add-authtoken <token>` (dashboard.ngrok.com → Your Authtoken).".to_string())
        }
        None => None,
    };
    Signal::Error { message: first, hint, fatal }
}

/// `tailscale funnel <port>` prints
/// ```text
/// Available on the internet:
///
/// https://devbox.tail1234.ts.net/
/// |-- proxy http://127.0.0.1:4590
/// ```
fn tailscale(line: &str) -> Vec<Signal> {
    let mut out = Vec::new();
    let t = line.trim();
    if let Some(m) = TS_NET.find(t) {
        out.push(Signal::Url(m.as_str().to_string()));
        if t.starts_with("https://") {
            out.push(Signal::Connected);
        }
    }
    let lower = t.to_ascii_lowercase();
    let err = |message: &str, hint: String| Signal::Error { message: message.to_string(), hint: Some(hint), fatal: true };
    if lower.contains("funnel is not enabled") || lower.contains("funnel not available") || (lower.contains("nodeattr") && lower.contains("funnel")) {
        out.push(err(t, "Funnel isn't enabled for this device: enable it in the Tailscale admin console (the link tailscale printed, or Access controls → add the `funnel` node attribute), then try again.".into()));
    } else if lower.contains("https") && (lower.contains("not enabled") || lower.contains("certificate")) && !lower.starts_with("https://") {
        out.push(err(t, "Enable HTTPS certificates (and MagicDNS) for your tailnet in the admin console → DNS, then try again.".into()));
    } else if lower.contains("doesn't appear to be running") || lower.contains("failed to connect to local tailscale") || lower.contains("tailscale is stopped") {
        out.push(err(t, "Start Tailscale (open the app or `tailscale up`) and try again.".into()));
    } else if lower.contains("logged out") || lower.contains("needslogin") || lower.contains("not logged in") {
        out.push(err(t, "Log in to Tailscale (`tailscale up` or the app) and try again.".into()));
    } else if lower.contains("access denied") || lower.contains("permission denied") {
        out.push(err(t, "Tailscale refused to change the serve config: on Linux run `sudo tailscale set --operator=$USER`; elsewhere run the app as the logged-in user.".into()));
    } else if lower.starts_with("error") || lower.contains("invalid argument") {
        out.push(Signal::Error { message: t.to_string(), hint: None, fatal: false });
    } else if lower.starts_with("https://login.tailscale.com/") {
        out.push(Signal::Error { message: format!("Action needed: {t}"), hint: Some("Open this link to enable Funnel for this device.".into()), fatal: false });
    }
    out
}

/// `2024-10-21T10:00:01Z ERR message key=value` → `message key=value`.
fn strip_log_prefix(line: &str) -> String {
    let t = line.trim();
    let mut parts = t.splitn(3, ' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(ts), Some(lvl), Some(rest)) if ts.contains('T') && lvl.len() == 3 && lvl.chars().all(|c| c.is_ascii_uppercase()) => rest.to_string(),
        _ => t.to_string(),
    }
}

/// The first `https://…` URL in a line (used for links tailscale prints).
pub fn first_url(line: &str) -> Option<String> {
    ANY_URL.find(line).map(|m| m.as_str().trim_end_matches(['.', ',']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(kind: TunnelKind, lines: &str) -> Option<String> {
        lines.lines().flat_map(|l| parse_line(kind, l)).find_map(|s| match s {
            Signal::Url(u) => Some(u),
            _ => None,
        })
    }

    #[test]
    fn cloudflared_quick_tunnel() {
        let out = "\
2024-10-21T10:00:00Z INF Thank you for trying Cloudflare Tunnel. Doing so, without a Cloudflare account, is a quick way to experiment and try it out. However, be aware that these account-less Tunnels have no uptime guarantee, are subject to the Cloudflare Online Services Terms of Use (https://www.cloudflare.com/website-terms/), and Cloudflare reserves the right to investigate your use of Tunnels for violations of such terms. If you intend to use Tunnels in production you should use a pre-created named tunnel by following: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/
2024-10-21T10:00:00Z INF Requesting new quick Tunnel on trycloudflare.com...
2024-10-21T10:00:01Z INF +--------------------------------------------------------------------------------------------+
2024-10-21T10:00:01Z INF |  Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  |
2024-10-21T10:00:01Z INF |  https://seasonal-deck-organisms-sf.trycloudflare.com                                      |
2024-10-21T10:00:01Z INF +--------------------------------------------------------------------------------------------+
2024-10-21T10:00:02Z INF Registered tunnel connection connIndex=0 connection=8c3b ip=198.41.192.227 location=ams01 protocol=quic";
        assert_eq!(url(TunnelKind::Cloudflared, out).as_deref(), Some("https://seasonal-deck-organisms-sf.trycloudflare.com"));
        assert!(out.lines().flat_map(|l| parse_line(TunnelKind::Cloudflared, l)).any(|s| s == Signal::Connected));
        // No false errors in a clean run.
        assert!(!out.lines().flat_map(|l| parse_line(TunnelKind::Cloudflared, l)).any(|s| matches!(s, Signal::Error { .. })));
    }

    #[test]
    fn cloudflared_errors() {
        let line = r#"2024-10-21T10:00:00Z ERR Error unmarshaling QuickTunnel response: error="failed to request quick Tunnel: Post \"https://api.trycloudflare.com/tunnel\": dial tcp: lookup api.trycloudflare.com: no such host""#;
        let sigs = parse_line(TunnelKind::Cloudflared, line);
        assert!(!sigs.iter().any(|s| matches!(s, Signal::Url(_))), "api host is not the tunnel URL");
        match &sigs[0] {
            Signal::Error { message, hint, fatal } => {
                assert!(message.starts_with("Error unmarshaling"));
                assert!(hint.as_deref().unwrap().contains("couldn't reach Cloudflare"));
                assert!(*fatal);
            }
            s => panic!("{s:?}"),
        }
        let quic = "2024-10-21T10:00:03Z ERR Failed to dial a quic connection error=\"failed to dial to edge with quic: timeout\" connIndex=0";
        assert!(matches!(&parse_line(TunnelKind::Cloudflared, quic)[0], Signal::Error { fatal: false, .. }));
    }

    #[test]
    fn ngrok_json_logs() {
        let started = r#"{"addr":"http://127.0.0.1:4590","lvl":"info","msg":"started tunnel","name":"command_line","obj":"tunnels","t":"2024-10-21T10:00:00.123+02:00","url":"https://1a2b-203-0-113-7.ngrok-free.app"}"#;
        assert_eq!(parse_line(TunnelKind::Ngrok, started), vec![Signal::Url("https://1a2b-203-0-113-7.ngrok-free.app".into())]);
        let session = r#"{"lvl":"info","msg":"client session established","obj":"tunnels.session","t":"2024-10-21T10:00:00+02:00"}"#;
        assert_eq!(parse_line(TunnelKind::Ngrok, session), vec![Signal::Connected]);
        let noise = r#"{"lvl":"info","msg":"open config file","path":"C:\\Users\\me\\AppData\\Local\\ngrok\\ngrok.yml","err":"<nil>"}"#;
        assert!(parse_line(TunnelKind::Ngrok, noise).is_empty());
    }

    #[test]
    fn ngrok_auth_errors_are_explained() {
        let missing = r#"{"err":"authentication failed: Usage of ngrok requires a verified account and authtoken.\n\nSign up for an account: https://dashboard.ngrok.com/signup\nInstall your authtoken: https://dashboard.ngrok.com/get-started/your-authtoken\r\n\r\nERR_NGROK_4018\r\n","lvl":"eror","msg":"session closing","obj":"tunnels.session","t":"2024-10-21T10:00:00+02:00"}"#;
        match &parse_line(TunnelKind::Ngrok, missing)[0] {
            Signal::Error { message, hint, fatal } => {
                assert_eq!(message, "authentication failed: Usage of ngrok requires a verified account and authtoken.");
                assert!(hint.as_deref().unwrap().starts_with("ngrok authtoken missing"));
                assert!(*fatal);
            }
            s => panic!("{s:?}"),
        }
        let invalid = "ERROR:  authentication failed: The authtoken you specified does not look like a proper ngrok tunnel authtoken.\nERROR:  ERR_NGROK_105";
        let sigs: Vec<Signal> = invalid.lines().flat_map(|l| parse_line(TunnelKind::Ngrok, l)).collect();
        assert!(sigs.iter().any(|s| matches!(s, Signal::Error { hint: Some(h), .. } if h.contains("invalid or was reset"))), "{sigs:?}");
        let limit = r#"{"lvl":"crit","msg":"command failed","err":"failed to start tunnel: Your account is limited to 1 simultaneous ngrok agent sessions.\r\n\r\nERR_NGROK_108\r\n"}"#;
        assert!(matches!(&parse_line(TunnelKind::Ngrok, limit)[0], Signal::Error { hint: Some(h), .. } if h.contains("agent sessions")));
    }

    #[test]
    fn tailscale_funnel() {
        let out = "Available on the internet:\n\nhttps://devbox.tail1234.ts.net/\n|-- proxy http://127.0.0.1:4590\n\nPress Ctrl+C to exit.";
        assert_eq!(url(TunnelKind::Tailscale, out).as_deref(), Some("https://devbox.tail1234.ts.net"));
        let disabled = "Funnel is not enabled on your tailnet.\nTo enable, visit:\n\n         https://login.tailscale.com/f/funnel?node=nXXXXXCNTRL\n";
        let sigs: Vec<Signal> = disabled.lines().flat_map(|l| parse_line(TunnelKind::Tailscale, l)).collect();
        assert!(matches!(&sigs[0], Signal::Error { fatal: true, hint: Some(h), .. } if h.contains("admin console")));
        assert!(sigs.iter().any(|s| matches!(s, Signal::Error { message, .. } if message.contains("login.tailscale.com/f/funnel"))));
        let stopped = "failed to connect to local tailscaled; it doesn't appear to be running (sudo systemctl start tailscaled ?)";
        assert!(matches!(&parse_line(TunnelKind::Tailscale, stopped)[0], Signal::Error { hint: Some(h), .. } if h.contains("Start Tailscale")));
    }

    #[test]
    fn helpers() {
        assert_eq!(strip_log_prefix("2024-10-21T10:00:00Z ERR boom x=1"), "boom x=1");
        assert_eq!(strip_log_prefix("plain"), "plain");
        assert_eq!(first_url("visit: https://login.tailscale.com/f/funnel?node=n1."), Some("https://login.tailscale.com/f/funnel?node=n1".into()));
    }
}
