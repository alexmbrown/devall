//! Finds local model servers already running on this machine (only when the user asks):
//! Ollama (`:11434`, `/api/version` + `/api/tags`; warns below 0.40), LM Studio (`:1234`,
//! `/v1/models`), a llama.cpp `llama-server` (`:8080`, `/health` + `/v1/models`) and Jan
//! (`:1337`). `OLLAMA_HOST` is honoured when it points at this machine.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::provider::{ProviderConfig, ProviderKind, is_loopback_url};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerKind {
    Ollama,
    LmStudio,
    LlamaServer,
    Jan,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub size_bytes: Option<u64>,
    pub details: Option<String>,
    /// Set when /dev/all advises against the model (e.g. Qwen2.5 family).
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredServer {
    pub kind: ServerKind,
    pub name: String,
    pub base_url: String,
    pub version: Option<String>,
    pub models: Vec<DiscoveredModel>,
    pub warnings: Vec<String>,
    /// Ready-to-save provider config (first suitable model preselected).
    pub suggested: ProviderConfig,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub kind: ServerKind,
    pub base: String,
}

/// Default probe targets.
pub fn default_targets() -> Vec<Target> {
    let mut ollama = "http://127.0.0.1:11434".to_string();
    if let Ok(h) = std::env::var("OLLAMA_HOST") {
        let h = if h.starts_with("http") { h } else { format!("http://{h}") };
        if is_loopback_url(&h) {
            ollama = h.trim_end_matches('/').to_string();
        }
    }
    vec![
        Target { kind: ServerKind::Ollama, base: ollama },
        Target { kind: ServerKind::LmStudio, base: "http://127.0.0.1:1234".into() },
        Target { kind: ServerKind::LlamaServer, base: "http://127.0.0.1:8080".into() },
        Target { kind: ServerKind::Jan, base: "http://127.0.0.1:1337".into() },
    ]
}

/// `0.40.7` → (0, 40, 7).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+', ' ']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

pub fn ollama_version_warnings(version: &str) -> Vec<String> {
    match parse_version(version) {
        Some(v) if v < (0, 40, 0) => vec![format!(
            "Ollama {version} is older than 0.40 — Qwen3.8, Gemma 4 and EmbeddingGemma-2 need ≥ 0.40 (and models upgraded by 0.40.2 must be re-pulled after a downgrade). Please update Ollama."
        )],
        Some(v) if v < (0,40, 5) => vec![format!("Ollama {version}: Nemotron 3.5 models need ≥ 0.40.5.")],
        Some(_) => vec![],
        None => vec![format!("Could not parse the Ollama version `{version}`.")],
    }
}

/// Advice for models /dev/all won't recommend.
pub fn model_warning(id: &str) -> Option<String> {
    let n = id.to_lowercase().replace(['-', '_', ' '], "");
    (n.contains("qwen2.5") || n.contains("qwen25")).then(|| "Qwen2.5-family model — /dev/all recommends Qwen3.5 / Gemma 4 / Qwen3.8 instead.".to_string())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_millis(500)).timeout(Duration::from_millis(1500)).build().expect("client")
}

async fn get_json(c: &reqwest::Client, url: &str) -> Option<Value> {
    let r = c.get(url).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    serde_json::from_slice(&r.bytes().await.ok()?).ok()
}

fn openai_models(v: &Value) -> Vec<DiscoveredModel> {
    v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["id"].as_str())
                .map(|id| DiscoveredModel { id: id.to_string(), size_bytes: None, details: None, warning: model_warning(id) })
                .collect()
        })
        .unwrap_or_default()
}

fn pick_model(models: &[DiscoveredModel]) -> String {
    models
        .iter()
        .find(|m| m.warning.is_none() && !m.id.to_lowercase().contains("embed"))
        .or_else(|| models.first())
        .map(|m| m.id.clone())
        .unwrap_or_default()
}

