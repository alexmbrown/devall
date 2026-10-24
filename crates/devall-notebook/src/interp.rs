//! Finding interpreters on Windows and macOS (Linux best-effort).
//!
//! | Language     | Windows                                                       | macOS / Linux                         |
//! |--------------|---------------------------------------------------------------|---------------------------------------|
//! | `shell`      | Git for Windows' `bin\bash.exe` (next to `git.exe` on PATH, in | `bash` (run as a login shell), else   |
//! |              | Program Files / LocalAppData / scoop, or a non-WSL `bash.exe`  | `sh`                                  |
//! |              | on PATH), else PowerShell (reported as a fallback)             |                                       |
//! | `powershell` | `pwsh` (PATH or Program Files\PowerShell\7), else Windows      | `pwsh`                                |
//! |              | PowerShell (`System32\WindowsPowerShell\v1.0\powershell.exe`)  |                                       |
//! | `cmd`        | `%ComSpec%` / `System32\cmd.exe`                               | — (Windows only)                      |
//! | `javascript` | `node` on PATH, `Program Files\nodejs`                         | `node` on the login-shell PATH, nvm,  |
//! |              |                                                               | Volta, Homebrew, /usr/local           |
//! | `python`     | `python`, then the `py -3` launcher, then `python3` — each    | `python3`, then `python` (verified    |
//! |              | verified with `--version` (skips Microsoft Store stubs)       | to be Python 3)                       |
//!
//! GUI apps on macOS start with a minimal PATH, so the user's login-shell PATH (`$SHELL -ilc`) is
//! resolved once and used both for lookups and as the PATH of every cell process. Users can pin
//! an interpreter per language ([`Interpreters::set_override`], "Settings").

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Language;
use crate::error::{NotebookError, Result};
use crate::process::{output_with_timeout, std_command};

const VERSION_TIMEOUT: Duration = Duration::from_secs(8);

/// How an interpreter is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Flavor {
    Bash,
    Sh,
    Pwsh,
    WindowsPowershell,
    Cmd,
    Node,
    Python,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interpreter {
    pub language: Language,
    pub flavor: Flavor,
    /// `Git Bash`, `bash`, `sh`, `PowerShell`, `Windows PowerShell`, `cmd`, `Node.js`, `Python`.
    pub name: String,
    pub program: PathBuf,
    /// Arguments that select the interpreter before anything else (`-3` for the `py` launcher).
    pub args: Vec<String>,
    pub version: Option<String>,
    /// Set when this is a fallback, e.g. shell cells running in PowerShell without Git Bash.
    pub note: Option<String>,
    /// Chosen by the user rather than detected.
    pub custom: bool,
}

