//! OAuth 2.0 token acquisition: client credentials, password, and authorization code with PKCE
//! (the browser redirects to a short-lived callback server on 127.0.0.1). Tokens are cached through
//! a [`TokenStore`] (the app uses the OS keychain) and refreshed automatically.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use devall_core::{OAuth2ClientAuth, OAuth2Config, OAuth2Grant};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Refresh this long before the token actually expires.
const EXPIRY_SKEW_SECS: i64 = 30;
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix seconds; `None` = no expiry given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub token_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

impl Token {
    pub fn is_valid(&self, now: i64) -> bool {
        self.expires_at.is_none_or(|e| e - EXPIRY_SKEW_SECS > now)
    }
}

/// Where tokens are cached. Keys are derived from the config (see [`cache_key`]).
pub trait TokenStore: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
    fn delete(&self, key: &str);
}

/// Opens a URL in the user's browser (authorization-code flow).
pub type BrowserOpener = dyn Fn(&str) -> Result<(), String> + Send + Sync;

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("OAuth2 configuration: {0}")]
    Config(String),
    #[error("token request failed: {0}")]
    Http(String),
    #[error("token endpoint returned {status}: {body}")]
    Endpoint { status: u16, body: String },
    #[error("authorization was not completed: {0}")]
    Authorization(String),
    #[error("interactive sign-in is required (authorization code flow) but no browser is available")]
    NeedsBrowser,
}

/// Stable cache key for a (resolved) config: same endpoint + client + scope + user ⇒ same token.
pub fn cache_key(c: &OAuth2Config) -> String {
    let material = format!("{:?}|{}|{}|{}|{}|{}", c.grant, c.token_url, c.client_id, c.scope, c.audience.as_deref().unwrap_or(""), c.username);
    format!("oauth2:{}", &hex::encode(Sha256::digest(material.as_bytes()))[..24])
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<serde_json::Value>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

/// Returns a valid access token: cached → refreshed → newly acquired.
pub async fn get_token(c: &OAuth2Config, store: &dyn TokenStore, browser: Option<&BrowserOpener>, force: bool) -> Result<Token, OAuthError> {
    if c.token_url.trim().is_empty() {
        return Err(OAuthError::Config("token URL is empty".into()));
    }
    let key = cache_key(c);
    let cached: Option<Token> = store.get(&key).and_then(|s| serde_json::from_str(&s).ok());
    if !force && let Some(t) = &cached {
        if t.is_valid(now()) {
            return Ok(t.clone());
        }
        if let Some(rt) = &t.refresh_token
            && let Ok(fresh) = refresh(c, rt).await
        {
            store.set(&key, &serde_json::to_string(&fresh).unwrap_or_default());
            return Ok(fresh);
        }
    }
    let token = match c.grant {
        OAuth2Grant::ClientCredentials => token_request(c, vec![("grant_type", "client_credentials".into())]).await?,
        OAuth2Grant::Password => {
            token_request(c, vec![("grant_type", "password".into()), ("username", c.username.clone()), ("password", c.password.clone())]).await?
        }
        OAuth2Grant::AuthorizationCode => authorization_code(c, browser.ok_or(OAuthError::NeedsBrowser)?).await?,
    };
    store.set(&key, &serde_json::to_string(&token).unwrap_or_default());
    Ok(token)
}

pub fn clear_token(c: &OAuth2Config, store: &dyn TokenStore) {
    store.delete(&cache_key(c));
}

async fn refresh(c: &OAuth2Config, refresh_token: &str) -> Result<Token, OAuthError> {
    let mut t = token_request(c, vec![("grant_type", "refresh_token".into()), ("refresh_token", refresh_token.to_string())]).await?;
    // Some servers don't rotate refresh tokens; keep the old one.
    if t.refresh_token.is_none() {
        t.refresh_token = Some(refresh_token.to_string());
    }
    Ok(t)
}

async fn token_request(c: &OAuth2Config, mut form: Vec<(&str, String)>) -> Result<Token, OAuthError> {
    if !c.scope.is_empty() && !form.iter().any(|(k, _)| *k == "refresh_token") {
        form.push(("scope", c.scope.clone()));
    }
    if let Some(a) = c.audience.as_ref().filter(|a| !a.is_empty()) {
        form.push(("audience", a.clone()));
    }
    for (k, v) in &c.extra_params {
        form.push((k.as_str(), v.clone()));
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| OAuthError::Http(e.to_string()))?;
    let mut rb = client.post(&c.token_url).header("Accept", "application/json");
    match c.client_auth {
        OAuth2ClientAuth::Basic if !c.client_id.is_empty() => {
            let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
            rb = rb.header("Authorization", format!("Basic {}", STANDARD.encode(format!("{}:{}", enc(&c.client_id), enc(&c.client_secret)))));
        }
        _ => {
            form.push(("client_id", c.client_id.clone()));
            if !c.client_secret.is_empty() {
                form.push(("client_secret", c.client_secret.clone()));
            }
        }
    }
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(form.iter().map(|(k, v)| (*k, v.as_str()))).finish();
    let resp = rb
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| OAuthError::Http(crate::error_chain(&e)))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(|e| OAuthError::Http(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(OAuthError::Endpoint { status, body: text.chars().take(2000).collect() });
    }
    parse_token(&text).ok_or_else(|| OAuthError::Endpoint { status, body: format!("no access_token in response: {}", text.chars().take(500).collect::<String>()) })
}

fn parse_token(text: &str) -> Option<Token> {
    // JSON (RFC 6749) or, from some legacy servers, form-encoded.
    let tr: TokenResponse = serde_json::from_str(text).ok().or_else(|| {
        let pairs: serde_json::Map<String, serde_json::Value> =
            url::form_urlencoded::parse(text.as_bytes()).map(|(k, v)| (k.into_owned(), serde_json::Value::String(v.into_owned()))).collect();
        serde_json::from_value(serde_json::Value::Object(pairs)).ok()
    })?;
    let expires_in = match tr.expires_in {
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.parse().ok(),
        _ => None,
    };
    Some(Token {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token,
        expires_at: expires_in.map(|s| now() + s),
        token_type: tr.token_type.unwrap_or_else(|| "Bearer".into()),
        scope: tr.scope,
    })
}

/// PKCE verifier/challenge pair (RFC 7636, S256).
pub fn pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 32];
    for b in bytes.iter_mut() {
        *b = fastrand::u8(..);
    }
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

