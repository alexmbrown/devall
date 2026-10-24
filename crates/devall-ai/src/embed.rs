//! Embedding providers for local semantic search ([`crate::semantic`]):
//!
//! | Kind | Endpoint | Covers |
//! |---|---|---|
//! | Ollama | `POST {base}/api/embed` | `embeddinggemma-2:270m`, `qwen3-embedding:0.6b`, … |
//! | OpenAI-compatible | `POST {base}/embeddings` | llama.cpp `llama-server --embedding` (incl. the managed sidecar), LM Studio, vLLM; cloud APIs only when the user opted in |
//!
//! Everything is behind the [`Embed`] trait so the index and the MCP tool can be tested with a
//! fake embedder. Retrieval prompts follow each model family's card (EmbeddingGemma's
//! `task: … | query:` prefixes, Qwen3-Embedding's query instruction); other models get plain text.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{AiConfig, load_api_key};
use crate::provider::{ProviderConfig, ProviderKind, is_loopback_url};
use crate::{AiError, Result};

/// Provider id of the managed embedding sidecar (`llama-server --embedding`; not in `ai.json`).
pub const MANAGED_EMBED_ID: &str = "managed-embed";

pub type EmbedFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<Vec<f32>>>> + Send + 'a>>;

/// Something that turns texts into vectors.
pub trait Embed: Send + Sync {
    /// Identifies the model (and so the vector space): an index built with another id is rebuilt.
    fn id(&self) -> String;
    /// Runs on this machine (or a host the user marked as trusted).
    fn is_local(&self) -> bool;
    /// Human-readable `model @ endpoint`.
    fn label(&self) -> String;
    /// One vector per input, in order.
    fn embed<'a>(&'a self, inputs: &'a [String]) -> EmbedFuture<'a>;
    /// How a search query is phrased for this model.
    fn query_text(&self, query: &str) -> String {
        query.to_string()
    }
    /// How an indexed document is phrased for this model.
    fn document_text(&self, title: &str, text: &str) -> String {
        if title.is_empty() { text.to_string() } else { format!("{title}\n{text}") }
    }
}

/// Retrieval prompt conventions per model family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptStyle {
    Plain,
    /// `task: search result | query: …` / `title: … | text: …`.
    EmbeddingGemma,
    /// `Instruct: … \nQuery:…` for queries, plain documents.
    Qwen3,
}

impl PromptStyle {
    pub fn for_model(model: &str) -> Self {
        let m = model.to_ascii_lowercase().replace(['_', ' '], "-");
        if m.contains("embeddinggemma") {
            PromptStyle::EmbeddingGemma
        } else if m.contains("qwen3-embedding") {
            PromptStyle::Qwen3
        } else {
            PromptStyle::Plain
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    Ollama,
    OpenAi,
}

/// An HTTP embedding endpoint.
#[derive(Clone)]
pub struct HttpEmbedder {
    pub api: Api,
    pub base_url: String,
    pub model: String,
    id: String,
    local: bool,
    style: PromptStyle,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl std::fmt::Debug for HttpEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpEmbedder").field("api", &self.api).field("base_url", &self.base_url).field("model", &self.model).field("local", &self.local).finish_non_exhaustive()
    }
}

impl HttpEmbedder {
    /// `id` is what the index remembers (use a stable one for endpoints whose port changes, like
    /// the managed sidecar).
    pub fn new(api: Api, base_url: &str, model: &str, api_key: Option<String>, local: bool, id: Option<String>) -> Result<Self> {
        let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).read_timeout(Duration::from_secs(300));
        if is_loopback_url(base_url) {
            b = b.no_proxy();
        }
        let http = b.build().map_err(|e| AiError::Protocol(e.to_string()))?;
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        let id = id.unwrap_or_else(|| format!("{}|{}|{model}", if api == Api::Ollama { "ollama" } else { "openai" }, base_url));
        Ok(Self { api, model: model.to_string(), id, local, style: PromptStyle::for_model(model), api_key: api_key.filter(|k| !k.trim().is_empty()), http, base_url })
    }

    fn url(&self) -> String {
        match self.api {
            Api::Ollama => format!("{}/api/embed", self.base_url.trim_end_matches("/api").trim_end_matches("/v1")),
            Api::OpenAi => format!("{}/embeddings", self.base_url),
        }
    }

