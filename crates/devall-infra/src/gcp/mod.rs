//! Google Cloud, emulator-first: Pub/Sub, Firestore/Datastore, Cloud Storage (fake-gcs-server
//! or GCS), Cloud Logging tail, Secret Manager (real projects only; import into the keychain). Plain REST with reqwest.
//!
//! Real-cloud calls use the user's own `gcloud` login (`gcloud auth print-access-token`) and are
//! read-only unless the caller sets [`GcpTarget::allow_writes`]. Emulators need no credentials.

pub mod datastore;
pub mod firestore;
pub mod logging;
pub mod pubsub;
pub mod secretmanager;
pub mod storage;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use reqwest::Method;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::sys;

const TOKEN_TTL: Duration = Duration::from_secs(45 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Where a call goes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GcpTarget {
    pub project: String,
    /// Emulator address (`localhost:8085`, `http://localhost:4443`). Empty = the real Google
    /// Cloud API with the user's gcloud credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulator: Option<String>,
    /// Allow writes (create/delete/publish/pull+ack/upload) against a real project. Emulators
    /// always allow writes.
    #[serde(default)]
    pub allow_writes: bool,
}

impl GcpTarget {
    pub fn emulator(project: &str, host: &str) -> Self {
        Self { project: project.into(), emulator: Some(host.into()), allow_writes: true }
    }

    pub fn is_emulator(&self) -> bool {
        self.emulator.as_deref().is_some_and(|h| !h.trim().is_empty())
    }

    /// API base URL: the emulator, or `cloud` for the real service.
    pub fn base(&self, cloud: &str) -> String {
        match self.emulator.as_deref().filter(|h| !h.trim().is_empty()) {
            Some(h) => emulator_base(h),
            None => cloud.to_string(),
        }
    }

    pub fn check_write(&self, what: &str) -> Result<()> {
        if self.is_emulator() || self.allow_writes {
            return Ok(());
        }
        Err(Error::ReadOnly(format!(
            "{what} is a write against the real project \"{}\"; real-cloud access is read-only until you enable writes for it",
            self.project
        )))
    }

    fn check_project(&self) -> Result<&str> {
        let p = self.project.trim();
        if p.is_empty() || p.contains('/') {
            return Err(Error::Invalid("a GCP project id is required".into()));
        }
        Ok(p)
    }
}

/// `localhost:8085` → `http://localhost:8085` (scheme kept when given, trailing `/` dropped).
pub fn emulator_base(host: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    if h.starts_with("http://") || h.starts_with("https://") { h.to_string() } else { format!("http://{h}") }
}

/// Characters escaped in a URL path segment (`/` included: object names may contain it).
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// One path segment, percent-encoded.
pub fn seg(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, SEGMENT).to_string()
}

/// Everything but RFC 3986 unreserved characters.
const QUERY: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// A query-string value, percent-encoded.
pub fn q(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, QUERY).to_string()
}

/// The last `/`-separated part: `projects/p/topics/orders` → `orders`.
pub fn short_name(full: &str) -> String {
    full.rsplit('/').next().unwrap_or(full).to_string()
}

/// Cheap to clone (clones share connections and the cached token).
#[derive(Clone)]
pub struct GcpClient {
    http: reqwest::Client,
    /// Emulators often serve self-signed TLS (fake-gcs-server); only used for emulator targets.
    insecure: reqwest::Client,
    token: std::sync::Arc<tokio::sync::Mutex<Option<(String, Instant)>>>,
}

pub(crate) struct Call<'a> {
    pub method: Method,
    pub url: String,
    pub body: Option<Value>,
    pub target: &'a GcpTarget,
    /// Authorization for emulators (the Firestore emulator accepts `Bearer owner` to skip rules).
    pub emulator_bearer: Option<&'static str>,
}

