//! Datastore (and Firestore in Datastore mode) kinds and entities over REST `v1`.

use reqwest::Method;
use serde::Serialize;
use serde_json::{Value, json};

use super::firestore::{fields_to_json, key_path};
use super::{Call, GcpClient, GcpTarget, seg};
use crate::error::Result;

pub const API: &str = "https://datastore.googleapis.com";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DsEntity {
    /// `Kind:name/Child:123`.
    pub key: String,
    pub properties: Value,
}

/// The `runQuery` body for `kind` (`__kind__` lists kinds).
pub fn query_body(project: &str, namespace: Option<&str>, kind: &str, limit: u32) -> Value {
    let mut partition = json!({ "projectId": project });
    if let Some(ns) = namespace.filter(|n| !n.is_empty()) {
        partition["namespaceId"] = json!(ns);
    }
    json!({ "partitionId": partition, "query": { "kind": [ { "name": kind } ], "limit": limit.clamp(1, 1000) } })
}

pub fn parse_entities(v: &Value) -> Vec<DsEntity> {
    v.pointer("/batch/entityResults")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|r| DsEntity {
                    key: r.pointer("/entity/key").map(key_path).unwrap_or_default(),
                    properties: fields_to_json(r.pointer("/entity/properties")),
                })
                .collect()
        })
        .unwrap_or_default()
}

impl GcpClient {
    async fn datastore_run(&self, t: &GcpTarget, namespace: Option<&str>, kind: &str, limit: u32) -> Result<Vec<DsEntity>> {
        let project = t.check_project()?;
        let url = format!("{}/v1/projects/{}:runQuery", t.base(API), seg(project));
        let v = self.call(Call { method: Method::POST, url, body: Some(query_body(project, namespace, kind, limit)), target: t, emulator_bearer: None }).await?;
        Ok(parse_entities(&v))
    }

    /// Kind names (from the `__kind__` metadata query).
    pub async fn datastore_kinds(&self, t: &GcpTarget, namespace: Option<&str>) -> Result<Vec<String>> {
        let mut kinds: Vec<String> = self
            .datastore_run(t, namespace, "__kind__", 1000)
            .await?
            .into_iter()
            .filter_map(|e| e.key.strip_prefix("__kind__:").map(str::to_string))
            .filter(|k| !k.starts_with("__"))
            .collect();
        kinds.sort();
        Ok(kinds)
    }

    pub async fn datastore_entities(&self, t: &GcpTarget, namespace: Option<&str>, kind: &str, limit: u32) -> Result<Vec<DsEntity>> {
        self.datastore_run(t, namespace, kind, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_parse() {
        let b = query_body("demo", Some("tenant-a"), "Task", 50);
        assert_eq!(b["partitionId"]["namespaceId"], "tenant-a");
        assert_eq!(b["query"]["kind"][0]["name"], "Task");
        assert_eq!(b["query"]["limit"], 50);
        assert!(query_body("demo", None, "Task", 5000)["partitionId"].get("namespaceId").is_none());
        let v = json!({ "batch": { "entityResults": [
            { "entity": { "key": { "path": [ { "kind": "Task", "id": "5629499534213120" } ] }, "properties": { "done": { "booleanValue": false }, "title": { "stringValue": "Ship it" } } } },
            { "entity": { "key": { "path": [ { "kind": "__kind__", "name": "Task" } ] } } }
        ] } });
        let e = parse_entities(&v);
        assert_eq!(e[0].key, "Task:5629499534213120");
        assert_eq!(e[0].properties["title"], "Ship it");
        assert_eq!(e[1].key, "__kind__:Task");
    }
}
