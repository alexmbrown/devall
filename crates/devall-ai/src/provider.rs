//! Chat providers over plain HTTP (reqwest, no SDKs):
//!
//! | Kind               | Endpoint                                   | Covers                                              |
//! |--------------------|--------------------------------------------|-----------------------------------------------------|
//! | `openAiCompatible` | `POST {base}/chat/completions`             | llama.cpp `llama-server`, LM Studio, vLLM, Jan, OpenRouter, OpenAI |
//! | `ollama`           | `POST {base}/api/chat` (native)            | Ollama ≥ 0.40 (`think: false`, `format: <schema>`)  |
//! | `anthropic`        | `POST {base}/v1/messages`                  | Anthropic Messages API (`output_config.format`)     |
//! | `gemini`           | `POST {base}/v1beta/models/{m}:generateContent` | Google Gemini (`responseJsonSchema`)           |
//!
//! JSON-schema constrained output is requested where the backend supports it; when a server
//! rejects the field we retry once without it (the prompt always describes the shape too, and
//! callers validate in Rust — see [`crate::structured`]). Thinking is switched off for local
//! reasoning models (Qwen3.5/3.8, Granite 4.2, Gemma 4) with
//! `chat_template_kwargs.enable_thinking=false` (llama-server, vLLM, LM Studio) or Ollama's
//! `think: false`, and any `<think>…</think>` block that still appears is stripped.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::sse::{LineBuffer, SseParser};
use crate::{AiError, Result};

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
const MASK: &str = "••••••";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    OpenAiCompatible,
    Ollama,
    Anthropic,
    Gemini,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => "OpenAI-compatible",
            ProviderKind::Ollama => "Ollama",
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::Gemini => "Google Gemini",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => "http://127.0.0.1:8080/v1",
            ProviderKind::Ollama => "http://127.0.0.1:11434",
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::Gemini => "https://generativelanguage.googleapis.com",
        }
    }
}

fn yes() -> bool {
    true
}

/// One configured model endpoint. Stored in `<app data>/ai.json`; the API key is never part of
/// it (it lives in the OS keychain, see [`crate::config`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Stable id (`[a-z0-9-_]`), also the keychain account for the API key.
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    /// Whether an API key is stored in the keychain for this provider.
    #[serde(default)]
    pub has_api_key: bool,
    /// Treat a non-loopback endpoint (a LAN box you control) as local for redaction purposes.
    #[serde(default)]
    pub trusted_local: bool,
    /// Ask local reasoning models to answer without a thinking phase (faster, JSON-friendly).
    #[serde(default = "yes")]
    pub disable_thinking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl ProviderConfig {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: ProviderKind, base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            base_url: base_url.into(),
            model: model.into(),
            has_api_key: false,
            trusted_local: false,
            disable_thinking: true,
            temperature: None,
            max_tokens: None,
        }
    }

    /// Local providers run on this machine (loopback) or were explicitly trusted. Everything else
    /// is *remote*: prompts sent there get the full credential-pattern redaction.
    pub fn is_local(&self) -> bool {
        self.trusted_local || is_loopback_url(&self.base_url)
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(AiError::Invalid(format!("provider id `{}` must be letters, digits, '-' or '_'", self.id)));
        }
        if self.name.trim().is_empty() {
            return Err(AiError::Invalid("provider name is empty".into()));
        }
        let url = url::Url::parse(&self.base_url).map_err(|e| AiError::Invalid(format!("base URL `{}`: {e}", self.base_url)))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(AiError::Invalid("base URL must start with http:// or https://".into()));
        }
        if !url.username().is_empty() || url.password().is_some() || url.query_pairs().any(|(k, _)| k.eq_ignore_ascii_case("key") || k.to_ascii_lowercase().contains("token")) {
            return Err(AiError::Invalid("don't put credentials in the base URL — use the API key field (stored in the OS keychain)".into()));
        }
        if self.model.trim().is_empty() {
            return Err(AiError::Invalid("model is empty".into()));
        }
        Ok(())
    }
}

