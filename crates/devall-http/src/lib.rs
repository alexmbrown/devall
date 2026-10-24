//! HTTP execution: [`prepare`] turns a project request (with folder inheritance already applied)
//! into a fully resolved [`PreparedRequest`]; [`ClientPool::execute`] sends it and measures it.
//! [`checks`] evaluates assertions/captures, [`oauth2`] fetches tokens, [`sse`] parses streams.

pub mod checks;
pub mod cookies;
mod curl_export;
pub mod digest;
pub mod ntlm;
pub mod oauth2;
pub mod plugins;
pub mod runner;
mod prepare;
pub mod sigv4;
pub mod snapshot;
pub mod sse;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use devall_core::{Auth, HttpRequestFile, OAuth2Config, Resolver};
use serde::Serialize;

pub use cookies::{CookieInfo, CookieJar};
pub use curl_export::to_curl;
pub use prepare::{PrepareOptions, PreparedBody, PreparedPart, PreparedRequest, prepare, prepare_with};

/// Bodies larger than this are truncated (the UI shows a notice).
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
pub const USER_AGENT: &str = concat!("devall/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
pub enum HttpError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("request failed: {0}")]
    Request(String),
    #[error("reading body failed: {0}")]
    Body(String),
}

impl HttpError {
    /// Same error with every secret value masked.
    pub fn redacted(self, secrets: &std::collections::BTreeSet<String>) -> Self {
        let r = |m: String| devall_core::vars::redact_values(&m, secrets);
        match self {
            HttpError::InvalidUrl(m) => HttpError::InvalidUrl(r(m)),
            HttpError::InvalidRequest(m) => HttpError::InvalidRequest(r(m)),
            HttpError::Connect(m) => HttpError::Connect(r(m)),
            HttpError::Timeout(m) => HttpError::Timeout(r(m)),
            HttpError::Request(m) => HttpError::Request(r(m)),
            HttpError::Body(m) => HttpError::Body(r(m)),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timings {
    /// Request start → response headers received (DNS + connect + TLS + server time).
    pub ttfb_ms: f64,
    /// Response headers → last body byte.
    pub download_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpResponseData {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    /// In wire order, duplicates preserved.
    pub headers: Vec<(String, String)>,
    #[serde(skip)]
    pub body: Vec<u8>,
    pub body_truncated: bool,
    /// Final URL after redirects.
    pub url: String,
    pub redirected: bool,
    pub remote_addr: Option<String>,
    pub timings: Timings,
    pub body_size: usize,
}

/// JSON-friendly view of a response: text when the body is valid UTF-8, base64 otherwise.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseView {
    #[serde(flatten)]
    pub meta: HttpResponseData,
    pub body_text: Option<String>,
    pub body_base64: Option<String>,
    pub content_type: Option<String>,
}

impl HttpResponseData {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn into_view(self) -> ResponseView {
        let content_type = self.header("content-type").map(str::to_string);
        let (body_text, body_base64) = match std::str::from_utf8(&self.body) {
            Ok(t) => (Some(t.to_string()), None),
            Err(_) => (None, Some(base64::engine::general_purpose::STANDARD.encode(&self.body))),
        };
        ResponseView { meta: self, body_text, body_base64, content_type }
    }
}

/// Live notifications while a response streams in (Server-Sent Events, progress).
pub trait ExecObserver: Send + Sync {
    fn sse_event(&self, _event: &sse::SseEvent) {}
    fn progress(&self, _bytes: usize) {}
}

/// Reuses connections across sends. One client per distinct TLS/redirect/proxy/certificate
/// configuration, all sharing a cookie jar. Cloning is cheap and shares everything.
#[derive(Default, Clone)]
pub struct ClientPool {
    inner: Arc<PoolInner>,
}

#[derive(Default)]
struct PoolInner {
    clients: Mutex<HashMap<String, reqwest::Client>>,
    jar: Arc<CookieJar>,
}

impl ClientPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cookies(&self) -> &CookieJar {
        &self.inner.jar
    }

    fn client(&self, req: &PreparedRequest) -> Result<reqwest::Client, HttpError> {
        let key = format!("{}|{}|{:?}|{:?}", req.verify_tls, req.follow_redirects, req.proxy, req.client_cert);
        let mut map = self.inner.clients.lock().unwrap();
        if let Some(c) = map.get(&key) {
            return Ok(c.clone());
        }
        let client = self.builder(req)?.build().map_err(|e| HttpError::Request(error_chain(&e)))?;
        map.insert(key, client.clone());
        Ok(client)
    }

    /// Client settings shared by pooled and per-exchange clients (TLS, redirects, proxy, mTLS, cookies).
    fn builder(&self, req: &PreparedRequest) -> Result<reqwest::ClientBuilder, HttpError> {
        let mut b = reqwest::Client::builder()
            .tls_danger_accept_invalid_certs(!req.verify_tls)
            .redirect(if req.follow_redirects { reqwest::redirect::Policy::limited(10) } else { reqwest::redirect::Policy::none() })
            .cookie_provider(self.inner.jar.clone());
        match req.proxy.as_deref() {
            None => {}
            Some(p) if p.eq_ignore_ascii_case("none") => b = b.no_proxy(),
            Some(p) => b = b.proxy(reqwest::Proxy::all(p).map_err(|e| HttpError::InvalidRequest(format!("proxy {p}: {e}")))?),
        }
        if let Some((cert, key)) = &req.client_cert {
            let mut pem = std::fs::read(cert).map_err(|e| HttpError::InvalidRequest(format!("client certificate {}: {e}", cert.display())))?;
            if let Some(k) = key {
                pem.push(b'\n');
                pem.extend(std::fs::read(k).map_err(|e| HttpError::InvalidRequest(format!("client key {}: {e}", k.display())))?);
            }
            let id = reqwest::Identity::from_pem(&pem).map_err(|e| HttpError::InvalidRequest(format!("client certificate: {}", error_chain(&e))))?;
            b = b.identity(id);
        }
        Ok(b)
    }

    pub async fn execute(&self, req: &PreparedRequest) -> Result<HttpResponseData, HttpError> {
        self.execute_with(req, None).await
    }

    /// Sends `req`; Digest challenges are answered once automatically and NTLM runs its
    /// three-leg handshake. `observer` receives SSE events and progress as the body streams in.
    pub async fn execute_with(&self, req: &PreparedRequest, observer: Option<&dyn ExecObserver>) -> Result<HttpResponseData, HttpError> {
        if let Some(creds) = &req.ntlm {
            return self.execute_ntlm(req, creds, observer).await;
        }
        let client = self.client(req)?;
        let start = Instant::now();
        let first = send_once(&client, req, None, observer, start).await?;
        if let Some((user, pass)) = &req.digest
            && first.status == 401
            && let Some(ch) = first
                .headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("www-authenticate"))
                .find_map(|(_, v)| digest::parse_challenge(v))
        {
            let url = url::Url::parse(&req.url).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
            let uri = match url.query() {
                Some(q) => format!("{}?{q}", url.path()),
                None => url.path().to_string(),
            };
            let cnonce = format!("{:016x}", fastrand::u64(..));
            let auth = digest::authorization(&ch, &req.method, &uri, user, pass, &cnonce, 1);
            return send_once(&client, req, Some(auth), observer, Instant::now()).await;
        }
        Ok(first)
    }

    /// NTLM authenticates the connection, so the negotiate and authenticate legs go through a
    /// dedicated HTTP/1.1 client holding one kept-alive connection (the cookie jar is shared).
    /// Servers that close the connection between legs can't complete the handshake.
    async fn execute_ntlm(&self, req: &PreparedRequest, creds: &ntlm::Credentials, observer: Option<&dyn ExecObserver>) -> Result<HttpResponseData, HttpError> {
        let client = self
            .builder(req)?
            .http1_only()
            .pool_max_idle_per_host(1)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| HttpError::Request(error_chain(&e)))?;
        let negotiate = ntlm::header("NTLM", &ntlm::negotiate_message());
        let first = send_once(&client, req, Some(negotiate), observer, Instant::now()).await?;
        if first.status != 401 {
            return Ok(first);
        }
        let offers = first.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("www-authenticate")).map(|(_, v)| v.as_str());
        let Some((scheme, msg)) = ntlm::find_challenge(offers) else { return Ok(first) };
        let challenge = ntlm::parse_challenge(&msg).map_err(HttpError::Request)?;
        let client_challenge = fastrand::u64(..).to_le_bytes();
        let auth = ntlm::authenticate_message(creds, &challenge, client_challenge, ntlm::filetime_now());
        send_once(&client, req, Some(ntlm::header(scheme, &auth)), observer, Instant::now()).await
    }
}

