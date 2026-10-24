//! Secret Manager (real projects only — there is no emulator): list secrets and versions, and
//! access a version's payload for importing it into the OS keychain. Uses the user's gcloud login
//! like the other real-cloud views. Nothing here writes to Google Cloud.
//!
//! Payloads are returned to the caller (the desktop app) only to be stored in the keychain right
//! away; they are never sent to the UI or written to files.

use std::collections::BTreeMap;

use base64::Engine;
use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

use super::{Call, GcpClient, GcpTarget, q, seg, short_name};
use crate::error::{Error, Result};

pub const API: &str = "https://secretmanager.googleapis.com";
const MAX_PAGES: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Secret {
    /// Short id (`stripe-key`).
    pub name: String,
    /// `projects/<p>/secrets/<id>`.
    pub full_name: String,
    pub create_time: Option<String>,
    pub labels: BTreeMap<String, String>,
    /// `automatic` or `user-managed` replication.
    pub replication: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretVersion {
    /// Version id (`3`).
    pub name: String,
    pub full_name: String,
    pub create_time: Option<String>,
    /// `ENABLED`, `DISABLED` or `DESTROYED`.
    pub state: String,
}

fn no_emulator(t: &GcpTarget) -> Result<()> {
    if t.is_emulator() {
        return Err(Error::Invalid("Secret Manager has no emulator — clear the emulator host to use your gcloud login against a real project".into()));
    }
    Ok(())
}

pub fn parse_secrets(v: &Value) -> Vec<Secret> {
    v["secrets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    let full = s["name"].as_str().unwrap_or_default().to_string();
                    Secret {
                        name: short_name(&full),
                        create_time: s["createTime"].as_str().map(str::to_string),
                        labels: s["labels"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()).unwrap_or_default(),
                        replication: s["replication"].as_object().and_then(|o| o.keys().next()).map(|k| if k == "userManaged" { "user-managed".to_string() } else { k.clone() }),
                        full_name: full,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_versions(v: &Value) -> Vec<SecretVersion> {
    v["versions"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    let full = x["name"].as_str().unwrap_or_default().to_string();
                    SecretVersion {
                        name: short_name(&full),
                        create_time: x["createTime"].as_str().map(str::to_string),
                        state: x["state"].as_str().unwrap_or("STATE_UNSPECIFIED").to_string(),
                        full_name: full,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The UTF-8 payload of an `:access` response.
pub fn decode_payload(v: &Value) -> Result<String> {
    let data = v.pointer("/payload/data").and_then(Value::as_str).ok_or_else(|| Error::Request("the secret version has no payload".into()))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| Error::Request(format!("invalid payload encoding: {e}")))?;
    String::from_utf8(bytes).map_err(|_| Error::Invalid("this secret is binary; only text secrets can be imported into the keychain".into()))
}

/// `gcp-sm://project/secret[/version]` reference for an environment's `secretSources`.
pub fn reference(project: &str, secret: &str, version: Option<&str>) -> String {
    match version.filter(|v| !v.is_empty() && *v != "latest") {
        Some(v) => format!("gcp-sm://{project}/{secret}/{v}"),
        None => format!("gcp-sm://{project}/{secret}"),
    }
}

impl GcpClient {
    /// Every secret of the project (metadata only).
    pub async fn secrets(&self, t: &GcpTarget) -> Result<Vec<Secret>> {
        no_emulator(t)?;
        let project = t.check_project()?;
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut url = format!("{API}/v1/projects/{}/secrets?pageSize=250", seg(project));
            if let Some(tok) = &token {
                url.push_str(&format!("&pageToken={}", q(tok)));
            }
            let v = self.call(Call { method: Method::GET, url, body: None, target: t, emulator_bearer: None }).await?;
            out.extend(parse_secrets(&v));
            token = v["nextPageToken"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Versions of a secret, newest first.
    pub async fn secret_versions(&self, t: &GcpTarget, secret: &str) -> Result<Vec<SecretVersion>> {
        no_emulator(t)?;
        let project = t.check_project()?;
        let url = format!("{API}/v1/projects/{}/secrets/{}/versions?pageSize=100", seg(project), seg(secret));
        let v = self.call(Call { method: Method::GET, url, body: None, target: t, emulator_bearer: None }).await?;
        let mut versions = parse_versions(&v);
        versions.sort_by_key(|x| std::cmp::Reverse(x.name.parse::<u64>().unwrap_or(0)));
        Ok(versions)
    }

    /// A version's text payload (`latest` by default). Only for storing in the keychain.
    pub async fn access_secret(&self, t: &GcpTarget, secret: &str, version: Option<&str>) -> Result<String> {
        no_emulator(t)?;
        let project = t.check_project()?;
        let version = version.filter(|v| !v.trim().is_empty()).unwrap_or("latest");
        let url = format!("{API}/v1/projects/{}/secrets/{}/versions/{}:access", seg(project), seg(secret), seg(version));
        let v = self.call(Call { method: Method::GET, url, body: None, target: t, emulator_bearer: None }).await?;
        decode_payload(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_and_decode() {
        let v = json!({ "secrets": [
            { "name": "projects/shop/secrets/stripe-key", "createTime": "2026-01-02T03:04:05Z", "labels": { "team": "payments" }, "replication": { "automatic": {} } },
            { "name": "projects/shop/secrets/db-password", "replication": { "userManaged": { "replicas": [] } } }
        ], "nextPageToken": "" });
        let s = parse_secrets(&v);
        assert_eq!(s[0].name, "stripe-key");
        assert_eq!(s[0].labels["team"], "payments");
        assert_eq!(s[0].replication.as_deref(), Some("automatic"));
        assert_eq!(s[1].replication.as_deref(), Some("user-managed"));
        let vs = parse_versions(&json!({ "versions": [{ "name": "projects/shop/secrets/stripe-key/versions/2", "state": "ENABLED" }] }));
        assert_eq!((vs[0].name.as_str(), vs[0].state.as_str()), ("2", "ENABLED"));
        assert_eq!(decode_payload(&json!({ "payload": { "data": "c2tfdGVzdF8xMjM=" } })).unwrap(), "sk_test_123");
        assert!(decode_payload(&json!({ "payload": { "data": "//79" } })).is_err(), "binary payloads are refused");
        assert_eq!(reference("shop", "stripe-key", Some("latest")), "gcp-sm://shop/stripe-key");
        assert_eq!(reference("shop", "stripe-key", Some("3")), "gcp-sm://shop/stripe-key/3");
    }

    #[tokio::test]
    async fn emulator_targets_are_refused() {
        let c = GcpClient::new().unwrap();
        let err = c.secrets(&GcpTarget::emulator("demo", "localhost:8085")).await.unwrap_err().to_string();
        assert!(err.contains("no emulator"), "{err}");
    }
}