    async fn call(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let url = self.url();
        let body = match self.api {
            Api::Ollama => json!({ "model": self.model, "input": inputs, "truncate": true }),
            Api::OpenAi => json!({ "model": self.model, "input": inputs, "encoding_format": "float" }),
        };
        let mut rb = self.http.post(&url).header(reqwest::header::CONTENT_TYPE, "application/json").body(body.to_string());
        if let Some(k) = &self.api_key {
            rb = rb.bearer_auth(k);
        }
        let resp = rb.send().await.map_err(|e| AiError::Connect { url: url.clone(), message: crate::error_chain(&e) })?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| AiError::Connect { url: url.clone(), message: crate::error_chain(&e) })?;
        if !status.is_success() {
            let body = text.chars().take(500).collect::<String>();
            let hint = if self.api == Api::OpenAi && (status.as_u16() == 501 || body.contains("embedding")) {
                " (llama-server needs --embedding and an embedding model)"
            } else if self.api == Api::Ollama && status.as_u16() == 404 {
                " (pull the model first: ollama pull <model>)"
            } else {
                ""
            };
            return Err(AiError::Http { provider: self.label(), status: status.as_u16(), body: format!("{body}{hint}") });
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| AiError::Protocol(format!("embeddings: {e}")))?;
        let vectors: Vec<Vec<f32>> = match self.api {
            Api::Ollama => v["embeddings"].as_array().cloned().unwrap_or_default().iter().map(to_vec).collect(),
            Api::OpenAi => {
                let mut data: Vec<(usize, Vec<f32>)> = v["data"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .map(|(i, d)| (d["index"].as_u64().map(|x| x as usize).unwrap_or(i), to_vec(&d["embedding"])))
                    .collect();
                data.sort_by_key(|d| d.0);
                data.into_iter().map(|d| d.1).collect()
            }
        };
        if vectors.len() != inputs.len() || vectors.iter().any(Vec::is_empty) {
            return Err(AiError::Protocol(format!("expected {} embeddings, got {} (is `{}` an embedding model?)", inputs.len(), vectors.len(), self.model)));
        }
        Ok(vectors)
    }
}

fn to_vec(v: &Value) -> Vec<f32> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|f| f as f32).collect()).unwrap_or_default()
}

impl Embed for HttpEmbedder {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn is_local(&self) -> bool {
        self.local
    }
    fn label(&self) -> String {
        format!("{} @ {}", self.model, self.base_url)
    }
    fn embed<'a>(&'a self, inputs: &'a [String]) -> EmbedFuture<'a> {
        Box::pin(self.call(inputs))
    }
    fn query_text(&self, query: &str) -> String {
        match self.style {
            PromptStyle::EmbeddingGemma => format!("task: search result | query: {query}"),
            PromptStyle::Qwen3 => format!("Instruct: Given a question about a software project, retrieve the notes, API requests, mocks and environments that answer it\nQuery:{query}"),
            PromptStyle::Plain => query.to_string(),
        }
    }
    fn document_text(&self, title: &str, text: &str) -> String {
        match self.style {
            PromptStyle::EmbeddingGemma => format!("title: {} | text: {text}", if title.is_empty() { "none" } else { title }),
            _ if title.is_empty() => text.to_string(),
            _ => format!("{title}\n{text}"),
        }
    }
}

/// Which endpoint and model embed the project (`ai.json` → `embeddings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingSettings {
    /// A configured provider id (Ollama or OpenAI-compatible), or [`MANAGED_EMBED_ID`] for the
    /// managed llama.cpp embedding sidecar.
    pub provider: String,
    /// Embedding model: an Ollama tag (`embeddinggemma-2:270m`), the server's model name, or a
    /// catalog id for the managed sidecar (`embeddinggemma-2`).
    pub model: String,
}

/// Builds the embedder for configured `settings`. Cloud endpoints are refused unless
/// `allowRemoteContext` is on (project text would leave the machine).
pub fn embedder_for(config: &AiConfig, settings: &EmbeddingSettings, store: &dyn devall_core::SecretStore) -> Result<HttpEmbedder> {
    let p: &ProviderConfig = config.get(&settings.provider).ok_or_else(|| AiError::UnknownProvider(settings.provider.clone()))?;
    if !p.is_local() && !config.allow_remote_context {
        return Err(AiError::RemoteContextBlocked(p.name.clone()));
    }
    let model = if settings.model.trim().is_empty() { p.model.clone() } else { settings.model.trim().to_string() };
    let api = match p.kind {
        ProviderKind::Ollama => Api::Ollama,
        ProviderKind::OpenAiCompatible => Api::OpenAi,
        ProviderKind::Anthropic | ProviderKind::Gemini => {
            return Err(AiError::Invalid(format!("{} has no OpenAI-compatible embeddings endpoint; pick Ollama, LM Studio, llama.cpp or the managed embedding model", p.kind.label())));
        }
    };
    let key = if p.has_api_key { load_api_key(store, &p.id)? } else { None };
    HttpEmbedder::new(api, &p.base_url, &model, key, p.is_local(), None)
}

/// Resolves the embedder a host should use right now (the desktop app also knows its managed
/// sidecar; the CLI only knows `ai.json`).
pub type EmbedderFuture<'a> = Pin<Box<dyn Future<Output = Option<Arc<dyn Embed>>> + Send + 'a>>;

