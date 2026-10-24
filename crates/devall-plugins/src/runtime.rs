//! Running one plugin call in a fresh, capability-restricted wasmi instance.
//!
//! ABI (apiVersion 1), all integers are wasm `i32`/`i64`:
//! - the module exports `memory`, `alloc(len) -> ptr`, `dealloc(ptr, len)` (optional) and
//!   `devall_call(ptr, len) -> i64`, which reads a JSON [`CallEnvelope`] and returns
//!   `(out_ptr << 32) | out_len` pointing at `{"ok": …}` or `{"error": "…"}`;
//! - it may import `devall.log(level, ptr, len)` and `devall.host_call(ptr, len) -> i64` (JSON
//!   [`HostCall`] in, `{"ok": …}` / `{"error": "…"}` out, written into memory the host gets from
//!   the plugin's own `alloc`). Nothing else is provided — no WASI, no clock, no filesystem.
//!
//! Every call gets a new store (fresh memory, nothing survives between calls). Execution is
//! metered with fuel handed out in slices: between slices the host checks the total instruction
//! budget and the wall-clock deadline, so a busy loop ends with a deterministic
//! [`PluginError::OutOfFuel`] or [`PluginError::Timeout`]. Memory growth beyond the cap is refused
//! (`memory.grow` returns -1, which Rust guests turn into an abort) and reported as
//! [`PluginError::MemoryLimit`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wasmi::{Caller, Config, Engine, Extern, Linker, Memory, Module, Store, TypedFunc, TypedResumableCall};

use crate::contract::{CallEnvelope, HostCall, PluginRequest, PluginResponse};
use crate::error::{PluginError, Result};
use crate::manifest::{API_VERSION, Manifest, host_allowed};

/// Per-call resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Total fuel (≈ executed wasm instructions) for one call, including instantiation.
    pub fuel: u64,
    /// Wall-clock limit for one call, including host calls (HTTP).
    pub timeout: Duration,
    /// Linear memory cap in bytes.
    pub memory_bytes: usize,
    /// Largest accepted output document.
    pub max_output_bytes: usize,
    /// Outbound HTTP requests allowed per call.
    pub max_http_calls: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { fuel: 1_000_000_000, timeout: Duration::from_secs(5), memory_bytes: 64 << 20, max_output_bytes: 16 << 20, max_http_calls: 8 }
    }
}

impl Limits {
    /// Tighter limits for code on the request path (hooks, auth): 2 s.
    pub fn request_path() -> Self {
        Self { fuel: 500_000_000, timeout: Duration::from_secs(2), ..Self::default() }
    }
}

/// What a call may do beyond computing: the manifest's capabilities narrowed to what the user
/// approved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    pub http_hosts: Vec<String>,
    pub secrets: Vec<String>,
    pub fs_project_read: bool,
}

/// Services the host lends to plugins. Implementations must be callable from a blocking thread.
pub trait HostServices: Send + Sync {
    /// Sends an HTTP request (the host already checked the allow-list).
    fn http(&self, req: &PluginRequest, timeout: Duration) -> std::result::Result<PluginResponse, String>;
    /// A plugin secret's value (`None` when not set).
    fn secret(&self, plugin: &str, name: &str) -> Option<String>;
}

/// Services that refuse everything (tests, previews).
pub struct NoServices;

impl HostServices for NoServices {
    fn http(&self, _req: &PluginRequest, _timeout: Duration) -> std::result::Result<PluginResponse, String> {
        Err("HTTP is not available here".into())
    }
    fn secret(&self, _plugin: &str, _name: &str) -> Option<String> {
        None
    }
}

/// Inputs for one call besides the payload.
#[derive(Clone)]
pub struct CallContext {
    pub permissions: Permissions,
    pub services: Arc<dyn HostServices>,
    pub project_root: Option<PathBuf>,
    pub limits: Limits,
}

impl CallContext {
    pub fn new(services: Arc<dyn HostServices>) -> Self {
        Self { permissions: Permissions::default(), services, project_root: None, limits: Limits::default() }
    }
}

/// A line the plugin logged with `devall.log`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// `debug`, `info`, `warn` or `error`.
    pub level: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallOutcome {
    /// The `ok` value.
    pub value: Value,
    pub logs: Vec<LogLine>,
    pub fuel_used: u64,
    pub elapsed: Duration,
}

