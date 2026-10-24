//! Process environment helpers: finding CLIs a GUI-launched app can't see, and spawning them
//! without console windows.
//!
//! Apps started from Finder/Dock (macOS) or the Start menu (Windows) don't inherit the shell's
//! `PATH`, so `gke-gcloud-auth-plugin`, `aws`, `kubelogin`, `gcloud` and `docker` are often
//! "not found" even though they work in a terminal. [`search_path`] is the process `PATH` plus,
//! on macOS/Linux, the login shell's `PATH`, plus well-known install folders.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn home_dir() -> Option<PathBuf> {
    let primary = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env_nonempty(primary).or_else(|| env_nonempty("HOME")).map(PathBuf::from)
}

/// Folders where developer CLIs are commonly installed, for `os`. `env` looks up environment
/// variables (injected for tests). Paths are returned whether or not they exist.
pub fn common_bin_dirs_for(os: Os, env: &dyn Fn(&str) -> Option<String>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let h = |rel: &str| home.map(|h| h.join(rel));
    match os {
        Os::Windows => {
            let gcloud = "Google\\Cloud SDK\\google-cloud-sdk\\bin";
            if let Some(local) = env("LOCALAPPDATA") {
                let local = PathBuf::from(local);
                out.push(local.join(gcloud));
                out.push(local.join("Microsoft\\WinGet\\Links"));
                out.push(local.join("Programs\\Rancher Desktop\\resources\\resources\\win32\\bin"));
                out.push(local.join("Programs\\Podman"));
            }
            for var in ["ProgramFiles", "ProgramFiles(x86)"] {
                if let Some(pf) = env(var) {
                    let pf = PathBuf::from(pf);
                    out.push(pf.join(gcloud));
                    out.push(pf.join("Amazon\\AWSCLIV2"));
                    out.push(pf.join("Docker\\Docker\\resources\\bin"));
                    out.push(pf.join("RedHat\\Podman"));
                    out.push(pf.join("Rancher Desktop\\resources\\resources\\win32\\bin"));
                    out.push(pf.join("Kubernetes\\bin"));
                }
            }
            if let Some(pd) = env("ProgramData") {
                out.push(PathBuf::from(pd).join("chocolatey\\bin"));
            }
            out.extend(h(".azure-kubelogin"));
            out.extend(h("scoop\\shims"));
            out.extend(h(".krew\\bin"));
            out.extend(h(".rd\\bin"));
            out.extend(h("bin"));
        }
        Os::MacOs => {
            for p in [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/opt/homebrew/share/google-cloud-sdk/bin",
                "/usr/local/share/google-cloud-sdk/bin",
                "/opt/homebrew/Caskroom/google-cloud-sdk/latest/google-cloud-sdk/bin",
                "/usr/local/Caskroom/google-cloud-sdk/latest/google-cloud-sdk/bin",
                "/usr/local/google-cloud-sdk/bin",
                "/Applications/Docker.app/Contents/Resources/bin",
                "/opt/podman/bin",
                "/opt/local/bin",
                "/usr/bin",
                "/bin",
            ] {
                out.push(PathBuf::from(p));
            }
            out.extend(h("google-cloud-sdk/bin"));
            out.extend(h(".orbstack/bin"));
            out.extend(h(".rd/bin"));
            out.extend(h(".docker/bin"));
            out.extend(h(".krew/bin"));
            out.extend(h(".local/bin"));
        }
        Os::Linux => {
            for p in [
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/snap/bin",
                "/usr/lib/google-cloud-sdk/bin",
                "/home/linuxbrew/.linuxbrew/bin",
            ] {
                out.push(PathBuf::from(p));
            }
            out.extend(h("google-cloud-sdk/bin"));
            out.extend(h(".rd/bin"));
            out.extend(h(".krew/bin"));
            out.extend(h(".local/bin"));
        }
    }
    out
}

/// `PATH` reported by the user's login shell (macOS/Linux), so a Dock-launched app sees the same
/// tools as a terminal. Bounded to 3 s; `None` on Windows or on failure.
fn login_shell_path() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let shell = env_nonempty("SHELL").unwrap_or_else(|| "/bin/sh".into());
    let mut child = std::process::Command::new(shell)
        .args(["-l", "-c", "printf '%s' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let mut out = String::new();
    use std::io::Read;
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// Directories searched for CLIs: process `PATH`, login-shell `PATH`, common install folders.
/// Deduplicated, existing directories only. Computed once (the first call may take up to 3 s on
/// macOS/Linux; call it from a background thread at startup to warm it).
pub fn search_path() -> &'static [PathBuf] {
    static CACHE: OnceLock<Vec<PathBuf>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(p) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&p));
        }
        if let Some(p) = login_shell_path() {
            dirs.extend(std::env::split_paths(&OsString::from(p)));
        }
        let env = |k: &str| env_nonempty(k);
        dirs.extend(common_bin_dirs_for(Os::current(), &env, home_dir().as_deref()));
        dedupe_existing(dirs)
    })
}