async fn authorization_code(c: &OAuth2Config, browser: &BrowserOpener) -> Result<Token, OAuthError> {
    if c.auth_url.trim().is_empty() {
        return Err(OAuthError::Config("authorization URL is empty".into()));
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", c.redirect_port.unwrap_or(0)))
        .await
        .map_err(|e| OAuthError::Authorization(format!("cannot listen for the callback: {e}")))?;
    let port = listener.local_addr().map_err(|e| OAuthError::Authorization(e.to_string()))?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let state = format!("{:016x}", fastrand::u64(..));
    let (verifier, challenge) = pkce_pair();

    let mut auth_url = url::Url::parse(&c.auth_url).map_err(|e| OAuthError::Config(format!("authorization URL: {e}")))?;
    {
        let mut q = auth_url.query_pairs_mut();
        q.append_pair("response_type", "code").append_pair("client_id", &c.client_id).append_pair("redirect_uri", &redirect_uri).append_pair("state", &state);
        if !c.scope.is_empty() {
            q.append_pair("scope", &c.scope);
        }
        if let Some(a) = c.audience.as_ref().filter(|a| !a.is_empty()) {
            q.append_pair("audience", a);
        }
        if c.use_pkce {
            q.append_pair("code_challenge", &challenge).append_pair("code_challenge_method", "S256");
        }
        for (k, v) in &c.extra_params {
            q.append_pair(k, v);
        }
    }
    browser(auth_url.as_str()).map_err(OAuthError::Authorization)?;

    let code = tokio::time::timeout(CALLBACK_TIMEOUT, wait_for_code(&listener, &state))
        .await
        .map_err(|_| OAuthError::Authorization("timed out waiting for the browser redirect".into()))??;

    let mut form = vec![("grant_type", "authorization_code".to_string()), ("code", code), ("redirect_uri", redirect_uri)];
    if c.use_pkce {
        form.push(("code_verifier", verifier));
    }
    // `scope` is not sent on the code exchange.
    let mut cfg = c.clone();
    cfg.scope.clear();
    token_request(&cfg, form).await
}

async fn wait_for_code(listener: &tokio::net::TcpListener, state: &str) -> Result<String, OAuthError> {
    loop {
        let (mut sock, _) = listener.accept().await.map_err(|e| OAuthError::Authorization(e.to_string()))?;
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]).to_string();
        let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
        let parsed = url::Url::parse(&format!("http://localhost{target}")).ok();
        let get = |k: &str| parsed.as_ref().and_then(|u| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()));
        if !target.starts_with("/callback") {
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        }
        let (ok, message) = if let Some(err) = get("error") {
            (false, format!("{err}: {}", get("error_description").unwrap_or_default()))
        } else if get("state").as_deref() != Some(state) {
            (false, "state mismatch (possible CSRF) — try again".to_string())
        } else if get("code").is_none() {
            (false, "no authorization code in the redirect".to_string())
        } else {
            (true, "Signed in. You can close this tab and return to /dev/all.".to_string())
        };
        let html = format!(
            "<!doctype html><meta charset=utf-8><title>/dev/all</title><body style=\"font-family:system-ui;padding:40px\"><h2>{}</h2><p>{}</p></body>",
            if ok { "✓ Authorization complete" } else { "Authorization failed" },
            html_escape(&message)
        );
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len());
        let _ = sock.write_all(resp.as_bytes()).await;
        let _ = sock.shutdown().await;
        return if ok { Ok(get("code").unwrap_or_default()) } else { Err(OAuthError::Authorization(message)) };
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// In-memory [`TokenStore`] (CLI, tests).
#[derive(Default)]
pub struct MemoryTokenStore(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl TokenStore for MemoryTokenStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) {
        self.0.lock().unwrap().insert(key.into(), value.into());
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Token endpoint that answers every POST with a token whose value counts requests and echoes
    /// the grant type, so tests can see caching/refresh behaviour.
    async fn token_server(expires_in: i64) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h2 = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let n = h2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let len = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..len]).to_string();
                    let grant = req.split("grant_type=").nth(1).and_then(|r| r.split('&').next()).unwrap_or("?").to_string();
                    let auth_ok = req.to_lowercase().contains("authorization: basic");
                    let body = format!(r#"{{"access_token":"tok-{n}-{grant}-{auth_ok}","refresh_token":"r{n}","expires_in":{expires_in},"token_type":"Bearer"}}"#);
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}/token"), hits)
    }

    fn cfg(token_url: &str) -> OAuth2Config {
        OAuth2Config { token_url: token_url.into(), client_id: "cid".into(), client_secret: "sec".into(), scope: "read".into(), ..Default::default() }
    }

    #[tokio::test]
    async fn client_credentials_cached_until_expiry() {
        let (url, hits) = token_server(3600).await;
        let store = MemoryTokenStore::default();
        let t1 = get_token(&cfg(&url), &store, None, false).await.unwrap();
        assert_eq!(t1.access_token, "tok-1-client_credentials-true");
        let t2 = get_token(&cfg(&url), &store, None, false).await.unwrap();
        assert_eq!(t1, t2);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        let t3 = get_token(&cfg(&url), &store, None, true).await.unwrap();
        assert_ne!(t3.access_token, t1.access_token);
    }

    #[tokio::test]
    async fn expired_token_is_refreshed_with_body_client_auth() {
        let (url, _) = token_server(0).await;
        let store = MemoryTokenStore::default();
        let mut c = cfg(&url);
        c.client_auth = OAuth2ClientAuth::Body;
        let t1 = get_token(&c, &store, None, false).await.unwrap();
        assert!(t1.access_token.ends_with("client_credentials-false"));
        let t2 = get_token(&c, &store, None, false).await.unwrap();
        assert!(t2.access_token.contains("refresh_token"), "{}", t2.access_token);
    }

    #[tokio::test]
    async fn authorization_code_with_pkce_via_simulated_browser() {
        let (url, _) = token_server(3600).await;
        let store = MemoryTokenStore::default();
        let mut c = cfg(&url);
        c.grant = OAuth2Grant::AuthorizationCode;
        c.auth_url = "https://idp.example.com/authorize".into();
        // The "browser" follows the redirect immediately with the state it was given.
        let browser = |auth: &str| -> Result<(), String> {
            let u = url::Url::parse(auth).unwrap();
            let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
            assert_eq!(q["code_challenge_method"], "S256");
            let redirect = format!("{}?code=abc&state={}", q["redirect_uri"], q["state"]);
            tokio::spawn(async move {
                let _ = reqwest::get(redirect).await;
            });
            Ok(())
        };
        let t = get_token(&c, &store, Some(&browser), false).await.unwrap();
        assert!(t.access_token.contains("authorization_code"));
        // Without a browser the flow is refused clearly.
        let other = MemoryTokenStore::default();
        assert!(matches!(get_token(&c, &other, None, false).await, Err(OAuthError::NeedsBrowser)));
    }

    #[test]
    fn pkce_and_parsing() {
        let (v, ch) = pkce_pair();
        assert!(v.len() >= 43 && ch.len() == 43);
        let t = parse_token("access_token=a&expires_in=60&token_type=bearer").unwrap();
        assert_eq!(t.access_token, "a");
        assert!(t.expires_at.is_some());
        assert_ne!(cache_key(&cfg("x")), cache_key(&cfg("y")));
    }
}
