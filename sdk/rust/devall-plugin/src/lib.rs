//! Write /dev/all plugins in Rust.
//!
//! A plugin is a `cdylib` built for `wasm32-unknown-unknown` plus a `plugin.yaml` manifest (see
//! `docs/PLUGINS.md`). Implement [`Plugin`] for your type — only the methods for the contribution
//! points your manifest declares — and export it:
//!
//! ```ignore
//! use devall_plugin::*;
//!
//! struct Upper;
//!
//! impl Plugin for Upper {
//!     fn utility(&self, id: &str, input: UtilityInput) -> Result<UtilityOutput, String> {
//!         match id {
//!             "upper" => Ok(UtilityOutput::text(input.input.to_uppercase())),
//!             other => Err(format!("unknown utility {other}")),
//!         }
//!     }
//! }
//!
//! devall_plugin::export!(Upper);
//! ```
//!
//! Build with `cargo build --target wasm32-unknown-unknown --release` and copy
//! `target/wasm32-unknown-unknown/release/<crate>.wasm` next to `plugin.yaml` as `plugin.wasm`.
//!
//! The host gives a plugin no WASI: no clock, files, network or randomness except through
//! [`host`] functions, which only work for the capabilities the manifest declares and the user
//! approved (anything else returns `Err("capability denied: …")`).

pub mod contract;
pub mod host;

pub use contract::*;

/// One method per contribution kind; the `id` is the contribution's id from `plugin.yaml`.
/// Unimplemented kinds answer with an error.
pub trait Plugin {
    fn utility(&self, id: &str, input: UtilityInput) -> Result<UtilityOutput, String> {
        let _ = input;
        Err(format!("utility {id} is not implemented"))
    }
    fn pre_request(&self, id: &str, input: PreRequestInput) -> Result<PreRequestOutput, String> {
        let _ = input;
        Err(format!("pre-request hook {id} is not implemented"))
    }
    fn post_response(&self, id: &str, input: PostResponseInput) -> Result<PostResponseOutput, String> {
        let _ = input;
        Err(format!("post-response hook {id} is not implemented"))
    }
    fn import_detect(&self, id: &str, input: ImportInput) -> Result<DetectOutput, String> {
        let _ = (id, input);
        Ok(DetectOutput { confidence: 0.0 })
    }
    fn import_parse(&self, id: &str, input: ImportInput) -> Result<ImportOutput, String> {
        let _ = input;
        Err(format!("importer {id} is not implemented"))
    }
    fn codegen(&self, id: &str, input: CodegenInput) -> Result<CodegenOutput, String> {
        let _ = input;
        Err(format!("codegen target {id} is not implemented"))
    }
    fn auth(&self, id: &str, input: AuthInput) -> Result<AuthOutput, String> {
        let _ = input;
        Err(format!("auth provider {id} is not implemented"))
    }
}

fn run<I: serde::de::DeserializeOwned, O: serde::Serialize>(input: serde_json::Value, f: impl FnOnce(I) -> Result<O, String>) -> Result<serde_json::Value, String> {
    let i: I = serde_json::from_value(input).map_err(|e| format!("bad input: {e}"))?;
    let o = f(i)?;
    serde_json::to_value(o).map_err(|e| e.to_string())
}

/// Dispatches one call envelope to the plugin: `{"ok": …}` or `{"error": "…"}`. Pure (no ABI),
/// so plugins can unit-test their behaviour on the host.
pub fn dispatch<P: Plugin + ?Sized>(plugin: &P, envelope: &[u8]) -> serde_json::Value {
    let env: CallEnvelope = match serde_json::from_slice(envelope) {
        Ok(e) => e,
        Err(e) => return serde_json::json!({ "error": format!("bad call envelope: {e}") }),
    };
    let id = env.id.as_str();
    let result = match env.kind.as_str() {
        KIND_UTILITY => run(env.input, |i| plugin.utility(id, i)),
        KIND_PRE_REQUEST => run(env.input, |i| plugin.pre_request(id, i)),
        KIND_POST_RESPONSE => run(env.input, |i| plugin.post_response(id, i)),
        KIND_IMPORT_DETECT => run(env.input, |i| plugin.import_detect(id, i)),
        KIND_IMPORT_PARSE => run(env.input, |i| plugin.import_parse(id, i)),
        KIND_CODEGEN => run(env.input, |i| plugin.codegen(id, i)),
        KIND_AUTH => run(env.input, |i| plugin.auth(id, i)),
        other => Err(format!("this plugin does not handle `{other}` calls")),
    };
    match result {
        Ok(v) => serde_json::json!({ "ok": v }),
        Err(e) => serde_json::json!({ "error": e }),
    }
}

