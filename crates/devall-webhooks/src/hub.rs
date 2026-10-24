//! The webhook pipeline for one project: route → store → (verify ∥ forward), shared by the local
//! receiver and [`WebhookHub::ingest`] (the hosted relay, imports). Also replays and re-signing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::WebhookError;
use crate::event::{EventSource, ForwardResult, IncomingEvent, StoreNotice, WebhookEvent, header, now_ms, remove_header, without_hop_by_hop};
use crate::forward::{Forwarder, OutboundRequest, UpstreamResponse, forward_request, with_query};
use crate::inbox::Router;
use crate::model::{InboxFile, ProviderKind};
use crate::store::{EventStore, event_from_incoming};
use crate::verify::{self, SignInput, SignOptions, VerifyOptions, VerifyResult};

/// Resolves `{{…}}` templates (variables and `{{secret.NAME}}`) — typically with the project's
/// active environment and the OS keychain. May block (keychain, secret-manager CLIs); the hub
/// calls it on a blocking thread. Errors are user-facing ("secret.X is not set …").
pub trait TemplateResolver: Send + Sync {
    fn resolve(&self, template: &str) -> Result<String, String>;
}

/// Literal values pass through; any `{{…}}` reference fails.
pub struct NoTemplates;

impl TemplateResolver for NoTemplates {
    fn resolve(&self, template: &str) -> Result<String, String> {
        resolve_with(&mut devall_core::Resolver::new(&devall_core::vars::NoSecrets), template)
    }
}

/// Fixed values: `vars` for `{{name}}`, `secrets` for `{{secret.NAME}}` (tests, CLI, the relay).
#[derive(Default, Clone)]
pub struct StaticTemplates {
    pub vars: devall_core::Variables,
    pub secrets: devall_core::Variables,
}

struct MapSecrets<'a>(&'a devall_core::Variables);

impl devall_core::SecretLookup for MapSecrets<'_> {
    fn lookup(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
}

impl TemplateResolver for StaticTemplates {
    fn resolve(&self, template: &str) -> Result<String, String> {
        let secrets = MapSecrets(&self.secrets);
        let mut r = devall_core::Resolver::new(&secrets);
        r.push_layer(self.vars.clone());
        resolve_with(&mut r, template)
    }
}

/// Resolves `template`, turning unresolved references into a readable error.
pub fn resolve_with(r: &mut devall_core::Resolver, template: &str) -> Result<String, String> {
    let out = r.resolve(template);
    if r.unresolved.is_empty() {
        return Ok(out);
    }
    let parts: Vec<String> = r
        .unresolved
        .iter()
        .map(|k| match k.strip_prefix("secret.") {
            Some(name) => format!("secret {name} is not set (add it on the Environments page; it is stored in the OS keychain)"),
            None => format!("variable {k} is not defined in the active environment"),
        })
        .collect();
    Err(parts.join("; "))
}

/// Called once per received or ingested event (not replays), after its response status is
/// known (`None` for ingested events). Used by the app to write the event log.
pub type ReceivedHook = Arc<dyn Fn(&WebhookEvent) + Send + Sync>;

/// What the receiver answers.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    fn json(status: u16, value: serde_json::Value) -> Self {
        Self { status, headers: vec![("Content-Type".into(), "application/json".into())], body: value.to_string().into_bytes() }
    }
}

/// Changes applied to an event before replaying it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplayEdits {
    pub method: Option<String>,
    /// Replaces all headers (hop-by-hop ones are still dropped).
    pub headers: Option<Vec<(String, String)>>,
    /// New body as text.
    pub body: Option<String>,
    /// New body as base64 (binary payloads); wins over `body`.
    pub body_base64: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplayOptions {
    /// Target URL (may use `{{variables}}`); defaults to the inbox's forward URL, then the original URL.
    pub target: Option<String>,
    pub edits: Option<ReplayEdits>,
    /// Replace signature headers with fresh ones (current timestamp) using the inbox's secret.
    pub resign: bool,
    pub timeout_ms: Option<u64>,
}