impl Interpreter {
    /// `Git Bash 5.2.37` / `Node.js 22.21.0`.
    pub fn label(&self) -> String {
        match &self.version {
            Some(v) => format!("{} {v}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Detection result for one language (for the interpreters list in the UI).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterpreterStatus {
    pub language: Language,
    pub interpreter: Option<Interpreter>,
    /// Why nothing was found and how to fix it.
    pub error: Option<String>,
    /// The user's override, if any.
    pub custom_path: Option<PathBuf>,
}

struct SearchPath {
    dirs: Vec<PathBuf>,
    /// PATH for child processes (`None` = inherit the app's).
    env_path: Option<OsString>,
}

/// Interpreter discovery with a per-language cache and user overrides.
#[derive(Default)]
pub struct Interpreters {
    overrides: Mutex<HashMap<Language, PathBuf>>,
    cache: Mutex<HashMap<Language, std::result::Result<Interpreter, String>>>,
    search: OnceLock<SearchPath>,
}

impl Interpreters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pins (or with `None`, unpins) the program used for a language.
    pub fn set_override(&self, language: Language, program: Option<PathBuf>) {
        let mut o = self.overrides.lock().unwrap();
        match program {
            Some(p) => o.insert(language, p),
            None => o.remove(&language),
        };
        self.cache.lock().unwrap().remove(&language);
    }

    pub fn overrides(&self) -> HashMap<Language, PathBuf> {
        self.overrides.lock().unwrap().clone()
    }

    /// Forgets detection results (e.g. after the user installed something).
    pub fn refresh(&self) {
        self.cache.lock().unwrap().clear();
    }

    fn search(&self) -> &SearchPath {
        self.search.get_or_init(build_search_path)
    }

    /// PATH to give cell processes (`None` = inherit). On macOS this is the login-shell PATH.
    pub fn env_path(&self) -> Option<OsString> {
        self.search().env_path.clone()
    }

    /// The interpreter for `language`, detecting it on first use.
    pub async fn get(&self, language: Language) -> Result<Interpreter> {
        if let Some(r) = self.cache.lock().unwrap().get(&language) {
            return r.clone().map_err(NotebookError::InterpreterMissing);
        }
        let custom = self.overrides.lock().unwrap().get(&language).cloned();
        // Detection runs `--version` processes; keep it off the async workers.
        let dirs = self.search().dirs.clone();
        let r = tokio::task::spawn_blocking(move || detect(language, custom.as_deref(), &dirs))
            .await
            .map_err(|e| NotebookError::Kernel(e.to_string()))?;
        self.cache.lock().unwrap().insert(language, r.clone());
        r.map_err(NotebookError::InterpreterMissing)
    }

    /// Every language's interpreter (or the reason it's missing).
    pub async fn statuses(&self) -> Vec<InterpreterStatus> {
        let overrides = self.overrides();
        let mut out = Vec::new();
        for language in Language::ALL {
            let (interpreter, error) = match self.get(language).await {
                Ok(i) => (Some(i), None),
                Err(e) => (None, Some(e.to_string())),
            };
            out.push(InterpreterStatus { language, interpreter, error, custom_path: overrides.get(&language).cloned() });
        }
        out
    }
}

/// What to tell the user when nothing was found.
pub fn missing_hint(language: Language) -> String {
    match language {
        Language::Shell => "No shell found (bash or sh). Set the path in Settings.".into(),
        Language::Powershell => {
            "PowerShell not found — install PowerShell 7 (https://aka.ms/powershell; on macOS `brew install --cask powershell`) or set the path in Settings."
                .into()
        }
        Language::Cmd => "cmd cells only run on Windows — use a shell cell on this machine.".into(),
        Language::Javascript => "Node.js not found — install it from https://nodejs.org or set the path in Settings.".into(),
        Language::Python => "Python not found — install from https://python.org or set the path in Settings.".into(),
    }
}

fn detect(language: Language, custom: Option<&Path>, dirs: &[PathBuf]) -> std::result::Result<Interpreter, String> {
    if let Some(p) = custom {
        return custom_interpreter(language, p);
    }
    let found = match language {
        Language::Shell => detect_shell(dirs),
        Language::Powershell => detect_powershell(dirs),
        Language::Cmd => detect_cmd(),
        Language::Javascript => detect_node(dirs),
        Language::Python => detect_python(dirs),
    };
    found.ok_or_else(|| missing_hint(language))
}

fn custom_interpreter(language: Language, program: &Path) -> std::result::Result<Interpreter, String> {
    if !exists_file(program) {
        return Err(format!("The {} interpreter set in Settings doesn't exist: {}", language.display_name(), program.display()));
    }
    let file = program.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let flavor = match language {
        Language::Shell if file == "sh" || file == "dash" => Flavor::Sh,
        Language::Shell if file.contains("pwsh") => Flavor::Pwsh,
        Language::Shell if file.contains("powershell") => Flavor::WindowsPowershell,
        Language::Shell => Flavor::Bash,
        Language::Powershell if file.contains("pwsh") => Flavor::Pwsh,
        Language::Powershell => Flavor::WindowsPowershell,
        Language::Cmd => Flavor::Cmd,
        Language::Javascript => Flavor::Node,
        Language::Python => Flavor::Python,
    };
    let version = version_of(program, &[], flavor);
    if flavor == Flavor::Python && version.as_deref().is_none_or(|v| !v.starts_with('3')) {
        return Err(format!("{} isn't a working Python 3 interpreter (set in Settings).", program.display()));
    }
    let name = program.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    Ok(Interpreter { language, flavor, name, program: program.to_path_buf(), args: Vec::new(), version, note: None, custom: true })
}

fn interp(language: Language, flavor: Flavor, name: &str, program: PathBuf, args: Vec<String>) -> Interpreter {
    let version = version_of(&program, &args, flavor);
    Interpreter { language, flavor, name: name.to_string(), program, args, version, note: None, custom: false }
}

fn detect_shell(dirs: &[PathBuf]) -> Option<Interpreter> {
    if cfg!(windows) {
        if let Some(bash) = git_bash(dirs) {
            return Some(interp(Language::Shell, Flavor::Bash, "Git Bash", bash, Vec::new()));
        }
        let mut ps = detect_powershell(dirs)?;
        ps.language = Language::Shell;
        ps.note = Some("Git Bash wasn't found, so shell cells run in PowerShell. Install Git for Windows (https://git-scm.com) to run them in bash.".into());
        return Some(ps);
    }
    if let Some(bash) = which_all("bash", dirs).into_iter().next() {
        return Some(interp(Language::Shell, Flavor::Bash, "bash", bash, Vec::new()));
    }
    let sh = which_all("sh", dirs).into_iter().next().or_else(|| Some(PathBuf::from("/bin/sh")).filter(|p| p.is_file()))?;
    Some(interp(Language::Shell, Flavor::Sh, "sh", sh, Vec::new()))
}

fn detect_powershell(dirs: &[PathBuf]) -> Option<Interpreter> {
    let mut pwsh = which_all("pwsh", dirs);
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432"] {
            if let Some(pf) = std::env::var_os(var) {
                pwsh.push(PathBuf::from(pf).join("PowerShell").join("7").join("pwsh.exe"));
            }
        }
    }
    if let Some(p) = pwsh.into_iter().find(|p| exists_file(p)) {
        return Some(interp(Language::Powershell, Flavor::Pwsh, "PowerShell", p, Vec::new()));
    }
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let builtin = root.join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe");
        let p = if builtin.is_file() { Some(builtin) } else { which_all("powershell", dirs).into_iter().next() };
        return p.map(|p| interp(Language::Powershell, Flavor::WindowsPowershell, "Windows PowerShell", p, Vec::new()));
    }
    None
}

