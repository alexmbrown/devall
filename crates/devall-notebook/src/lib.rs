//! Runnable notebooks for /dev/all: run notebook cells and Markdown code blocks on the user's
//! machine, Jupyter style.
//!
//! | Language     | How it runs                                                                    |
//! |--------------|--------------------------------------------------------------------------------|
//! | `shell`      | one process per cell: `bash -l` (macOS/Linux, else `sh`); Windows: Git Bash's  |
//! |              | `bash.exe`, falling back to PowerShell when Git for Windows isn't installed     |
//! | `powershell` | one process per cell: `pwsh`, else Windows PowerShell                          |
//! | `cmd`        | one process per cell, Windows only                                             |
//! | `javascript` | persistent Node.js kernel per notebook (V8 REPL mode: top-level `await`,       |
//! |              | re-runnable `let`/`const`, `require`)                                          |
//! | `python`     | persistent Python 3 kernel per notebook (shared globals, last expression shown |
//! |              | like Jupyter, rich `_repr_*_`, matplotlib figures as PNG)                      |
//!
//! [`Engine::start`] returns a [`RunHandle`] that streams [`CellEvent`]s; [`RunHandle::finish`]
//! folds them into a [`CellResult`] with nbformat 4 outputs ([`outputs`]). Cells can be
//! interrupted (process tree killed — Windows job objects / `taskkill`, Unix process groups — or a
//! soft `KeyboardInterrupt` in the kernels), kernels restarted or shut down per notebook.
//!
//! Notebooks come from git repos, so nothing runs without consent: [`trust::TrustStore`] keeps a
//! user-level record of trusted notebook versions (SHA-256 of their code cells) and Markdown code
//! blocks. Project variables reach cells as `DEVALL_<name>` environment variables; `{{var}}`
//! templating and secrets are opt-in per cell ([`env`]).

pub mod ansi;
pub mod engine;
pub mod env;
pub mod error;
pub mod events;
pub mod interp;
mod kernel;
pub mod outputs;
mod process;
mod shell;
pub mod trust;

use serde::{Deserialize, Serialize};

pub use engine::{Engine, RunHandle, RunOptions};
pub use error::{NotebookError, Result};
pub use events::{CellEvent, CellResult};
pub use interp::{Flavor, Interpreter, InterpreterStatus, Interpreters};
pub use trust::TrustStore;

/// A language /dev/all can run (see the table in the crate docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Shell,
    Powershell,
    Cmd,
    Javascript,
    Python,
}

impl Language {
    pub const ALL: [Language; 5] = [Language::Shell, Language::Powershell, Language::Cmd, Language::Javascript, Language::Python];

    /// Accepts the names and aliases understood by [`devall_core::notebook::normalize_language`]
    /// (`bash`, `sh`, `zsh`, `pwsh`, `ps1`, `bat`, `js`, `node`, `py`, …).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match devall_core::notebook::normalize_language(name)? {
            "shell" => Self::Shell,
            "powershell" => Self::Powershell,
            "cmd" => Self::Cmd,
            "javascript" => Self::Javascript,
            "python" => Self::Python,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Powershell => "powershell",
            Self::Cmd => "cmd",
            Self::Javascript => "javascript",
            Self::Python => "python",
        }
    }

    /// Persistent kernels keep state (variables) between cells of the same notebook.
    pub fn is_persistent(self) -> bool {
        matches!(self, Self::Javascript | Self::Python)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Powershell => "PowerShell",
            Self::Cmd => "cmd",
            Self::Javascript => "JavaScript (Node.js)",
            Self::Python => "Python",
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_aliases() {
        assert_eq!(Language::parse("bash"), Some(Language::Shell));
        assert_eq!(Language::parse("zsh"), Some(Language::Shell));
        assert_eq!(Language::parse("pwsh"), Some(Language::Powershell));
        assert_eq!(Language::parse("bat"), Some(Language::Cmd));
        assert_eq!(Language::parse("node"), Some(Language::Javascript));
        assert_eq!(Language::parse("py"), Some(Language::Python));
        assert_eq!(Language::parse("ruby"), None);
        assert_eq!(serde_json::to_string(&Language::Powershell).unwrap(), "\"powershell\"");
        assert!(Language::Python.is_persistent() && !Language::Shell.is_persistent());
    }
}
