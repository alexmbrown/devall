//! Local-first AI for /dev/all.
//!
//! * [`provider`] — one chat interface over OpenAI-compatible servers (llama.cpp, LM Studio, vLLM,
//!   Jan, OpenRouter, OpenAI), Ollama's native API, Anthropic Messages and Gemini; streaming and
//!   JSON-schema constrained output.
//! * [`config`] — `ai.json` in the app data dir; API keys in the OS keychain.
//! * [`discover`] — finds Ollama / LM Studio / llama-server / Jan on this machine.
//! * [`hardware`], [`catalog`], [`download`], [`sidecar`] — the opt-in managed llama.cpp sidecar:
//!   hardware tiers, curated Apache-2.0 models (no Qwen2.5), resumable SHA-256-verified
//!   downloads, `llama-server` on 127.0.0.1 with a random port and API key.
//! * [`context`], [`redact`], [`features`] — project-aware features over notes, personal to-dos,
//!   requests and environments, with secret redaction and an exact "what will be sent" preview.
//! * [`embed`], [`semantic`] — local embeddings (Ollama, OpenAI-compatible servers, the managed
//!   sidecar with an embedding model) and the per-project vector index in `<project data>/embeddings/`
//!   behind semantic search, `ai_ask` source ranking and the MCP `search_project` tool.
//!
//! No network call happens unless the user triggers it (discovery, a download, a feature run).

pub mod catalog;
pub mod config;
pub mod context;
pub mod discover;
pub mod download;
pub mod embed;
pub mod features;
pub mod hardware;
pub mod nlquery;
pub mod openapi_names;
pub mod prompts;
pub mod provider;
pub mod redact;
pub mod schema;
pub mod semantic;
pub mod sidecar;
mod sse;
pub mod structured;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

pub use config::AiConfig;
pub use features::{AiEnv, FailureContext, FailureSource, FeatureInput, Preview, ResponseContext};
pub use provider::{ChatMessage, ChatRequest, ChatResponse, Provider, ProviderConfig, ProviderKind};
pub use redact::Redactor;

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("no AI provider is configured — add one (or start the managed llama.cpp model) under AI → Models")]
    NoProvider,
    #[error("unknown AI provider `{0}`")]
    UnknownProvider(String),
    #[error("could not reach {url}: {message}")]
    Connect { url: String, message: String },
    #[error("{provider} returned HTTP {status}: {body}")]
    Http { provider: String, status: u16, body: String },
    #[error("unexpected answer from the model provider: {0}")]
    Protocol(String),
    #[error("the model's answer could not be used: {message}")]
    InvalidOutput { message: String, raw: String },
    #[error("the model declined: {0}")]
    Refused(String),
    #[error(
        "project notes and to-dos are only sent to local models; `{0}` is a cloud provider. Enable \"Allow project context for cloud providers\" in AI → Models, or use a local model"
    )]
    RemoteContextBlocked(String),
    #[error("cancelled")]
    Cancelled,
    #[error("download failed: {0}")]
    Download(String),
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    Checksum { file: String, expected: String, actual: String },
    #[error("llama.cpp: {0}")]
    Sidecar(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Core(#[from] devall_core::Error),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
}

pub type Result<T> = std::result::Result<T, AiError>;

/// Full `source()` chain of an error (reqwest's top-level message hides the useful part).
pub(crate) fn error_chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let msg = s.to_string();
        if !parts.iter().any(|p| p.contains(&msg)) {
            parts.push(msg);
        }
        cur = s.source();
    }
    parts.join(": ")
}
