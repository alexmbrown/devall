//! `devall-plugins`: sandboxed WebAssembly plugins.
//!
//! A plugin is a folder with a `plugin.yaml` manifest ([`Manifest`]) and a `plugin.wasm` module
//! built for `wasm32-unknown-unknown`. It contributes any of: utility ops (Utilities, command
//! palette, `devall util`), pre-request / post-response hooks, import formats, code generation
//! targets and auth providers. Every call is JSON in → JSON out ([`contract`]); the ABI is in
//! [`runtime`] and the author guide in `docs/PLUGINS.md` (SDK: `sdk/rust/devall-plugin`).
//!
//! # Why wasmi (and not wasmtime)
//!
//! Plugins are small, short-lived calls (sign a request, convert a format, render a snippet), so
//! raw execution speed matters far less than build weight, portability and control:
//! - wasmi is a pure-Rust interpreter with a handful of small dependencies — no Cranelift JIT, no
//!   platform code generator, no `unsafe` executable memory, which keeps Windows/macOS builds (and
//!   their code-signing / hardened-runtime rules on macOS) simple; wasmtime would roughly double the
//!   app's Rust build time and binary size for a speed-up these calls don't need;
//! - it meters execution with fuel and supports *resumable* out-of-fuel calls, which lets the host
//!   hand out fuel in slices and enforce a wall-clock deadline between them without epoch threads;
//! - memory growth goes through a [`wasmi::ResourceLimiter`], so the memory cap is exact.
//!
//! wasmi 1.x is used rather than 2.0: 2.0.0's rewritten executor crashed (access violation) in
//! optimised builds on Windows during testing, while 1.1 (the long-standing register-machine
//! executor) is stable and has the same resumable-fuel API. wasmi is compiled with `opt-level = 2`
//! even in dev builds (root `Cargo.toml`), otherwise plugin calls in debug builds would hit their
//! time limits.
//!
//! # Sandbox
//!
//! No WASI and no ambient authority: the only imports are `devall.log` and `devall.host_call`.
//! Host calls (HTTP, secrets, project files) are checked against the manifest's capabilities as
//! approved by the user ([`Permissions`]); denials produce a deterministic
//! [`PluginError::CapabilityDenied`]. Each call runs in a fresh instance with fuel, wall-clock and
//! memory limits ([`Limits`]).
//!
//! # Install and trust
//!
//! [`PluginManager`] discovers installed plugins (`<app data>/plugins/<id>/`) and project plugins
//! (`<project>/plugins/<id>/`). Project plugins never run until trusted on this machine; trust is
//! pinned to the module's SHA-256 and the approved capabilities (stored in `<app data>/plugins.json`).

mod bridge;
pub mod contract;
mod error;
mod manager;
pub mod manifest;
pub mod runtime;
mod services;

pub use bridge::{HttpBridge, to_plugin_request, to_plugin_response};
pub use contract::{AuthOutput, Header, ImportOutput, ImportedBody, ImportedRequest, PluginRequest, PluginResponse, TestResult};
pub use error::{PluginError, Result};
pub use manager::{
    Approval, AuthProviderInfo, Catalog, CodegenInfo, HookInfo, ImportMatch, ImporterInfo, InstallPreview, PLUGINS_DIR, PluginInfo, PluginManager,
    PostResponseRun, PreRequestRun, ProjectRef, STATE_FILE, Scope, SecretStatus, Status, UtilityInfo,
};
pub use manifest::{API_VERSION, Capabilities, Manifest};
pub use runtime::{CallContext, CallOutcome, CompiledPlugin, HostServices, Limits, LogLine, NoServices, Permissions};
pub use services::{DefaultServices, secret_scope};