async fn probe(c: &reqwest::Client, t: &Target) -> Option<DiscoveredServer> {
    let (name, models, version, mut warnings, provider_kind, provider_base) = match t.kind {
        ServerKind::Ollama => {
            let ver = get_json(c, &format!("{}/api/version", t.base)).await?;
            let version = ver["version"].as_str().map(str::to_string);
            let tags = get_json(c, &format!("{}/api/tags", t.base)).await.unwrap_or(Value::Null);
            let models = tags["models"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| {
                            let id = m["name"].as_str()?.to_string();
                            let d = &m["details"];
                            let details = [d["parameter_size"].as_str(), d["quantization_level"].as_str(), d["family"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                            Some(DiscoveredModel { warning: model_warning(&id), id, size_bytes: m["size"].as_u64(), details: (!details.is_empty()).then_some(details) })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let warnings = version.as_deref().map(ollama_version_warnings).unwrap_or_default();
            ("Ollama", models, version, warnings, ProviderKind::Ollama, t.base.clone())
        }
        ServerKind::LlamaServer => {
            let health = c.get(format!("{}/health", t.base)).send().await.ok()?;
            if !(health.status().is_success() || health.status().as_u16() == 503) {
                return None;
            }
            let models = get_json(c, &format!("{}/v1/models", t.base)).await.map(|v| openai_models(&v)).unwrap_or_default();
            ("llama.cpp server", models, None, vec![], ProviderKind::OpenAiCompatible, format!("{}/v1", t.base))
        }
        ServerKind::LmStudio | ServerKind::Jan => {
            let v = get_json(c, &format!("{}/v1/models", t.base)).await?;
            if !v["data"].is_array() {
                return None;
            }
            let name = if t.kind == ServerKind::LmStudio { "LM Studio" } else { "Jan" };
            (name, openai_models(&v), None, vec![], ProviderKind::OpenAiCompatible, format!("{}/v1", t.base))
        }
    };
    if models.is_empty() {
        warnings.push("No models installed/loaded yet.".into());
    }
    let id = match t.kind {
        ServerKind::Ollama => "ollama",
        ServerKind::LmStudio => "lm-studio",
        ServerKind::LlamaServer => "llama-server",
        ServerKind::Jan => "jan",
    };
    let suggested = ProviderConfig::new(id, name, provider_kind, provider_base, pick_model(&models));
    Some(DiscoveredServer { kind: t.kind, name: name.into(), base_url: t.base.clone(), version, models, warnings, suggested })
}

/// Probes all targets concurrently; returns the servers that answered.
pub async fn discover_at(targets: &[Target]) -> Vec<DiscoveredServer> {
    let c = client();
    let mut set = tokio::task::JoinSet::new();
    for (i, t) in targets.iter().cloned().enumerate() {
        let c = c.clone();
        set.spawn(async move { (i, probe(&c, &t).await) });
    }
    let mut found: Vec<(usize, DiscoveredServer)> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok((i, Some(s))) = res {
            found.push((i, s));
        }
    }
    found.sort_by_key(|(i, _)| *i);
    found.into_iter().map(|(_, s)| s).collect()
}

pub async fn discover() -> Vec<DiscoveredServer> {
    discover_at(&default_targets()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serves fixed JSON per path.
    async fn server(routes: Vec<(&'static str, &'static str)>) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let resp = match routes.iter().find(|(p, _)| *p == path) {
                        Some((_, body)) => format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
                        None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                    };
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn finds_ollama_and_lm_studio() {
        let ollama = server(vec![
            ("/api/version", r#"{"version":"0.39.2"}"#),
            ("/api/tags", r#"{"models":[{"name":"qwen2.5:7b","size":4683087332,"details":{"parameter_size":"7.6B","quantization_level":"Q4_K_M","family":"qwen2"}},{"name":"qwen3.5:9b","size":6600000000,"details":{"parameter_size":"9B"}}]}"#),
        ])
        .await;
        let lms = server(vec![("/v1/models", r#"{"data":[{"id":"gemma-4-12b-it"},{"id":"text-embedding-embeddinggemma-2"}]}"#)]).await;
        let targets = vec![
            Target { kind: ServerKind::Ollama, base: ollama.clone() },
            Target { kind: ServerKind::LmStudio, base: lms.clone() },
            Target { kind: ServerKind::Jan, base: "http://127.0.0.1:1".into() },
        ];
        let found = discover_at(&targets).await;
        assert_eq!(found.len(), 2);
        let o = &found[0];
        assert_eq!(o.kind, ServerKind::Ollama);
        assert_eq!(o.version.as_deref(), Some("0.39.2"));
        assert!(o.warnings[0].contains("older than 0.40"));
        assert!(o.models[0].warning.is_some(), "qwen2.5 flagged");
        assert_eq!(o.suggested.model, "qwen3.5:9b", "skips the flagged model");
        assert_eq!(o.suggested.kind, ProviderKind::Ollama);
        let l = &found[1];
        assert_eq!(l.suggested.base_url, format!("{lms}/v1"));
        assert_eq!(l.suggested.model, "gemma-4-12b-it");
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("0.40.7"), Some((0, 40, 7)));
        assert_eq!(parse_version("v0.41.0-rc1"), Some((0, 41, 0)));
        assert!(ollama_version_warnings("0.40.7").is_empty());
        assert!(!ollama_version_warnings("0.40.2").is_empty());
        assert!(ollama_version_warnings("0.12.11")[0].contains("older than 0.40"));
    }
}