/// `localhost`, `*.localhost`, `127.0.0.0/8`, `::1`.
pub fn is_loopback_url(u: &str) -> bool {
    let Ok(url) = url::Url::parse(u) else { return false };
    match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: content.into() }
    }
}

/// A JSON Schema the answer must follow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonSchemaSpec {
    /// `[a-zA-Z0-9_-]` name (OpenAI requires one).
    pub name: String,
    pub schema: Value,
    /// Our own schemas are written for strict mode (all properties required, closed objects);
    /// user-supplied schemas (mock data) are sent non-strict.
    pub strict: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub json_schema: Option<JsonSchemaSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    /// Answer text with any `<think>` block removed.
    pub text: String,
    pub model: String,
    pub finish_reason: Option<String>,
    pub usage: Usage,
    /// Whether the backend accepted the JSON-schema constraint (false = prompt-only fallback).
    pub schema_enforced: bool,
}

/// The exact HTTP request a call would make (credentials masked) — the "what will be sent" view.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadPreview {
    pub provider_id: String,
    pub provider_name: String,
    pub kind: ProviderKind,
    pub model: String,
    pub remote: bool,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

#[derive(Debug, Clone, Copy)]
struct BodyOpts {
    schema: bool,
    thinking_off: bool,
}

/// A provider plus its (keychain-loaded) API key and an HTTP client.
#[derive(Clone)]
pub struct Provider {
    pub config: ProviderConfig,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider").field("config", &self.config).field("api_key", &self.api_key.as_ref().map(|_| MASK)).finish()
    }
}

