//! Trusting the local CA: the exact OS commands (shown to the user before anything runs) and a
//! runner for them. Installing is always an explicit user action — nothing here runs on its own,
//! and tests only build command lines.
//!
//! - **Windows:** `certutil -user -addstore Root <cert>` — the current user's Root store, no
//!   admin rights; Windows shows its own "Security Warning" confirmation.
//! - **macOS:** `security add-trusted-cert -d -r trustRoot -k ~/Library/Keychains/login.keychain-db
//!   <cert>` — macOS asks for the user's password.
//! - **Linux:** shown as instructions only (needs `sudo`).
//!
//! Firefox, Node.js, Python, curl and Java keep their own trust stores; [`TrustInfo::manual`]
//! lists how to point each at the CA.

use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::ca::CertAuthority;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Os {
    Windows,
    Macos,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Linux
        }
    }
}

/// A command line the UI shows verbatim and may run after the user confirms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustCommand {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    /// The command as a copy-pasteable shell line.
    pub display: String,
    /// False when it needs elevated rights (shown as instructions only).
    pub runnable: bool,
    /// What the OS will ask the user.
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustNote {
    pub title: String,
    pub body: String,
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustInfo {
    pub os: Os,
    /// `Some(true)` when the CA is in the OS trust store, `None` when that cannot be checked.
    pub installed: Option<bool>,
    pub install: Option<TrustCommand>,
    pub uninstall: Option<TrustCommand>,
    pub manual: Vec<TrustNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn quote(arg: &str, os: Os) -> String {
    let needs = arg.is_empty() || arg.chars().any(|c| c.is_whitespace() || "\"'$`&|;<>()*?!~#".contains(c));
    if !needs {
        return arg.to_string();
    }
    match os {
        Os::Windows => format!("\"{}\"", arg.replace('"', "\\\"")),
        _ => format!("'{}'", arg.replace('\'', "'\\''")),
    }
}

fn command(label: &str, os: Os, program: &str, args: Vec<String>, runnable: bool, prompt: Option<&str>) -> TrustCommand {
    let display = std::iter::once(program.to_string()).chain(args.iter().map(|a| quote(a, os))).collect::<Vec<_>>().join(" ");
    TrustCommand { label: label.into(), program: program.into(), args, display, runnable, prompt: prompt.map(str::to_string) }
}

fn mac_login_keychain() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "~".into());
    format!("{home}/Library/Keychains/login.keychain-db")
}

pub fn install_command(os: Os, cert_path: &Path) -> TrustCommand {
    let cert = cert_path.display().to_string();
    match os {
        Os::Windows => command(
            "Trust for the current Windows user",
            os,
            "certutil",
            vec!["-user".into(), "-addstore".into(), "Root".into(), cert],
            true,
            Some("Windows shows a Security Warning asking whether to install the certificate — choose Yes."),
        ),
        Os::Macos => command(
            "Trust in your login keychain",
            os,
            "security",
            vec!["add-trusted-cert".into(), "-d".into(), "-r".into(), "trustRoot".into(), "-k".into(), mac_login_keychain(), cert],
            true,
            Some("macOS asks for your password to change certificate trust settings."),
        ),
        Os::Linux => command(
            "Trust system-wide (Debian/Ubuntu)",
            os,
            "sh",
            vec!["-c".into(), format!("sudo cp {} /usr/local/share/ca-certificates/devall-ca.crt && sudo update-ca-certificates", quote(&cert, os))],
            false,
            None,
        ),
    }
}

pub fn uninstall_command(os: Os, cert_path: &Path, sha1_hex: &str) -> TrustCommand {
    match os {
        Os::Windows => command(
            "Remove from the current Windows user",
            os,
            "certutil",
            vec!["-user".into(), "-delstore".into(), "Root".into(), sha1_hex.to_string()],
            true,
            Some("Windows may ask you to confirm removing the certificate."),
        ),
        Os::Macos => command(
            "Remove from your login keychain",
            os,
            "security",
            vec!["delete-certificate".into(), "-Z".into(), sha1_hex.to_string(), "-t".into(), mac_login_keychain()],
            true,
            Some("macOS may ask for your password."),
        ),
        Os::Linux => {
            let _ = cert_path;
            command(
                "Remove system-wide (Debian/Ubuntu)",
                os,
                "sh",
                vec!["-c".into(), "sudo rm -f /usr/local/share/ca-certificates/devall-ca.crt && sudo update-ca-certificates --fresh".into()],
                false,
                None,
            )
        }
    }
}

fn check_command(os: Os, ca: &CertAuthority) -> Option<(String, Vec<String>)> {
    match os {
        Os::Windows => Some(("certutil".into(), vec!["-user".into(), "-store".into(), "Root".into(), ca.sha1_hex()])),
        Os::Macos => {
            Some(("security".into(), vec!["find-certificate".into(), "-a".into(), "-Z".into(), "-c".into(), ca.common_name().into(), mac_login_keychain()]))
        }
        Os::Linux => None,
    }
}

