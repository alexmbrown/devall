#![allow(dead_code)]

use std::path::Path;

use devall_mock::{MockConfig, MockHandle, MockServer};
use serde_json::Value;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub handle: MockHandle,
    pub http: reqwest::Client,
}

pub fn write_mapping(root: &Path, name: &str, json: &Value) {
    let dir = root.join("mocks").join("mappings");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), serde_json::to_string_pretty(json).unwrap()).unwrap();
}

pub fn write_body_file(root: &Path, name: &str, data: &[u8]) {
    let path = root.join("mocks").join("__files").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

pub async fn start() -> Fixture {
    start_with(|_| {}, |_| {}).await
}

pub async fn start_with(files: impl FnOnce(&Path), configure: impl FnOnce(&mut MockConfig)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    files(dir.path());
    let mut cfg = MockConfig::new(dir.path());
    cfg.trash_deleted_files = false;
    configure(&mut cfg);
    let handle = MockServer::start(cfg).await.unwrap();
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    Fixture { dir, handle, http }
}

pub struct Resp {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub text: String,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text))
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).map(|v| v.to_str().unwrap().to_string())
    }
}

impl Fixture {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.url())
    }

    pub async fn send(&self, rb: reqwest::RequestBuilder) -> Resp {
        let r = rb.send().await.unwrap();
        let status = r.status().as_u16();
        let headers = r.headers().clone();
        let text = r.text().await.unwrap();
        Resp { status, headers, text }
    }

    pub async fn get(&self, path: &str) -> Resp {
        self.send(self.http.get(self.url(path))).await
    }

    pub async fn request(&self, method: &str, path: &str, body: Option<&str>) -> Resp {
        let mut rb = self.http.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), self.url(path));
        if let Some(b) = body {
            rb = rb.body(b.to_string());
        }
        self.send(rb).await
    }

    pub async fn admin(&self, method: &str, path: &str, body: Option<Value>) -> Resp {
        let mut rb = self.http.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), self.url(&format!("/__admin{path}")));
        if let Some(b) = body {
            rb = rb.header("Content-Type", "application/json").body(b.to_string());
        }
        self.send(rb).await
    }

    /// Creates a stub through the admin API and returns its JSON.
    pub async fn stub(&self, stub: Value) -> Value {
        let r = self.admin("POST", "/mappings", Some(stub)).await;
        assert_eq!(r.status, 201, "{}", r.text);
        r.json()
    }

    pub fn mapping_files(&self) -> Vec<String> {
        let dir = self.root().join("mocks").join("mappings");
        let mut out: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect())
            .unwrap_or_default();
        out.sort();
        out
    }
}