fn dedupe_existing(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    dirs.into_iter()
        .filter(|d| !d.as_os_str().is_empty() && d.is_dir())
        .filter(|d| {
            let key = if cfg!(windows) { d.to_string_lossy().to_lowercase() } else { d.to_string_lossy().to_string() };
            seen.insert(key.trim_end_matches(['/', '\\']).to_string())
        })
        .collect()
}

/// [`search_path`] joined for a child's `PATH`.
pub fn search_path_var() -> OsString {
    std::env::join_paths(search_path()).unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
}

/// Executable extensions tried on Windows (`PATHEXT`), lowercase with dot.
fn path_exts() -> Vec<String> {
    if !cfg!(windows) {
        return vec![String::new()];
    }
    let mut exts = vec![String::new()];
    let raw = env_nonempty("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    exts.extend(raw.split(';').filter(|e| !e.is_empty()).map(|e| e.to_lowercase()));
    exts
}

/// Finds `cmd` in `dirs`. Commands containing a path separator are checked as-is.
pub fn which_in(cmd: &str, dirs: &[PathBuf], exts: &[String]) -> Option<PathBuf> {
    let candidate_names = |base: &Path| -> Vec<PathBuf> {
        let has_ext = base.extension().is_some();
        exts.iter()
            .filter(|e| e.is_empty() || !has_ext || cfg!(windows))
            .map(|e| if e.is_empty() { base.to_path_buf() } else { PathBuf::from(format!("{}{e}", base.display())) })
            .collect()
    };
    if cmd.contains('/') || cmd.contains('\\') {
        return candidate_names(Path::new(cmd)).into_iter().find(|p| p.is_file());
    }
    dirs.iter().flat_map(|d| candidate_names(&d.join(cmd))).find(|p| p.is_file())
}

/// Finds a CLI on [`search_path`] (with `PATHEXT` on Windows).
pub fn which(cmd: &str) -> Option<PathBuf> {
    which_in(cmd, search_path(), &path_exts())
}

/// A command with the augmented `PATH`, no console window on Windows, and killed when dropped.
pub fn command(program: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.env("PATH", search_path_var()).stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `program args…` and captures its output, failing after `timeout`.
pub async fn run_capture(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    envs: &[(String, String)],
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut cmd = command(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let display = format!("{} {}", program.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), args.join(" "));
    let child = cmd.spawn().map_err(|e| Error::Command { command: display.clone(), message: e.to_string() })?;
    let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| Error::Command { command: display.clone(), message: e.to_string() })?,
        Err(_) => return Err(Error::Command { command: display, message: format!("timed out after {}s", timeout.as_secs()) }),
    };
    Ok(CommandOutput {
        success: out.status.success(),
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_dirs_per_os() {
        let env = |k: &str| match k {
            "LOCALAPPDATA" => Some("C:\\Users\\me\\AppData\\Local".to_string()),
            "ProgramFiles" => Some("C:\\Program Files".to_string()),
            _ => None,
        };
        let win = common_bin_dirs_for(Os::Windows, &env, Some(Path::new("C:\\Users\\me")));
        let s: Vec<String> = win.iter().map(|p| p.to_string_lossy().replace('/', "\\")).collect();
        assert!(s.iter().any(|p| p.ends_with("Google\\Cloud SDK\\google-cloud-sdk\\bin") && p.contains("AppData")));
        assert!(s.iter().any(|p| p.ends_with("Amazon\\AWSCLIV2")));
        assert!(s.iter().any(|p| p.ends_with(".azure-kubelogin")));

        let mac = common_bin_dirs_for(Os::MacOs, &|_| None, Some(Path::new("/Users/me")));
        assert!(mac.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(mac.iter().any(|p| p.ends_with("google-cloud-sdk/bin")));

        let linux = common_bin_dirs_for(Os::Linux, &|_| None, None);
        assert!(linux.contains(&PathBuf::from("/snap/bin")));
    }

    #[test]
    fn which_finds_files_with_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "fake-plugin.exe" } else { "fake-plugin" };
        std::fs::write(dir.path().join(name), b"").unwrap();
        let exts = path_exts();
        let found = which_in("fake-plugin", &[dir.path().to_path_buf()], &exts).unwrap();
        assert_eq!(found.file_name().unwrap().to_string_lossy(), name);
        assert!(which_in("missing-plugin", &[dir.path().to_path_buf()], &exts).is_none());
        // Absolute paths are checked directly.
        let abs = dir.path().join(name);
        assert_eq!(which_in(&abs.to_string_lossy(), &[], &exts).unwrap(), abs);
    }

    #[test]
    fn dedupes_and_drops_missing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        let out = dedupe_existing(vec![p.clone(), p.clone(), PathBuf::from("/definitely/not/here"), PathBuf::new()]);
        assert_eq!(out, vec![p]);
    }
}