pub struct WebhookHub {
    root: PathBuf,
    store: Arc<EventStore>,
    templates: RwLock<Arc<dyn TemplateResolver>>,
    forwarder: Forwarder,
    /// Public base URL for this session (an exposed tunnel), without trailing slash.
    public_base: RwLock<Option<String>>,
    /// `http://127.0.0.1:<port>` while the receiver runs.
    local_base: RwLock<Option<String>>,
    on_received: RwLock<Option<ReceivedHook>>,
}

impl WebhookHub {
    pub fn new(root: impl Into<PathBuf>, store: Arc<EventStore>, templates: Arc<dyn TemplateResolver>) -> Arc<Self> {
        Arc::new(Self {
            root: root.into(),
            store,
            templates: RwLock::new(templates),
            forwarder: Forwarder::new(),
            public_base: RwLock::new(None),
            local_base: RwLock::new(None),
            on_received: RwLock::new(None),
        })
    }

    /// Opens `<project data>/webhooks.sqlite`.
    pub fn open(root: impl Into<PathBuf>, templates: Arc<dyn TemplateResolver>) -> Result<Arc<Self>, WebhookError> {
        let root = root.into();
        let store = Arc::new(EventStore::open_project(&root)?);
        Ok(Self::new(root, store, templates))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store(&self) -> &Arc<EventStore> {
        &self.store
    }

    pub fn templates(&self) -> Arc<dyn TemplateResolver> {
        self.templates.read().unwrap().clone()
    }

    /// E.g. after the active environment changed.
    pub fn set_templates(&self, t: Arc<dyn TemplateResolver>) {
        *self.templates.write().unwrap() = t;
    }

    /// The URL of an exposed tunnel for this session (`None` to clear).
    pub fn set_public_base(&self, base: Option<String>) {
        *self.public_base.write().unwrap() = base.map(|b| b.trim().trim_end_matches('/').to_string()).filter(|b| !b.is_empty());
    }

    pub fn public_base(&self) -> Option<String> {
        self.public_base.read().unwrap().clone()
    }

    pub(crate) fn set_local_base(&self, base: Option<String>) {
        *self.local_base.write().unwrap() = base;
    }

    /// Registers the [`ReceivedHook`] (replacing any previous one).
    pub fn set_on_received(&self, hook: Option<ReceivedHook>) {
        *self.on_received.write().unwrap() = hook;
    }

    fn notify_received(&self, e: &WebhookEvent) {
        let hook = self.on_received.read().unwrap().clone();
        if let Some(h) = hook {
            h(e);
        }
    }

    pub fn local_base(&self) -> Option<String> {
        self.local_base.read().unwrap().clone()
    }

    pub fn router(&self) -> Router {
        Router::load(&self.root)
    }

    /// The URL the sender called, when it differs from what the receiver saw: relay-provided →
    /// the inbox's `publicUrl` → `X-Forwarded-Proto/Host` (or a non-local `Host`) → the session's
    /// tunnel URL.
    fn public_url_for(&self, i: &IncomingEvent, inbox: Option<&InboxFile>) -> Option<String> {
        if let Some(u) = i.public_url.as_deref().filter(|u| !u.is_empty()) {
            return Some(u.to_string());
        }
        let pq = if i.query.is_empty() { i.path.clone() } else { format!("{}?{}", i.path, i.query) };
        if let Some(u) = inbox.and_then(|f| f.public_url.as_deref()).map(str::trim).filter(|u| !u.is_empty()) {
            return Some(with_query(u, &i.query));
        }
        let first = |v: &str| v.split(',').next().unwrap_or("").trim().to_string();
        let host = header(&i.headers, "x-forwarded-host").map(first).or_else(|| header(&i.headers, "host").map(str::to_string)).filter(|h| !h.is_empty());
        if let Some(h) = host.filter(|h| !is_local_host(h)) {
            let proto = header(&i.headers, "x-forwarded-proto").map(first).filter(|p| p == "http" || p == "https").unwrap_or_else(|| {
                if crate::tunnels::is_tunnel_host(&h) { "https".into() } else { "http".into() }
            });
            let url = format!("{proto}://{h}{pq}");
            return (url != i.url).then_some(url);
        }
        self.public_base().map(|b| format!("{b}{pq}"))
    }

    fn prepare(&self, mut incoming: IncomingEvent, routed: Option<(&str, &InboxFile)>) -> WebhookEvent {
        incoming.public_url = self.public_url_for(&incoming, routed.map(|(_, f)| f));
        event_from_incoming(incoming, routed.map(|(id, _)| id), routed.map(|(_, f)| f.provider))
    }

    fn insert(&self, e: &WebhookEvent) {
        if let Err(err) = self.store.insert(e) {
            self.store.notify(StoreNotice::Error { message: format!("could not store webhook {} {}: {err}", e.method, e.path) });
        }
    }

    /// Handles one request from the local receiver: stores it, answers with the inbox's response
    /// (or the local app's, in pass-through mode) and verifies/forwards in the background.
    pub async fn receive(self: &Arc<Self>, incoming: IncomingEvent) -> Reply {
        let router = self.router();
        let routed = router.route(&incoming.path).map(|(id, f)| (id.to_string(), f.clone()));
        let mut event = self.prepare(incoming, routed.as_ref().map(|(id, f)| (id.as_str(), f)));
        let inbox = routed.map(|(_, f)| f);

        if event.body_truncated {
            event.response_status = Some(413);
            self.insert(&event);
            self.notify_received(&event);
            self.spawn_verify(&event, inbox.as_ref());
            let mut r = Reply::json(413, serde_json::json!({ "error": "payload too large", "limit": crate::event::MAX_BODY_BYTES }));
            r.headers.push(("X-Devall-Event-Id".into(), event.id.clone()));
            return r;
        }

        let forward = inbox.as_ref().and_then(|f| f.forward.as_ref()).filter(|fw| fw.enabled);
        if let (Some(f), Some(fw)) = (inbox.as_ref(), forward)
            && fw.pass_through
        {
            self.insert(&event);
            self.spawn_verify(&event, Some(f));
            let (result, upstream) = self.forward_event(&event, f, true).await;
            let mut reply = match upstream {
                Some(u) => upstream_reply(u),
                None => Reply::json(502, serde_json::json!({ "error": result.error.clone().unwrap_or_else(|| "forward failed".into()) })),
            };
            // Status first: the forward update below broadcasts the row with both.
            let _ = self.store.set_response_status(&event.id, reply.status);
            let _ = self.store.set_forward(&event.id, &result);
            event.response_status = Some(reply.status);
            self.notify_received(&event);
            crate::event::set_header(&mut reply.headers, "X-Devall-Event-Id", &event.id);
            return reply;
        }

        let mut reply = match &inbox {
            Some(f) => configured_reply(f),
            None => Reply::json(200, serde_json::json!({ "received": true, "inbox": null })),
        };
        reply.headers.push(("X-Devall-Event-Id".into(), event.id.clone()));
        event.response_status = Some(reply.status);
        self.insert(&event);
        self.notify_received(&event);
        self.spawn_verify(&event, inbox.as_ref());
        if let (Some(f), Some(_)) = (inbox.as_ref(), forward) {
            self.spawn_forward(&event, f);
        }
        if let Some(f) = &inbox
            && f.response.delay_ms > 0
        {
            tokio::time::sleep(Duration::from_millis(f.response.delay_ms.min(120_000))).await;
        }
        reply
    }

    /// Entry point for events that arrived elsewhere — the hosted relay (store-and-forward while
    /// the laptop slept) or imports. Routes by `incoming.path`, stores and broadcasts the event,
    /// then verifies and forwards it in the background exactly like a locally received one.
    /// Set `incoming.public_url` to the URL the sender called (Twilio signs it) and
    /// `incoming.source` to [`EventSource::Relay`].
    pub async fn ingest(self: &Arc<Self>, mut incoming: IncomingEvent) -> Result<WebhookEvent, WebhookError> {
        if let Some(existing) = incoming.external_id.as_deref().map(|x| self.store.find_external(x)).transpose()?.flatten() {
            return Ok(existing); // redelivery (at-least-once sources)
        }
        crate::store::split_path_query(&mut incoming);
        let router = self.router();
        let routed = router.route(&incoming.path);
        let event = self.prepare(incoming, routed);
        self.store.insert(&event)?;
        self.notify_received(&event);
        let inbox = routed.map(|(_, f)| f);
        self.spawn_verify(&event, inbox);
        if let Some(f) = inbox
            && f.forward.as_ref().is_some_and(|fw| fw.enabled)
        {
            self.spawn_forward(&event, f);
        }
        Ok(event)
    }

    fn spawn_verify(self: &Arc<Self>, event: &WebhookEvent, inbox: Option<&InboxFile>) {
        let Some(f) = inbox.filter(|f| f.provider != ProviderKind::None).cloned() else { return };
        let hub = self.clone();
        let event = event.clone();
        tokio::spawn(async move {
            let result = verify_event(&event, &f, hub.templates(), None).await;
            let _ = hub.store.set_verification(&event.id, &result);
        });
    }

    fn spawn_forward(self: &Arc<Self>, event: &WebhookEvent, inbox: &InboxFile) {
        let hub = self.clone();
        let event = event.clone();
        let inbox = inbox.clone();
        tokio::spawn(async move {
            let (result, _) = hub.forward_event(&event, &inbox, false).await;
            let _ = hub.store.set_forward(&event.id, &result);
        });
    }

    async fn forward_event(&self, event: &WebhookEvent, inbox: &InboxFile, keep_body: bool) -> (ForwardResult, Option<UpstreamResponse>) {
        let Some(fw) = inbox.forward.as_ref() else { return (ForwardResult::failed("", "forwarding is not configured"), None) };
        let target = match resolve(self.templates(), fw.url.clone()).await {
            Ok(u) => u,
            Err(e) => return (ForwardResult::failed(&fw.url, format!("forward URL: {e}")), None),
        };
        if let Some(e) = self.loop_check(&target) {
            return (ForwardResult::failed(&target, e), None);
        }
        let req = forward_request(event, &target, fw.preserve_host, fw.timeout_ms);
        self.forwarder.send(&req, keep_body).await
    }

    /// Refuses targets that point back at this receiver.
    fn loop_check(&self, target: &str) -> Option<String> {
        let local = self.local_base()?;
        let (t, l) = (url::Url::parse(target).ok()?, url::Url::parse(&local).ok()?);
        let host_local = t.host_str().is_some_and(is_local_host);
        (host_local && t.port_or_known_default() == l.port_or_known_default())
            .then(|| format!("the forward target {target} is this webhook receiver — forwarding would loop; point it at your app"))
    }

    /// Re-verifies a stored event with the current secret (`templates` overrides the hub's, e.g.
    /// another environment) and stores the result.
    pub async fn verify(&self, id: &str, templates: Option<Arc<dyn TemplateResolver>>) -> Result<VerifyResult, WebhookError> {
        let event = self.store.get(id)?.ok_or_else(|| WebhookError::NotFound(format!("event {id}")))?;
        let router = self.router();
        let inbox = event.inbox.as_deref().and_then(|i| router.get(i)).cloned();
        let result = match inbox {
            Some(f) if f.provider != ProviderKind::None => verify_event(&event, &f, templates.unwrap_or_else(|| self.templates()), None).await,
            Some(_) => return Err(WebhookError::Invalid("this inbox has no signature scheme — choose a provider in its settings".into())),
            None if event.inbox.is_some() => return Err(WebhookError::NotFound(format!("inbox {} (deleted?)", event.inbox.unwrap_or_default()))),
            None => return Err(WebhookError::Invalid("this event matched no inbox, so there is no signature scheme to verify".into())),
        };
        self.store.set_verification(id, &result)?;
        Ok(result)
    }

    /// Builds the request a replay would send (edited, optionally re-signed), without sending it.
    pub async fn replay_request(&self, original: &WebhookEvent, opts: &ReplayOptions, templates: Arc<dyn TemplateResolver>) -> Result<OutboundRequest, WebhookError> {
        let router = self.router();
        let inbox = original.inbox.as_deref().and_then(|i| router.get(i)).cloned();
        let target_t = opts
            .target
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| inbox.as_ref().and_then(|f| f.forward.as_ref()).map(|fw| fw.url.clone()))
            .unwrap_or_else(|| original.url.clone());
        let target = resolve(templates.clone(), target_t.trim().to_string()).await.map_err(|e| WebhookError::Invalid(format!("target URL: {e}")))?;
        if !(target.starts_with("http://") || target.starts_with("https://")) {
            return Err(WebhookError::Invalid(format!("target URL must start with http:// or https:// (got {target})")));
        }
        let edits = opts.edits.clone().unwrap_or_default();
        let method = edits.method.filter(|m| !m.trim().is_empty()).map(|m| m.trim().to_ascii_uppercase()).unwrap_or_else(|| original.method.clone());
        let mut headers = without_hop_by_hop(edits.headers.as_ref().unwrap_or(&original.headers));
        remove_header(&mut headers, "host");
        let body = match (&edits.body_base64, &edits.body) {
            (Some(b), _) => base64::engine::general_purpose::STANDARD.decode(b.trim()).map_err(|e| WebhookError::Invalid(format!("body is not valid base64: {e}")))?,
            (None, Some(t)) => t.clone().into_bytes(),
            (None, None) => original.body.clone(),
        };
        // Keep the original query unless the target sets its own.
        let mut url = with_query(&target, &original.query);
        if opts.resign {
            let f = inbox.as_ref().filter(|f| f.provider != ProviderKind::None).ok_or_else(|| {
                WebhookError::Invalid("re-signing needs the event's inbox to have a provider and secret".into())
            })?;
            let secret = secret_for(f, templates).await.map_err(WebhookError::Secret)?;
            let input = SignInput { method: &method, url: &url, headers: &headers, body: &body };
            let signed = verify::sign(f.provider, f.generic_hmac.as_ref(), &secret, &input, &SignOptions::default()).map_err(WebhookError::Invalid)?;
            signed.apply(&mut headers);
            if let Some(u) = signed.url {
                url = u;
            }
        }
        let timeout_ms = opts.timeout_ms.or_else(|| inbox.as_ref().and_then(|f| f.forward.as_ref()).map(|fw| fw.timeout_ms)).unwrap_or(crate::model::DEFAULT_FORWARD_TIMEOUT_MS);
        Ok(OutboundRequest { method, url, headers, body, timeout_ms })
    }