/// Removes `<think>…</think>` blocks (an unterminated block hides everything after it).
pub fn strip_think(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match rest.find("<think>") {
            Some(i) => {
                out.push_str(&rest[..i]);
                match rest[i..].find("</think>") {
                    Some(j) => rest = &rest[i + j + "</think>".len()..],
                    None => return out,
                }
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

/// Emits only text outside `<think>` blocks while streaming, holding back a partial `<think`.
#[derive(Default)]
struct ThinkFilter {
    full: String,
    emitted: usize,
}

impl ThinkFilter {
    fn push(&mut self, delta: &str) -> Option<String> {
        self.full.push_str(delta);
        // Always trim the leading whitespace (e.g. the blank lines after `</think>`): text is only
        // ever appended, so once something visible exists the trimmed prefix never changes and
        // `emitted` stays a valid offset.
        let visible = strip_think(&self.full).trim_start().to_string();
        let tag = "<think>";
        let hold = (1..tag.len()).rev().find(|n| visible.ends_with(&tag[..*n])).unwrap_or(0);
        let safe_len = visible.len() - hold;
        if safe_len > self.emitted && visible.is_char_boundary(self.emitted) && visible.is_char_boundary(safe_len) {
            let out = visible[self.emitted..safe_len].to_string();
            self.emitted = safe_len;
            return Some(out);
        }
        None
    }
}

fn trim_base(base: &str, suffixes: &[&str]) -> String {
    let mut b = base.trim().trim_end_matches('/').to_string();
    for s in suffixes {
        if let Some(stripped) = b.strip_suffix(s) {
            b = stripped.trim_end_matches('/').to_string();
        }
    }
    b
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

impl Provider {
    pub fn new(config: ProviderConfig, api_key: Option<String>) -> Result<Self> {
        let mut b = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(300))
            .user_agent(concat!("devall-ai/", env!("CARGO_PKG_VERSION")));
        if is_loopback_url(&config.base_url) {
            // Never route localhost model servers through a system/corporate proxy.
            b = b.no_proxy();
        }
        let http = b.build().map_err(|e| AiError::Protocol(e.to_string()))?;
        Ok(Self { config, api_key: api_key.filter(|k| !k.trim().is_empty()), http })
    }

    pub fn id(&self) -> &str {
        &self.config.id
    }

    pub fn is_remote(&self) -> bool {
        !self.config.is_local()
    }

    /// Whether this backend gets a "thinking off" parameter.
    fn sends_thinking_param(&self) -> bool {
        self.config.disable_thinking
            && match self.config.kind {
                ProviderKind::Ollama => true,
                // Hosted OpenAI-compatible APIs reject unknown fields; local servers accept them.
                ProviderKind::OpenAiCompatible => self.config.is_local(),
                ProviderKind::Anthropic | ProviderKind::Gemini => false,
            }
    }

    fn base(&self) -> String {
        match self.config.kind {
            ProviderKind::OpenAiCompatible => trim_base(&self.config.base_url, &["/chat/completions"]),
            ProviderKind::Ollama => trim_base(&self.config.base_url, &["/api/chat", "/v1", "/api"]),
            ProviderKind::Anthropic => trim_base(&self.config.base_url, &["/v1/messages", "/v1"]),
            ProviderKind::Gemini => trim_base(&self.config.base_url, &["/v1beta"]),
        }
    }

    pub fn chat_url(&self, stream: bool) -> String {
        let base = self.base();
        match self.config.kind {
            ProviderKind::OpenAiCompatible => format!("{base}/chat/completions"),
            ProviderKind::Ollama => format!("{base}/api/chat"),
            ProviderKind::Anthropic => format!("{base}/v1/messages"),
            ProviderKind::Gemini => {
                let model = self.config.model.trim_start_matches("models/");
                if stream {
                    format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse")
                } else {
                    format!("{base}/v1beta/models/{model}:generateContent")
                }
            }
        }
    }

    fn models_url(&self) -> String {
        let base = self.base();
        match self.config.kind {
            ProviderKind::OpenAiCompatible => format!("{base}/models"),
            ProviderKind::Ollama => format!("{base}/api/tags"),
            ProviderKind::Anthropic => format!("{base}/v1/models"),
            ProviderKind::Gemini => format!("{base}/v1beta/models"),
        }
    }

    fn headers(&self, mask: bool) -> Vec<(String, String)> {
        let key = |k: &str| if mask { MASK.to_string() } else { k.to_string() };
        let mut h = vec![("Content-Type".to_string(), "application/json".to_string())];
        match self.config.kind {
            ProviderKind::OpenAiCompatible | ProviderKind::Ollama => {
                if let Some(k) = &self.api_key {
                    h.push(("Authorization".into(), format!("Bearer {}", key(k))));
                }
                if self.config.base_url.contains("openrouter.ai") {
                    h.push(("X-Title".into(), "/dev/all".into()));
                }
            }
            ProviderKind::Anthropic => {
                if let Some(k) = &self.api_key {
                    h.push(("x-api-key".into(), key(k)));
                }
                h.push(("anthropic-version".into(), ANTHROPIC_VERSION.into()));
            }
            ProviderKind::Gemini => {
                if let Some(k) = &self.api_key {
                    h.push(("x-goog-api-key".into(), key(k)));
                }
            }
        }
        h
    }

    fn max_tokens(&self, req: &ChatRequest) -> u32 {
        req.max_tokens.or(self.config.max_tokens).unwrap_or(match self.config.kind {
            ProviderKind::Anthropic => 16_000,
            _ => 4_096,
        })
    }

    fn schema_for_backend(&self, spec: &JsonSchemaSpec) -> Value {
        crate::schema::for_backend(&spec.schema, self.config.kind, spec.strict && self.is_remote())
    }

    fn body(&self, req: &ChatRequest, stream: bool, opts: BodyOpts) -> Value {
        let temperature = req.temperature.or(self.config.temperature);
        let max_tokens = self.max_tokens(req);
        let schema = req.json_schema.as_ref().filter(|_| opts.schema);
        match self.config.kind {
            ProviderKind::OpenAiCompatible => {
                let mut messages = Vec::new();
                if let Some(s) = &req.system {
                    messages.push(json!({ "role": "system", "content": s }));
                }
                messages.extend(req.messages.iter().map(|m| json!({ "role": m.role, "content": m.content })));
                let mut b = json!({ "model": self.config.model, "messages": messages, "stream": stream });
                if self.config.base_url.contains("api.openai.com") {
                    b["max_completion_tokens"] = json!(max_tokens);
                } else {
                    b["max_tokens"] = json!(max_tokens);
                }
                if let Some(t) = temperature {
                    b["temperature"] = json!(t);
                }
                if let Some(s) = schema {
                    b["response_format"] = json!({
                        "type": "json_schema",
                        "json_schema": { "name": s.name, "schema": self.schema_for_backend(s), "strict": s.strict }
                    });
                }
                if opts.thinking_off {
                    b["chat_template_kwargs"] = json!({ "enable_thinking": false });
                }
                b
            }
            ProviderKind::Ollama => {
                let mut messages = Vec::new();
                if let Some(s) = &req.system {
                    messages.push(json!({ "role": "system", "content": s }));
                }
                messages.extend(req.messages.iter().map(|m| json!({ "role": m.role, "content": m.content })));
                let mut options = json!({ "num_predict": max_tokens, "num_ctx": 8192 });
                if let Some(t) = temperature {
                    options["temperature"] = json!(t);
                }
                let mut b = json!({ "model": self.config.model, "messages": messages, "stream": stream, "options": options });
                if let Some(s) = schema {
                    b["format"] = self.schema_for_backend(s);
                }
                if opts.thinking_off {
                    b["think"] = json!(false);
                }
                b
            }
            ProviderKind::Anthropic => {
                // Current Claude models reject sampling parameters and choose their own thinking
                // depth, so neither temperature nor a thinking toggle is sent.
                let messages: Vec<Value> = req.messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect();
                let mut b = json!({ "model": self.config.model, "max_tokens": max_tokens, "messages": messages });
                if let Some(s) = &req.system {
                    b["system"] = json!(s);
                }
                if stream {
                    b["stream"] = json!(true);
                }
                if let Some(s) = schema {
                    b["output_config"] = json!({ "format": { "type": "json_schema", "schema": self.schema_for_backend(s) } });
                }
                b
            }
            ProviderKind::Gemini => {
                let contents: Vec<Value> = req
                    .messages
                    .iter()
                    .map(|m| json!({ "role": if m.role == Role::User { "user" } else { "model" }, "parts": [{ "text": m.content }] }))
                    .collect();
                let mut gen_cfg = json!({ "maxOutputTokens": max_tokens });
                if let Some(t) = temperature {
                    gen_cfg["temperature"] = json!(t);
                }
                if let Some(s) = schema {
                    gen_cfg["responseMimeType"] = json!("application/json");
                    gen_cfg["responseJsonSchema"] = self.schema_for_backend(s);
                }
                let mut b = json!({ "contents": contents, "generationConfig": gen_cfg });
                if let Some(s) = &req.system {
                    b["systemInstruction"] = json!({ "parts": [{ "text": s }] });
                }
                b
            }
        }
    }

    fn default_opts(&self, req: &ChatRequest) -> BodyOpts {
        BodyOpts { schema: req.json_schema.is_some(), thinking_off: self.sends_thinking_param() }
    }

    /// The exact request [`Self::chat`] would send first (API key masked).
    pub fn preview(&self, req: &ChatRequest, stream: bool) -> PayloadPreview {
        PayloadPreview {
            provider_id: self.config.id.clone(),
            provider_name: self.config.name.clone(),
            kind: self.config.kind,
            model: self.config.model.clone(),
            remote: self.is_remote(),
            method: "POST".into(),
            url: self.chat_url(stream),
            headers: self.headers(true),
            body: self.body(req, stream, self.default_opts(req)),
        }
    }

    fn error_body(&self, text: &str) -> String {
        let mut t = truncate(text.trim(), 600);
        if let Some(k) = &self.api_key
            && k.len() >= 4
        {
            t = t.replace(k.as_str(), MASK);
        }
        t
    }

    /// POSTs the chat request, retrying without the thinking flag / schema when the backend
    /// rejects them. Returns the successful response and whether the schema was kept.
    async fn send(&self, req: &ChatRequest, stream: bool) -> Result<(reqwest::Response, bool)> {
        let mut opts = self.default_opts(req);
        let url = self.chat_url(stream);
        for _ in 0..3 {
            let body = self.body(req, stream, opts);
            let mut rb = self.http.post(&url);
            for (k, v) in self.headers(false) {
                rb = rb.header(k, v);
            }
            let bytes = serde_json::to_vec(&body).map_err(|e| AiError::Protocol(e.to_string()))?;
            let resp = rb.body(bytes).send().await.map_err(|e| AiError::Connect { url: url.clone(), message: crate::error_chain(&e) })?;
            if resp.status().is_success() {
                return Ok((resp, opts.schema));
            }
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            let lower = text.to_lowercase();
            if matches!(status, 400 | 422 | 500) {
                if opts.thinking_off && (lower.contains("think") || lower.contains("chat_template_kwargs")) {
                    opts.thinking_off = false;
                    continue;
                }
                if opts.schema && ["response_format", "json_schema", "schema", "format", "output_config", "grammar"].iter().any(|w| lower.contains(w)) {
                    opts.schema = false;
                    continue;
                }
            }
            return Err(AiError::Http { provider: self.config.name.clone(), status, body: self.error_body(&text) });
        }
        Err(AiError::Protocol("the provider kept rejecting the request".into()))
    }

    /// One complete (non-streaming) answer.
    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let (resp, schema_kept) = self.send(req, false).await?;
        let bytes = resp.bytes().await.map_err(|e| AiError::Protocol(crate::error_chain(&e)))?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| AiError::Protocol(format!("response is not JSON: {e}")))?;
        let mut out = self.parse_response(&v)?;
        out.schema_enforced = schema_kept && req.json_schema.is_some();
        Ok(out)
    }

    fn parse_response(&self, v: &Value) -> Result<ChatResponse> {
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            let msg = err.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| err.to_string());
            return Err(AiError::Protocol(self.error_body(&msg)));
        }
        let s = |v: &Value| v.as_str().map(str::to_string);
        let mut out = ChatResponse::default();
        match self.config.kind {
            ProviderKind::OpenAiCompatible => {
                let choice = &v["choices"][0];
                if choice.is_null() {
                    return Err(AiError::Protocol("response has no choices".into()));
                }
                out.text = choice["message"]["content"].as_str().unwrap_or_default().to_string();
                out.finish_reason = s(&choice["finish_reason"]);
                out.model = s(&v["model"]).unwrap_or_else(|| self.config.model.clone());
                out.usage = Usage { input_tokens: v["usage"]["prompt_tokens"].as_u64(), output_tokens: v["usage"]["completion_tokens"].as_u64() };
            }
            ProviderKind::Ollama => {
                out.text = v["message"]["content"].as_str().unwrap_or_default().to_string();
                out.finish_reason = s(&v["done_reason"]);
                out.model = s(&v["model"]).unwrap_or_else(|| self.config.model.clone());
                out.usage = Usage { input_tokens: v["prompt_eval_count"].as_u64(), output_tokens: v["eval_count"].as_u64() };
            }
            ProviderKind::Anthropic => {
                if v["stop_reason"].as_str() == Some("refusal") {
                    return Err(AiError::Refused("the model declined this request".into()));
                }
                out.text = v["content"]
                    .as_array()
                    .map(|blocks| blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""))
                    .unwrap_or_default();
                out.finish_reason = s(&v["stop_reason"]);
                out.model = s(&v["model"]).unwrap_or_else(|| self.config.model.clone());
                out.usage = Usage { input_tokens: v["usage"]["input_tokens"].as_u64(), output_tokens: v["usage"]["output_tokens"].as_u64() };
            }
            ProviderKind::Gemini => {
                let cand = &v["candidates"][0];
                if cand.is_null() {
                    let reason = v["promptFeedback"]["blockReason"].as_str().unwrap_or("no candidates returned");
                    return Err(AiError::Refused(reason.to_string()));
                }
                out.text = gemini_text(cand);
                out.finish_reason = s(&cand["finishReason"]);
                out.model = s(&v["modelVersion"]).unwrap_or_else(|| self.config.model.clone());
                out.usage = Usage { input_tokens: v["usageMetadata"]["promptTokenCount"].as_u64(), output_tokens: v["usageMetadata"]["candidatesTokenCount"].as_u64() };
            }
        }
        out.text = strip_think(&out.text).trim().to_string();
        Ok(out)
    }

    /// Streams the answer: `on_delta` receives visible text as it arrives. Returns the full answer.
    pub async fn chat_stream<F: FnMut(&str) + Send>(&self, req: &ChatRequest, mut on_delta: F) -> Result<ChatResponse> {
        let (mut resp, schema_kept) = self.send(req, true).await?;
        let mut out = ChatResponse { model: self.config.model.clone(), schema_enforced: schema_kept && req.json_schema.is_some(), ..Default::default() };
        let mut filter = ThinkFilter::default();
        let mut sse = SseParser::default();
        let mut lines = LineBuffer::default();
        let mut done = false;
        let mut emit = |delta: &str, filter: &mut ThinkFilter| {
            // `filter` is passed in rather than captured so it can be read after the loop.
            if let Some(d) = filter.push(delta) {
                on_delta(&d);
            }
        };
        while !done {
            let chunk = resp.chunk().await.map_err(|e| AiError::Protocol(crate::error_chain(&e)))?;
            let Some(chunk) = chunk else { break };
            match self.config.kind {
                ProviderKind::Ollama => {
                    for line in lines.feed(&chunk) {
                        done |= self.ollama_line(&line, &mut out, &mut filter, &mut emit)?;
                    }
                }
                _ => {
                    for ev in sse.feed(&chunk) {
                        done |= self.sse_event(&ev.data, &mut out, &mut filter, &mut emit)?;
                    }
                }
            }
        }
        if !done {
            match self.config.kind {
                ProviderKind::Ollama => {
                    if let Some(line) = lines.finish() {
                        self.ollama_line(&line, &mut out, &mut filter, &mut emit)?;
                    }
                }
                _ => {
                    for ev in sse.finish() {
                        self.sse_event(&ev.data, &mut out, &mut filter, &mut emit)?;
                    }
                }
            }
        }
        out.text = strip_think(&filter.full).trim().to_string();
        Ok(out)
    }

    fn ollama_line(&self, line: &str, out: &mut ChatResponse, filter: &mut ThinkFilter, emit: &mut impl FnMut(&str, &mut ThinkFilter)) -> Result<bool> {
        if line.trim().is_empty() {
            return Ok(false);
        }
        let v: Value = serde_json::from_str(line).map_err(|e| AiError::Protocol(format!("bad stream line: {e}")))?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Err(AiError::Protocol(self.error_body(e)));
        }
        if let Some(t) = v["message"]["content"].as_str().filter(|t| !t.is_empty()) {
            emit(t, filter);
        }
        if v["done"].as_bool() == Some(true) {
            out.finish_reason = v["done_reason"].as_str().map(str::to_string);
            out.usage = Usage { input_tokens: v["prompt_eval_count"].as_u64(), output_tokens: v["eval_count"].as_u64() };
            if let Some(m) = v["model"].as_str() {
                out.model = m.to_string();
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn sse_event(&self, data: &str, out: &mut ChatResponse, filter: &mut ThinkFilter, emit: &mut impl FnMut(&str, &mut ThinkFilter)) -> Result<bool> {
        if data.trim() == "[DONE]" {
            return Ok(true);
        }
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(false),
        };
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            let msg = err.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| err.to_string());
            return Err(AiError::Protocol(self.error_body(&msg)));
        }
        match self.config.kind {
            ProviderKind::OpenAiCompatible => {
                if let Some(m) = v["model"].as_str() {
                    out.model = m.to_string();
                }
                let choice = &v["choices"][0];
                if let Some(t) = choice["delta"]["content"].as_str().filter(|t| !t.is_empty()) {
                    emit(t, filter);
                }
                if let Some(r) = choice["finish_reason"].as_str() {
                    out.finish_reason = Some(r.to_string());
                }
                if v["usage"].is_object() {
                    out.usage = Usage { input_tokens: v["usage"]["prompt_tokens"].as_u64(), output_tokens: v["usage"]["completion_tokens"].as_u64() };
                }
            }
            ProviderKind::Anthropic => match v["type"].as_str().unwrap_or_default() {
                "message_start" => {
                    if let Some(m) = v["message"]["model"].as_str() {
                        out.model = m.to_string();
                    }
                    out.usage.input_tokens = v["message"]["usage"]["input_tokens"].as_u64();
                }
                "content_block_delta" if v["delta"]["type"] == "text_delta" => {
                    if let Some(t) = v["delta"]["text"].as_str() {
                        emit(t, filter);
                    }
                }
                "message_delta" => {
                    if let Some(r) = v["delta"]["stop_reason"].as_str() {
                        if r == "refusal" {
                            return Err(AiError::Refused("the model declined this request".into()));
                        }
                        out.finish_reason = Some(r.to_string());
                    }
                    if let Some(n) = v["usage"]["output_tokens"].as_u64() {
                        out.usage.output_tokens = Some(n);
                    }
                }
                "message_stop" => return Ok(true),
                _ => {}
            },
            ProviderKind::Gemini => {
                let cand = &v["candidates"][0];
                let t = gemini_text(cand);
                if !t.is_empty() {
                    emit(&t, filter);
                }
                if let Some(r) = cand["finishReason"].as_str() {
                    out.finish_reason = Some(r.to_string());
                }
                if v["usageMetadata"].is_object() {
                    out.usage = Usage { input_tokens: v["usageMetadata"]["promptTokenCount"].as_u64(), output_tokens: v["usageMetadata"]["candidatesTokenCount"].as_u64() };
                }
            }
            ProviderKind::Ollama => {}
        }
        Ok(false)
    }

    /// Lists model ids the endpoint serves — doubles as "test connection".
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = self.models_url();
        let mut rb = self.http.get(&url).timeout(Duration::from_secs(15));
        for (k, v) in self.headers(false) {
            if k != "Content-Type" {
                rb = rb.header(k, v);
            }
        }
        let resp = rb.send().await.map_err(|e| AiError::Connect { url: url.clone(), message: crate::error_chain(&e) })?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| AiError::Protocol(crate::error_chain(&e)))?;
        if !(200..300).contains(&status) {
            return Err(AiError::Http { provider: self.config.name.clone(), status, body: self.error_body(&String::from_utf8_lossy(&bytes)) });
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| AiError::Protocol(format!("model list is not JSON: {e}")))?;
        let ids = |arr: &Value, key: &str| -> Vec<String> {
            arr.as_array().map(|a| a.iter().filter_map(|m| m[key].as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        Ok(match self.config.kind {
            ProviderKind::OpenAiCompatible | ProviderKind::Anthropic => ids(&v["data"], "id"),
            ProviderKind::Ollama => ids(&v["models"], "name"),
            ProviderKind::Gemini => ids(&v["models"], "name").into_iter().map(|n| n.trim_start_matches("models/").to_string()).collect(),
        })
    }
}

fn gemini_text(cand: &Value) -> String {
    cand["content"]["parts"]
        .as_array()
        .map(|parts| parts.iter().filter(|p| p["thought"].as_bool() != Some(true)).filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_blocks_are_stripped_and_filtered_while_streaming() {
        assert_eq!(strip_think("<think>\nplan\n</think>\n\nAnswer"), "\n\nAnswer");
        assert_eq!(strip_think("before <think>never closed"), "before ");
        let mut f = ThinkFilter::default();
        let mut out = String::new();
        for d in ["<th", "ink>hidden", " stuff</th", "ink>\n\nHel", "lo <", "b>"] {
            if let Some(x) = f.push(d) {
                out.push_str(&x);
            }
        }
        assert_eq!(out, "Hello <b>");
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_url("http://127.0.0.1:11434"));
        assert!(is_loopback_url("http://localhost:1234/v1"));
        assert!(is_loopback_url("http://[::1]:8080"));
        assert!(is_loopback_url("http://ollama.localhost"));
        assert!(!is_loopback_url("https://api.openai.com/v1"));
        assert!(!is_loopback_url("http://192.168.1.20:11434"));
        let mut c = ProviderConfig::new("lan", "LAN", ProviderKind::Ollama, "http://192.168.1.20:11434", "qwen3.5:9b");
        assert!(!c.is_local());
        c.trusted_local = true;
        assert!(c.is_local());
    }

    #[test]
    fn config_validation_rejects_credentials_in_urls() {
        let ok = ProviderConfig::new("p1", "P", ProviderKind::OpenAiCompatible, "https://openrouter.ai/api/v1", "m");
        assert!(ok.validate().is_ok());
        let bad = ProviderConfig { base_url: "https://user:pw@example.com/v1".into(), ..ok.clone() };
        assert!(bad.validate().is_err());
        let bad = ProviderConfig { base_url: "https://generativelanguage.googleapis.com?key=abc".into(), ..ok.clone() };
        assert!(bad.validate().is_err());
        let bad = ProviderConfig { id: "no spaces".into(), ..ok };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn bodies_per_kind() {
        let req = ChatRequest {
            system: Some("sys".into()),
            messages: vec![ChatMessage::user("hi")],
            temperature: Some(0.2),
            max_tokens: Some(100),
            json_schema: Some(JsonSchemaSpec { name: "out".into(), schema: json!({"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}), strict: true }),
        };
        let local = Provider::new(ProviderConfig::new("l", "L", ProviderKind::OpenAiCompatible, "http://127.0.0.1:8080/v1/", "qwen"), None).unwrap();
        let p = local.preview(&req, false);
        assert_eq!(p.url, "http://127.0.0.1:8080/v1/chat/completions");
        assert_eq!(p.body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(p.body["response_format"]["type"], "json_schema");
        assert_eq!(p.body["messages"][0]["role"], "system");
        assert!(!p.remote);

        let openai = Provider::new(ProviderConfig::new("o", "O", ProviderKind::OpenAiCompatible, "https://api.openai.com/v1", "gpt"), Some("sk-secret-key".into())).unwrap();
        let p = openai.preview(&req, true);
        assert!(p.body.get("chat_template_kwargs").is_none(), "hosted APIs don't get unknown fields");
        assert_eq!(p.body["max_completion_tokens"], 100);
        assert!(p.remote);
        assert!(p.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer ••••••"));

        let ollama = Provider::new(ProviderConfig::new("ol", "Ollama", ProviderKind::Ollama, "http://127.0.0.1:11434/v1", "qwen3.5:4b"), None).unwrap();
        let p = ollama.preview(&req, false);
        assert_eq!(p.url, "http://127.0.0.1:11434/api/chat");
        assert_eq!(p.body["think"], false);
        assert_eq!(p.body["format"]["type"], "object");

        let anthropic = Provider::new(ProviderConfig::new("a", "A", ProviderKind::Anthropic, "https://api.anthropic.com", "claude-opus-5-5"), Some("k".into())).unwrap();
        let p = anthropic.preview(&req, false);
        assert_eq!(p.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(p.body["system"], "sys");
        assert!(p.body.get("temperature").is_none());
        assert_eq!(p.body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(p.body["output_config"]["format"]["schema"]["additionalProperties"], false);

        let gemini = Provider::new(ProviderConfig::new("g", "G", ProviderKind::Gemini, "https://generativelanguage.googleapis.com/v1beta", "gemini-x"), Some("k".into())).unwrap();
        let p = gemini.preview(&req, true);
        assert_eq!(p.url, "https://generativelanguage.googleapis.com/v1beta/models/gemini-x:streamGenerateContent?alt=sse");
        assert_eq!(p.body["generationConfig"]["responseMimeType"], "application/json");
    }
}
