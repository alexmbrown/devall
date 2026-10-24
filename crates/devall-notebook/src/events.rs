//! What a running cell reports. Every event crosses IPC (`tauri::ipc::Channel<CellEvent>`).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Language;

/// A streamed event of one cell run. `at` is milliseconds since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CellEvent {
    /// The cell was handed to its interpreter.
    Started {
        run_id: String,
        language: Language,
        /// Human name of what runs the cell: `Git Bash`, `bash`, `Windows PowerShell`, `Node.js`, …
        interpreter: String,
        program: String,
        version: Option<String>,
        at: i64,
    },
    /// A chunk of standard output. `text` is raw (may contain ANSI escapes and `\r`), `plain` has
    /// the escapes removed and `\r\n` normalized to `\n`.
    Stdout { text: String, plain: String, at: i64 },
    Stderr { text: String, plain: String, at: i64 },
    /// A rich value. `data` is a MIME bundle: `text/plain`, `application/json`, `text/html`
    /// (sanitize before rendering), `text/markdown`, `image/png` / `image/jpeg` (base64),
    /// `image/svg+xml`. `result` is true for a cell's last-expression value (`execute_result`).
    Display { data: Map<String, Value>, result: bool, at: i64 },
    /// An exception in a kernel (Python traceback, JavaScript stack).
    Error { ename: String, evalue: String, traceback: Vec<String>, at: i64 },
    /// A message from /dev/all itself, e.g. that a fallback interpreter ran the cell or that a
    /// kernel was restarted. Not saved into notebooks.
    Notice { text: String, at: i64 },
    /// Always the last event. `code` is the process exit code (shell cells) or 0/1 for kernel cells
    /// (1 = the cell raised); `None` when the process was killed.
    Exit { code: Option<i32>, duration_ms: u64, interrupted: bool, timed_out: bool, at: i64 },
}

impl CellEvent {
    pub fn is_exit(&self) -> bool {
        matches!(self, Self::Exit { .. })
    }
}

/// The outcome of a cell run, returned by the run command once the cell finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CellResult {
    pub run_id: String,
    pub language: Language,
    pub interpreter: Option<String>,
    pub exit_code: Option<i32>,
    /// Exit code 0, no error, not interrupted.
    pub ok: bool,
    pub interrupted: bool,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// nbformat 4 output objects (`stream`, `execute_result`, `display_data`, `error`) with ANSI
    /// escapes stripped, ready to store in the cell's `outputs` when the user saves outputs.
    pub outputs: Vec<Value>,
}