async fn send_once(
    client: &reqwest::Client,
    req: &PreparedRequest,
    authorization: Option<String>,
    observer: Option<&dyn ExecObserver>,
    start: Instant,
) -> Result<HttpResponseData, HttpError> {
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|_| HttpError::InvalidRequest(format!("bad method {}", req.method)))?;
    let url = reqwest::Url::parse(&req.url).map_err(|e| HttpError::InvalidUrl(format!("{} ({e})", req.url)))?;
    let mut rb = client.request(method, url.clone());
    if req.timeout_ms > 0 {
        rb = rb.timeout(Duration::from_millis(req.timeout_ms));
    }
    for (k, v) in &req.headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    if let Some(a) = authorization {
        rb = rb.header("Authorization", a);
    }
    rb = match &req.body {
        PreparedBody::None => rb,
        PreparedBody::Bytes(data) => rb.body(data.clone()),
        PreparedBody::Form(pairs) => {
            let encoded = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs.iter()).finish();
            if !req.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                rb = rb.header("Content-Type", "application/x-www-form-urlencoded");
            }
            rb.body(encoded)
        }
        PreparedBody::Multipart(parts) => {
            let mut form = reqwest::multipart::Form::new();
            for p in parts {
                let mut part = match &p.file {
                    Some(path) => {
                        let bytes = tokio::fs::read(path)
                            .await
                            .map_err(|e| HttpError::InvalidRequest(format!("cannot read {}: {e}", path.display())))?;
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        reqwest::multipart::Part::bytes(bytes).file_name(name)
                    }
                    None => reqwest::multipart::Part::text(p.value.clone()),
                };
                if let Some(ct) = &p.content_type {
                    part = part.mime_str(ct).map_err(|e| HttpError::InvalidRequest(format!("bad content type {ct}: {e}")))?;
                }
                form = form.part(p.name.clone(), part);
            }
            rb.multipart(form)
        }
    };

    let mut resp = rb.send().await.map_err(map_send_error)?;
    let ttfb = start.elapsed();

    let status = resp.status();
    let http_version = format!("{:?}", resp.version());
    let headers: Vec<(String, String)> =
        resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let final_url = resp.url().to_string();
    let remote_addr = resp.remote_addr().map(|a| a.to_string());
    let is_sse = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().starts_with("text/event-stream"));
    let mut sse = is_sse.then(sse::SseParser::default);

    let mut body = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = resp.chunk().await.map_err(|e| HttpError::Body(error_chain(&e)))? {
        if let (Some(p), Some(obs)) = (sse.as_mut(), observer) {
            for ev in p.feed(&chunk) {
                obs.sse_event(&ev);
            }
        }
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            body.extend_from_slice(&chunk[..MAX_BODY_BYTES - body.len()]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
        if let Some(obs) = observer {
            obs.progress(body.len());
        }
    }
    let total = start.elapsed();

    Ok(HttpResponseData {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        http_version,
        headers,
        body_size: body.len(),
        body,
        body_truncated: truncated,
        redirected: final_url != url.as_str(),
        url: final_url,
        remote_addr,
        timings: Timings { ttfb_ms: ms(ttfb), download_ms: ms(total.saturating_sub(ttfb)), total_ms: ms(total) },
    })
}

