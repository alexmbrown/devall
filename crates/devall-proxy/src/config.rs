//! Proxy settings (per user, saved next to the CA in the app data directory).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{IoContext, Result};

pub const DEFAULT_PORT: u16 = 8899;
pub const DEFAULT_MAX_EXCHANGES: usize = 5000;
pub const DEFAULT_MAX_BODY: usize = 20 * 1024 * 1024;
pub const DEFAULT_MAX_TOTAL_BODY: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProxyConfig {
    pub version: u32,
    /// Port on 127.0.0.1 (the proxy never listens on other interfaces).
    pub port: u16,
    /// Hosts tunnelled without decryption, e.g. certificate-pinned apps. A plain name matches the
    /// host and its subdomains; `*`/`?` make it a glob; `host:port` also matches the port.
    pub bypass: Vec<String>,
    /// Hosts whose upstream TLS certificate is not verified (same pattern syntax).
    pub ignore_tls_errors: Vec<String>,
    /// Exchanges kept in memory (oldest dropped first).
    pub max_exchanges: usize,
    /// Body bytes kept per request/response; larger bodies are truncated in the capture (the
    /// transfer itself is never cut).
    pub max_body_bytes: usize,
    /// Body bytes kept across the whole capture; the oldest bodies are dropped beyond this.
    pub max_total_body_bytes: usize,
    /// Negotiate HTTP/2 with upstream servers over TLS.
    pub upstream_http2: bool,
    /// Trust the operating system's root certificates for upstream TLS.
    pub system_roots: bool,
    /// Extra PEM roots trusted for upstream TLS (corporate or local test CAs).
    pub extra_roots_pem: Vec<String>,
    /// A paused breakpoint resumes unchanged after this many seconds.
    pub breakpoint_timeout_secs: u64,
    /// Upstream must send response headers within this many seconds.
    pub upstream_timeout_secs: u64,
    /// WebSocket frames kept per exchange.
    pub max_ws_frames: usize,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            version: 1,
            port: DEFAULT_PORT,
            bypass: Vec::new(),
            ignore_tls_errors: Vec::new(),
            max_exchanges: DEFAULT_MAX_EXCHANGES,
            max_body_bytes: DEFAULT_MAX_BODY,
            max_total_body_bytes: DEFAULT_MAX_TOTAL_BODY,
            upstream_http2: true,
            system_roots: true,
            extra_roots_pem: Vec::new(),
            breakpoint_timeout_secs: 300,
            upstream_timeout_secs: 120,
            max_ws_frames: 2000,
        }
    }
}

impl ProxyConfig {
    /// Clamps values into sane ranges.
    pub fn normalized(mut self) -> Self {
        self.max_exchanges = self.max_exchanges.clamp(100, 200_000);
        self.max_body_bytes = self.max_body_bytes.clamp(1024, 256 * 1024 * 1024);
        self.max_total_body_bytes = self.max_total_body_bytes.max(self.max_body_bytes);
        self.breakpoint_timeout_secs = self.breakpoint_timeout_secs.clamp(5, 24 * 3600);
        self.upstream_timeout_secs = self.upstream_timeout_secs.clamp(1, 3600);
        self.max_ws_frames = self.max_ws_frames.min(100_000);
        self.version = 1;
        self
    }

    /// Reads settings, falling back to defaults when the file is missing or invalid.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<ProxyConfig>(&b).ok()).unwrap_or_default().normalized()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        devall_core::fsutil::atomic_write(path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}