    /// Sends a (possibly edited and re-signed) copy of event `id` and records it as a new event
    /// linked to the original (`replayOf`), with the target's response as its forward result.
    pub async fn replay(self: &Arc<Self>, id: &str, opts: ReplayOptions, templates: Arc<dyn TemplateResolver>) -> Result<WebhookEvent, WebhookError> {
        let original = self.store.get(id)?.ok_or_else(|| WebhookError::NotFound(format!("event {id}")))?;
        let req = self.replay_request(&original, &opts, templates.clone()).await?;
        let (result, _) = self.forwarder.send(&req, false).await;
        let parsed = url::Url::parse(&req.url).ok();
        let mut event = WebhookEvent {
            id: ulid::Ulid::generate().to_string(),
            inbox: original.inbox.clone(),
            received_at: now_ms(),
            source: EventSource::Replay,
            method: req.method.clone(),
            url: req.url.clone(),
            path: parsed.as_ref().map(|u| u.path().to_string()).unwrap_or_else(|| original.path.clone()),
            query: parsed.as_ref().and_then(|u| u.query()).unwrap_or("").to_string(),
            content_type: header(&req.headers, "content-type").map(str::to_string),
            body_size: req.body.len(),
            body_truncated: false,
            headers: req.headers.clone(),
            body: req.body.clone(),
            remote_addr: None,
            public_url: None,
            label: original.label.clone(),
            provider: original.provider,
            response_status: None,
            verification: None,
            forward: Some(result),
            replay_of: Some(original.id.clone()),
            external_id: None,
        };
        let router = self.router();
        if let Some(f) = event.inbox.as_deref().and_then(|i| router.get(i)).filter(|f| f.provider != ProviderKind::None) {
            // Verify against "now": shows whether the target will accept the timestamp.
            let now = chrono::Utc::now().timestamp();
            event.verification = Some(verify_event(&event, f, templates, Some(now)).await);
        }
        self.store.insert(&event)?;
        Ok(event)
    }