/// Fuel handed out per slice; the deadline is checked between slices.
const FUEL_SLICE: u64 = 10_000_000;
/// Extra fuel granted to the plugin's `alloc` when the host writes results into its memory.
const ALLOC_FUEL: u64 = 1_000_000;
const MAX_LOG_LINES: usize = 200;
const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_HOST_CALL_INPUT: usize = 8 << 20;
const MAX_FS_READ: u64 = 4 << 20;

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut c = Config::default();
        c.consume_fuel(true);
        // Deep recursion ends in a trap, not a host stack overflow.
        c.set_max_recursion_depth(4096);
        Engine::new(&c)
    })
}

/// A validated, compiled plugin ready to be called.
pub struct CompiledPlugin {
    pub manifest: Manifest,
    /// Hex SHA-256 of the module bytes.
    pub wasm_sha256: String,
    module: Module,
}

impl std::fmt::Debug for CompiledPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledPlugin").field("id", &self.manifest.id).field("sha256", &self.wasm_sha256).finish()
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

struct MemCap {
    max: usize,
    hit: bool,
}

impl wasmi::ResourceLimiter for MemCap {
    fn memory_growing(&mut self, _current: usize, desired: usize, maximum: Option<usize>) -> std::result::Result<bool, wasmi_core::LimiterError> {
        if desired > self.max {
            self.hit = true;
            return Ok(false);
        }
        Ok(maximum.is_none_or(|m| desired <= m))
    }
    fn table_growing(&mut self, _current: usize, desired: usize, maximum: Option<usize>) -> std::result::Result<bool, wasmi_core::LimiterError> {
        Ok(desired <= 100_000 && maximum.is_none_or(|m| desired <= m))
    }
    fn instances(&self) -> usize {
        1
    }
    fn tables(&self) -> usize {
        4
    }
    fn memories(&self) -> usize {
        1
    }
}

struct HostState {
    plugin: String,
    ctx: CallContext,
    deadline: Instant,
    mem: MemCap,
    logs: Vec<LogLine>,
    log_bytes: usize,
    http_calls: u32,
    /// First capability denial (reported instead of whatever the plugin made of it).
    denied: Option<String>,
    timed_out: bool,
    /// Fuel added by the host (for `alloc` during host calls).
    bonus_fuel: u64,
}

fn host_err(msg: impl Into<String>) -> wasmi::Error {
    wasmi::Error::new(msg.into())
}

fn memory_of(caller: &Caller<'_, HostState>) -> std::result::Result<Memory, wasmi::Error> {
    caller.get_export("memory").and_then(Extern::into_memory).ok_or_else(|| host_err("plugin does not export `memory`"))
}

fn read_guest(caller: &Caller<'_, HostState>, ptr: i32, len: i32, max: usize) -> std::result::Result<Vec<u8>, wasmi::Error> {
    let mem = memory_of(caller)?;
    let (ptr, len) = (ptr as u32 as usize, len as u32 as usize);
    if len > max {
        return Err(host_err(format!("host call input too large ({len} bytes)")));
    }
    let data = mem.data(caller);
    data.get(ptr..ptr.saturating_add(len)).map(<[u8]>::to_vec).ok_or_else(|| host_err("host call pointer out of bounds"))
}

/// Copies `bytes` into memory obtained from the plugin's `alloc`; returns the packed pointer.
fn write_guest(caller: &mut Caller<'_, HostState>, bytes: &[u8]) -> std::result::Result<i64, wasmi::Error> {
    let alloc = caller.get_export("alloc").and_then(Extern::into_func).ok_or_else(|| host_err("plugin does not export `alloc`"))?;
    let alloc = alloc.typed::<i32, i32>(&*caller)?;
    let fuel = caller.get_fuel()?;
    caller.set_fuel(fuel.saturating_add(ALLOC_FUEL))?;
    caller.data_mut().bonus_fuel += ALLOC_FUEL;
    let ptr = alloc.call(&mut *caller, bytes.len() as i32)?;
    let mem = memory_of(caller)?;
    mem.write(&mut *caller, ptr as u32 as usize, bytes).map_err(|_| host_err("plugin `alloc` returned memory out of bounds"))?;
    Ok(pack(ptr, bytes.len()))
}

fn pack(ptr: i32, len: usize) -> i64 {
    (((ptr as u32 as u64) << 32) | (len as u64 & 0xffff_ffff)) as i64
}

fn unpack(v: i64) -> (usize, usize) {
    let v = v as u64;
    ((v >> 32) as usize, (v & 0xffff_ffff) as usize)
}