fn detect_cmd() -> Option<Interpreter> {
    if !cfg!(windows) {
        return None;
    }
    let p = std::env::var_os("ComSpec").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        Some(root.join("System32").join("cmd.exe")).filter(|p| p.is_file())
    })?;
    Some(Interpreter { language: Language::Cmd, flavor: Flavor::Cmd, name: "cmd".into(), program: p, args: Vec::new(), version: None, note: None, custom: false })
}

fn detect_node(dirs: &[PathBuf]) -> Option<Interpreter> {
    let mut candidates = which_all("node", dirs);
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432"] {
            if let Some(pf) = std::env::var_os(var) {
                candidates.push(PathBuf::from(pf).join("nodejs").join("node.exe"));
            }
        }
    }
    candidates.into_iter().filter(|p| exists_file(p)).find_map(|p| {
        let i = interp(Language::Javascript, Flavor::Node, "Node.js", p, Vec::new());
        i.version.is_some().then_some(i)
    })
}

fn detect_python(dirs: &[PathBuf]) -> Option<Interpreter> {
    let mut candidates: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let names: &[&str] = if cfg!(windows) { &["python", "py", "python3"] } else { &["python3", "python"] };
    for name in names {
        for p in which_all(name, dirs) {
            let args = if *name == "py" { vec!["-3".to_string()] } else { Vec::new() };
            candidates.push((p, args));
        }
    }
    candidates.into_iter().find_map(|(p, args)| {
        let i = interp(Language::Python, Flavor::Python, "Python", p, args);
        i.version.as_deref().is_some_and(|v| v.starts_with('3')).then_some(i)
    })
}

