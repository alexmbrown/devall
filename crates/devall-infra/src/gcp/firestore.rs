//! Firestore document browser (emulator first; real projects read-only) over REST `v1`.

use reqwest::Method;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{Call, GcpClient, GcpTarget, q, seg};
use crate::error::{Error, Result};

pub const API: &str = "https://firestore.googleapis.com";

/// Converts Firestore/Datastore typed values (`{"stringValue": "x"}`, `{"mapValue": {...}}`, …)
/// to plain JSON. Integers arrive as strings and become numbers when they fit.
pub fn typed_to_json(v: &Value) -> Value {
    let Some(obj) = v.as_object() else { return v.clone() };
    let Some((k, inner)) = obj.iter().find(|(k, _)| k.ends_with("Value")) else { return v.clone() };
    match k.as_str() {
        "nullValue" => Value::Null,
        "booleanValue" | "doubleValue" | "stringValue" | "timestampValue" | "bytesValue" | "blobValue" | "referenceValue" => inner.clone(),
        "integerValue" => match inner.as_str().and_then(|s| s.parse::<i64>().ok()) {
            Some(n) => json!(n),
            None => inner.clone(),
        },
        "geoPointValue" => json!({ "latitude": inner.get("latitude"), "longitude": inner.get("longitude") }),
        "arrayValue" => Value::Array(inner.get("values").and_then(Value::as_array).map(|a| a.iter().map(typed_to_json).collect()).unwrap_or_default()),
        "mapValue" => fields_to_json(inner.get("fields")),
        "entityValue" => fields_to_json(inner.get("properties")),
        "keyValue" => json!(key_path(inner)),
        _ => inner.clone(),
    }
}

/// `{"a": <typed>, …}` → plain JSON object.
pub fn fields_to_json(fields: Option<&Value>) -> Value {
    let mut out = Map::new();
    if let Some(m) = fields.and_then(Value::as_object) {
        for (k, v) in m {
            out.insert(k.clone(), typed_to_json(v));
        }
    }
    Value::Object(out)
}