/// Notes for tools that ignore the OS store.
pub fn manual_notes(os: Os, ca_path: &Path, proxy_port: u16) -> Vec<TrustNote> {
    let ca = ca_path.display().to_string();
    let q = quote(&ca, os);
    let proxy = format!("http://127.0.0.1:{proxy_port}");
    let env = |name: &str, value: &str| match os {
        Os::Windows => format!("$env:{name} = \"{value}\""),
        _ => format!("export {name}={}", quote(value, os)),
    };
    vec![
        TrustNote {
            title: "Browser without changing system trust".into(),
            body: "\"Launch browser\" starts Chrome or Edge with an isolated profile that routes through the proxy and accepts this CA by its key pin — nothing is installed.".into(),
            command: None,
        },
        TrustNote {
            title: "Firefox".into(),
            body: "Settings → Privacy & Security → Certificates → View Certificates → Authorities → Import…, select the CA file and tick \"Trust this CA to identify websites\". (Or set security.enterprise_roots.enabled = true in about:config after trusting the CA in the OS.)".into(),
            command: None,
        },
        TrustNote {
            title: "Node.js".into(),
            body: "Node uses its own CA list; add ours. Node ≥ 24 also honours the proxy variables with NODE_USE_ENV_PROXY=1.".into(),
            command: Some(format!("{}\n{}\n{}", env("NODE_EXTRA_CA_CERTS", &ca), env("HTTPS_PROXY", &proxy), env("NODE_USE_ENV_PROXY", "1"))),
        },
        TrustNote {
            title: "Python (requests / httpx)".into(),
            body: "These variables replace the default bundle, so only hosts routed through the proxy will verify.".into(),
            command: Some(format!("{}\n{}\n{}", env("REQUESTS_CA_BUNDLE", &ca), env("SSL_CERT_FILE", &ca), env("HTTPS_PROXY", &proxy))),
        },
        TrustNote { title: "curl".into(), body: "Pass the proxy and the CA explicitly.".into(), command: Some(format!("curl -x {proxy} --cacert {q} https://example.com")) },
        TrustNote {
            title: "Java".into(),
            body: "Import into a copy of the JDK trust store and point the JVM at the proxy.".into(),
            command: Some(format!(
                "keytool -importcert -noprompt -alias devall-ca -file {q} -keystore devall-truststore.jks -storepass changeit\njava -Djavax.net.ssl.trustStore=devall-truststore.jks -Dhttps.proxyHost=127.0.0.1 -Dhttps.proxyPort={proxy_port} …"
            )),
        },
    ]
}

/// Runs a program without a console window (Windows) and captures its output.
pub fn run(program: &str, args: &[String]) -> CommandOutput {
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.output() {
        Ok(o) => CommandOutput {
            success: o.status.success(),
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => CommandOutput { success: false, code: None, stdout: String::new(), stderr: format!("could not run {program}: {e}") },
    }
}

/// Whether the CA is in the OS trust store (`None` when it cannot be checked). Runs a read-only
/// query command.
pub fn is_installed(ca: &CertAuthority) -> Option<bool> {
    let os = Os::current();
    let (program, args) = check_command(os, ca)?;
    let out = run(&program, &args);
    match os {
        Os::Windows => Some(out.success),
        Os::Macos => Some(out.success && out.stdout.to_ascii_uppercase().contains(&ca.sha1_hex())),
        Os::Linux => None,
    }
}

/// Install/uninstall commands and notes for the current OS. With `check`, also queries whether
/// the CA is installed.
pub fn trust_info(ca: &CertAuthority, proxy_port: u16, check: bool) -> TrustInfo {
    let os = Os::current();
    TrustInfo {
        os,
        installed: if check { is_installed(ca) } else { None },
        install: Some(install_command(os, &ca.cert_path())),
        uninstall: Some(uninstall_command(os, &ca.cert_path(), &ca.sha1_hex())),
        manual: manual_notes(os, &ca.cert_path(), proxy_port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_commands() {
        let p = Path::new("C:/Users/A B/AppData/devall-ca.pem");
        let w = install_command(Os::Windows, p);
        assert_eq!(w.program, "certutil");
        assert_eq!(w.args, ["-user", "-addstore", "Root", "C:/Users/A B/AppData/devall-ca.pem"]);
        assert_eq!(w.display, "certutil -user -addstore Root \"C:/Users/A B/AppData/devall-ca.pem\"");
        let m = install_command(Os::Macos, Path::new("/Users/a/Library/devall-ca.pem"));
        assert_eq!(m.program, "security");
        assert_eq!(&m.args[..5], ["add-trusted-cert", "-d", "-r", "trustRoot", "-k"]);
        assert!(m.args[5].ends_with("/Library/Keychains/login.keychain-db"));
        let l = install_command(Os::Linux, Path::new("/home/a/ca.pem"));
        assert!(!l.runnable);
        let u = uninstall_command(Os::Windows, p, "ABCDEF");
        assert_eq!(u.args, ["-user", "-delstore", "Root", "ABCDEF"]);
        let um = uninstall_command(Os::Macos, p, "ABCDEF");
        assert_eq!(&um.args[..3], ["delete-certificate", "-Z", "ABCDEF"]);
    }

    #[test]
    fn notes_mention_node() {
        let notes = manual_notes(Os::Macos, Path::new("/x/ca.pem"), 8899);
        let node = notes.iter().find(|n| n.title == "Node.js").unwrap();
        assert!(node.command.as_ref().unwrap().contains("export NODE_EXTRA_CA_CERTS=/x/ca.pem"));
        let win = manual_notes(Os::Windows, Path::new("C:/x/ca.pem"), 8899);
        assert!(win.iter().any(|n| { n.command.as_deref().is_some_and(|c| c.contains("$env:NODE_EXTRA_CA_CERTS")) }));
    }
}