/// Git for Windows' bash (never WSL's `System32\bash.exe`).
fn git_bash(dirs: &[PathBuf]) -> Option<PathBuf> {
    let ok = |p: &Path| p.is_file() && !is_windows_system(p);
    for git in which_all("git", dirs) {
        for anc in git.ancestors().skip(1).take(4) {
            let b = anc.join("bin").join("bash.exe");
            if ok(&b) {
                return Some(b);
            }
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(v) = std::env::var_os(var) {
            roots.push(PathBuf::from(v).join("Git"));
        }
    }
    if let Some(v) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(v).join("Programs").join("Git"));
    }
    if let Some(v) = std::env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(v).join("scoop").join("apps").join("git").join("current"));
    }
    if let Some(b) = roots.into_iter().map(|r| r.join("bin").join("bash.exe")).find(|b| ok(b)) {
        return Some(b);
    }
    which_all("bash", dirs).into_iter().find(|p| ok(p))
}

fn is_windows_system(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase().replace('/', "\\");
    s.contains("\\windows\\system32\\") || s.contains("\\windowsapps\\") || s.contains("\\windows\\sysnative\\")
}

/// Regular files, plus Windows app-execution aliases (reparse points `metadata` can't follow).
fn exists_file(p: &Path) -> bool {
    p.is_file() || std::fs::symlink_metadata(p).is_ok_and(|m| !m.is_dir())
}

/// Every match for `name` in `dirs`, in order (`.exe`/`.com` on Windows).
pub(crate) fn which_all(name: &str, dirs: &[PathBuf]) -> Vec<PathBuf> {
    let names: Vec<String> = if cfg!(windows) && Path::new(name).extension().is_none() {
        vec![format!("{name}.exe"), format!("{name}.com")]
    } else {
        vec![name.to_string()]
    };
    let mut out = Vec::new();
    for dir in dirs {
        for n in &names {
            let p = dir.join(n);
            if exists_file(&p) && is_executable(&p) && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_: &Path) -> bool {
    true
}

/// Version string from `--version` style output.
fn version_of(program: &Path, args: &[String], flavor: Flavor) -> Option<String> {
    let mut cmd = std_command(program);
    cmd.args(args);
    match flavor {
        Flavor::WindowsPowershell => {
            cmd.args(["-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.ToString()"]);
        }
        Flavor::Cmd | Flavor::Sh => return None,
        _ => {
            cmd.arg("--version");
        }
    }
    let (ok, out, err) = output_with_timeout(cmd, VERSION_TIMEOUT)?;
    if !ok {
        return None;
    }
    let text = if out.trim().is_empty() { err } else { out };
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    parse_version(flavor, line)
}

fn parse_version(flavor: Flavor, line: &str) -> Option<String> {
    let v = match flavor {
        Flavor::Python => line.strip_prefix("Python ")?.trim().to_string(),
        Flavor::Node => line.strip_prefix('v').unwrap_or(line).trim().to_string(),
        Flavor::Bash => line.split("version ").nth(1)?.split_whitespace().next()?.to_string(),
        Flavor::Pwsh => line.strip_prefix("PowerShell ").unwrap_or(line).trim().to_string(),
        Flavor::WindowsPowershell => line.trim().to_string(),
        Flavor::Cmd | Flavor::Sh => return None,
    };
    (!v.is_empty() && v.chars().next().is_some_and(|c| c.is_ascii_digit())).then_some(v)
}

fn build_search_path() -> SearchPath {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let push = |d: PathBuf, dirs: &mut Vec<PathBuf>| {
        if !d.as_os_str().is_empty() && !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    #[cfg(unix)]
    let login = login_shell_path();
    #[cfg(not(unix))]
    let login: Option<OsString> = None;
    if let Some(lp) = &login {
        for d in std::env::split_paths(lp) {
            push(d, &mut dirs);
        }
    }
    if let Some(p) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&p) {
            push(d, &mut dirs);
        }
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    if cfg!(unix) {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            push(PathBuf::from(d), &mut dirs);
        }
        if let Some(h) = &home {
            for d in [".volta/bin", ".local/bin", ".cargo/bin", ".pyenv/shims", ".asdf/shims"] {
                push(h.join(d), &mut dirs);
            }
            if let Some(nvm) = newest_subdir(&h.join(".nvm").join("versions").join("node")) {
                push(nvm.join("bin"), &mut dirs);
            }
        }
    }
    // Only Unix children get a PATH override (the login PATH plus the extra locations above).
    let env_path = if cfg!(unix) { std::env::join_paths(&dirs).ok() } else { None };
    SearchPath { dirs, env_path }
}

/// The highest-versioned child directory (`v22.1.0` beats `v9.0.0`).
#[cfg_attr(not(unix), allow(dead_code))]
fn newest_subdir(dir: &Path) -> Option<PathBuf> {
    let key = |p: &PathBuf| -> Vec<u64> {
        p.file_name()
            .map(|n| n.to_string_lossy().trim_start_matches('v').split('.').map(|x| x.parse().unwrap_or(0)).collect())
            .unwrap_or_default()
    };
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).max_by_key(key)
}

