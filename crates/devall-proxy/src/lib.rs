//! `devall-proxy`: the /dev/all traffic inspector — a local MITM debugging proxy.
//!
//! - **Capture** every HTTP(S) request an app or browser makes through `127.0.0.1:<port>`
//!   (default 8899): plain HTTP proxying, HTTPS decrypted via `CONNECT` with on-the-fly leaf
//!   certificates from a per-user root CA ([`ca`]), WebSocket frames, HTTP/2 to clients and
//!   upstreams, DNS/TCP/TLS/wait/receive timings. Bodies stream through untouched while up to
//!   20 MB per message is kept (raw; decoded for display).
//! - **Bypass** certificate-pinned hosts (tunnelled without decryption) and **ignore TLS errors**
//!   per upstream host; upstream certificates are verified against the OS roots by default.
//! - **Rules** ([`rules`]): breakpoints (pause, edit, resume/abort), Map Local, Map Remote, mock
//!   responses, header edits, throttling and blocking.
//! - **Exports** ([`export`]): HAR 1.2, WireMock stubs for `crates/devall-mock`, saved requests,
//!   an OpenAPI 3.1 draft, a `crates/devall-load` scenario and cURL.
//!
//! ```no_run
//! # async fn demo() -> devall_proxy::Result<()> {
//! use devall_proxy::{CertAuthority, ProxyConfig, ProxyEngine};
//! let ca = CertAuthority::load_or_create("/path/to/app-data/proxy/ca")?;
//! let engine = ProxyEngine::new(ca, ProxyConfig::default())?;
//! let status = engine.start(None).await?; // 127.0.0.1:8899
//! let mut events = engine.subscribe();
//! # let _ = (status, events.recv().await);
//! engine.stop();
//! # Ok(()) }
//! ```
//!
//! Nothing here installs certificates or changes system proxy settings on its own: [`trust`]
//! and [`browser`] only build commands, and run them when the caller (after user confirmation)
//! asks.

mod body;
mod breakpoints;
pub mod browser;
pub mod ca;
pub mod config;
mod error;
pub mod export;
pub mod filter;
pub mod model;
mod pathgen;
mod pipeline;
pub mod rules;
mod server;
pub mod session;
pub mod store;
pub mod trust;
mod upstream;
mod util;
mod ws;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use bytes::Bytes;
use http::{HeaderValue, Method, Request, header};
use http_body_util::BodyExt;
use serde::Serialize;
use tokio::sync::broadcast;

pub use body::decode_body;
pub use ca::{CaInfo, CertAuthority};
pub use config::{DEFAULT_PORT, ProxyConfig};
pub use error::{ProxyError, Result};
pub use filter::{ContentKind, ExchangeFilter};
pub use model::*;
pub use rules::{Rule, RuleAction, RuleMatch, RulesFile};
pub use store::CaptureStore;

use crate::breakpoints::Breakpoints;
use crate::rules::{CompiledRules, Target};
use crate::server::RunningServer;
use crate::upstream::Upstream;
use crate::util::{HostMatcher, header_map, is_hop_by_hop};

/// Settings, compiled rules and the upstream client, swapped atomically on change.
pub(crate) struct Runtime {
    pub cfg: ProxyConfig,
    pub bypass: HostMatcher,
    pub rules: Arc<CompiledRules>,
    pub upstream: Arc<Upstream>,
}

pub(crate) struct Inner {
    pub ca: Arc<CertAuthority>,
    pub store: Arc<CaptureStore>,
    pub events: broadcast::Sender<ProxyEvent>,
    pub breakpoints: Breakpoints,
    runtime: RwLock<Arc<Runtime>>,
    rules_src: RwLock<Vec<Rule>>,
    server: Mutex<Option<RunningServer>>,
    pub tls_failures: Mutex<HashMap<String, Instant>>,
}

