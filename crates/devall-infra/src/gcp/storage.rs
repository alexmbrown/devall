//! Cloud Storage browser: fake-gcs-server (or another GCS emulator) or real GCS (read-only by
//! default) over the JSON API.

use std::path::Path;

use futures::StreamExt;
use reqwest::Method;
use reqwest::header::{CONTENT_TYPE, RANGE};
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use super::{Call, GcpClient, GcpTarget, api_error, q, seg};
use crate::error::{Error, Result};

pub const API: &str = "https://storage.googleapis.com";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub name: String,
    pub location: Option<String>,
    pub storage_class: Option<String>,
    pub created: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectInfo {
    pub name: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub updated: Option<String>,
    pub md5: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectList {
    /// "Folders" one level below the prefix.
    pub prefixes: Vec<String>,
    pub objects: Vec<ObjectInfo>,
    pub next_page_token: Option<String>,
}

pub fn parse_object(v: &Value) -> ObjectInfo {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    ObjectInfo {
        name: s("name").unwrap_or_default(),
        // The JSON API sends sizes as strings; fake-gcs-server sometimes as numbers.
        size: v.get("size").and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0),
        content_type: s("contentType"),
        updated: s("updated"),
        md5: s("md5Hash"),
    }
}

/// A content type from the file extension (upload default).
pub fn guess_content_type(name: &str) -> &'static str {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "json" => "application/json",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "parquet" => "application/vnd.apache.parquet",
        _ => "application/octet-stream",
    }
}

impl GcpClient {
    fn storage_base(&self, t: &GcpTarget) -> String {
        t.base(API)
    }

    pub async fn storage_buckets(&self, t: &GcpTarget) -> Result<Vec<Bucket>> {
        let url = format!("{}/storage/v1/b?project={}&maxResults=1000", self.storage_base(t), q(t.check_project()?));
        let v = self.call(Call { method: Method::GET, url, body: None, target: t, emulator_bearer: None }).await?;
        let mut out: Vec<Bucket> = v
            .get("items")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|b| {
                        let s = |k: &str| b.get(k).and_then(Value::as_str).map(str::to_string);
                        Bucket { name: s("name").unwrap_or_default(), location: s("location"), storage_class: s("storageClass"), created: s("timeCreated") }
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// One level of a bucket: objects directly under `prefix` plus sub-"folders".
    pub async fn storage_objects(&self, t: &GcpTarget, bucket: &str, prefix: &str, page_token: Option<&str>) -> Result<ObjectList> {
        let mut url = format!("{}/storage/v1/b/{}/o?delimiter=%2F&maxResults=500&prefix={}", self.storage_base(t), seg(bucket), q(prefix));
        if let Some(tok) = page_token.filter(|s| !s.is_empty()) {
            url.push_str(&format!("&pageToken={}", q(tok)));
        }
        let v = self.call(Call { method: Method::GET, url, body: None, target: t, emulator_bearer: None }).await?;
        Ok(ObjectList {
            prefixes: v.get("prefixes").and_then(Value::as_array).map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            objects: v.get("items").and_then(Value::as_array).map(|a| a.iter().map(parse_object).filter(|o| o.name != prefix).collect()).unwrap_or_default(),
            next_page_token: v.get("nextPageToken").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
        })
    }

    fn media_url(&self, t: &GcpTarget, bucket: &str, object: &str) -> String {
        format!("{}/storage/v1/b/{}/o/{}?alt=media", self.storage_base(t), seg(bucket), seg(object))
    }

    /// Streams an object to `dest`; returns the byte count.
    pub async fn storage_download(&self, t: &GcpTarget, bucket: &str, object: &str, dest: &Path) -> Result<u64> {
        let rb = self.client_for(t).get(self.media_url(t, bucket, object));
        let resp = self.authorize(rb, t).await?.send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            return Err(api_error(status, &resp.bytes().await.unwrap_or_default()));
        }
        let tmp = dest.with_extension("devall-download");
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut stream = resp.bytes_stream();
        let mut total: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            total += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, dest).await?;
        Ok(total)
    }