    /// The event as a cURL command (optionally re-signed for `target`).
    pub async fn curl(&self, id: &str, opts: &ReplayOptions, templates: Arc<dyn TemplateResolver>) -> Result<String, WebhookError> {
        let original = self.store.get(id)?.ok_or_else(|| WebhookError::NotFound(format!("event {id}")))?;
        let req = self.replay_request(&original, opts, templates).await?;
        Ok(crate::convert::curl_for(&req))
    }
}

/// Resolves the inbox's secret template on a blocking thread.
pub async fn secret_for(inbox: &InboxFile, templates: Arc<dyn TemplateResolver>) -> Result<String, String> {
    if inbox.secret.trim().is_empty() {
        return Err(format!("no signing secret configured for inbox {} — set secret: '{{{{secret.NAME}}}}'", inbox.name));
    }
    resolve(templates, inbox.secret.clone()).await.and_then(|s| if s.is_empty() { Err("the signing secret is empty".into()) } else { Ok(s) })
}

async fn resolve(templates: Arc<dyn TemplateResolver>, template: String) -> Result<String, String> {
    if !template.contains("{{") {
        return Ok(template);
    }
    tokio::task::spawn_blocking(move || templates.resolve(&template)).await.map_err(|e| e.to_string())?
}

/// Verifies `event` as `inbox` describes. `now` overrides the reference time (default: arrival).
pub async fn verify_event(event: &WebhookEvent, inbox: &InboxFile, templates: Arc<dyn TemplateResolver>, now: Option<i64>) -> VerifyResult {
    match secret_for(inbox, templates).await {
        Err(reason) => VerifyResult::missing_secret(inbox.provider, reason),
        Ok(secret) => {
            let mut opts = VerifyOptions::at(event.received_at, inbox.tolerance_seconds);
            if let Some(n) = now {
                opts.now = n;
            }
            let input = SignInput { method: &event.method, url: event.signed_url(), headers: &event.headers, body: &event.body };
            verify::verify(inbox.provider, inbox.generic_hmac.as_ref(), &secret, &input, opts)
        }
    }
}

