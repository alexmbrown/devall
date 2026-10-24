//! `GET /v1/account`: plan, limits, usage, live tunnels, inboxes and reserved subdomains.

use serde::{Deserialize, Serialize};
use url::Url;

use crate::auth::{RelayAuth, TokenProvider};
use crate::error::{RelayError, Result, chain};
use crate::protocol::{Limits, Usage};
use crate::session::USER_AGENT;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountTunnel {
    pub subdomain: String,
    pub url: String,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInbox {
    pub project_key: String,
    pub inbox_id: String,
    pub url: String,
}

/// A DNS record the customer adds for a custom domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomDomainRecord {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub value: String,
    /// `routing` (required), `ownership` / `certificate` (only when the CNAME can't come first).
    #[serde(default)]
    pub purpose: String,
}

/// A custom hostname serving one of the account's tunnels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomDomain {
    pub hostname: String,
    pub subdomain: String,
    /// `pending`, `active`, `error`, or `static` (a self-hosted relay's CUSTOM_HOSTS).
    pub status: String,
    #[serde(default)]
    pub ssl_status: Option<String>,
    #[serde(default)]
    pub records: Vec<CustomDomainRecord>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedMockRef {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub uid: String,
    #[serde(default)]
    pub email: Option<String>,
    pub plan: String,
    #[serde(default)]
    pub self_hosted: bool,
    pub domain: String,
    pub limits: Limits,
    pub usage: Usage,
    #[serde(default)]
    pub tunnels: Vec<AccountTunnel>,
    #[serde(default)]
    pub inboxes: Vec<AccountInbox>,
    #[serde(default)]
    pub reserved_subdomains: Vec<String>,
    #[serde(default)]
    pub upgrade_url: Option<String>,
    #[serde(default)]
    pub custom_domains: Vec<CustomDomain>,
    /// What custom domains CNAME to.
    #[serde(default)]
    pub custom_domain_target: Option<String>,
    #[serde(default)]
    pub hosted_mocks: Vec<HostedMockRef>,
}

/// `wss://relay.x` → `https://relay.x/v1/account` (`ws://` → `http://` for local relays).
pub fn account_url(endpoint: &Url) -> Result<Url> {
    let mut u = endpoint.clone();
    let scheme = match u.scheme() {
        "wss" | "https" => "https",
        "ws" | "http" => "http",
        s => return Err(RelayError::Config(format!("unsupported endpoint scheme {s}"))),
    };
    u.set_scheme(scheme).map_err(|_| RelayError::Config("invalid endpoint".into()))?;
    let base = u.path().trim_end_matches('/').to_string();
    u.set_path(&format!("{base}/v1/account"));
    u.set_query(None);
    Ok(u)
}

#[derive(Deserialize)]
struct ErrBody {
    error: ErrInner,
}

#[derive(Deserialize)]
struct ErrInner {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

pub(crate) async fn fetch(http: &reqwest::Client, endpoint: &Url, tokens: &TokenProvider) -> Result<AccountInfo> {
    let url = account_url(endpoint)?;
    let mut retried = false;
    loop {
        let token = tokens.bearer().await?;
        let res = http.get(url.clone()).bearer_auth(&token).header(reqwest::header::USER_AGENT, USER_AGENT).send().await.map_err(|e| RelayError::Connect(chain(&e)))?;
        let status = res.status().as_u16();
        if status == 401 && !retried && matches!(tokens.auth(), RelayAuth::FirebaseRefreshToken { .. }) {
            retried = true;
            tokens.invalidate().await;
            continue;
        }
        let text = res.text().await.map_err(|e| RelayError::Http(chain(&e)))?;
        if !(200..300).contains(&status) {
            let (code, message) = serde_json::from_str::<ErrBody>(&text).map(|b| (b.error.code, b.error.message)).unwrap_or_else(|_| (String::new(), text.chars().take(200).collect()));
            if status == 401 {
                return Err(RelayError::Auth(message));
            }
            return Err(RelayError::Rejected { status, code, message, retry_after: None });
        }
        return serde_json::from_str(&text).map_err(|e| RelayError::Http(format!("unexpected account response: {e}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_socket_endpoints_to_https() {
        assert_eq!(account_url(&Url::parse("wss://relay.devall.dev").unwrap()).unwrap().as_str(), "https://relay.devall.dev/v1/account");
        assert_eq!(account_url(&Url::parse("ws://127.0.0.1:8787/").unwrap()).unwrap().as_str(), "http://127.0.0.1:8787/v1/account");
        assert!(account_url(&Url::parse("ftp://x").unwrap()).is_err());
    }
}