    /// The first `max_bytes` of an object as text (preview).
    pub async fn storage_preview(&self, t: &GcpTarget, bucket: &str, object: &str, max_bytes: usize) -> Result<String> {
        let rb = self.client_for(t).get(self.media_url(t, bucket, object)).header(RANGE, format!("bytes=0-{}", max_bytes.saturating_sub(1)));
        let resp = self.authorize(rb, t).await?.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            return Err(api_error(status.as_u16(), &bytes));
        }
        let cut = &bytes[..bytes.len().min(max_bytes)];
        Ok(String::from_utf8_lossy(cut).into_owned())
    }

    /// Uploads `src` as `object` (simple media upload). A write: needs an emulator or allowWrites.
    pub async fn storage_upload(&self, t: &GcpTarget, bucket: &str, object: &str, src: &Path, content_type: Option<&str>) -> Result<ObjectInfo> {
        t.check_write("Uploading")?;
        if object.is_empty() {
            return Err(Error::Invalid("an object name is required".into()));
        }
        let data = tokio::fs::read(src).await?;
        let url = format!("{}/upload/storage/v1/b/{}/o?uploadType=media&name={}", self.storage_base(t), seg(bucket), q(object));
        let ct = content_type.filter(|c| !c.is_empty()).unwrap_or_else(|| guess_content_type(object));
        let rb = self.client_for(t).post(url).header(CONTENT_TYPE, ct).body(data);
        let resp = self.authorize(rb, t).await?.send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            return Err(api_error(status.as_u16(), &bytes));
        }
        Ok(parse_object(&serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::gcp::testserver;

    #[tokio::test]
    async fn browse_download_upload_against_fake_gcs() {
        let router: testserver::Router = Arc::new(|r: &testserver::Recorded| {
            let p = r.path.as_str();
            if p.starts_with("/storage/v1/b?project=demo") {
                (200, json!({ "kind": "storage#buckets", "items": [ { "name": "uploads", "location": "US" }, { "name": "assets" } ] }).to_string())
            } else if p.starts_with("/storage/v1/b/uploads/o?delimiter=%2F") {
                (200, json!({ "prefixes": ["2026/"], "items": [ { "name": "readme.txt", "size": "5", "contentType": "text/plain" } ] }).to_string())
            } else if p == "/storage/v1/b/uploads/o/2026%2Freport.csv?alt=media" {
                (200, "a,b\n1,2".into())
            } else if p.starts_with("/upload/storage/v1/b/uploads/o?uploadType=media&name=in%2Fdata.json") {
                (200, json!({ "name": "in/data.json", "size": 2 }).to_string())
            } else {
                (404, json!({ "error": { "message": format!("no route {p}") } }).to_string())
            }
        });
        let (host, log) = testserver::start(router).await;
        let client = GcpClient::new().unwrap();
        let t = GcpTarget::emulator("demo", &format!("http://{host}"));
        let buckets = client.storage_buckets(&t).await.unwrap();
        assert_eq!(buckets.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), vec!["assets", "uploads"]);
        let list = client.storage_objects(&t, "uploads", "", None).await.unwrap();
        assert_eq!(list.prefixes, vec!["2026/"]);
        assert_eq!(list.objects[0].size, 5);

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("report.csv");
        assert_eq!(client.storage_download(&t, "uploads", "2026/report.csv", &dest).await.unwrap(), 7);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "a,b\n1,2");
        assert_eq!(client.storage_preview(&t, "uploads", "2026/report.csv", 3).await.unwrap(), "a,b");

        let src = dir.path().join("data.json");
        std::fs::write(&src, "{}").unwrap();
        let up = client.storage_upload(&t, "uploads", "in/data.json", &src, None).await.unwrap();
        assert_eq!(up.size, 2);
        let missing = client.storage_download(&t, "uploads", "nope", &dir.path().join("x")).await.unwrap_err().to_string();
        assert!(missing.contains("404"), "{missing}");

        // Only inspect the log once no request is in flight: the fake server takes the same lock.
        let log = log.lock().unwrap();
        let upload = log.iter().find(|r| r.path.starts_with("/upload/")).unwrap();
        assert_eq!(upload.header("content-type"), Some("application/json"));
        assert_eq!(upload.body, b"{}");
        let preview = log.iter().find(|r| r.header("range").is_some()).unwrap();
        assert_eq!(preview.header("range"), Some("bytes=0-2"));
    }

    #[test]
    fn content_types() {
        assert_eq!(guess_content_type("a/b.JSON"), "application/json");
        assert_eq!(guess_content_type("noext"), "application/octet-stream");
    }

    #[tokio::test]
    async fn real_gcs_upload_is_read_only() {
        let client = GcpClient::new().unwrap();
        let t = GcpTarget { project: "prod".into(), emulator: None, allow_writes: false };
        let err = client.storage_upload(&t, "b", "o", Path::new("missing"), None).await.unwrap_err();
        assert!(matches!(err, Error::ReadOnly(_)));
    }
}