fn configured_reply(f: &InboxFile) -> Reply {
    let mut headers: Vec<(String, String)> = f.response.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let body = f.response.body.clone().into_bytes();
    if !body.is_empty() && header(&headers, "content-type").is_none() {
        let t = f.response.body.trim_start();
        let ct = if t.starts_with('{') || t.starts_with('[') {
            "application/json"
        } else if t.starts_with('<') {
            "application/xml"
        } else {
            "text/plain; charset=utf-8"
        };
        headers.push(("Content-Type".into(), ct.into()));
    }
    Reply { status: f.response.status, headers, body }
}

/// The local app's response, relayed as is (the forwarder never decompresses, so
/// `Content-Encoding` still matches the bytes).
fn upstream_reply(u: UpstreamResponse) -> Reply {
    Reply { status: u.status, headers: without_hop_by_hop(&u.headers), body: u.body }
}

pub(crate) fn is_local_host(host_port: &str) -> bool {
    let h = host_port.trim();
    let host = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest).to_string()
    } else {
        h.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map(|(h, _)| h).unwrap_or(h).to_string()
    };
    let host = host.to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost") || host == "0.0.0.0" || host == "::1" || host == "::" || host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        let t = StaticTemplates {
            vars: [("baseUrl".to_string(), "http://localhost:3000".to_string())].into_iter().collect(),
            secrets: [("S".to_string(), "v".to_string())].into_iter().collect(),
        };
        assert_eq!(t.resolve("{{baseUrl}}/x").unwrap(), "http://localhost:3000/x");
        assert_eq!(t.resolve("{{secret.S}}").unwrap(), "v");
        let e = t.resolve("{{secret.MISSING}} {{nope}}").unwrap_err();
        assert!(e.contains("secret MISSING is not set") && e.contains("variable nope"), "{e}");
        assert_eq!(NoTemplates.resolve("plain").unwrap(), "plain");
        assert!(NoTemplates.resolve("{{secret.X}}").is_err());
    }

    #[test]
    fn local_hosts() {
        for h in ["127.0.0.1:4590", "localhost", "[::1]:80", "app.localhost:3000", "0.0.0.0"] {
            assert!(is_local_host(h), "{h}");
        }
        for h in ["abc.trycloudflare.com", "192.168.1.5:4590", "example.com:443"] {
            assert!(!is_local_host(h), "{h}");
        }
    }

    fn hub() -> Arc<WebhookHub> {
        WebhookHub::new(std::env::temp_dir(), Arc::new(EventStore::open_in_memory().unwrap()), Arc::new(NoTemplates))
    }

    #[test]
    fn public_url_resolution() {
        let h = hub();
        let mut i = IncomingEvent {
            method: "POST".into(),
            url: "http://127.0.0.1:4590/twilio?x=1".into(),
            path: "/twilio".into(),
            query: "x=1".into(),
            headers: vec![("host".into(), "127.0.0.1:4590".into())],
            ..Default::default()
        };
        assert_eq!(h.public_url_for(&i, None), None);
        h.set_public_base(Some("https://abc.trycloudflare.com/".into()));
        assert_eq!(h.public_url_for(&i, None).as_deref(), Some("https://abc.trycloudflare.com/twilio?x=1"));
        // Headers from the tunnel describe the real URL.
        i.headers = vec![("host".into(), "def.ngrok-free.app".into())];
        assert_eq!(h.public_url_for(&i, None).as_deref(), Some("https://def.ngrok-free.app/twilio?x=1"));
        i.headers.push(("x-forwarded-proto".into(), "http".into()));
        assert_eq!(h.public_url_for(&i, None).as_deref(), Some("http://def.ngrok-free.app/twilio?x=1"));
        // An explicit inbox publicUrl wins.
        let mut f = InboxFile::new("t", "/twilio");
        f.public_url = Some("https://hooks.example.com/twilio".into());
        assert_eq!(h.public_url_for(&i, Some(&f)).as_deref(), Some("https://hooks.example.com/twilio?x=1"));
    }

    #[test]
    fn loop_detection() {
        let h = hub();
        h.set_local_base(Some("http://127.0.0.1:4590".into()));
        assert!(h.loop_check("http://localhost:4590/stripe").is_some());
        assert!(h.loop_check("http://localhost:3000/stripe").is_none());
    }
}