impl GcpClient {
    pub fn new() -> Result<Self> {
        crate::tls::ensure_crypto_provider();
        let http = reqwest::Client::builder().timeout(REQUEST_TIMEOUT).connect_timeout(Duration::from_secs(10)).build()?;
        let insecure = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(5))
            .tls_danger_accept_invalid_certs(true)
            .no_proxy()
            .build()?;
        Ok(Self { http, insecure, token: Default::default() })
    }

    pub(crate) fn client_for(&self, target: &GcpTarget) -> &reqwest::Client {
        if target.is_emulator() { &self.insecure } else { &self.http }
    }

    /// An OAuth access token from the user's gcloud login, cached for 45 minutes.
    pub async fn access_token(&self) -> Result<String> {
        let mut guard = self.token.lock().await;
        if let Some((tok, at)) = guard.as_ref()
            && at.elapsed() < TOKEN_TTL
        {
            return Ok(tok.clone());
        }
        let gcloud = gcloud_path()?;
        let out = sys::run_capture(&gcloud, &["auth".into(), "print-access-token".into()], None, &[], Duration::from_secs(60)).await?;
        let tok = out.stdout.trim().to_string();
        if !out.success || tok.is_empty() {
            return Err(Error::AuthPlugin(format!(
                "gcloud could not print an access token: {}\nRun `gcloud auth login` in a terminal, then retry.",
                out.stderr.trim()
            )));
        }
        *guard = Some((tok.clone(), Instant::now()));
        Ok(tok)
    }

    pub async fn forget_token(&self) {
        *self.token.lock().await = None;
    }

    /// Sends a JSON request and returns the JSON response (`Null` for empty bodies). Real-cloud
    /// calls get the gcloud token and one retry after a 401.
    pub(crate) async fn call(&self, c: Call<'_>) -> Result<Value> {
        let emulator = c.target.is_emulator();
        for attempt in 0..2 {
            let mut rb = self.client_for(c.target).request(c.method.clone(), &c.url);
            if emulator {
                if let Some(b) = c.emulator_bearer {
                    rb = rb.bearer_auth(b);
                }
            } else {
                rb = rb.bearer_auth(self.access_token().await?);
            }
            if let Some(body) = &c.body {
                rb = rb.header(CONTENT_TYPE, "application/json").body(serde_json::to_vec(body)?);
            }
            let resp = rb.send().await.map_err(|e| connect_error(e, c.target))?;
            let status = resp.status();
            if status.as_u16() == 401 && !emulator && attempt == 0 {
                self.forget_token().await;
                continue;
            }
            let bytes = resp.bytes().await?;
            if !status.is_success() {
                return Err(api_error(status.as_u16(), &bytes));
            }
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return Ok(Value::Null);
            }
            return serde_json::from_slice(&bytes).map_err(|e| Error::Request(format!("unexpected response from {}: {e}", c.url)));
        }
        Err(Error::Http { status: 401, message: "unauthorized (after refreshing the gcloud token)".into() })
    }

    /// Adds auth to a raw request (downloads/uploads).
    pub(crate) async fn authorize(&self, rb: reqwest::RequestBuilder, target: &GcpTarget) -> Result<reqwest::RequestBuilder> {
        if target.is_emulator() { Ok(rb) } else { Ok(rb.bearer_auth(self.access_token().await?)) }
    }

    /// Emulator reachability + gcloud login state. Only local calls.
    pub async fn status(&self) -> GcpStatus {
        let probes = emulator_defaults().into_iter().map(|(kind, host, from_env)| async move {
            let reachable = self.probe(kind, &host).await;
            EmulatorStatus { kind: kind.into(), host, from_env, reachable }
        });
        let (emulators, gcloud) = tokio::join!(futures::future::join_all(probes), gcloud_config());
        GcpStatus { emulators, gcloud }
    }

    async fn probe(&self, kind: &str, host: &str) -> bool {
        let base = emulator_base(host);
        let url = if kind == "storage" { format!("{base}/storage/v1/b") } else { format!("{base}/") };
        let Ok(Ok(resp)) = tokio::time::timeout(Duration::from_millis(1200), self.insecure.get(&url).send()).await else { return false };
        let ok = resp.status().is_success();
        let body = resp.text().await.unwrap_or_default();
        match kind {
            // The Google emulators answer `Ok` on `/`; anything else is probably the user's app.
            "pubsub" | "firestore" | "datastore" => ok && body.trim() == "Ok",
            _ => ok && body.contains("storage#"),
        }
    }
}

fn connect_error(e: reqwest::Error, target: &GcpTarget) -> Error {
    if e.is_connect() && target.is_emulator() {
        return Error::Request(format!("the emulator at {} is not reachable: {}", target.base(""), crate::error::chain(&e)));
    }
    e.into()
}

/// `{"error":{"code":403,"message":"…","status":"PERMISSION_DENIED"}}` → readable error.
pub fn api_error(status: u16, body: &[u8]) -> Error {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(&body[..body.len().min(2000)]).trim().to_string());
    let status_text = parsed.as_ref().and_then(|v| v.pointer("/error/status").and_then(Value::as_str)).map(|s| format!(" {s}")).unwrap_or_default();
    Error::Http { status, message: format!("{}{status_text}", if message.is_empty() { "(no body)".to_string() } else { message }).trim().to_string() }
}

