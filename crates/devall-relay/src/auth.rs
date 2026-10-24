//! Credentials for the relay.
//!
//! - Hosted relay: a Firebase **refresh token** obtained by the loopback sign-in
//!   ([`crate::signin`]) lives only in the OS keychain (`devall_core::SecretStore`); it is
//!   exchanged for short-lived ID tokens at Google's securetoken endpoint when needed.
//! - Self-hosted relay: a static token, also kept in the keychain.
//!
//! Nothing here writes tokens to disk in plain text.

use std::sync::Arc;
use std::time::{Duration, Instant};

use devall_core::{SecretScope, SecretStore};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::{RelayError, Result, chain};

/// Keychain scope for relay credentials (`devall-relay/*/<name>`).
pub fn secret_scope() -> SecretScope {
    SecretScope::new("devall-relay", devall_core::secrets::ALL_ENVIRONMENTS)
}

pub const REFRESH_TOKEN_KEY: &str = "firebase-refresh-token";
pub const STATIC_TOKEN_KEY: &str = "static-token";

pub fn load_refresh_token(secrets: &dyn SecretStore) -> Result<Option<String>> {
    Ok(secrets.get(&secret_scope(), REFRESH_TOKEN_KEY)?.filter(|s| !s.is_empty()))
}

pub fn store_refresh_token(secrets: &dyn SecretStore, token: &str) -> Result<()> {
    secrets.set(&secret_scope(), REFRESH_TOKEN_KEY, token)?;
    Ok(())
}

pub fn load_static_token(secrets: &dyn SecretStore) -> Result<Option<String>> {
    Ok(secrets.get(&secret_scope(), STATIC_TOKEN_KEY)?.filter(|s| !s.is_empty()))
}

pub fn store_static_token(secrets: &dyn SecretStore, token: &str) -> Result<()> {
    secrets.set(&secret_scope(), STATIC_TOKEN_KEY, token)?;
    Ok(())
}

/// Removes every relay credential from the keychain (sign out).
pub fn clear_credentials(secrets: &dyn SecretStore) -> Result<()> {
    secrets.delete(&secret_scope(), REFRESH_TOKEN_KEY)?;
    Ok(())
}

/// How the client authenticates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayAuth {
    /// Hosted relay: Firebase refresh token from the keychain, exchanged at `token_url?key=api_key`.
    FirebaseRefreshToken { api_key: String, token_url: String },
    /// Self-hosted relay: this static token (`None` = read it from the keychain).
    StaticToken(Option<String>),
}

impl RelayAuth {
    /// Hosted defaults ([`crate::hosted`]).
    pub fn hosted() -> Self {
        RelayAuth::FirebaseRefreshToken { api_key: crate::hosted::FIREBASE_API_KEY.into(), token_url: crate::hosted::SECURETOKEN_URL.into() }
    }
}

/// Response of `POST https://securetoken.googleapis.com/v1/token?key=…`.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub id_token: String,
    pub refresh_token: String,
    /// Seconds, as a string.
    pub expires_in: String,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Deserialize)]
struct GoogleError {
    error: GoogleErrorBody,
}

#[derive(Deserialize)]
struct GoogleErrorBody {
    #[serde(default)]
    message: String,
}

/// Exchanges a refresh token for a fresh ID token (and possibly a rotated refresh token).
pub async fn exchange_refresh_token(http: &reqwest::Client, token_url: &str, api_key: &str, refresh_token: &str) -> Result<TokenResponse> {
    let mut url = url::Url::parse(token_url).map_err(|e| RelayError::Config(format!("token url: {e}")))?;
    url.query_pairs_mut().append_pair("key", api_key);
    let body = url::form_urlencoded::Serializer::new(String::new()).append_pair("grant_type", "refresh_token").append_pair("refresh_token", refresh_token).finish();
    let res = http
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| RelayError::Http(format!("token refresh: {}", chain(&e))))?;
    let status = res.status();
    let text = res.text().await.map_err(|e| RelayError::Http(chain(&e)))?;
    if !status.is_success() {
        let msg = serde_json::from_str::<GoogleError>(&text).map(|e| e.error.message).unwrap_or_else(|_| text.chars().take(200).collect());
        // 400/401/403 mean the refresh token is no longer valid (revoked, user disabled, …).
        if status.is_client_error() {
            return Err(RelayError::Auth(format!("sign-in expired ({msg}); sign in again")));
        }
        return Err(RelayError::Http(format!("token refresh failed ({status}): {msg}")));
    }
    serde_json::from_str(&text).map_err(|e| RelayError::Http(format!("token refresh: unexpected response: {e}")))
}

#[derive(Clone)]
struct Cached {
    token: String,
    valid_until: Instant,
    uid: Option<String>,
}

