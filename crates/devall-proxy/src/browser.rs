//! Pointing apps at the proxy without touching system settings: a Chromium-family browser with an
//! isolated profile that routes through the proxy and accepts the CA by its SPKI pin (no trust
//! store change), and copy-paste environment snippets for terminals.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::{ProxyError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowserKind {
    Chrome,
    Edge,
    Brave,
    Chromium,
}

impl BrowserKind {
    pub const ALL: [BrowserKind; 4] = [BrowserKind::Chrome, BrowserKind::Edge, BrowserKind::Brave, BrowserKind::Chromium];

    pub fn name(self) -> &'static str {
        match self {
            BrowserKind::Chrome => "Google Chrome",
            BrowserKind::Edge => "Microsoft Edge",
            BrowserKind::Brave => "Brave",
            BrowserKind::Chromium => "Chromium",
        }
    }

    fn id(self) -> &'static str {
        match self {
            BrowserKind::Chrome => "chrome",
            BrowserKind::Edge => "edge",
            BrowserKind::Brave => "brave",
            BrowserKind::Chromium => "chromium",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserInfo {
    pub kind: BrowserKind,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Launched {
    pub kind: BrowserKind,
    pub path: String,
    pub args: Vec<String>,
    pub profile_dir: String,
    pub pid: u32,
}

fn candidates(kind: BrowserKind) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        let env = |k: &str| std::env::var(k).ok().map(PathBuf::from);
        let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"].iter().filter_map(|k| env(k)).collect();
        let rel: &[&str] = match kind {
            BrowserKind::Chrome => &["Google/Chrome/Application/chrome.exe"],
            BrowserKind::Edge => &["Microsoft/Edge/Application/msedge.exe"],
            BrowserKind::Brave => &["BraveSoftware/Brave-Browser/Application/brave.exe"],
            BrowserKind::Chromium => &["Chromium/Application/chrome.exe"],
        };
        for r in &roots {
            for p in rel {
                out.push(p.split('/').fold(r.clone(), |acc, part| acc.join(part)));
            }
        }
    } else if cfg!(target_os = "macos") {
        let app = match kind {
            BrowserKind::Chrome => "Google Chrome.app/Contents/MacOS/Google Chrome",
            BrowserKind::Edge => "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            BrowserKind::Brave => "Brave Browser.app/Contents/MacOS/Brave Browser",
            BrowserKind::Chromium => "Chromium.app/Contents/MacOS/Chromium",
        };
        out.push(Path::new("/Applications").join(app));
        if let Ok(home) = std::env::var("HOME") {
            out.push(Path::new(&home).join("Applications").join(app));
        }
    } else {
        let names: &[&str] = match kind {
            BrowserKind::Chrome => &["google-chrome", "google-chrome-stable"],
            BrowserKind::Edge => &["microsoft-edge", "microsoft-edge-stable"],
            BrowserKind::Brave => &["brave-browser", "brave"],
            BrowserKind::Chromium => &["chromium", "chromium-browser"],
        };
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                for n in names {
                    out.push(dir.join(n));
                }
            }
        }
    }
    out
}

pub fn find_browser(kind: BrowserKind) -> Option<PathBuf> {
    candidates(kind).into_iter().find(|p| p.is_file())
}

/// Installed Chromium-family browsers.
pub fn available() -> Vec<BrowserInfo> {
    BrowserKind::ALL.iter().filter_map(|k| find_browser(*k).map(|p| BrowserInfo { kind: *k, name: k.name().into(), path: p.display().to_string() })).collect()
}

/// Command-line flags: proxy everything (including localhost), accept the CA by its key pin, and
/// keep the profile separate from the user's normal one.
pub fn launch_args(proxy_port: u16, spki_sha256: &str, profile_dir: &Path, start_url: Option<&str>) -> Vec<String> {
    let mut args = vec![
        format!("--proxy-server=http://127.0.0.1:{proxy_port}"),
        "--proxy-bypass-list=<-loopback>".to_string(),
        format!("--ignore-certificate-errors-spki-list={spki_sha256}"),
        format!("--user-data-dir={}", profile_dir.display()),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-background-networking".to_string(),
    ];
    args.push(start_url.unwrap_or("http://devall.proxy/").to_string());
    args
}

/// The isolated profile folder for a browser kind (inside the OS temp directory).
pub fn profile_dir(kind: BrowserKind) -> PathBuf {
    std::env::temp_dir().join(format!("devall-proxy-{}", kind.id()))
}

