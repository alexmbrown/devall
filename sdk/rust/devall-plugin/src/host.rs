//! Host calls: logging, plus HTTP / secrets / project files / time / randomness when the
//! manifest declares the capability and the user approved it. Outside a wasm32 build (e.g. unit
//! tests on your machine) every call returns an error and logs go to stderr.

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::contract::{Request, Response};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

#[cfg(target_arch = "wasm32")]
mod imports {
    #[link(wasm_import_module = "devall")]
    unsafe extern "C" {
        pub fn log(level: i32, ptr: i32, len: i32);
        pub fn host_call(ptr: i32, len: i32) -> i64;
    }
}

/// A line in the plugin's log (shown with the call's result in the app).
pub fn log(level: Level, message: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        imports::log(level as i32, message.as_ptr() as usize as i32, message.len() as i32)
    }
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("[{level:?}] {message}");
}

/// Raw host call: JSON in, the `ok` value out.
pub fn call(request: &Value) -> Result<Value, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        let packed = unsafe { imports::host_call(bytes.as_ptr() as usize as i32, bytes.len() as i32) };
        let (ptr, len) = crate::abi::unpack(packed);
        let reply = unsafe { crate::abi::take(ptr, len) };
        let v: Value = serde_json::from_slice(&reply).map_err(|e| format!("bad host reply: {e}"))?;
        match (v.get("ok"), v.get("error")) {
            (Some(ok), _) => Ok(ok.clone()),
            (None, Some(Value::String(e))) => Err(e.clone()),
            _ => Err(format!("bad host reply: {v}")),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = request;
        Err("host calls are only available inside /dev/all".into())
    }
}

fn typed<T: DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| format!("bad host reply: {e}"))
}

/// Sends an HTTP request (capability `http` with the URL's host allowed). Redirects are not
/// followed.
pub fn http(request: &Request, timeout_ms: Option<u64>) -> Result<Response, String> {
    let mut v = json!({ "op": "http.request", "request": request });
    if let Some(t) = timeout_ms {
        v["timeoutMs"] = json!(t);
    }
    typed(call(&v)?)
}

/// A plugin secret the user stored and granted (capability `secrets.read`).
pub fn secret(name: &str) -> Result<String, String> {
    let v = call(&json!({ "op": "secrets.get", "name": name }))?;
    v.get("value").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "bad host reply".into())
}

/// A text file in the open project (capability `fs.project.read`); binary files come back
/// Base64-encoded with `true`.
pub fn read_file(path: &str) -> Result<(String, bool), String> {
    let v = call(&json!({ "op": "fs.read", "path": path }))?;
    Ok((v.get("content").and_then(Value::as_str).unwrap_or_default().to_string(), v.get("base64").and_then(Value::as_bool).unwrap_or(false)))
}

/// Milliseconds since the Unix epoch and an ISO 8601 UTC timestamp.
pub fn now() -> Result<(i64, String), String> {
    let v = call(&json!({ "op": "time.now" }))?;
    Ok((v.get("unixMs").and_then(Value::as_i64).unwrap_or(0), v.get("iso").and_then(Value::as_str).unwrap_or_default().to_string()))
}

/// `len` (≤ 1024) random bytes from the OS, hex-encoded.
pub fn random_hex(len: u32) -> Result<String, String> {
    let v = call(&json!({ "op": "random.bytes", "len": len }))?;
    v.get("hex").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "bad host reply".into())
}
