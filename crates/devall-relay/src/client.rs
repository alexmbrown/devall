//! [`RelayClient`]: one configured relay (hosted or self-hosted) and its credentials.

use std::sync::Arc;
use std::time::Duration;

use devall_core::SecretStore;
use serde_json::Value;
use url::Url;

use crate::account::{self, AccountInfo};
use crate::api::{self, MockPublished};
use crate::hosted_mock::MockPayload;
use crate::tcp::{self, TcpConnectHandle, TcpConnectOptions, TcpTunnelHandle, TcpTunnelOptions};
use crate::auth::{self, RelayAuth, TokenProvider};
use crate::error::{RelayError, Result};
use crate::inbox::{self, InboxHandle, InboxHandler, InboxOptions};
use crate::session::Connector;
use crate::signin::{self, SignedIn};
use crate::tunnel::{self, TunnelHandle, TunnelOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayConfig {
    /// `wss://relay.devall.dev` (hosted) or a self-hosted relay (`wss://relay.example.com`).
    pub endpoint: String,
    pub auth: RelayAuth,
    /// Verify the relay's TLS certificate against the OS trust store (always on outside tests).
    pub verify_tls: bool,
    pub connect_timeout: Duration,
    /// Cloud Functions base for the hosted callables (custom domains); `None` on self-hosted relays.
    pub functions_url: Option<String>,
}

impl RelayConfig {
    /// The hosted relay ([`crate::hosted`]).
    pub fn hosted() -> Self {
        Self {
            endpoint: crate::hosted::HOSTED_ENDPOINT.into(),
            auth: RelayAuth::hosted(),
            verify_tls: true,
            connect_timeout: Duration::from_secs(15),
            functions_url: Some(crate::hosted::FUNCTIONS_URL.into()),
        }
    }

    /// A self-hosted relay with a static token (`None` = read it from the keychain).
    pub fn self_hosted(endpoint: impl Into<String>, token: Option<String>) -> Self {
        Self { endpoint: endpoint.into(), auth: RelayAuth::StaticToken(token), verify_tls: true, connect_timeout: Duration::from_secs(15), functions_url: None }
    }
}

/// Normalizes a user-entered relay address to a `ws(s)://` URL (`https://` → `wss://`, bare host → `wss://host`).
pub fn normalize_endpoint(input: &str) -> Result<Url> {
    let s = input.trim();
    if s.is_empty() {
        return Err(RelayError::Config("relay address is empty".into()));
    }
    let with_scheme = if s.contains("://") { s.to_string() } else { format!("wss://{s}") };
    let mut u = Url::parse(&with_scheme).map_err(|e| RelayError::Config(format!("relay address: {e}")))?;
    let scheme = match u.scheme() {
        "wss" | "https" => "wss",
        "ws" | "http" => "ws",
        other => return Err(RelayError::Config(format!("relay address must be wss:// or https:// (got {other}://)"))),
    };
    u.set_scheme(scheme).map_err(|_| RelayError::Config("invalid relay address".into()))?;
    if u.host_str().is_none() {
        return Err(RelayError::Config("relay address has no host".into()));
    }
    u.set_query(None);
    u.set_fragment(None);
    Ok(u)
}

pub struct RelayClient {
    config: RelayConfig,
    connector: Arc<Connector>,
    tokens: Arc<TokenProvider>,
    secrets: Arc<dyn SecretStore>,
    http: reqwest::Client,
}

impl std::fmt::Debug for RelayClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayClient").field("endpoint", &self.config.endpoint).finish()
    }
}

impl RelayClient {
    pub fn new(config: RelayConfig, secrets: Arc<dyn SecretStore>) -> Result<Self> {
        let endpoint = normalize_endpoint(&config.endpoint)?;
        let http = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(Duration::from_secs(30))
            .danger_accept_invalid_certs(!config.verify_tls)
            .build()
            .map_err(|e| RelayError::Config(e.to_string()))?;
        let tokens = Arc::new(TokenProvider::new(config.auth.clone(), secrets.clone(), http.clone()));
        let connector = Arc::new(Connector { endpoint, tokens: tokens.clone(), verify_tls: config.verify_tls, connect_timeout: config.connect_timeout });
        Ok(Self { config, connector, tokens, secrets, http })
    }

    pub fn config(&self) -> &RelayConfig {
        &self.config
    }

    pub fn endpoint(&self) -> &Url {
        &self.connector.endpoint
    }