/// ABI glue used by [`export!`]. Pointers are wasm32 linear-memory offsets.
#[doc(hidden)]
pub mod abi {
    /// Memory for `len` bytes the host will write into (owned by whoever reads it next).
    pub fn alloc(len: i32) -> i32 {
        let mut v: Vec<u8> = Vec::with_capacity(len.max(0) as usize);
        let p = v.as_mut_ptr();
        core::mem::forget(v);
        p as usize as i32
    }

    /// # Safety
    /// `ptr`/`len` must come from [`alloc`].
    pub unsafe fn dealloc(ptr: i32, len: i32) {
        unsafe { drop(Vec::from_raw_parts(ptr as usize as *mut u8, 0, len.max(0) as usize)) }
    }

    /// Takes ownership of bytes the host wrote into memory from [`alloc`].
    ///
    /// # Safety
    /// `ptr`/`len` must come from [`alloc`] and be fully written.
    pub unsafe fn take(ptr: i32, len: i32) -> Vec<u8> {
        let len = len.max(0) as usize;
        unsafe { Vec::from_raw_parts(ptr as usize as *mut u8, len, len) }
    }

    /// Leaks `bytes` and packs `(ptr << 32) | len` for the host.
    pub fn give(bytes: Vec<u8>) -> i64 {
        let b = bytes.into_boxed_slice();
        let len = b.len() as u64;
        let ptr = Box::into_raw(b) as *mut u8 as usize as u64;
        ((ptr << 32) | (len & 0xffff_ffff)) as i64
    }

    /// Splits a packed `(ptr << 32) | len`.
    pub fn unpack(v: i64) -> (i32, i32) {
        let v = v as u64;
        ((v >> 32) as u32 as i32, (v & 0xffff_ffff) as u32 as i32)
    }

    /// # Safety
    /// `ptr`/`len` must be the call envelope the host wrote into memory from [`alloc`].
    pub unsafe fn call<P: crate::Plugin + ?Sized>(plugin: &P, ptr: i32, len: i32) -> i64 {
        let input = unsafe { take(ptr, len) };
        let out = crate::dispatch(plugin, &input);
        give(serde_json::to_vec(&out).unwrap_or_else(|_| br#"{"error":"could not encode the result"}"#.to_vec()))
    }
}

/// Exports `alloc`, `dealloc` and `devall_call` for a [`Plugin`] value.
#[macro_export]
macro_rules! export {
    ($plugin:expr) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn alloc(len: i32) -> i32 {
            $crate::abi::alloc(len)
        }

        /// # Safety
        /// Called by the host with memory it got from `alloc`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn dealloc(ptr: i32, len: i32) {
            unsafe { $crate::abi::dealloc(ptr, len) }
        }

        /// # Safety
        /// Called by the host with the call envelope it wrote into memory from `alloc`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn devall_call(ptr: i32, len: i32) -> i64 {
            unsafe { $crate::abi::call(&$plugin, ptr, len) }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Upper;

    impl Plugin for Upper {
        fn utility(&self, id: &str, input: UtilityInput) -> Result<UtilityOutput, String> {
            match id {
                "upper" => Ok(UtilityOutput::text(input.input.to_uppercase())),
                "fail" => Err("nope".into()),
                other => Err(format!("unknown {other}")),
            }
        }
        fn auth(&self, _id: &str, input: AuthInput) -> Result<AuthOutput, String> {
            Ok(AuthOutput { headers: vec![Header::new("X-Key", input.config.get("key").cloned().unwrap_or_default())], query: vec![] })
        }
    }

    fn call(kind: &str, id: &str, input: serde_json::Value) -> serde_json::Value {
        let env = serde_json::json!({ "apiVersion": 1, "kind": kind, "id": id, "input": input });
        dispatch(&Upper, env.to_string().as_bytes())
    }

    #[test]
    fn dispatches_by_kind() {
        assert_eq!(call("utility", "upper", serde_json::json!({ "input": "abc", "args": {} })), serde_json::json!({ "ok": { "output": "ABC" } }));
        assert_eq!(call("utility", "fail", serde_json::json!({ "input": "" })), serde_json::json!({ "error": "nope" }));
        let out = call("auth", "x", serde_json::json!({ "request": { "method": "GET", "url": "https://x" }, "config": { "key": "k" } }));
        assert_eq!(out["ok"]["headers"][0]["value"], "k");
        assert_eq!(call("codegen", "x", serde_json::json!({ "request": { "method": "GET", "url": "u" } }))["error"], "codegen target x is not implemented");
        assert_eq!(call("import.detect", "x", serde_json::json!({ "text": "" }))["ok"]["confidence"], 0.0);
        assert!(call("utility", "upper", serde_json::json!(42))["error"].as_str().unwrap().starts_with("bad input"));
        assert!(dispatch(&Upper, b"not json")["error"].as_str().unwrap().starts_with("bad call envelope"));
        assert_eq!(abi::unpack((5i64 << 32) | 9), (5, 9));
    }
}