fn level_name(l: i32) -> &'static str {
    match l {
        i32::MIN..=0 => "debug",
        1 => "info",
        2 => "warn",
        _ => "error",
    }
}

fn ok(v: Value) -> Value {
    json!({ "ok": v })
}

fn fail(msg: impl Into<String>) -> Value {
    json!({ "error": msg.into() })
}

/// Runs a host call; capability problems become `{"error": "capability denied: …"}` for the
/// plugin and are remembered for the final error.
fn dispatch(state: &mut HostState, call: HostCall) -> Value {
    let deny = |state: &mut HostState, detail: String| {
        if state.denied.is_none() {
            state.denied = Some(detail.clone());
        }
        fail(format!("capability denied: {detail}"))
    };
    match call {
        HostCall::Now => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            ok(json!({ "unixMs": now.as_millis() as i64, "iso": iso8601(now) }))
        }
        HostCall::RandomBytes { len } => {
            if len > 1024 {
                return fail("random.bytes: len must be ≤ 1024");
            }
            let mut buf = vec![0u8; len as usize];
            match getrandom::fill(&mut buf) {
                Ok(()) => ok(json!({ "hex": hex::encode(buf) })),
                Err(e) => fail(format!("random.bytes: {e}")),
            }
        }
        HostCall::SecretGet { name } => {
            if !state.ctx.permissions.secrets.iter().any(|s| s == &name) {
                return deny(state, format!("secret {name} was not granted (declare it under capabilities.secrets.read and allow it when installing)"));
            }
            match state.ctx.services.secret(&state.plugin, &name) {
                Some(v) => ok(json!({ "value": v })),
                None => fail(format!("secret {name} is not set (set it on the Plugins page)")),
            }
        }
        HostCall::FsRead { path } => {
            if !state.ctx.permissions.fs_project_read {
                return deny(state, "fs.project.read was not requested or not granted".into());
            }
            let Some(root) = state.ctx.project_root.clone() else { return fail("fs.read: no project is open") };
            let rel = devall_core::fsutil::normalize_rel(&path);
            let first = rel.split('/').next().unwrap_or_default();
            if first == ".git" || first == ".devall" {
                return deny(state, format!("fs.read: {rel} is outside what plugins may read"));
            }
            let full = match devall_core::fsutil::safe_join(&root, &rel) {
                Ok(p) => p,
                Err(_) => return deny(state, format!("fs.read: {path} is outside the project")),
            };
            match std::fs::metadata(&full) {
                Ok(m) if m.is_file() && m.len() <= MAX_FS_READ => {}
                Ok(m) if m.is_file() => return fail(format!("fs.read: {rel} is larger than {} MiB", MAX_FS_READ >> 20)),
                _ => return fail(format!("fs.read: {rel} not found")),
            }
            match std::fs::read(&full) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(t) => ok(json!({ "content": t, "base64": false })),
                    Err(e) => ok(json!({ "content": b64(e.as_bytes()), "base64": true })),
                },
                Err(e) => fail(format!("fs.read: {rel}: {e}")),
            }
        }
        HostCall::HttpRequest { request, timeout_ms } => {
            let url = match url_host(&request.url) {
                Ok(u) => u,
                Err(e) => return fail(format!("http.request: {e}")),
            };
            if !host_allowed(&state.ctx.permissions.http_hosts, &url.0, Some(url.1)) {
                let allowed = if state.ctx.permissions.http_hosts.is_empty() { "none".to_string() } else { state.ctx.permissions.http_hosts.join(", ") };
                return deny(state, format!("http request to {} is not allowed (allowed hosts: {allowed})", url.0));
            }
            if state.http_calls >= state.ctx.limits.max_http_calls {
                return fail(format!("http.request: at most {} requests per call", state.ctx.limits.max_http_calls));
            }
            state.http_calls += 1;
            let remaining = state.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.timed_out = true;
                return fail("http.request: the call's time limit is used up");
            }
            let timeout = timeout_ms.map(Duration::from_millis).unwrap_or(remaining).min(remaining);
            match state.ctx.services.http(&request, timeout) {
                Ok(resp) => ok(serde_json::to_value(resp).unwrap_or(Value::Null)),
                Err(e) => {
                    if Instant::now() >= state.deadline {
                        state.timed_out = true;
                    }
                    fail(format!("http.request: {e}"))
                }
            }
        }
    }
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `(host, port)` of an http(s) URL.
fn url_host(raw: &str) -> std::result::Result<(String, u16), String> {
    let (scheme, rest) = raw.split_once("://").ok_or_else(|| format!("{raw} is not an absolute URL"))?;
    let default = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        other => return Err(format!("scheme {other} is not supported (http/https only)")),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    if authority.is_empty() {
        return Err(format!("{raw} has no host"));
    }
    if let Some(v6) = authority.strip_prefix('[') {
        let (addr, tail) = v6.split_once(']').ok_or("bad IPv6 host")?;
        let port = tail.strip_prefix(':').map(|p| p.parse::<u16>().map_err(|_| "bad port")).transpose()?.unwrap_or(default);
        return Ok((addr.to_ascii_lowercase(), port));
    }
    match authority.rsplit_once(':') {
        Some((h, p)) => Ok((h.to_ascii_lowercase(), p.parse::<u16>().map_err(|_| format!("bad port in {raw}"))?)),
        None => Ok((authority.to_ascii_lowercase(), default)),
    }
}