impl Inner {
    pub fn runtime(&self) -> Arc<Runtime> {
        self.runtime.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStatus {
    pub running: bool,
    pub port: Option<u16>,
    /// `127.0.0.1:<port>` while running.
    pub address: Option<String>,
    pub started_at: Option<i64>,
    pub exchanges: usize,
    pub paused: usize,
    /// Enabled rules.
    pub rules: usize,
    pub ca_sha256: String,
}

/// The proxy: capture store, rules, breakpoints and (when started) the listener. Cloning is
/// cheap and shares everything.
#[derive(Clone)]
pub struct ProxyEngine {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ProxyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyEngine").field("status", &self.status()).finish()
    }
}

impl ProxyEngine {
    pub fn new(ca: CertAuthority, config: ProxyConfig) -> Result<Self> {
        let config = config.normalized();
        let (events, _) = broadcast::channel(4096);
        let store = Arc::new(CaptureStore::new(config.max_exchanges, config.max_total_body_bytes, events.clone()));
        let runtime = Runtime {
            bypass: HostMatcher::new(&config.bypass),
            rules: Arc::new(CompiledRules::default()),
            upstream: Arc::new(Upstream::new(&config)?),
            cfg: config,
        };
        Ok(Self {
            inner: Arc::new(Inner {
                ca: Arc::new(ca),
                store,
                events,
                breakpoints: Breakpoints::default(),
                runtime: RwLock::new(Arc::new(runtime)),
                rules_src: RwLock::new(Vec::new()),
                server: Mutex::new(None),
                tls_failures: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn ca(&self) -> &CertAuthority {
        &self.inner.ca
    }

    pub fn store(&self) -> &CaptureStore {
        &self.inner.store
    }

    pub fn config(&self) -> ProxyConfig {
        self.inner.runtime().cfg.clone()
    }

    /// Applies new settings (the upstream connection pool is rebuilt; a running listener keeps
    /// its port until restarted).
    pub fn set_config(&self, config: ProxyConfig) -> Result<()> {
        let config = config.normalized();
        let current = self.inner.runtime();
        let upstream_changed = current.cfg.ignore_tls_errors != config.ignore_tls_errors
            || current.cfg.system_roots != config.system_roots
            || current.cfg.extra_roots_pem != config.extra_roots_pem
            || current.cfg.upstream_http2 != config.upstream_http2;
        let upstream = if upstream_changed { Arc::new(Upstream::new(&config)?) } else { current.upstream.clone() };
        self.inner.store.set_limits(config.max_exchanges, config.max_total_body_bytes);
        let rt = Runtime { bypass: HostMatcher::new(&config.bypass), rules: current.rules.clone(), upstream, cfg: config };
        *self.inner.runtime.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(rt);
        Ok(())
    }

    pub fn rules(&self) -> Vec<Rule> {
        self.inner.rules_src.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Validates and installs rules (ids are assigned to rules without one). Returns them.
    pub fn set_rules(&self, mut rules: Vec<Rule>) -> Result<Vec<Rule>> {
        rules::assign_ids(&mut rules);
        let compiled = Arc::new(rules::compile(&rules)?);
        let current = self.inner.runtime();
        let rt = Runtime { cfg: current.cfg.clone(), bypass: current.bypass.clone(), rules: compiled, upstream: current.upstream.clone() };
        *self.inner.runtime.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(rt);
        *self.inner.rules_src.write().unwrap_or_else(|e| e.into_inner()) = rules.clone();
        Ok(rules)
    }

    /// Starts listening on `127.0.0.1:<port>` (default: the configured port; `0` picks a free
    /// one). Already running on that port: returns the status; on another port: restarts.
    pub async fn start(&self, port: Option<u16>) -> Result<ProxyStatus> {
        let port = port.unwrap_or_else(|| self.inner.runtime().cfg.port);
        {
            let mut guard = self.inner.server.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = guard.as_ref() {
                if port == 0 || s.addr.port() == port {
                    drop(guard);
                    return Ok(self.status());
                }
                if let Some(old) = guard.take() {
                    old.stop();
                }
            }
        }
        match server::start(self.inner.clone(), port).await {
            Ok(running) => {
                let actual = running.addr.port();
                *self.inner.server.lock().unwrap_or_else(|e| e.into_inner()) = Some(running);
                let _ = self.inner.events.send(ProxyEvent::Status { running: true, port: Some(actual), error: None });
                Ok(self.status())
            }
            Err(e) => {
                let _ = self.inner.events.send(ProxyEvent::Status { running: false, port: None, error: Some(e.to_string()) });
                Err(e)
            }
        }
    }

    /// Stops listening (open connections are closed; paused breakpoints resume unchanged).
    pub fn stop(&self) {
        let running = self.inner.server.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(s) = running {
            s.stop();
            self.inner.breakpoints.release_all();
            let _ = self.inner.events.send(ProxyEvent::Status { running: false, port: None, error: None });
        }
    }

    pub fn status(&self) -> ProxyStatus {
        let guard = self.inner.server.lock().unwrap_or_else(|e| e.into_inner());
        ProxyStatus {
            running: guard.is_some(),
            port: guard.as_ref().map(|s| s.addr.port()),
            address: guard.as_ref().map(|s| s.addr.to_string()),
            started_at: guard.as_ref().map(|s| s.started_at),
            exchanges: self.inner.store.len(),
            paused: self.inner.breakpoints.len(),
            rules: self.inner.rules_src.read().unwrap_or_else(|e| e.into_inner()).iter().filter(|r| r.enabled).count(),
            ca_sha256: self.inner.ca.info().sha256,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProxyEvent> {
        self.inner.events.subscribe()
    }

    pub fn list(&self, filter: &ExchangeFilter) -> Vec<ExchangeSummary> {
        self.inner.store.list(filter)
    }

    pub fn get(&self, id: ExchangeId) -> Option<ExchangeDetail> {
        self.inner.store.get(id).map(|e| e.detail())
    }

    pub fn exchanges(&self, ids: &[ExchangeId]) -> Vec<Exchange> {
        self.inner.store.get_many(ids)
    }

    pub fn clear(&self) {
        self.inner.store.clear();
    }

    /// Adds exchanges (e.g. a loaded HAR) to the capture; returns their new ids.
    pub fn import(&self, exchanges: Vec<Exchange>) -> Vec<ExchangeId> {
        exchanges.into_iter().map(|e| self.inner.store.insert(e)).collect()
    }

    pub fn paused(&self) -> Vec<PausedExchange> {
        self.inner.breakpoints.list()
    }

    pub fn resume(&self, id: ExchangeId, edit: Option<BreakpointEdit>) -> Result<()> {
        if self.inner.breakpoints.resume(id, edit) { Ok(()) } else { Err(ProxyError::NotPaused(id)) }
    }

    pub fn abort(&self, id: ExchangeId) -> Result<()> {
        if self.inner.breakpoints.abort(id) { Ok(()) } else { Err(ProxyError::NotPaused(id)) }
    }

    /// Sends a captured request again through the rules (not breakpoints), with optional edits,
    /// and waits for the full response. Returns the new exchange's id.
    pub async fn replay(&self, id: ExchangeId, edit: Option<BreakpointEdit>) -> Result<ExchangeId> {
        let ex = self.inner.store.get(id).ok_or(ProxyError::NotFound(id))?;
        if ex.request.method == "CONNECT" {
            return Err(ProxyError::Invalid("tunnels cannot be replayed".into()));
        }
        let edit = edit.unwrap_or_default();
        let new_body = edit.new_body();
        if new_body.is_none() && (ex.request.body.truncated || ex.request.body.evicted) {
            return Err(ProxyError::Invalid("the request body was not fully captured, so it cannot be replayed unchanged".into()));
        }
        let method = edit.method.as_deref().map(str::trim).filter(|m| !m.is_empty()).unwrap_or(&ex.request.method).to_ascii_uppercase();
        let url = edit.url.as_deref().map(str::trim).filter(|u| !u.is_empty()).unwrap_or(&ex.request.url).to_string();
        let target = Target::parse(&url).ok_or_else(|| ProxyError::Invalid(format!("invalid URL {url}")))?;
        let mut headers = header_map(edit.headers.as_ref().unwrap_or(&ex.request.headers));
        let names: Vec<header::HeaderName> = headers.keys().filter(|k| is_hop_by_hop(k.as_str()) || k.as_str().starts_with(':')).cloned().collect();
        for n in names {
            headers.remove(n);
        }
        let body: Bytes = match new_body {
            Some(b) => {
                headers.remove(header::CONTENT_ENCODING);
                headers.insert(header::CONTENT_LENGTH, HeaderValue::from(b.len()));
                b
            }
            None => ex.request.body.raw.clone(),
        };
        let mut req = Request::new(());
        *req.method_mut() = Method::from_bytes(method.as_bytes()).map_err(|_| ProxyError::Invalid(format!("invalid method {method}")))?;
        *req.uri_mut() = target.url().parse().map_err(|e| ProxyError::Invalid(format!("invalid URL {url}: {e}")))?;
        *req.headers_mut() = headers;
        let (parts, ()) = req.into_parts();
        let origin = pipeline::Origin { client_addr: Some("replay".into()), replay: true };
        let (new_id, resp) = pipeline::handle(&self.inner, origin, parts, body::full(body), target).await;
        if let Ok(resp) = resp {
            // Drive the body so the capture completes.
            let _ = resp.into_body().collect().await;
        }
        Ok(new_id)
    }
}

#[cfg(test)]
mod tests;