/// `$SHELL -ilc 'echo $PATH'` (interactive + login so nvm/Homebrew setups in rc files count),
/// detached from any terminal, with a timeout.
#[cfg(unix)]
fn login_shell_path() -> Option<OsString> {
    use std::os::unix::process::CommandExt;
    let shell = std::env::var_os("SHELL").map(PathBuf::from).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("/bin/sh"));
    let mut cmd = std_command(&shell);
    cmd.args(["-ilc", "printf '\\n__DEVALL_PATH__%s__DEVALL_END__\\n' \"$PATH\""]);
    // SAFETY: setsid is async-signal-safe; it detaches the probe from our controlling terminal so
    // an interactive shell can't stop on terminal I/O.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let (_, out, _) = output_with_timeout(cmd, Duration::from_secs(5))?;
    let start = out.find("__DEVALL_PATH__")? + "__DEVALL_PATH__".len();
    let end = out[start..].find("__DEVALL_END__")? + start;
    let path = out[start..end].trim();
    (!path.is_empty()).then(|| OsString::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version(Flavor::Python, "Python 3.12.10").as_deref(), Some("3.12.10"));
        assert_eq!(parse_version(Flavor::Python, "Python was not found; run without arguments"), None);
        assert_eq!(parse_version(Flavor::Node, "v22.21.0").as_deref(), Some("22.21.0"));
        assert_eq!(parse_version(Flavor::Bash, "GNU bash, version 5.2.37(1)-release (x86_64-pc-msys)").as_deref(), Some("5.2.37(1)-release"));
        assert_eq!(parse_version(Flavor::Pwsh, "PowerShell 7.4.1").as_deref(), Some("7.4.1"));
        assert_eq!(parse_version(Flavor::WindowsPowershell, "5.1.26100.9711").as_deref(), Some("5.1.26100.9711"));
    }

    #[test]
    fn wsl_bash_is_not_git_bash() {
        assert!(is_windows_system(Path::new(r"C:\Windows\System32\bash.exe")));
        assert!(is_windows_system(Path::new(r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\bash.exe")));
        assert!(!is_windows_system(Path::new(r"C:\Program Files\Git\bin\bash.exe")));
    }

    #[tokio::test]
    async fn detects_something_and_reports_missing() {
        let i = Interpreters::new();
        let all = i.statuses().await;
        assert_eq!(all.len(), 5);
        // Every machine has some shell (bash/sh, or PowerShell on Windows).
        let shell = all.iter().find(|s| s.language == Language::Shell).unwrap();
        assert!(shell.interpreter.is_some(), "{shell:?}");
        if !cfg!(windows) {
            let cmd = all.iter().find(|s| s.language == Language::Cmd).unwrap();
            assert!(cmd.error.as_deref().unwrap().contains("only run on Windows"));
        }
        i.set_override(Language::Python, Some(PathBuf::from("/definitely/not/python")));
        let err = i.get(Language::Python).await.unwrap_err().to_string();
        assert!(err.contains("doesn't exist"), "{err}");
        i.set_override(Language::Python, None);
        for s in &all {
            eprintln!("{:?}: {:?} {:?}", s.language, s.interpreter.as_ref().map(|i| (i.label(), &i.program)), s.error);
        }
    }
}