fn iso8601(d: Duration) -> String {
    // Civil-from-days (Howard Hinnant), UTC, millisecond precision.
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, rem / 60 % 60, rem % 60, d.subsec_millis())
}

fn linker() -> std::result::Result<Linker<HostState>, wasmi::Error> {
    let mut l = Linker::<HostState>::new(engine());
    l.func_wrap("devall", "log", |caller: Caller<'_, HostState>, level: i32, ptr: i32, len: i32| -> std::result::Result<(), wasmi::Error> {
        let bytes = read_guest(&caller, ptr, len, MAX_LOG_BYTES)?;
        let mut caller = caller;
        let st = caller.data_mut();
        if st.logs.len() < MAX_LOG_LINES && st.log_bytes < MAX_LOG_BYTES {
            st.log_bytes += bytes.len();
            st.logs.push(LogLine { level: level_name(level), message: String::from_utf8_lossy(&bytes).into_owned() });
        }
        Ok(())
    })?;
    l.func_wrap("devall", "host_call", |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> std::result::Result<i64, wasmi::Error> {
        let bytes = read_guest(&caller, ptr, len, MAX_HOST_CALL_INPUT)?;
        let reply = match serde_json::from_slice::<HostCall>(&bytes) {
            Ok(call) => dispatch(caller.data_mut(), call),
            Err(e) => fail(format!("invalid host call: {e}")),
        };
        let out = serde_json::to_vec(&reply).unwrap_or_default();
        write_guest(&mut caller, &out)
    })?;
    Ok(l)
}