/// Hands out bearer tokens for relay requests, refreshing Firebase ID tokens before they expire.
pub struct TokenProvider {
    auth: RelayAuth,
    secrets: Arc<dyn SecretStore>,
    http: reqwest::Client,
    cache: Mutex<Option<Cached>>,
}

impl std::fmt::Debug for TokenProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenProvider").field("auth", &self.auth_kind()).finish()
    }
}

impl TokenProvider {
    pub fn new(auth: RelayAuth, secrets: Arc<dyn SecretStore>, http: reqwest::Client) -> Self {
        Self { auth, secrets, http, cache: Mutex::new(None) }
    }

    pub fn auth(&self) -> &RelayAuth {
        &self.auth
    }

    fn auth_kind(&self) -> &'static str {
        match self.auth {
            RelayAuth::FirebaseRefreshToken { .. } => "firebase",
            RelayAuth::StaticToken(_) => "static",
        }
    }

    /// Whether credentials exist (signed in / static token configured).
    pub fn has_credentials(&self) -> bool {
        match &self.auth {
            RelayAuth::FirebaseRefreshToken { .. } => matches!(load_refresh_token(self.secrets.as_ref()), Ok(Some(_))),
            RelayAuth::StaticToken(Some(t)) => !t.is_empty(),
            RelayAuth::StaticToken(None) => matches!(load_static_token(self.secrets.as_ref()), Ok(Some(_))),
        }
    }

    /// A bearer token for the next request.
    pub async fn bearer(&self) -> Result<String> {
        match &self.auth {
            RelayAuth::StaticToken(Some(t)) if !t.is_empty() => Ok(t.clone()),
            RelayAuth::StaticToken(_) => load_static_token(self.secrets.as_ref())?.ok_or(RelayError::NotSignedIn),
            RelayAuth::FirebaseRefreshToken { api_key, token_url } => {
                let mut cache = self.cache.lock().await;
                if let Some(c) = cache.as_ref()
                    && c.valid_until > Instant::now()
                {
                    return Ok(c.token.clone());
                }
                let refresh = load_refresh_token(self.secrets.as_ref())?.ok_or(RelayError::NotSignedIn)?;
                let res = exchange_refresh_token(&self.http, token_url, api_key, &refresh).await?;
                if res.refresh_token != refresh && !res.refresh_token.is_empty() {
                    store_refresh_token(self.secrets.as_ref(), &res.refresh_token)?;
                }
                let secs = res.expires_in.parse::<u64>().unwrap_or(3600);
                // Refresh five minutes early so a token never expires mid-handshake.
                let valid_for = Duration::from_secs(secs.saturating_sub(300).max(30));
                *cache = Some(Cached { token: res.id_token.clone(), valid_until: Instant::now() + valid_for, uid: res.user_id.clone() });
                Ok(res.id_token)
            }
        }
    }

    /// Firebase uid from the last refresh, if any.
    pub async fn uid(&self) -> Option<String> {
        self.cache.lock().await.as_ref().and_then(|c| c.uid.clone())
    }

    /// Drops the cached ID token (after the relay answered 401).
    pub async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devall_core::MemorySecretStore;

    #[test]
    fn keychain_helpers_round_trip() {
        let s = MemorySecretStore::default();
        assert_eq!(load_refresh_token(&s).unwrap(), None);
        store_refresh_token(&s, "r1").unwrap();
        assert_eq!(load_refresh_token(&s).unwrap().as_deref(), Some("r1"));
        store_static_token(&s, "t1").unwrap();
        assert_eq!(load_static_token(&s).unwrap().as_deref(), Some("t1"));
        clear_credentials(&s).unwrap();
        assert_eq!(load_refresh_token(&s).unwrap(), None);
        assert_eq!(secret_scope().account(REFRESH_TOKEN_KEY), "devall-relay/*/firebase-refresh-token");
    }

    #[tokio::test]
    async fn static_tokens_need_no_network() {
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let p = TokenProvider::new(RelayAuth::StaticToken(Some("abc".into())), secrets.clone(), reqwest::Client::new());
        assert_eq!(p.bearer().await.unwrap(), "abc");
        let from_keychain = TokenProvider::new(RelayAuth::StaticToken(None), secrets.clone(), reqwest::Client::new());
        assert!(matches!(from_keychain.bearer().await, Err(RelayError::NotSignedIn)));
        store_static_token(secrets.as_ref(), "kc").unwrap();
        assert_eq!(from_keychain.bearer().await.unwrap(), "kc");
        assert!(from_keychain.has_credentials());
    }

    #[tokio::test]
    async fn firebase_without_refresh_token_is_not_signed_in() {
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let p = TokenProvider::new(RelayAuth::FirebaseRefreshToken { api_key: "k".into(), token_url: "http://127.0.0.1:9/token".into() }, secrets, reqwest::Client::new());
        assert!(!p.has_credentials());
        assert!(matches!(p.bearer().await, Err(RelayError::NotSignedIn)));
    }
}