/// Datastore key → `Kind:name/Child:123`.
pub fn key_path(key: &Value) -> String {
    key.get("path")
        .and_then(Value::as_array)
        .map(|p| {
            p.iter()
                .map(|e| {
                    let kind = e.get("kind").and_then(Value::as_str).unwrap_or("?");
                    let id = e.get("name").and_then(Value::as_str).map(str::to_string).or_else(|| e.get("id").map(|i| i.as_str().map(str::to_string).unwrap_or_else(|| i.to_string()))).unwrap_or_default();
                    format!("{kind}:{id}")
                })
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsDocument {
    pub id: String,
    /// Path below `documents/`, e.g. `users/alice`.
    pub path: String,
    pub fields: Value,
    pub create_time: Option<String>,
    pub update_time: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsPage {
    pub documents: Vec<FsDocument>,
    pub next_page_token: Option<String>,
}

fn clean_path(path: &str) -> Result<String> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.iter().any(|p| *p == "." || *p == "..") {
        return Err(Error::Invalid(format!("invalid document path {path:?}")));
    }
    Ok(parts.iter().map(|p| seg(p)).collect::<Vec<_>>().join("/"))
}

pub fn parse_document(v: &Value) -> FsDocument {
    let name = v.get("name").and_then(Value::as_str).unwrap_or("");
    let path = name.split_once("/documents/").map(|(_, p)| p.to_string()).unwrap_or_default();
    FsDocument {
        id: path.rsplit('/').next().unwrap_or("").to_string(),
        path,
        fields: fields_to_json(v.get("fields")),
        create_time: v.get("createTime").and_then(Value::as_str).map(str::to_string),
        update_time: v.get("updateTime").and_then(Value::as_str).map(str::to_string),
    }
}

impl GcpClient {
    fn firestore_docs_url(&self, t: &GcpTarget, database: Option<&str>) -> Result<String> {
        let db = database.filter(|d| !d.is_empty()).unwrap_or("(default)");
        Ok(format!("{}/v1/projects/{}/databases/{}/documents", t.base(API), seg(t.check_project()?), seg(db)))
    }

    /// Collection ids under `parent` (empty = root collections, else a document path).
    pub async fn firestore_collections(&self, t: &GcpTarget, database: Option<&str>, parent: &str) -> Result<Vec<String>> {
        let parent = clean_path(parent)?;
        let base = self.firestore_docs_url(t, database)?;
        let url = if parent.is_empty() { format!("{base}:listCollectionIds") } else { format!("{base}/{parent}:listCollectionIds") };
        let mut ids: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..20 {
            let mut body = json!({ "pageSize": 300 });
            if let Some(tok) = &token {
                body["pageToken"] = json!(tok);
            }
            let v = self.call(Call { method: Method::POST, url: url.clone(), body: Some(body), target: t, emulator_bearer: Some("owner") }).await?;
            ids.extend(v.get("collectionIds").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default());
            token = v.get("nextPageToken").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// One page of documents in `collection` (a collection path like `users/alice/orders`).
    pub async fn firestore_documents(&self, t: &GcpTarget, database: Option<&str>, collection: &str, page_size: u32, page_token: Option<&str>) -> Result<FsPage> {
        let col = clean_path(collection)?;
        if col.is_empty() {
            return Err(Error::Invalid("a collection path is required".into()));
        }
        let mut url = format!("{}/{col}?pageSize={}", self.firestore_docs_url(t, database)?, page_size.clamp(1, 300));
        if let Some(tok) = page_token.filter(|s| !s.is_empty()) {
            url.push_str(&format!("&pageToken={}", q(tok)));
        }
        let v = self.call(Call { method: Method::GET, url, body: None, target: t, emulator_bearer: Some("owner") }).await?;
        Ok(FsPage {
            documents: v.get("documents").and_then(Value::as_array).map(|a| a.iter().map(parse_document).collect()).unwrap_or_default(),
            next_page_token: v.get("nextPageToken").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::gcp::testserver;

    #[test]
    fn typed_values_to_plain_json() {
        let fields = json!({
            "name": { "stringValue": "Alice" },
            "age": { "integerValue": "42" },
            "big": { "integerValue": "99999999999999999999" },
            "score": { "doubleValue": 9.5 },
            "admin": { "booleanValue": true },
            "deleted": { "nullValue": null },
            "tags": { "arrayValue": { "values": [ { "stringValue": "a" }, { "integerValue": "1" } ] } },
            "empty": { "arrayValue": {} },
            "address": { "mapValue": { "fields": { "city": { "stringValue": "Lyon" } } } },
            "at": { "timestampValue": "2026-10-21T10:00:00Z" },
            "loc": { "geoPointValue": { "latitude": 45.7, "longitude": 4.8 } },
            "parent": { "keyValue": { "path": [ { "kind": "User", "name": "alice" }, { "kind": "Order", "id": "12" } ] } }
        });
        let plain = fields_to_json(Some(&fields));
        assert_eq!(plain["name"], "Alice");
        assert_eq!(plain["age"], 42);
        assert_eq!(plain["big"], "99999999999999999999");
        assert_eq!(plain["score"], 9.5);
        assert_eq!(plain["admin"], true);
        assert!(plain["deleted"].is_null());
        assert_eq!(plain["tags"], json!(["a", 1]));
        assert_eq!(plain["empty"], json!([]));
        assert_eq!(plain["address"]["city"], "Lyon");
        assert_eq!(plain["loc"]["latitude"], 45.7);
        assert_eq!(plain["parent"], "User:alice/Order:12");
    }

    #[tokio::test]
    async fn browses_the_emulator() {
        let router: testserver::Router = Arc::new(|r: &testserver::Recorded| match (r.method.as_str(), r.path.as_str()) {
            ("POST", "/v1/projects/demo/databases/(default)/documents:listCollectionIds") => (200, json!({ "collectionIds": ["users", "orders"] }).to_string()),
            ("POST", "/v1/projects/demo/databases/(default)/documents/users/alice:listCollectionIds") => (200, json!({ "collectionIds": ["carts"] }).to_string()),
            ("GET", p) if p.starts_with("/v1/projects/demo/databases/(default)/documents/users?pageSize=20") => (
                200,
                json!({ "documents": [ { "name": "projects/demo/databases/(default)/documents/users/alice", "fields": { "n": { "integerValue": "1" } }, "createTime": "t1", "updateTime": "t2" } ], "nextPageToken": "next" }).to_string(),
            ),
            _ => (404, json!({ "error": { "message": format!("no route {}", r.path) } }).to_string()),
        });
        let (host, log) = testserver::start(router).await;
        let client = GcpClient::new().unwrap();
        let t = GcpTarget::emulator("demo", &host);
        assert_eq!(client.firestore_collections(&t, None, "").await.unwrap(), vec!["orders", "users"]);
        assert_eq!(client.firestore_collections(&t, None, "users/alice").await.unwrap(), vec!["carts"]);
        let page = client.firestore_documents(&t, None, "users", 20, None).await.unwrap();
        assert_eq!(page.documents[0].id, "alice");
        assert_eq!(page.documents[0].path, "users/alice");
        assert_eq!(page.documents[0].fields["n"], 1);
        assert_eq!(page.next_page_token.as_deref(), Some("next"));
        assert!(client.firestore_documents(&t, None, "../x", 20, None).await.is_err());
        // The emulator gets `Bearer owner` so security rules don't hide data.
        assert_eq!(log.lock().unwrap()[0].header("authorization"), Some("Bearer owner"));
    }
}