pub trait EmbedderSource: Send + Sync {
    fn embedder(&self) -> EmbedderFuture<'_>;
}

/// `ai.json`-only resolution (no managed sidecar): what `devall mcp` uses.
pub struct ConfigEmbedderSource {
    pub data_dir: std::path::PathBuf,
    pub store: Arc<dyn devall_core::SecretStore>,
}

impl EmbedderSource for ConfigEmbedderSource {
    fn embedder(&self) -> EmbedderFuture<'_> {
        Box::pin(async move {
            let cfg = AiConfig::load(&self.data_dir).ok()?;
            let settings = cfg.embeddings.clone()?;
            if settings.provider == MANAGED_EMBED_ID {
                return None;
            }
            embedder_for(&cfg, &settings, self.store.as_ref()).ok().map(|e| Arc::new(e) as Arc<dyn Embed>)
        })
    }
}

/// A deterministic embedder for tests: hashed bag of words (plus character trigrams for
/// near-miss spellings), so texts sharing words are close.
#[derive(Debug, Clone)]
pub struct FakeEmbedder {
    pub dims: usize,
    pub calls: Arc<std::sync::atomic::AtomicUsize>,
    pub embedded: Arc<std::sync::atomic::AtomicUsize>,
    pub local: bool,
}

impl FakeEmbedder {
    pub fn new(dims: usize) -> Self {
        Self { dims, calls: Default::default(), embedded: Default::default(), local: true }
    }

    pub fn vector(&self, text: &str) -> Vec<f32> {
        use sha2::{Digest, Sha256};
        let mut v = vec![0f32; self.dims];
        let lower = text.to_lowercase();
        for w in lower.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2) {
            let stem = w.trim_end_matches('s');
            let h = Sha256::digest(stem.as_bytes());
            let i = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as usize % self.dims;
            v[i] += 1.0;
        }
        v
    }
}

impl Embed for FakeEmbedder {
    fn id(&self) -> String {
        format!("fake|{}", self.dims)
    }
    fn is_local(&self) -> bool {
        self.local
    }
    fn label(&self) -> String {
        "fake embedder".into()
    }
    fn embed<'a>(&'a self, inputs: &'a [String]) -> EmbedFuture<'a> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.embedded.fetch_add(inputs.len(), std::sync::atomic::Ordering::SeqCst);
        let out = inputs.iter().map(|t| self.vector(t)).collect();
        Box::pin(async move { Ok(out) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_styles_and_urls() {
        assert_eq!(PromptStyle::for_model("embeddinggemma-2:270m"), PromptStyle::EmbeddingGemma);
        assert_eq!(PromptStyle::for_model("Qwen3-Embedding-0.6B-Q8_0"), PromptStyle::Qwen3);
        assert_eq!(PromptStyle::for_model("nomic-embed-text"), PromptStyle::Plain);
        let o = HttpEmbedder::new(Api::Ollama, "http://127.0.0.1:11434/", "embeddinggemma-2:270m", None, true, None).unwrap();
        assert_eq!(o.url(), "http://127.0.0.1:11434/api/embed");
        assert_eq!(o.query_text("refunds"), "task: search result | query: refunds");
        assert_eq!(o.document_text("Payments", "Stripe"), "title: Payments | text: Stripe");
        assert_eq!(o.id(), "ollama|http://127.0.0.1:11434|embeddinggemma-2:270m");
        let l = HttpEmbedder::new(Api::OpenAi, "http://127.0.0.1:8080/v1", "local", None, true, Some("managed|embeddinggemma-2".into())).unwrap();
        assert_eq!(l.url(), "http://127.0.0.1:8080/v1/embeddings");
        assert_eq!(l.id(), "managed|embeddinggemma-2");
    }

    #[test]
    fn cloud_embeddings_need_the_opt_in() {
        let mut cfg = AiConfig::default();
        cfg.upsert(ProviderConfig::new("oai", "OpenAI", ProviderKind::OpenAiCompatible, "https://api.openai.com/v1", "gpt"));
        cfg.upsert(ProviderConfig::new("claude", "Claude", ProviderKind::Anthropic, "http://127.0.0.1:9/", "x"));
        let store = devall_core::MemorySecretStore::default();
        let s = EmbeddingSettings { provider: "oai".into(), model: "text-embedding-3-small".into() };
        assert!(matches!(embedder_for(&cfg, &s, &store), Err(AiError::RemoteContextBlocked(_))));
        cfg.allow_remote_context = true;
        let e = embedder_for(&cfg, &s, &store).unwrap();
        assert!(!e.is_local());
        assert!(embedder_for(&cfg, &EmbeddingSettings { provider: "claude".into(), model: String::new() }, &store).is_err());
    }
}
