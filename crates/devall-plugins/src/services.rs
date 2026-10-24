//! The default [`HostServices`]: HTTP through reqwest (no redirects, no cookies, rustls) and plugin
//! secrets from a [`SecretStore`] scope `plugin:<id>` (the OS keychain in the app and CLI).

use std::time::{Duration, Instant};

use devall_core::{SecretScope, SecretStore};

use crate::contract::{Header, PluginRequest, PluginResponse};
use crate::runtime::HostServices;

/// Largest response body handed to a plugin.
const MAX_BODY: usize = 8 << 20;

/// Keychain scope of a plugin's secrets.
pub fn secret_scope(plugin: &str) -> SecretScope {
    SecretScope::new(format!("plugin:{plugin}"), devall_core::secrets::ALL_ENVIRONMENTS)
}

pub struct DefaultServices {
    secrets: Option<Box<dyn SecretStore>>,
}

impl DefaultServices {
    pub fn new(secrets: Option<Box<dyn SecretStore>>) -> Self {
        Self { secrets }
    }
}

impl HostServices for DefaultServices {
    fn http(&self, req: &PluginRequest, timeout: Duration) -> Result<PluginResponse, String> {
        let req = req.clone();
        // Plugin calls run on blocking threads (sometimes inside a runtime, sometimes not): give
        // the request its own small runtime on a scoped thread so it never nests block_on.
        std::thread::scope(|s| {
            s.spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
                rt.block_on(send(req, timeout))
            })
            .join()
            .map_err(|_| "HTTP request panicked".to_string())?
        })
    }

    fn secret(&self, plugin: &str, name: &str) -> Option<String> {
        self.secrets.as_ref()?.get(&secret_scope(plugin), name).ok().flatten()
    }
}

async fn send(req: PluginRequest, timeout: Duration) -> Result<PluginResponse, String> {
    use base64::Engine;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .user_agent(concat!("devall-plugin/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let method = reqwest::Method::from_bytes(req.method.trim().to_ascii_uppercase().as_bytes()).map_err(|_| format!("bad method {}", req.method))?;
    let mut b = client.request(method, &req.url);
    for h in &req.headers {
        b = b.header(h.name.as_str(), h.value.as_str());
    }
    if let Some(body) = &req.body {
        let bytes = if req.body_base64 { base64::engine::general_purpose::STANDARD.decode(body).map_err(|e| format!("bad base64 body: {e}"))? } else { body.clone().into_bytes() };
        b = b.body(bytes);
    }
    let t0 = Instant::now();
    let resp = b.send().await.map_err(|e| if e.is_timeout() { "timed out".to_string() } else { e.to_string() })?;
    let status = resp.status().as_u16();
    let headers = resp.headers().iter().map(|(k, v)| Header::new(k.as_str(), String::from_utf8_lossy(v.as_bytes()))).collect();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BODY {
        return Err(format!("response body is larger than {} MiB", MAX_BODY >> 20));
    }
    let (body, body_base64) = match String::from_utf8(bytes.to_vec()) {
        Ok(t) => (t, false),
        Err(e) => (base64::engine::general_purpose::STANDARD.encode(e.as_bytes()), true),
    };
    Ok(PluginResponse { status, headers, body: (!body.is_empty()).then_some(body), body_base64, time_ms: t0.elapsed().as_millis() as u64 })
}
