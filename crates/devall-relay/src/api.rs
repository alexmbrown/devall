//! HTTP account API for the hosted extras (PROTOCOL.md §6): hosted mocks, shared snapshots,
//! scheduled monitors, custom domains — plus the Firebase callables that manage hosted custom
//! domains (`addCustomDomain`, `removeCustomDomain`, `refreshCustomDomain`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::auth::{RelayAuth, TokenProvider};
use crate::error::{RelayError, Result, chain};
use crate::session::USER_AGENT;

/// `wss://relay.x` + `/v1/mocks` → `https://relay.x/v1/mocks` (`ws://` → `http://` for local relays).
pub fn api_url(endpoint: &Url, path: &str) -> Result<Url> {
    let mut u = endpoint.clone();
    let scheme = match u.scheme() {
        "wss" | "https" => "https",
        "ws" | "http" => "http",
        s => return Err(RelayError::Config(format!("unsupported endpoint scheme {s}"))),
    };
    u.set_scheme(scheme).map_err(|_| RelayError::Config("invalid endpoint".into()))?;
    let base = u.path().trim_end_matches('/').to_string();
    let (p, q) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    u.set_path(&format!("{base}{p}"));
    u.set_query(q);
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

/// Calls the relay's JSON API with the account's bearer token (one retry after a 401 with a
/// refreshed Firebase ID token). Errors become [`RelayError::Rejected`] with the relay's code.
/// `timeout` replaces the client's default (30 s) for long calls such as cloud load runs.
pub(crate) async fn request_json(
    http: &reqwest::Client,
    endpoint: &Url,
    tokens: &TokenProvider,
    method: reqwest::Method,
    path: &str,
    body: Option<&Value>,
    timeout: Option<std::time::Duration>,
) -> Result<Value> {
    let url = api_url(endpoint, path)?;
    let mut retried = false;
    loop {
        let token = tokens.bearer().await?;
        let mut req = http.request(method.clone(), url.clone()).bearer_auth(&token).header(reqwest::header::USER_AGENT, USER_AGENT);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        if let Some(b) = body {
            req = req.header(reqwest::header::CONTENT_TYPE, "application/json").body(serde_json::to_vec(b).unwrap_or_default());
        }
        let res = req.send().await.map_err(|e| if e.is_timeout() { RelayError::Timeout(format!("{path}: {}", chain(&e))) } else { RelayError::Connect(chain(&e)) })?;
        let status = res.status().as_u16();
        if status == 401 && !retried && matches!(tokens.auth(), RelayAuth::FirebaseRefreshToken { .. }) {
            retried = true;
            tokens.invalidate().await;
            continue;
        }
        let retry_after = res.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()).map(std::time::Duration::from_secs);
        let text = res.text().await.map_err(|e| RelayError::Http(chain(&e)))?;
        if !(200..300).contains(&status) {
            let (code, message) = serde_json::from_str::<ErrBody>(&text).map(|b| (b.error.code, b.error.message)).unwrap_or_else(|_| (String::new(), text.chars().take(200).collect()));
            if status == 401 {
                return Err(RelayError::Auth(message));
            }
            let message = if message.trim().is_empty() { format!("the relay answered HTTP {status}") } else { message };
            return Err(RelayError::Rejected { status, code, message, retry_after });
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        return serde_json::from_str(&text).map_err(|e| RelayError::Http(format!("unexpected relay response: {e}")));
    }
}

#[derive(Deserialize)]
struct CallableErr {
    error: CallableErrInner,
}

#[derive(Deserialize)]
struct CallableErrInner {
    #[serde(default)]
    message: String,
    #[serde(default)]
    status: String,
}

/// Firebase callable (`POST <functions base>/<name>` with `{"data": …}`, answer `{"result": …}`).
pub(crate) async fn call_function(http: &reqwest::Client, base: &str, tokens: &TokenProvider, name: &str, data: &Value) -> Result<Value> {
    if !matches!(tokens.auth(), RelayAuth::FirebaseRefreshToken { .. }) {
        return Err(RelayError::Config("this is managed on the hosted relay only (self-hosted relays use their own configuration)".into()));
    }
    let url = format!("{}/{name}", base.trim_end_matches('/'));
    let mut retried = false;
    loop {
        let token = tokens.bearer().await?;
        let res = http
            .post(&url)
            .bearer_auth(&token)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&serde_json::json!({ "data": data })).unwrap_or_default())
            .send()
            .await
            .map_err(|e| RelayError::Connect(chain(&e)))?;
        let status = res.status().as_u16();
        if status == 401 && !retried {
            retried = true;
            tokens.invalidate().await;
            continue;
        }
        let text = res.text().await.map_err(|e| RelayError::Http(chain(&e)))?;
        if !(200..300).contains(&status) {
            let (code, message) = serde_json::from_str::<CallableErr>(&text).map(|b| (b.error.status.to_ascii_lowercase(), b.error.message)).unwrap_or_else(|_| (String::new(), text.chars().take(200).collect()));
            if status == 401 {
                return Err(RelayError::Auth(message));
            }
            return Err(RelayError::Rejected { status, code, message, retry_after: None });
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| RelayError::Http(format!("unexpected function response: {e}")))?;
        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
    }
}

// ---------- typed shapes used by the CLI ----------

/// Something in a stub that hosted mocks don't support (from the local check or the relay).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockWarning {
    /// Stub name (or id).
    pub stub: String,
    /// The feature, e.g. `request.bodyPatterns[0]` or `response.proxyBaseUrl`.
    pub feature: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockPublished {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub host_url: Option<String>,
    #[serde(default)]
    pub path_url: Option<String>,
    pub stubs: u64,
    pub bytes: u64,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub warnings: Vec<MockWarning>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_api_urls() {
        let e = Url::parse("wss://relay.devall.dev").unwrap();
        assert_eq!(api_url(&e, "/v1/mocks/x/requests?limit=5").unwrap().as_str(), "https://relay.devall.dev/v1/mocks/x/requests?limit=5");
        let local = Url::parse("ws://127.0.0.1:8787/base/").unwrap();
        assert_eq!(api_url(&local, "/v1/shares").unwrap().as_str(), "http://127.0.0.1:8787/base/v1/shares");
    }
}
