//! Script tooling commands — a thin layer over `crates/devall-script` (pre-request /
//! post-response JavaScript with a Postman-compatible `pm` API). Running scripts happens inside
//! the send pipeline (`send.rs`), not here.
//!
//! # Integration (for whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    `devall-script = { path = "../../../crates/devall-script" }`
//!    (or add `devall-script = { path = "crates/devall-script" }` to the root
//!    `[workspace.dependencies]` and use `devall-script.workspace = true`).
//! 2. `src/modules/mod.rs`: `pub mod scripts;`
//! 3. No managed state: nothing to `.manage(...)`.
//! 4. `src/lib.rs`, inside `tauri::generate_handler![…]`:
//!    ```ignore
//!    modules::scripts::script_check,
//!    ```
//! 5. Frontend: `src/lib/api/scripts.ts` (`scriptsApi.check`) is used by
//!    `features/scripts/ScriptsEditor` for inline syntax errors; it degrades gracefully while the
//!    command isn't registered.

use devall_script::{ScriptEngine, ScriptError};

/// Parses `code` without running it. `None` when it compiles; otherwise the syntax error with its
/// 1-based line/column in `code`. Async so parsing never blocks the UI thread.
#[tauri::command]
pub async fn script_check(code: String) -> Option<ScriptError> {
    if code.trim().is_empty() {
        return None;
    }
    ScriptEngine::new().check_syntax(&code)
}