pub fn gcloud_path() -> Result<PathBuf> {
    sys::which("gcloud").ok_or_else(|| {
        Error::ToolMissing("the gcloud CLI was not found. Install the Google Cloud CLI (https://cloud.google.com/sdk/docs/install), run `gcloud auth login`, then reopen /dev/all.".into())
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmulatorStatus {
    /// `pubsub`, `firestore`, `datastore`, `storage`.
    pub kind: String,
    pub host: String,
    /// The host came from `*_EMULATOR_HOST` (else the emulator's default port).
    pub from_env: bool,
    pub reachable: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GcloudInfo {
    pub path: Option<String>,
    pub account: Option<String>,
    pub project: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GcpStatus {
    pub emulators: Vec<EmulatorStatus>,
    pub gcloud: GcloudInfo,
}

/// `(kind, host, from_env)` for each emulator: `*_EMULATOR_HOST`, else the default port.
pub fn emulator_defaults() -> Vec<(&'static str, String, bool)> {
    [
        ("pubsub", "PUBSUB_EMULATOR_HOST", "localhost:8085"),
        ("firestore", "FIRESTORE_EMULATOR_HOST", "localhost:8080"),
        ("datastore", "DATASTORE_EMULATOR_HOST", "localhost:8081"),
        ("storage", "STORAGE_EMULATOR_HOST", "http://localhost:4443"),
    ]
    .into_iter()
    .map(|(kind, var, default)| match std::env::var(var).ok().filter(|v| !v.trim().is_empty()) {
        Some(v) => (kind, v, true),
        None => (kind, default.to_string(), false),
    })
    .collect()
}

/// Active gcloud account/project (local config only, no network).
pub async fn gcloud_config() -> GcloudInfo {
    let Ok(path) = gcloud_path() else { return GcloudInfo::default() };
    let args: Vec<String> = ["config", "list", "--format=json"].iter().map(|s| s.to_string()).collect();
    let mut info = GcloudInfo { path: Some(path.display().to_string()), ..Default::default() };
    if let Ok(out) = sys::run_capture(&path, &args, None, &[], Duration::from_secs(20)).await
        && let Ok(v) = serde_json::from_str::<Value>(&out.stdout)
    {
        info.account = v.pointer("/core/account").and_then(Value::as_str).map(str::to_string);
        info.project = v.pointer("/core/project").and_then(Value::as_str).map(str::to_string);
    }
    info
}

#[cfg(test)]
pub(crate) mod testserver {
    //! A tiny HTTP/1.1 server on 127.0.0.1:0 that records requests and replies from a routing fn.

    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Debug, Clone)]
    pub struct Recorded {
        pub method: String,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Recorded {
        pub fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
        }
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        }
    }

    pub type Router = Arc<dyn Fn(&Recorded) -> (u16, String) + Send + Sync>;

    pub async fn start(router: Router) -> (String, Arc<Mutex<Vec<Recorded>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let log2 = log.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let router = router.clone();
                let log = log2.clone();
                tokio::spawn(async move {
                    loop {
                        let mut buf = Vec::new();
                        let mut tmp = [0u8; 4096];
                        let head_end = loop {
                            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break i + 4;
                            }
                            match sock.read(&mut tmp).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                        let mut lines = head.split("\r\n");
                        let first = lines.next().unwrap_or("");
                        let mut parts = first.split(' ');
                        let method = parts.next().unwrap_or("").to_string();
                        let path = parts.next().unwrap_or("").to_string();
                        let headers: Vec<(String, String)> =
                            lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
                        let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
                        let mut body = buf[head_end..].to_vec();
                        while body.len() < len {
                            match sock.read(&mut tmp).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => body.extend_from_slice(&tmp[..n]),
                            }
                        }
                        let rec = Recorded { method, path, headers, body };
                        let (status, reply) = router(&rec);
                        log.lock().unwrap().push(rec);
                        let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}", reply.len());
                        if sock.write_all(resp.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (format!("127.0.0.1:{}", addr.port()), log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_and_names() {
        assert_eq!(emulator_base("localhost:8085"), "http://localhost:8085");
        assert_eq!(emulator_base("https://localhost:4443/"), "https://localhost:4443");
        let cloud = GcpTarget { project: "shop-prod".into(), emulator: None, allow_writes: false };
        assert_eq!(cloud.base("https://pubsub.googleapis.com"), "https://pubsub.googleapis.com");
        let err = cloud.check_write("publishing").unwrap_err().to_string();
        assert!(err.contains("read-only") && err.contains("shop-prod"), "{err}");
        assert!(GcpTarget { allow_writes: true, ..cloud.clone() }.check_write("x").is_ok());
        let emu = GcpTarget::emulator("demo", "localhost:8085");
        assert!(emu.is_emulator() && emu.check_write("x").is_ok());
        assert!(!GcpTarget { emulator: Some(" ".into()), ..cloud }.is_emulator());
        assert_eq!(short_name("projects/p/topics/orders"), "orders");
        assert_eq!(seg("a/b c"), "a%2Fb%20c");
        assert_eq!(seg("(default)"), "(default)");
        assert_eq!(q("a+b/c="), "a%2Bb%2Fc%3D");
        assert_eq!(q("in/data.json"), "in%2Fdata.json");
    }

    #[test]
    fn api_errors_are_readable() {
        let e = api_error(403, br#"{"error":{"code":403,"message":"Permission denied on topic","status":"PERMISSION_DENIED"}}"#).to_string();
        assert_eq!(e, "HTTP 403: Permission denied on topic PERMISSION_DENIED");
        assert_eq!(api_error(500, b"boom").to_string(), "HTTP 500: boom");
    }
}