impl CompiledPlugin {
    /// Validates and compiles a module. Imports other than `devall.log` / `devall.host_call` are
    /// refused up front with a message that names them (typically WASI from a `wasm32-wasip1` build).
    pub fn compile(manifest: Manifest, wasm: &[u8]) -> Result<Self> {
        let id = manifest.id.clone();
        let invalid = |detail: String| PluginError::InvalidModule { plugin: id.clone(), detail };
        if wasm.len() as u64 > crate::manifest::MAX_WASM_BYTES {
            return Err(invalid(format!("{} bytes is larger than the {} MiB limit", wasm.len(), crate::manifest::MAX_WASM_BYTES >> 20)));
        }
        let module = Module::new(engine(), wasm).map_err(|e| invalid(e.to_string()))?;
        for imp in module.imports() {
            let known = imp.module() == "devall" && matches!(imp.name(), "log" | "host_call");
            if !known {
                let hint = if imp.module().starts_with("wasi") { " (plugins get no WASI: build for wasm32-unknown-unknown)" } else { "" };
                return Err(invalid(format!("imports {}.{}, which /dev/all does not provide{hint}", imp.module(), imp.name())));
            }
        }
        for export in ["memory", "alloc", "devall_call"] {
            if module.exports().all(|e| e.name() != export) {
                return Err(PluginError::Abi { plugin: manifest.id.clone(), detail: format!("module does not export `{export}` (see docs/PLUGINS.md)") });
            }
        }
        Ok(Self { wasm_sha256: sha256_hex(wasm), manifest, module })
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// Calls `kind`/`id` with `input`; returns the plugin's `ok` value.
    pub fn call(&self, kind: &str, id: &str, input: Value, ctx: &CallContext) -> Result<CallOutcome> {
        let started = Instant::now();
        let plugin = self.manifest.id.clone();
        let limits = ctx.limits;
        let state = HostState {
            plugin: plugin.clone(),
            ctx: ctx.clone(),
            deadline: started + limits.timeout,
            mem: MemCap { max: limits.memory_bytes, hit: false },
            logs: Vec::new(),
            log_bytes: 0,
            http_calls: 0,
            denied: None,
            timed_out: false,
            bonus_fuel: 0,
        };
        let mut store = Store::new(engine(), state);
        store.limiter(|s| &mut s.mem);
        let mut runner = Runner::new(store, plugin.clone(), limits, started);
        let result = runner.run(&self.module, kind, id, input);
        let st = runner.store.data();
        let logs = st.logs.clone();
        let fuel_used = runner.spent;
        let mapped = match result {
            Ok(v) => match v {
                Value::Object(mut o) if o.contains_key("ok") => Ok(o.remove("ok").unwrap_or(Value::Null)),
                Value::Object(o) if o.contains_key("error") => {
                    let message = match o.get("error") {
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    Err(PluginError::Failed { plugin: plugin.clone(), message })
                }
                other => Err(PluginError::Abi { plugin: plugin.clone(), detail: format!("devall_call must return {{\"ok\": …}} or {{\"error\": \"…\"}}, got {}", short(&other)) }),
            },
            Err(e) => Err(e),
        };
        // Denials and limits win over whatever the plugin made of them.
        let st = runner.store.data();
        let mapped = match mapped {
            Err(_) if st.denied.is_some() => Err(PluginError::CapabilityDenied { plugin: plugin.clone(), detail: st.denied.clone().unwrap_or_default() }),
            Err(PluginError::Trap { .. }) | Err(PluginError::InvalidModule { .. }) if st.mem.hit => {
                Err(PluginError::MemoryLimit { plugin: plugin.clone(), mib: (limits.memory_bytes >> 20) as u64 })
            }
            Err(_) if st.timed_out => Err(PluginError::Timeout { plugin: plugin.clone(), ms: limits.timeout.as_millis() as u64 }),
            other => other,
        };
        mapped.map(|value| CallOutcome { value, logs, fuel_used, elapsed: started.elapsed() })
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 80 { format!("{}…", &s[..s.floor_char_boundary(80)]) } else { s }
}

struct Runner {
    store: Store<HostState>,
    plugin: String,
    limits: Limits,
    /// Fuel spent so far (instantiation, the call, `alloc` during host calls).
    spent: u64,
    /// Fuel set by the last refuel.
    last_set: u64,
    /// `bonus_fuel` at the last refuel.
    bonus_mark: u64,
    started: Instant,
}

impl Runner {
    fn new(store: Store<HostState>, plugin: String, limits: Limits, started: Instant) -> Self {
        Self { store, plugin, limits, spent: 0, last_set: 0, bonus_mark: 0, started }
    }

    fn abi(&self, detail: String) -> PluginError {
        PluginError::Abi { plugin: self.plugin.clone(), detail }
    }

    fn trap(&self, e: &wasmi::Error) -> PluginError {
        if matches!(e.as_trap_code(), Some(wasmi::TrapCode::OutOfFuel)) {
            return PluginError::OutOfFuel { plugin: self.plugin.clone(), fuel: self.limits.fuel };
        }
        PluginError::Trap { plugin: self.plugin.clone(), detail: e.to_string() }
    }

    /// Adds what was spent since the last refuel to `spent`.
    fn account(&mut self) {
        let bonus = self.store.data().bonus_fuel;
        let current = self.store.get_fuel().unwrap_or(0);
        let given = self.last_set.saturating_add(bonus.saturating_sub(self.bonus_mark));
        self.spent = self.spent.saturating_add(given.saturating_sub(current));
        self.bonus_mark = bonus;
        self.last_set = current;
    }

    /// Hands out the next slice of fuel, or fails when the budget or the deadline is used up.
    fn refuel(&mut self, at_least: u64) -> Result<()> {
        self.account();
        if Instant::now() >= self.started + self.limits.timeout {
            return Err(PluginError::Timeout { plugin: self.plugin.clone(), ms: self.limits.timeout.as_millis() as u64 });
        }
        let left = self.limits.fuel.saturating_sub(self.spent);
        if left == 0 || left < at_least {
            return Err(PluginError::OutOfFuel { plugin: self.plugin.clone(), fuel: self.limits.fuel });
        }
        let slice = FUEL_SLICE.max(at_least).min(left);
        self.last_set = slice;
        self.store.set_fuel(slice).map_err(|e| self.trap(&e))
    }

    fn run(&mut self, module: &Module, kind: &str, id: &str, input: Value) -> Result<Value> {
        let linker = linker().map_err(|e| self.abi(e.to_string()))?;
        // Instantiation (data segments, start function) runs on the first slice.
        self.refuel(0)?;
        let instance = linker.instantiate_and_start(&mut self.store, module).map_err(|e| {
            if self.store.data().mem.hit { PluginError::MemoryLimit { plugin: self.plugin.clone(), mib: (self.limits.memory_bytes >> 20) as u64 } } else { self.trap(&e) }
        })?;
        let memory = instance.get_memory(&self.store, "memory").ok_or_else(|| self.abi("module does not export `memory`".into()))?;
        let alloc: TypedFunc<i32, i32> = instance.get_typed_func(&self.store, "alloc").map_err(|e| self.abi(format!("`alloc` must be (i32) -> i32: {e}")))?;
        let call: TypedFunc<(i32, i32), i64> =
            instance.get_typed_func(&self.store, "devall_call").map_err(|e| self.abi(format!("`devall_call` must be (i32, i32) -> i64: {e}")))?;

        let envelope = CallEnvelope { api_version: API_VERSION, kind: kind.to_string(), id: id.to_string(), input };
        let bytes = serde_json::to_vec(&envelope).map_err(|e| self.abi(e.to_string()))?;
        let ptr = alloc.call(&mut self.store, bytes.len() as i32).map_err(|e| self.trap(&e))?;
        memory.write(&mut self.store, ptr as u32 as usize, &bytes).map_err(|_| self.abi("`alloc` returned memory out of bounds".into()))?;

        let mut pending = call.call_resumable(&mut self.store, (ptr, bytes.len() as i32)).map_err(|e| self.trap(&e))?;
        let packed = loop {
            match pending {
                TypedResumableCall::Finished(v) => break v,
                TypedResumableCall::OutOfFuel(inv) => {
                    self.refuel(inv.required_fuel())?;
                    pending = inv.resume(&mut self.store).map_err(|e| self.trap(&e))?;
                }
                TypedResumableCall::HostTrap(inv) => {
                    let e = inv.host_error();
                    return Err(if matches!(e.as_trap_code(), Some(wasmi::TrapCode::OutOfFuel)) {
                        PluginError::OutOfFuel { plugin: self.plugin.clone(), fuel: self.limits.fuel }
                    } else {
                        PluginError::Trap { plugin: self.plugin.clone(), detail: e.to_string() }
                    });
                }
            }
        };
        self.account();
        let (out_ptr, out_len) = unpack(packed);
        if out_len > self.limits.max_output_bytes {
            return Err(self.abi(format!("output of {out_len} bytes is larger than the {} MiB limit", self.limits.max_output_bytes >> 20)));
        }
        let data = memory.data(&self.store);
        let out = data.get(out_ptr..out_ptr.saturating_add(out_len)).ok_or_else(|| self.abi("devall_call returned a pointer out of bounds".into()))?;
        serde_json::from_slice::<Value>(out).map_err(|e| self.abi(format!("devall_call returned invalid JSON: {e}")))
    }
}

/// Reads and compiles `<dir>/<manifest.wasm>`.
pub fn compile_dir(dir: &Path, manifest: Manifest) -> Result<CompiledPlugin> {
    let path = dir.join(manifest.wasm_file());
    let len = std::fs::metadata(&path).map_err(|e| PluginError::io(&path, e))?.len();
    if len > crate::manifest::MAX_WASM_BYTES {
        return Err(PluginError::InvalidModule { plugin: manifest.id.clone(), detail: format!("{len} bytes is larger than the limit") });
    }
    let bytes = std::fs::read(&path).map_err(|e| PluginError::io(&path, e))?;
    CompiledPlugin::compile(manifest, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_and_urls() {
        assert_eq!(unpack(pack(0x1234, 99)), (0x1234, 99));
        assert_eq!(unpack(pack(-1, 0)), (0xffff_ffff, 0));
        assert_eq!(url_host("https://API.example.com/x?y").unwrap(), ("api.example.com".into(), 443));
        assert_eq!(url_host("http://u:p@h:8080").unwrap(), ("h".into(), 8080));
        assert_eq!(url_host("http://[::1]:9/").unwrap(), ("::1".into(), 9));
        assert!(url_host("ftp://x").is_err());
        assert!(url_host("/relative").is_err());
        assert_eq!(iso8601(Duration::from_millis(1_700_000_000_123)), "2023-11-14T22:13:20.123Z");
    }
}