/// Resolves every `{{template}}` in an OAuth2 config.
pub fn resolve_oauth2(c: &OAuth2Config, r: &mut Resolver) -> OAuth2Config {
    let mut out = c.clone();
    for f in [
        &mut out.auth_url,
        &mut out.token_url,
        &mut out.client_id,
        &mut out.client_secret,
        &mut out.scope,
        &mut out.username,
        &mut out.password,
    ] {
        *f = r.resolve(f);
    }
    out.audience = c.audience.as_ref().map(|a| r.resolve(a));
    out.extra_params = c.extra_params.iter().map(|(k, v)| (k.clone(), r.resolve(v))).collect();
    out
}

/// [`prepare`] plus OAuth 2.0: fetches (or reuses/refreshes) a token for `oauth2` auth first.
pub async fn prepare_authorized(
    req: &HttpRequestFile,
    r: &mut Resolver<'_>,
    project_root: &Path,
    tokens: &dyn oauth2::TokenStore,
    browser: Option<&oauth2::BrowserOpener>,
) -> Result<PreparedRequest, HttpError> {
    let mut opts = PrepareOptions::default();
    if let Auth::OAuth2(cfg) = &req.auth {
        let resolved = resolve_oauth2(cfg, r);
        let token = oauth2::get_token(&resolved, tokens, browser, false).await.map_err(|e| HttpError::InvalidRequest(e.to_string()))?;
        r.secret_values.insert(token.access_token.clone());
        opts.oauth2_token = Some(token.access_token);
    }
    prepare_with(req, r, project_root, &opts)
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1000.0 * 100.0).round() / 100.0
}

/// Full `source()` chain — reqwest's top-level message alone ("error sending request") hides the
/// useful part ("dns error: …", "certificate has expired", …).
pub fn error_chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let msg = s.to_string();
        if !parts.iter().any(|p| p.contains(&msg)) {
            parts.push(msg);
        }
        cur = s.source();
    }
    parts.join(": ")
}

fn map_send_error(e: reqwest::Error) -> HttpError {
    let msg = error_chain(&e);
    if e.is_timeout() {
        HttpError::Timeout(msg)
    } else if e.is_connect() {
        HttpError::Connect(msg)
    } else if e.is_builder() {
        HttpError::InvalidRequest(msg)
    } else {
        HttpError::Request(msg)
    }
}

#[cfg(test)]
mod tests;