    /// Whether credentials are available (signed in, or a static token is set).
    pub fn is_signed_in(&self) -> bool {
        self.tokens.has_credentials()
    }

    /// A bearer token for the relay (refreshing the Firebase ID token if needed).
    pub async fn bearer(&self) -> Result<String> {
        self.tokens.bearer().await
    }

    pub async fn account(&self) -> Result<AccountInfo> {
        account::fetch(&self.http, &self.connector.endpoint, &self.tokens).await
    }

    /// Starts a tunnel; it reconnects on its own until stopped.
    pub fn start_tunnel(&self, opts: TunnelOptions) -> TunnelHandle {
        tunnel::spawn(self.connector.clone(), opts)
    }

    /// Publishes a raw TCP tunnel (Supporter / self-hosted); reconnects until stopped.
    pub fn start_tcp_tunnel(&self, opts: TcpTunnelOptions) -> TcpTunnelHandle {
        tcp::spawn_publisher(self.connector.clone(), opts)
    }

    /// Listens on `opts.bind` and forwards each connection through the TCP tunnel `opts.name`.
    pub async fn tcp_connect(&self, opts: TcpConnectOptions) -> Result<TcpConnectHandle> {
        tcp::spawn_consumer(self.connector.clone(), opts).await
    }

    /// Calls the relay's JSON API (`/v1/mocks…`, `/v1/shares…`, `/v1/monitors…`, `/v1/domains`).
    pub async fn api(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value> {
        api::request_json(&self.http, &self.connector.endpoint, &self.tokens, method, path, body, None).await
    }

    /// [`RelayClient::api`] with its own timeout (cloud load runs last as long as the test).
    pub async fn api_with_timeout(&self, method: reqwest::Method, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value> {
        api::request_json(&self.http, &self.connector.endpoint, &self.tokens, method, path, body, Some(timeout)).await
    }

    /// `PUT /v1/mocks/<name>`: publishes (or replaces) a hosted mock.
    pub async fn publish_mock(&self, name: &str, payload: &MockPayload) -> Result<MockPublished> {
        crate::hosted_mock::check_name(name)?;
        let body = serde_json::to_value(payload).map_err(|e| RelayError::Config(e.to_string()))?;
        let v = self.api(reqwest::Method::PUT, &format!("/v1/mocks/{name}"), Some(&body)).await?;
        serde_json::from_value(v).map_err(|e| RelayError::Http(format!("unexpected publish response: {e}")))
    }

    /// A hosted Firebase callable (custom domains). Only on the hosted relay.
    pub async fn function(&self, name: &str, data: &Value) -> Result<Value> {
        let base = self.config.functions_url.as_deref().ok_or_else(|| RelayError::Config("custom domains on a self-hosted relay are set with CUSTOM_HOSTS (relay/README.md)".into()))?;
        api::call_function(&self.http, base, &self.tokens, name, data).await
    }

    /// Connects the hosted webhook inbox; `handler` gets every event in order.
    pub fn connect_inbox(&self, opts: InboxOptions, handler: InboxHandler) -> InboxHandle {
        inbox::spawn(self.connector.clone(), opts, handler)
    }

    /// Browser sign-in for the hosted relay (loopback flow); the refresh token goes to the keychain.
    pub async fn sign_in<F>(&self, signin_page: &str, open: F, timeout: Duration) -> Result<SignedIn>
    where
        F: FnOnce(&str) -> std::result::Result<(), String>,
    {
        if !matches!(self.config.auth, RelayAuth::FirebaseRefreshToken { .. }) {
            return Err(RelayError::Config("self-hosted relays use a token, not browser sign-in".into()));
        }
        let res = signin::sign_in_with_browser(signin_page, self.secrets.as_ref(), open, timeout).await?;
        self.tokens.invalidate().await;
        Ok(res)
    }

    /// Forgets the hosted-relay sign-in (keychain entry removed).
    pub async fn sign_out(&self) -> Result<()> {
        auth::clear_credentials(self.secrets.as_ref())?;
        self.tokens.invalidate().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_endpoints() {
        assert_eq!(normalize_endpoint("relay.example.com").unwrap().as_str(), "wss://relay.example.com/");
        assert_eq!(normalize_endpoint("https://relay.example.com/").unwrap().as_str(), "wss://relay.example.com/");
        assert_eq!(normalize_endpoint("http://127.0.0.1:8787").unwrap().as_str(), "ws://127.0.0.1:8787/");
        assert!(normalize_endpoint("").is_err());
        assert!(normalize_endpoint("ftp://x").is_err());
    }
}