/// Starts the browser through the proxy. Explicit user action only.
pub fn launch(kind: BrowserKind, proxy_port: u16, spki_sha256: &str, start_url: Option<&str>) -> Result<Launched> {
    let path = find_browser(kind).ok_or_else(|| ProxyError::Invalid(format!("{} is not installed (looked in the standard locations)", kind.name())))?;
    let profile = profile_dir(kind);
    std::fs::create_dir_all(&profile).map_err(|e| ProxyError::IoAt { path: profile.clone(), source: e })?;
    let args = launch_args(proxy_port, spki_sha256, &profile, start_url);
    let child = Command::new(&path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| ProxyError::Invalid(format!("could not start {}: {e}", path.display())))?;
    Ok(Launched { kind, path: path.display().to_string(), args, profile_dir: profile.display().to_string(), pid: child.id() })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvSnippet {
    /// `bash` (bash/zsh), `fish`, `powershell` or `cmd`.
    pub shell: String,
    pub label: String,
    pub text: String,
}

/// Environment variables that route CLI tools and runtimes through the proxy.
pub fn env_snippets(proxy_port: u16, ca_path: &Path) -> Vec<EnvSnippet> {
    let proxy = format!("http://127.0.0.1:{proxy_port}");
    let ca = ca_path.display().to_string();
    let vars: Vec<(&str, String)> = vec![
        ("HTTP_PROXY", proxy.clone()),
        ("HTTPS_PROXY", proxy.clone()),
        ("http_proxy", proxy.clone()),
        ("https_proxy", proxy.clone()),
        ("NODE_EXTRA_CA_CERTS", ca.clone()),
        ("NODE_USE_ENV_PROXY", "1".into()),
    ];
    let sq = |v: &str| format!("'{}'", v.replace('\'', "'\\''"));
    vec![
        EnvSnippet {
            shell: "bash".into(),
            label: "bash / zsh".into(),
            text: vars.iter().map(|(k, v)| format!("export {k}={}", sq(v))).collect::<Vec<_>>().join("\n"),
        },
        EnvSnippet {
            shell: "fish".into(),
            label: "fish".into(),
            text: vars.iter().map(|(k, v)| format!("set -gx {k} {}", sq(v))).collect::<Vec<_>>().join("\n"),
        },
        EnvSnippet {
            shell: "powershell".into(),
            label: "PowerShell".into(),
            text: vars
                .iter()
                .filter(|(k, _)| !k.starts_with("http"))
                .map(|(k, v)| format!("$env:{k} = \"{}\"", v.replace('"', "`\"")))
                .collect::<Vec<_>>()
                .join("\n"),
        },
        EnvSnippet {
            shell: "cmd".into(),
            label: "Command Prompt".into(),
            text: vars.iter().filter(|(k, _)| !k.starts_with("http")).map(|(k, v)| format!("set \"{k}={v}\"")).collect::<Vec<_>>().join("\n"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_pin_the_ca_and_isolate_the_profile() {
        let args = launch_args(8899, "AbC=", Path::new("/tmp/p"), None);
        assert!(args.contains(&"--proxy-server=http://127.0.0.1:8899".to_string()));
        assert!(args.contains(&"--ignore-certificate-errors-spki-list=AbC=".to_string()));
        assert!(args.iter().any(|a| a.starts_with("--user-data-dir=")));
        assert_eq!(args.last().unwrap(), "http://devall.proxy/");
        assert!(profile_dir(BrowserKind::Edge).ends_with("devall-proxy-edge"));
    }

    #[test]
    fn snippets() {
        let s = env_snippets(8899, Path::new("/x/ca.pem"));
        let bash = &s.iter().find(|x| x.shell == "bash").unwrap().text;
        assert!(bash.contains("export HTTPS_PROXY='http://127.0.0.1:8899'"));
        assert!(bash.contains("export NODE_EXTRA_CA_CERTS='/x/ca.pem'"));
        let ps = &s.iter().find(|x| x.shell == "powershell").unwrap().text;
        assert!(ps.contains("$env:HTTPS_PROXY = \"http://127.0.0.1:8899\""));
        assert!(!ps.contains("$env:http_proxy"));
    }

    #[test]
    fn candidate_paths_exist_for_every_kind() {
        for k in BrowserKind::ALL {
            // PATH may be empty on CI Linux; Windows/macOS always have fixed candidates.
            if cfg!(any(windows, target_os = "macos")) {
                assert!(!candidates(k).is_empty());
            }
        }
    }
}
