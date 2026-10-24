//! Project-aware AI features. Each one:
//!
//! 1. builds a prompt ([`prepare`]) from the user's input and, where useful, project context;
//! 2. redacts it ([`crate::redact`]) — the same request object is what [`preview`] shows and
//!    what is sent, so "what will be sent" is exact;
//! 3. asks for schema-constrained JSON and validates it ([`crate::structured`]), plus a
//!    feature-specific check (regex against samples, JSONPath against the document, mock items
//!    against the schema, assertions against the sample response);
//! 4. returns typed output with the sources it used.
//!
//! Notes and personal to-dos are only sent to cloud providers when the user enabled
//! `allowRemoteContext`; everything else sends just the user's explicit input.

use std::path::Path;
use std::sync::OnceLock;

use devall_core::todos::Priority;
use devall_core::todos::dates::{self, DateCtx};
use devall_core::{AssertOp, AssertTarget, Assertion, Auth, Body, HttpRequestFile, KeyValue, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::{self, Budget, SourceRef};
use crate::prompts::{self, fence};
use crate::provider::{ChatMessage, ChatRequest, JsonSchemaSpec, PayloadPreview, Provider};
use crate::redact::Redactor;
use crate::structured::{self, Structured};
use crate::{AiError, Result, schema};

/// Everything a feature may draw on besides its input.
pub struct AiEnv<'a> {
    pub project: Option<&'a Project>,
    /// App data directory holding personal to-dos (`<base>/todos/<project id>.json`).
    pub todo_base: Option<&'a Path>,
    pub redactor: Redactor,
    /// User opted in to sending notes/to-dos to cloud providers.
    pub allow_remote_context: bool,
    /// `YYYY-MM-DD`, for relative dates in extracted to-dos.
    pub today: String,
    /// Semantic-index hits for an Ask question (empty: keyword ranking only).
    pub semantic: Vec<crate::semantic::SearchHit>,
}

impl<'a> AiEnv<'a> {
    pub fn new(project: Option<&'a Project>, todo_base: Option<&'a Path>, redactor: Redactor, allow_remote_context: bool) -> Self {
        Self { project, todo_base, redactor, allow_remote_context, today: chrono::Local::now().format("%Y-%m-%d").to_string(), semantic: Vec::new() }
    }

    /// Ranks Ask sources with these semantic hits as well as keywords.
    pub fn with_semantic(mut self, hits: Vec<crate::semantic::SearchHit>) -> Self {
        self.semantic = hits;
        self
    }

    fn project(&self) -> Result<&'a Project> {
        self.project.ok_or_else(|| AiError::Invalid("open a project first".into()))
    }

    fn context_allowed(&self, provider: &Provider) -> Result<()> {
        if provider.is_remote() && !self.allow_remote_context {
            return Err(AiError::RemoteContextBlocked(provider.config.name.clone()));
        }
        Ok(())
    }
}

/// HTTP exchange handed to "explain this response" and "suggest tests".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseContext {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub request_headers: Vec<(String, String)>,
    #[serde(default)]
    pub request_body: Option<String>,
    pub status: u16,
    #[serde(default)]
    pub status_text: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub duration_ms: Option<f64>,
    /// Failed assertions (labels/messages).
    #[serde(default)]
    pub failures: Vec<String>,
    /// Extra context: matching mock stub, spec excerpt, recent log lines.
    #[serde(default)]
    pub notes: Option<String>,
}

/// One document the host gathered for "explain this failure" (already redacted by the host;
/// redacted again before sending).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureSource {
    /// `mockStub`, `mockNearMiss`, `event`, `podEvents`, `podLogs`, …
    pub kind: String,
    /// Stable reference the UI opens: `mocks/mappings/orders.json#<stub id>`, `event:<id>`,
    /// `pod:<context>/<namespace>/<name>`.
    pub source: String,
    pub title: String,
    pub text: String,
}

/// A failed exchange plus the context gathered around it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureContext {
    #[serde(flatten)]
    pub exchange: ResponseContext,
    /// Transport error when there was no response (connection refused, timeout, TLS…).
    #[serde(default)]
    pub error: Option<String>,
    /// Project context: mock stub, events, pod events/logs. Only sent to cloud providers when
    /// the user allowed project context; the request/response itself (S1) always is.
    #[serde(default)]
    pub sources: Vec<FailureSource>,
}

fn ten() -> usize {
    10
}

fn default_dialect() -> String {
    "postgres".into()
}

/// Feature + input, as sent by the UI: `{ "feature": "nlRegex", "input": { … } }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "feature", content = "input", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FeatureInput {
    Ask {
        question: String,
    },
    NlRegex {
        description: String,
        #[serde(default)]
        positive: Vec<String>,
        #[serde(default)]
        negative: Vec<String>,
    },
    NlJsonPath {
        description: String,
        sample_json: String,
    },
    NlJq {
        description: String,
        sample_json: String,
    },
    NlRequest {
        description: String,
    },
    MockData {
        schema_or_sample: String,
        #[serde(default = "ten")]
        count: usize,
    },
    ExplainResponse(ResponseContext),
    ExplainError {
        text: String,
    },
    /// "Explain this failure": the exchange plus gathered context, answered with citations.
    ExplainFailure(FailureContext),
    /// Names for an OpenAPI draft inferred from traffic (YAML or JSON text).
    OpenapiNames {
        spec: String,
    },
    /// NL → one SQL statement for a connection (schema summary only, never rows). `session`
    /// lets the host check it with `EXPLAIN`.
    NlSql {
        question: String,
        #[serde(default = "default_dialect")]
        dialect: String,
        schema: String,
        /// The user asked for a write and the session is read-write.
        #[serde(default)]
        allow_writes: bool,
        #[serde(default)]
        session: Option<String>,
    },
    /// NL → a MongoDB find or aggregate for one collection (sampled field names/types).
    NlMongo {
        question: String,
        database: String,
        collection: String,
        fields: String,
        #[serde(default)]
        session: Option<String>,
    },
    /// NL → one kubectl command for a context/namespace, checked by parsing and a server-side
    /// dry run.
    NlKubectl {
        question: String,
        #[serde(default)]
        context: String,
        #[serde(default)]
        namespace: String,
        /// Known resource kinds (comma-separated), so the model uses real ones.
        #[serde(default)]
        kinds: String,
    },
    SuggestTests(ResponseContext),
    SummarizeNotes {
        #[serde(default)]
        paths: Vec<String>,
    },
    TodosFromText {
        text: String,
    },
}

impl FeatureInput {
    pub fn name(&self) -> &'static str {
        match self {
            FeatureInput::Ask { .. } => "ask",
            FeatureInput::NlRegex { .. } => "nlRegex",
            FeatureInput::NlJsonPath { .. } => "nlJsonPath",
            FeatureInput::NlJq { .. } => "nlJq",
            FeatureInput::NlRequest { .. } => "nlRequest",
            FeatureInput::MockData { .. } => "mockData",
            FeatureInput::ExplainResponse(_) => "explainResponse",
            FeatureInput::ExplainError { .. } => "explainError",
            FeatureInput::ExplainFailure(_) => "explainFailure",
            FeatureInput::OpenapiNames { .. } => "openapiNames",
            FeatureInput::NlSql { .. } => "nlSql",
            FeatureInput::NlMongo { .. } => "nlMongo",
            FeatureInput::NlKubectl { .. } => "nlKubectl",
            FeatureInput::SuggestTests(_) => "suggestTests",
            FeatureInput::SummarizeNotes { .. } => "summarizeNotes",
            FeatureInput::TodosFromText { .. } => "todosFromText",
        }
    }

    /// Parses `{feature, input}` from loose JSON (UI / IPC).
    pub fn from_parts(feature: &str, input: Value) -> Result<Self> {
        serde_json::from_value(json!({ "feature": feature, "input": input })).map_err(|e| AiError::Invalid(format!("bad input for `{feature}`: {e}")))
    }
}

/// A fully built, redacted request.
#[derive(Debug, Clone)]
pub struct PreparedCall {
    pub feature: &'static str,
    pub request: ChatRequest,
    pub sources: Vec<SourceRef>,
    pub redactions: usize,
    pub stream: bool,
    /// Gathered sources left out (cloud provider without the project-context opt-in).
    pub omitted: Vec<String>,
}

/// "What will be sent".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub feature: String,
    pub payload: PayloadPreview,
    pub sources: Vec<SourceRef>,
    pub redactions: usize,
    pub remote: bool,
    pub notice: String,
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}\n…[truncated, {} characters total]", s.chars().take(n).collect::<String>(), s.chars().count()) }
}

fn schema_spec(name: &str, schema: Value, strict: bool) -> Option<JsonSchemaSpec> {
    Some(JsonSchemaSpec { name: name.into(), schema, strict })
}

fn system(base: &str) -> Option<String> {
    Some(format!("{base}\n{}", prompts::DATA_NOTICE))
}

fn pretty_json_or_text(s: &str, cap: usize) -> String {
    match serde_json::from_str::<Value>(s) {
        Ok(v) => clip(&serde_json::to_string_pretty(&v).unwrap_or_else(|_| s.to_string()), cap),
        Err(_) => clip(s, cap),
    }
}

fn headers_text(h: &[(String, String)]) -> String {
    h.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")
}

fn exchange_text(r: &ResponseContext, body_cap: usize) -> String {
    let mut s = String::new();
    if !r.method.is_empty() || !r.url.is_empty() {
        s.push_str(&format!("Request: {} {}\n", r.method, r.url));
    }
    if !r.request_headers.is_empty() {
        s.push_str(&format!("Request headers:\n{}\n", headers_text(&r.request_headers)));
    }
    if let Some(b) = r.request_body.as_deref().filter(|b| !b.trim().is_empty()) {
        s.push_str(&format!("Request body:\n{}\n", fence("data", &pretty_json_or_text(b, 2000))));
    }
    s.push_str(&format!("Response status: {} {}", r.status, r.status_text));
    if let Some(ms) = r.duration_ms {
        s.push_str(&format!(" ({ms:.0} ms)"));
    }
    s.push('\n');
    if !r.headers.is_empty() {
        s.push_str(&format!("Response headers:\n{}\n", headers_text(&r.headers)));
    }
    s.push_str(&format!("Response body:\n{}\n", fence("response", &pretty_json_or_text(&r.body, body_cap))));
    if !r.failures.is_empty() {
        s.push_str(&format!("Failed assertions:\n{}\n", r.failures.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")));
    }
    if let Some(n) = r.notes.as_deref().filter(|n| !n.trim().is_empty()) {
        s.push_str(&format!("Additional context:\n{}\n", fence("data", &clip(n, 3000))));
    }
    s
}

fn failure_exchange_text(f: &FailureContext) -> String {
    match &f.error {
        Some(e) if f.exchange.status == 0 => {
            let r = &f.exchange;
            let mut s = format!("Request: {} {}\n", r.method, r.url);
            if !r.request_headers.is_empty() {
                s.push_str(&format!("Request headers:\n{}\n", headers_text(&r.request_headers)));
            }
            if let Some(b) = r.request_body.as_deref().filter(|b| !b.trim().is_empty()) {
                s.push_str(&format!("Request body:\n{}\n", pretty_json_or_text(b, 2000)));
            }
            s.push_str(&format!("No response — the request failed: {e}\n"));
            if let Some(n) = r.notes.as_deref().filter(|n| !n.trim().is_empty()) {
                s.push_str(&format!("User note: {}\n", clip(n, 1000)));
            }
            s
        }
        _ => exchange_text(&f.exchange, 5000),
    }
}

/// Environment/variable *names* and the request outline (no values) for request drafting.
fn request_context(project: &Project) -> String {
    let mut s = String::new();
    if !project.manifest.variables.is_empty() {
        s.push_str(&format!("Project variables: {}\n", project.manifest.variables.keys().map(|k| format!("{{{{{k}}}}}")).collect::<Vec<_>>().join(", ")));
    }
    for e in project.environments().unwrap_or_default() {
        let vars: Vec<String> = e.file.variables.keys().map(|k| format!("{{{{{k}}}}}")).collect();
        let secrets: Vec<String> = e.file.secrets.iter().map(|k| format!("{{{{secret.{k}}}}}")).collect();
        s.push_str(&format!("Environment {}: variables {}; secrets {}\n", e.file.name, if vars.is_empty() { "none".into() } else { vars.join(", ") }, if secrets.is_empty() { "none".into() } else { secrets.join(", ") }));
    }
    if let Ok(docs) = devall_core::context::collect(project, None, &devall_core::context::ContextOptions { include_notes: false, include_todos: false, include_requests: true, max_chars_per_doc: 3000 })
        && let Some(r) = docs.iter().find(|d| d.kind == "requests")
    {
        s.push_str(&format!("Existing requests:\n{}\n", fence("data", &r.text)));
    }
    s
}

/// The item schema for mock data plus whether it was inferred from a sample.
fn mock_item_schema(schema_or_sample: &str) -> Result<(Value, bool, Option<Value>)> {
    let v: Value = serde_json::from_str(schema_or_sample.trim()).map_err(|e| AiError::Invalid(format!("paste a JSON Schema or a sample JSON document: {e}")))?;
    if schema::looks_like_schema(&v) {
        // A schema for an array of items → use its item schema.
        if v.get("type") == Some(&json!("array"))
            && let Some(items) = v.get("items")
        {
            let mut item = items.clone();
            for key in ["$defs", "definitions"] {
                if let Some(d) = v.get(key) {
                    item[key] = d.clone();
                }
            }
            return Ok((item, false, None));
        }
        return Ok((v, false, None));
    }
    let sample = match &v {
        Value::Array(a) => a.first().cloned().ok_or_else(|| AiError::Invalid("the sample array is empty".into()))?,
        other => other.clone(),
    };
    Ok((schema::infer(&sample), true, Some(sample)))
}

/// Builds the (redacted) request for a feature.
pub fn prepare(input: &FeatureInput, env: &AiEnv<'_>, provider: &Provider) -> Result<PreparedCall> {
    let mut sources = Vec::new();
    let mut omitted = Vec::new();
    let mut stream = false;
    let mut req = ChatRequest::default();
    match input {
        FeatureInput::Ask { question } => {
            let project = env.project()?;
            env.context_allowed(provider)?;
            if question.trim().is_empty() {
                return Err(AiError::Invalid("ask a question".into()));
            }
            let budget = Budget::default();
            let docs = if env.semantic.is_empty() {
                context::relevant(project, env.todo_base, question, budget)?
            } else {
                context::relevant_hybrid(project, env.todo_base, question, budget, &env.semantic)?
            };
            let (text, refs) = context::format_sources(&docs, budget.max_chars);
            sources = refs;
            req.system = system(prompts::ASK_SYSTEM);
            let body = if text.is_empty() { "(no notes, to-dos or requests in this project yet)".to_string() } else { text };
            req.messages.push(ChatMessage::user(format!("Project: {}\n\nSources:\n{body}\nQuestion: {}", project.manifest.name, question.trim())));
            req.temperature = Some(0.2);
            req.max_tokens = Some(1500);
            stream = true;
        }
        FeatureInput::NlRegex { description, positive, negative } => {
            req.system = system(prompts::REGEX_SYSTEM);
            let list = |v: &[String]| if v.is_empty() { "(none)".to_string() } else { v.join("\n") };
            req.messages.push(ChatMessage::user(format!(
                "Description: {}\nPositive samples (one per line, each must match):\n{}\nNegative samples (one per line, none may match):\n{}",
                description.trim(),
                fence("data", &list(positive)),
                fence("data", &list(negative))
            )));
            req.json_schema = schema_spec("regex", prompts::regex_schema(), true);
            req.temperature = Some(0.1);
            req.max_tokens = Some(800);
        }
        FeatureInput::NlJsonPath { description, sample_json } => {
            let v: Value = serde_json::from_str(sample_json.trim()).map_err(|e| AiError::Invalid(format!("the sample is not valid JSON: {e}")))?;
            req.system = system(prompts::JSONPATH_SYSTEM);
            req.messages.push(ChatMessage::user(format!(
                "Description: {}\nSample document:\n{}",
                description.trim(),
                fence("data", &clip(&serde_json::to_string_pretty(&v).unwrap_or_default(), 6000))
            )));
            req.json_schema = schema_spec("jsonpath", prompts::jsonpath_schema(), true);
            req.temperature = Some(0.1);
            req.max_tokens = Some(600);
        }
        FeatureInput::NlJq { description, sample_json } => {
            let v: Value = serde_json::from_str(sample_json.trim()).map_err(|e| AiError::Invalid(format!("the sample is not valid JSON: {e}")))?;
            req.system = system(prompts::JQ_SYSTEM);
            req.messages.push(ChatMessage::user(format!(
                "Description: {}\nSample document:\n{}",
                description.trim(),
                fence("data", &clip(&serde_json::to_string_pretty(&v).unwrap_or_default(), 6000))
            )));
            req.json_schema = schema_spec("jq", prompts::jq_schema(), true);
            req.temperature = Some(0.1);
            req.max_tokens = Some(600);
        }
        FeatureInput::NlRequest { description } => {
            req.system = system(prompts::REQUEST_SYSTEM);
            let ctx = env.project.map(request_context).unwrap_or_default();
            req.messages.push(ChatMessage::user(format!("{ctx}\nDescribe the request to create:\n{}", fence("data", description.trim()))));
            req.json_schema = schema_spec("http_request", prompts::request_schema(), true);
            req.temperature = Some(0.2);
            req.max_tokens = Some(1500);
        }
        FeatureInput::MockData { schema_or_sample, count } => {
            let n = (*count).clamp(1, 50);
            let (item, _, sample) = mock_item_schema(schema_or_sample)?;
            req.system = system(prompts::MOCK_SYSTEM);
            let mut msg = format!("Generate exactly {n} items. Each item must match this JSON Schema:\n{}", fence("data", &serde_json::to_string_pretty(&item).unwrap_or_default()));
            if let Some(s) = sample {
                msg.push_str(&format!("\nExample item (vary the values):\n{}", fence("data", &clip(&serde_json::to_string_pretty(&s).unwrap_or_default(), 3000))));
            }
            req.messages.push(ChatMessage::user(msg));
            req.json_schema = schema_spec("mock_data", prompts::mock_schema(&item), false);
            req.temperature = Some(0.8);
            req.max_tokens = Some((600 + n as u32 * 250).min(12_000));
        }
        FeatureInput::ExplainResponse(r) => {
            req.system = system(prompts::EXPLAIN_RESPONSE_SYSTEM);
            req.messages.push(ChatMessage::user(exchange_text(r, 6000)));
            req.json_schema = schema_spec("response_explanation", prompts::explain_response_schema(), true);
            req.temperature = Some(0.2);
            req.max_tokens = Some(1500);
        }
        FeatureInput::ExplainError { text } => {
            if text.trim().is_empty() {
                return Err(AiError::Invalid("paste a stack trace or log excerpt".into()));
            }
            req.system = system(prompts::EXPLAIN_ERROR_SYSTEM);
            req.messages.push(ChatMessage::user(format!("Explain this error:\n{}", fence("log", &clip(text, 12_000)))));
            req.json_schema = schema_spec("error_explanation", prompts::explain_error_schema(), true);
            req.temperature = Some(0.2);
            req.max_tokens = Some(1500);
        }
        FeatureInput::ExplainFailure(fc) => {
            // S1 (the user's own exchange) always goes; gathered project context only to local
            // models unless the user allowed project context for cloud providers.
            let allowed = !provider.is_remote() || env.allow_remote_context;
            let mut docs = vec![devall_core::context::ContextDoc {
                kind: "exchange".into(),
                source: "request".into(),
                title: format!("{} {}", fc.exchange.method, fc.exchange.url).trim().to_string(),
                text: failure_exchange_text(fc),
            }];
            for s in &fc.sources {
                if allowed {
                    docs.push(devall_core::context::ContextDoc { kind: s.kind.clone(), source: s.source.clone(), title: s.title.clone(), text: clip(&s.text, 4000) });
                } else {
                    omitted.push(s.title.clone());
                }
            }
            let (text, refs) = context::format_sources(&docs, 18_000);
            for d in docs.iter().skip(refs.len()) {
                omitted.push(format!("{} (over the size budget)", d.title));
            }
            sources = refs;
            req.system = system(prompts::EXPLAIN_FAILURE_SYSTEM);
            let mut msg = format!("Explain why this request failed.\n\nSources:\n{text}");
            if !allowed && !fc.sources.is_empty() {
                msg.push_str("(Other project context was gathered but not shared with this cloud model.)\n");
            }
            req.messages.push(ChatMessage::user(msg));
            req.json_schema = schema_spec("failure_explanation", prompts::explain_response_schema(), true);
            req.temperature = Some(0.2);
            req.max_tokens = Some(1800);
        }
        FeatureInput::OpenapiNames { spec } => {
            let doc = crate::openapi_names::parse_spec(spec)?;
            let ops = crate::openapi_names::operations(&doc).len();
            if ops == 0 {
                return Err(AiError::Invalid("the spec has no operations to name".into()));
            }
            if ops > 150 {
                return Err(AiError::Invalid(format!("{ops} operations is too many for one naming pass — export fewer exchanges")));
            }
            req.system = system(crate::openapi_names::NAMING_SYSTEM);
            req.messages.push(ChatMessage::user(crate::openapi_names::user_message(&doc)));
            req.json_schema = schema_spec("openapi_names", crate::openapi_names::naming_schema(), true);
            req.temperature = Some(0.2);
            req.max_tokens = Some((400 + ops as u32 * 140).min(12_000));
        }
        FeatureInput::NlSql { question, dialect, schema, allow_writes, .. } => {
            if question.trim().is_empty() {
                return Err(AiError::Invalid("describe the query".into()));
            }
            req.system = system(&crate::nlquery::sql_system(dialect, *allow_writes));
            req.messages.push(ChatMessage::user(crate::nlquery::sql_message(question, &clip(schema, 14_000))));
            req.json_schema = schema_spec("sql", crate::nlquery::sql_schema(), true);
            req.temperature = Some(0.1);
            req.max_tokens = Some(1200);
        }
        FeatureInput::NlMongo { question, database, collection, fields, .. } => {
            if question.trim().is_empty() {
                return Err(AiError::Invalid("describe the query".into()));
            }
            req.system = system(crate::nlquery::MONGO_SYSTEM);
            req.messages.push(ChatMessage::user(crate::nlquery::mongo_message(question, database, collection, &clip(fields, 8000))));
            req.json_schema = schema_spec("mongo_query", crate::nlquery::mongo_schema(), true);
            req.temperature = Some(0.1);
            req.max_tokens = Some(1200);
        }
        FeatureInput::NlKubectl { question, context, namespace, kinds } => {
            if question.trim().is_empty() {
                return Err(AiError::Invalid("describe what you want to do".into()));
            }
            let ns = if namespace.trim().is_empty() { "default" } else { namespace.trim() };
            let ctx = if context.trim().is_empty() { "current-context" } else { context.trim() };
            req.system = system(&crate::nlquery::kubectl_system(ctx, ns));
            let kinds = if kinds.trim().is_empty() { "pods, deployments, statefulsets, daemonsets, replicasets, jobs, cronjobs, services, ingresses, configmaps, secrets, persistentvolumeclaims, nodes, namespaces, events" } else { kinds.as_str() };
            req.messages.push(ChatMessage::user(crate::nlquery::kubectl_message(question, &clip(kinds, 4000))));
            req.json_schema = schema_spec("kubectl", crate::nlquery::kubectl_schema(), true);
            req.temperature = Some(0.1);
            req.max_tokens = Some(600);
        }
        FeatureInput::SuggestTests(r) => {
            req.system = system(prompts::SUGGEST_TESTS_SYSTEM);
            req.messages.push(ChatMessage::user(format!("Suggest assertions for this response:\n{}", exchange_text(r, 6000))));
            req.json_schema = schema_spec("tests", prompts::tests_schema(), true);
            req.temperature = Some(0.2);
            req.max_tokens = Some(1500);
        }
        FeatureInput::SummarizeNotes { paths } => {
            let project = env.project()?;
            env.context_allowed(provider)?;
            let budget = Budget::default();
            let docs = context::notes_docs(project, paths, budget.per_doc)?;
            if docs.is_empty() {
                return Err(AiError::Invalid("this project has no notes yet".into()));
            }
            let (text, refs) = context::format_sources(&docs, budget.max_chars);
            sources = refs;
            req.system = system(prompts::SUMMARIZE_SYSTEM);
            req.messages.push(ChatMessage::user(format!("Summarize these notes:\n{text}")));
            req.json_schema = schema_spec("notes_summary", prompts::summarize_schema(), true);
            req.temperature = Some(0.2);
            req.max_tokens = Some(1500);
        }
        FeatureInput::TodosFromText { text } => {
            if text.trim().is_empty() {
                return Err(AiError::Invalid("paste some text to turn into to-dos".into()));
            }
            req.system = system(&prompts::TODOS_SYSTEM.replace("{today}", &env.today));
            req.messages.push(ChatMessage::user(fence("data", &clip(text, 8000))));
            req.json_schema = schema_spec("todos", prompts::todos_schema(), true);
            req.temperature = Some(0.2);
            req.max_tokens = Some(1200);
        }
    }
    let redactions = env.redactor.redact_request(&mut req, provider.is_remote());
    Ok(PreparedCall { feature: input.name(), request: req, sources, redactions, stream, omitted })
}

/// The exact payload a feature would send, without sending it.
pub fn preview(input: &FeatureInput, env: &AiEnv<'_>, provider: &Provider) -> Result<Preview> {
    let call = prepare(input, env, provider)?;
    let payload = provider.preview(&call.request, call.stream);
    let host = url::Url::parse(&payload.url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let mut notice = if payload.remote {
        format!("Sent to {host} ({}). Secret values and credential-like strings are masked; {} replacement(s) made.", provider.config.name, call.redactions)
    } else {
        format!("Stays on this machine ({host}). Keychain/.env secret values are masked; {} replacement(s) made.", call.redactions)
    };
    if !call.omitted.is_empty() {
        notice.push_str(&format!(
            " Not sent: {} — project context only goes to cloud providers when \"Allow project context for cloud providers\" is on.",
            call.omitted.join(", ")
        ));
    }
    Ok(Preview { feature: call.feature.into(), remote: payload.remote, payload, sources: call.sources, redactions: call.redactions, notice })
}

// ── Outputs ─────────────────────────────────────────────────────────────────────────────────

/// Common metadata on every result.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub provider: String,
    pub model: String,
    pub remote: bool,
    pub redactions: usize,
    pub repaired: bool,
    pub schema_enforced: bool,
}

fn meta(provider: &Provider, call: &PreparedCall, s: Option<&Structured>, model: &str) -> Meta {
    Meta {
        provider: provider.config.name.clone(),
        model: model.to_string(),
        remote: provider.is_remote(),
        redactions: call.redactions,
        repaired: s.is_some_and(|s| s.repaired),
        schema_enforced: s.is_some_and(|s| s.schema_enforced),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskAnswer {
    /// Markdown with `[S1]` citation markers.
    pub answer: String,
    /// Sources the answer cites.
    pub citations: Vec<SourceRef>,
    /// Every source that was provided.
    pub sources: Vec<SourceRef>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleCheck {
    pub sample: String,
    pub should_match: bool,
    pub matched: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegexSuggestion {
    pub pattern: String,
    pub flags: String,
    pub explanation: String,
    pub compiles: bool,
    pub checks: Vec<SampleCheck>,
    /// Compiles and every sample behaves as expected.
    pub valid: bool,
    pub problem: Option<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonPathSuggestion {
    pub path: String,
    pub explanation: String,
    pub matches: Vec<Value>,
    pub count: usize,
    pub valid: bool,
    pub problem: Option<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JqSuggestion {
    pub filter: String,
    pub explanation: String,
    /// The filter's output on the sample (truncated).
    pub output: String,
    pub valid: bool,
    pub problem: Option<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDraft {
    pub request: HttpRequestFile,
    pub explanation: String,
    /// `{{references}}` the draft uses.
    pub variables: Vec<String>,
    /// References not defined by the project or any environment.
    pub unknown_variables: Vec<String>,
    pub problem: Option<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MockData {
    pub items: Vec<Value>,
    pub schema: Value,
    pub inferred_schema: bool,
    pub valid: bool,
    pub errors: Vec<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseExplanation {
    pub summary: String,
    pub likely_causes: Vec<String>,
    pub suggested_fixes: Vec<Fix>,
    pub severity: String,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorExplanation {
    pub summary: String,
    pub root_cause: String,
    pub suggested_fixes: Vec<Fix>,
    pub relevant_lines: Vec<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureExplanation {
    pub summary: String,
    pub likely_causes: Vec<String>,
    pub suggested_fixes: Vec<Fix>,
    pub severity: String,
    /// Sources the answer cites, in order of appearance.
    pub citations: Vec<SourceRef>,
    /// Every source that was sent (`S1` = the exchange).
    pub sources: Vec<SourceRef>,
    /// Gathered sources that were not sent (cloud provider without the opt-in).
    pub omitted: Vec<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedTest {
    pub assertion: Assertion,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestSuggestions {
    pub tests: Vec<SuggestedTest>,
    /// Suggestions dropped by validation, with the reason.
    pub dropped: Vec<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesSummary {
    pub summary: String,
    pub key_points: Vec<String>,
    pub open_questions: Vec<String>,
    pub citations: Vec<SourceRef>,
    pub sources: Vec<SourceRef>,
    pub meta: Meta,
}

/// A suggested task, in the shape of a to-do quick add (the UI adds the ones the user keeps).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoSuggestion {
    pub text: String,
    /// One sentence of context (may be empty).
    pub description: String,
    /// 1 (p1, most urgent) … 4 (none).
    pub priority: Priority,
    /// Due date as natural language the to-do parser understands (`next fri 5pm`, `every mon`,
    /// `2026-11-03`), validated.
    pub due: Option<String>,
    /// The day `due` resolves to (`YYYY-MM-DD`), for display.
    pub due_date: Option<String>,
    pub labels: Vec<String>,
    pub links: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoSuggestions {
    pub todos: Vec<TodoSuggestion>,
    pub meta: Meta,
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().trim().to_string()
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    v[k].as_array().map(|a| a.iter().filter_map(Value::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
}

fn fixes(v: &Value) -> Vec<Fix> {
    serde_json::from_value(v["suggestedFixes"].clone()).unwrap_or_default()
}

// ── Ask ─────────────────────────────────────────────────────────────────────────────────────

/// Q&A over notes, to-dos and requests, streaming tokens to `on_token`.
pub async fn ask_stream<F: FnMut(&str) + Send>(env: &AiEnv<'_>, provider: &Provider, question: &str, on_token: F) -> Result<AskAnswer> {
    let call = prepare(&FeatureInput::Ask { question: question.into() }, env, provider)?;
    let resp = provider.chat_stream(&call.request, on_token).await?;
    let citations = context::citations(&resp.text, &call.sources);
    Ok(AskAnswer { meta: meta(provider, &call, None, &resp.model), answer: resp.text, citations, sources: call.sources })
}

pub async fn ask(env: &AiEnv<'_>, provider: &Provider, question: &str) -> Result<AskAnswer> {
    let call = prepare(&FeatureInput::Ask { question: question.into() }, env, provider)?;
    let resp = provider.chat(&call.request).await?;
    let citations = context::citations(&resp.text, &call.sources);
    Ok(AskAnswer { meta: meta(provider, &call, None, &resp.model), answer: resp.text, citations, sources: call.sources })
}

// ── NL → regex ──────────────────────────────────────────────────────────────────────────────

fn normalize_pattern(p: &str) -> String {
    let p = p.trim();
    if p.len() > 2 && p.starts_with('/') && p.rfind('/').is_some_and(|i| i > 0) {
        let end = p.rfind('/').unwrap();
        return p[1..end].to_string();
    }
    p.to_string()
}

fn build_regex(pattern: &str, flags: &str) -> std::result::Result<regex::Regex, String> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'))
        .ignore_whitespace(flags.contains('x'))
        .size_limit(1 << 22)
        .build()
        .map_err(|e| e.to_string())
}

fn regex_checks(re: &regex::Regex, positive: &[String], negative: &[String]) -> Vec<SampleCheck> {
    positive
        .iter()
        .map(|s| SampleCheck { sample: s.clone(), should_match: true, matched: re.is_match(s) })
        .chain(negative.iter().map(|s| SampleCheck { sample: s.clone(), should_match: false, matched: re.is_match(s) }))
        .collect()
}

pub async fn nl_to_regex(env: &AiEnv<'_>, provider: &Provider, description: &str, positive: &[String], negative: &[String]) -> Result<RegexSuggestion> {
    let input = FeatureInput::NlRegex { description: description.into(), positive: positive.to_vec(), negative: negative.to_vec() };
    let call = prepare(&input, env, provider)?;
    let (pos, neg) = (positive.to_vec(), negative.to_vec());
    let check = move |v: &Value| -> std::result::Result<(), String> {
        let pattern = normalize_pattern(v["pattern"].as_str().unwrap_or_default());
        if pattern.is_empty() {
            return Err("the pattern is empty.".into());
        }
        let re = build_regex(&pattern, v["flags"].as_str().unwrap_or_default()).map_err(|e| format!("the pattern does not compile with the Rust regex crate: {e}"))?;
        let bad: Vec<String> = regex_checks(&re, &pos, &neg)
            .into_iter()
            .filter(|c| c.matched != c.should_match)
            .map(|c| format!("{} `{}`", if c.should_match { "does not match positive sample" } else { "matches negative sample" }, c.sample))
            .collect();
        if bad.is_empty() { Ok(()) } else { Err(format!("pattern `{pattern}` {}.", bad.join("; "))) }
    };
    let out = structured::generate(provider, &call.request, &check).await?;
    let pattern = normalize_pattern(&s(&out.value, "pattern"));
    let flags: String = s(&out.value, "flags").chars().filter(|c| "imsx".contains(*c)).collect();
    let (compiles, checks) = match build_regex(&pattern, &flags) {
        Ok(re) => (true, regex_checks(&re, positive, negative)),
        Err(_) => (false, Vec::new()),
    };
    let valid = compiles && checks.iter().all(|c| c.matched == c.should_match);
    Ok(RegexSuggestion { meta: meta(provider, &call, Some(&out), &out.model), pattern, flags, explanation: s(&out.value, "explanation"), compiles, checks, valid, problem: out.problem })
}

// ── NL → JSONPath ───────────────────────────────────────────────────────────────────────────

fn query_path(path: &str, doc: &Value) -> std::result::Result<Vec<Value>, String> {
    let p = serde_json_path::JsonPath::parse(path.trim()).map_err(|e| format!("invalid JSONPath: {e}"))?;
    Ok(p.query(doc).all().into_iter().cloned().collect())
}

pub async fn nl_to_jsonpath(env: &AiEnv<'_>, provider: &Provider, description: &str, sample_json: &str) -> Result<JsonPathSuggestion> {
    let input = FeatureInput::NlJsonPath { description: description.into(), sample_json: sample_json.into() };
    let call = prepare(&input, env, provider)?;
    let doc: Value = serde_json::from_str(sample_json.trim()).map_err(|e| AiError::Invalid(e.to_string()))?;
    let doc2 = doc.clone();
    let check = move |v: &Value| -> std::result::Result<(), String> {
        let path = v["path"].as_str().unwrap_or_default();
        let hits = query_path(path, &doc2)?;
        if hits.is_empty() { Err(format!("`{path}` selects nothing in the sample document.")) } else { Ok(()) }
    };
    let out = structured::generate(provider, &call.request, &check).await?;
    let path = s(&out.value, "path");
    let (matches, valid) = match query_path(&path, &doc) {
        Ok(m) => {
            let ok = !m.is_empty();
            (m, ok)
        }
        Err(_) => (Vec::new(), false),
    };
    let count = matches.len();
    Ok(JsonPathSuggestion {
        meta: meta(provider, &call, Some(&out), &out.model),
        path,
        explanation: s(&out.value, "explanation"),
        matches: matches.into_iter().take(20).collect(),
        count,
        valid,
        problem: out.problem,
    })
}

// ── NL → jq ─────────────────────────────────────────────────────────────────────────────────

/// Runs `filter` on `sample` with the same jq engine as the Utilities page and the response filter.
pub fn run_jq(filter: &str, sample: &str) -> std::result::Result<String, String> {
    if filter.trim().is_empty() {
        return Err("the filter is empty.".into());
    }
    devall_utils::run("jq", sample, &json!({ "filter": filter })).map_err(|e| e.to_string())
}

/// A filter is useful when it runs and yields something other than nothing / a lone `null`.
fn check_jq(filter: &str, sample: &str) -> std::result::Result<String, String> {
    let out = run_jq(filter, sample).map_err(|e| format!("`{filter}` fails on the sample: {e}"))?;
    match out.trim() {
        "" => Err(format!("`{filter}` produces no output on the sample document.")),
        "null" if filter.trim() != "null" => Err(format!("`{filter}` returns null on the sample document (wrong field names?).")),
        _ => Ok(out),
    }
}

pub async fn nl_to_jq(env: &AiEnv<'_>, provider: &Provider, description: &str, sample_json: &str) -> Result<JqSuggestion> {
    let input = FeatureInput::NlJq { description: description.into(), sample_json: sample_json.into() };
    let call = prepare(&input, env, provider)?;
    let sample = sample_json.trim().to_string();
    let sample2 = sample.clone();
    let check = move |v: &Value| -> std::result::Result<(), String> { check_jq(v["filter"].as_str().unwrap_or_default(), &sample2).map(|_| ()) };
    let out = structured::generate(provider, &call.request, &check).await?;
    let filter = s(&out.value, "filter");
    let (output, valid) = match check_jq(&filter, &sample) {
        Ok(o) => (o, true),
        Err(e) => (e, false),
    };
    Ok(JqSuggestion {
        meta: meta(provider, &call, Some(&out), &out.model),
        filter,
        explanation: s(&out.value, "explanation"),
        output: clip(&output, 2000),
        valid,
        problem: out.problem,
    })
}

// ── NL → request ────────────────────────────────────────────────────────────────────────────

fn template_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").unwrap())
}

/// JSON validity with `{{templates}}` treated as values.
fn json_ok_with_templates(body: &str) -> bool {
    let replaced = template_re().replace_all(body, "0");
    serde_json::from_str::<Value>(&replaced).is_ok()
}

fn kv_list(v: &Value) -> Vec<KeyValue> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| Some(KeyValue::new(x["name"].as_str()?.trim(), x["value"].as_str().unwrap_or_default()))).filter(|k| !k.name.is_empty()).collect())
        .unwrap_or_default()
}

/// Turns the model's JSON into a request file.
pub fn draft_to_request(v: &Value) -> HttpRequestFile {
    let mut r = HttpRequestFile::new(Some(s(v, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "New request".into()));
    r.method = s(v, "method").to_uppercase();
    if r.method.is_empty() {
        r.method = "GET".into();
    }
    r.url = s(v, "url");
    r.params = kv_list(&v["query"]);
    r.headers = kv_list(&v["headers"]);
    let body = v["body"].as_str().unwrap_or_default().to_string();
    r.body = match s(v, "bodyType").as_str() {
        "json" if !body.trim().is_empty() => Body::Json { content: pretty_json_or_text(&body, usize::MAX).trim_end_matches('\n').to_string() },
        "text" if !body.trim().is_empty() => Body::Text { content: body, content_type: None },
        "form" if !body.trim().is_empty() => Body::Form {
            fields: body
                .split(['&', '\n'])
                .filter_map(|pair| {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    (!k.trim().is_empty()).then(|| KeyValue::new(k.trim(), v.trim()))
                })
                .collect(),
        },
        _ => Body::None,
    };
    let auth_value = s(v, "authValue");
    r.auth = match s(v, "authType").as_str() {
        "none" => Auth::None,
        "bearer" => Auth::Bearer { token: auth_value, prefix: None },
        "basic" => {
            let (u, p) = auth_value.split_once(':').unwrap_or((auth_value.as_str(), ""));
            Auth::Basic { username: u.to_string(), password: p.to_string() }
        }
        "apiKey" => {
            let (k, val) = auth_value.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).unwrap_or(("X-Api-Key".into(), auth_value.clone()));
            Auth::ApiKey { key: k, value: val, location: devall_core::ApiKeyLocation::Header }
        }
        _ => Auth::Inherit,
    };
    r
}

fn known_variables(project: Option<&Project>) -> std::collections::BTreeSet<String> {
    let mut known: std::collections::BTreeSet<String> = devall_core::vars::DYNAMIC_NAMES.iter().map(|s| s.to_string()).collect();
    if let Some(p) = project {
        known.extend(p.manifest.variables.keys().cloned());
        for e in p.environments().unwrap_or_default() {
            known.extend(e.file.variables.keys().cloned());
            known.extend(e.file.secrets.iter().map(|n| format!("secret.{n}")));
        }
    }
    known
}

pub async fn nl_to_request(env: &AiEnv<'_>, provider: &Provider, description: &str) -> Result<RequestDraft> {
    let call = prepare(&FeatureInput::NlRequest { description: description.into() }, env, provider)?;
    let check = |v: &Value| -> std::result::Result<(), String> {
        let method = s(v, "method").to_uppercase();
        if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"].contains(&method.as_str()) {
            return Err(format!("`method` must be GET, POST, PUT, PATCH, DELETE, HEAD or OPTIONS, not `{method}`."));
        }
        let url = s(v, "url");
        if url.is_empty() {
            return Err("`url` is empty.".into());
        }
        if url.contains(char::is_whitespace) && !url.contains("{{") {
            return Err("`url` contains spaces.".into());
        }
        if s(v, "bodyType") == "json" && !v["body"].as_str().unwrap_or_default().trim().is_empty() && !json_ok_with_templates(v["body"].as_str().unwrap_or_default()) {
            return Err("`body` is not valid JSON.".into());
        }
        Ok(())
    };
    let out = structured::generate(provider, &call.request, &check).await?;
    let request = draft_to_request(&out.value);
    let text = serde_json::to_string(&request).unwrap_or_default();
    let variables = devall_core::vars::references(&text);
    let known = known_variables(env.project);
    let unknown_variables = variables.iter().filter(|v| !known.contains(*v) && !v.starts_with("$env.")).cloned().collect();
    Ok(RequestDraft { meta: meta(provider, &call, Some(&out), &out.model), request, explanation: s(&out.value, "explanation"), variables, unknown_variables, problem: out.problem })
}

// ── Mock data ───────────────────────────────────────────────────────────────────────────────

pub async fn generate_mock_data(env: &AiEnv<'_>, provider: &Provider, schema_or_sample: &str, count: usize) -> Result<MockData> {
    let n = count.clamp(1, 50);
    let input = FeatureInput::MockData { schema_or_sample: schema_or_sample.into(), count: n };
    let call = prepare(&input, env, provider)?;
    let (item, inferred, _) = mock_item_schema(schema_or_sample)?;
    let check = move |v: &Value| -> std::result::Result<(), String> {
        let got = v["items"].as_array().map(Vec::len).unwrap_or(0);
        if got < n { Err(format!("expected {n} items but got {got}.")) } else { Ok(()) }
    };
    let out = structured::generate(provider, &call.request, &check).await?;
    let mut items: Vec<Value> = out.value["items"].as_array().cloned().unwrap_or_default();
    items.truncate(n);
    let full = prompts::mock_schema(&item);
    let mut errors = schema::validate(&json!({ "items": items }), &full);
    if let Some(p) = &out.problem {
        errors.push(p.clone());
    }
    Ok(MockData { meta: meta(provider, &call, Some(&out), &out.model), valid: errors.is_empty(), items, schema: item, inferred_schema: inferred, errors })
}

// ── Explain ─────────────────────────────────────────────────────────────────────────────────

pub async fn explain_response(env: &AiEnv<'_>, provider: &Provider, ctx: &ResponseContext) -> Result<ResponseExplanation> {
    let call = prepare(&FeatureInput::ExplainResponse(ctx.clone()), env, provider)?;
    let out = structured::generate(provider, &call.request, &|_| Ok(())).await?;
    let v = &out.value;
    Ok(ResponseExplanation {
        meta: meta(provider, &call, Some(&out), &out.model),
        summary: s(v, "summary"),
        likely_causes: strings(v, "likelyCauses"),
        suggested_fixes: fixes(v),
        severity: Some(s(v, "severity")).filter(|x| ["info", "warning", "error"].contains(&x.as_str())).unwrap_or_else(|| if ctx.status >= 400 { "error".into() } else { "info".into() }),
    })
}

pub async fn explain_error(env: &AiEnv<'_>, provider: &Provider, text: &str) -> Result<ErrorExplanation> {
    let call = prepare(&FeatureInput::ExplainError { text: text.into() }, env, provider)?;
    let out = structured::generate(provider, &call.request, &|_| Ok(())).await?;
    let v = &out.value;
    Ok(ErrorExplanation {
        meta: meta(provider, &call, Some(&out), &out.model),
        summary: s(v, "summary"),
        root_cause: s(v, "rootCause"),
        suggested_fixes: fixes(v),
        relevant_lines: strings(v, "relevantLines"),
    })
}

/// Explains a failure from the exchange and the gathered context, citing `[S#]` sources.
pub async fn explain_failure(env: &AiEnv<'_>, provider: &Provider, ctx: &FailureContext) -> Result<FailureExplanation> {
    let call = prepare(&FeatureInput::ExplainFailure(ctx.clone()), env, provider)?;
    let n = call.sources.len();
    let check = move |v: &Value| -> std::result::Result<(), String> {
        let all = format!("{} {} {}", s(v, "summary"), strings(v, "likelyCauses").join(" "), v["suggestedFixes"]);
        let bad: Vec<String> = context_ids(&all).into_iter().filter(|i| *i == 0 || *i > n).map(|i| format!("S{i}")).collect();
        if bad.is_empty() { Ok(()) } else { Err(format!("it cites {} but only S1–S{n} exist.", bad.join(", "))) }
    };
    let out = structured::generate(provider, &call.request, &check).await?;
    let v = &out.value;
    let summary = s(v, "summary");
    let likely_causes = strings(v, "likelyCauses");
    let suggested_fixes = fixes(v);
    let all_text = format!("{summary}\n{}\n{}", likely_causes.join("\n"), suggested_fixes.iter().map(|f| format!("{} {}", f.title, f.detail)).collect::<Vec<_>>().join("\n"));
    Ok(FailureExplanation {
        meta: meta(provider, &call, Some(&out), &out.model),
        citations: context::citations(&all_text, &call.sources),
        severity: Some(s(v, "severity")).filter(|x| ["info", "warning", "error"].contains(&x.as_str())).unwrap_or_else(|| "error".into()),
        summary,
        likely_causes,
        suggested_fixes,
        sources: call.sources,
        omitted: call.omitted,
    })
}

/// `S<n>` numbers cited in `text`.
fn context_ids(text: &str) -> Vec<usize> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\bS(\d{1,3})\b").unwrap());
    re.captures_iter(text).filter_map(|c| c[1].parse().ok()).collect()
}

// ── OpenAPI naming ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiNaming {
    /// The validated names (invalid or unknown entries dropped).
    pub proposal: crate::openapi_names::NamingProposal,
    /// The deterministic document with the proposal applied.
    pub spec: Value,
    /// Human-readable list of what changed.
    pub changes: Vec<String>,
    pub problem: Option<String>,
    pub meta: Meta,
}

/// Proposes operationIds, summaries, tags and component schema names for a traffic-inferred
/// spec, and applies them as a patch. Nothing is saved.
pub async fn improve_openapi_names(env: &AiEnv<'_>, provider: &Provider, spec_text: &str) -> Result<OpenApiNaming> {
    let call = prepare(&FeatureInput::OpenapiNames { spec: spec_text.into() }, env, provider)?;
    let doc = crate::openapi_names::parse_spec(spec_text)?;
    let doc2 = doc.clone();
    let check = move |v: &Value| crate::openapi_names::check_proposal(v, &doc2);
    let out = structured::generate(provider, &call.request, &check).await?;
    let proposal = crate::openapi_names::clean_proposal(&out.value, &doc);
    let (spec, changes) = crate::openapi_names::apply_proposal(&doc, &proposal);
    Ok(OpenApiNaming { meta: meta(provider, &call, Some(&out), &out.model), proposal, spec, changes, problem: out.problem })
}

// ── NL → SQL / MongoDB / kubectl ────────────────────────────────────────────────────────────

/// NL → SQL; `checker` runs the database's `EXPLAIN` (read-only) in the host.
pub async fn nl_to_sql(env: &AiEnv<'_>, provider: &Provider, input: &FeatureInput, checker: &dyn structured::Checker) -> Result<crate::nlquery::SqlSuggestion> {
    let FeatureInput::NlSql { allow_writes, .. } = input else { return Err(AiError::Invalid("expected nlSql input".into())) };
    let call = prepare(input, env, provider)?;
    let aw = *allow_writes;
    let check = move |v: &Value| crate::nlquery::check_sql(v, aw);
    let c = structured::generate_checked(provider, &call.request, &check, checker).await?;
    let v = &c.out.value;
    Ok(crate::nlquery::SqlSuggestion {
        meta: meta(provider, &call, Some(&c.out), &c.out.model),
        sql: crate::nlquery::clean_sql(v["sql"].as_str().unwrap_or_default()),
        explanation: s(v, "explanation"),
        validated: c.note.is_some() && c.out.problem.is_none(),
        check: c.note.unwrap_or(Value::Null),
        problem: c.out.problem,
    })
}

/// NL → MongoDB find/aggregate; `checker` runs the server's `explain`.
pub async fn nl_to_mongo(env: &AiEnv<'_>, provider: &Provider, input: &FeatureInput, checker: &dyn structured::Checker) -> Result<crate::nlquery::MongoSuggestion> {
    let call = prepare(input, env, provider)?;
    let c = structured::generate_checked(provider, &call.request, &crate::nlquery::check_mongo, checker).await?;
    let v = &c.out.value;
    let (kind, filter, projection, sort, limit, pipeline) = crate::nlquery::mongo_from(v);
    Ok(crate::nlquery::MongoSuggestion {
        meta: meta(provider, &call, Some(&c.out), &c.out.model),
        kind,
        filter,
        projection,
        sort,
        limit,
        pipeline,
        explanation: s(v, "explanation"),
        validated: c.note.is_some() && c.out.problem.is_none(),
        check: c.note.unwrap_or(Value::Null),
        problem: c.out.problem,
    })
}

/// NL → kubectl; `checker` parses the command and dry-runs changes on the server.
pub async fn nl_to_kubectl(env: &AiEnv<'_>, provider: &Provider, input: &FeatureInput, checker: &dyn structured::Checker) -> Result<crate::nlquery::KubectlSuggestion> {
    let call = prepare(input, env, provider)?;
    let c = structured::generate_checked(provider, &call.request, &crate::nlquery::check_kubectl, checker).await?;
    let v = &c.out.value;
    Ok(crate::nlquery::KubectlSuggestion {
        meta: meta(provider, &call, Some(&c.out), &c.out.model),
        command: s(v, "command"),
        explanation: s(v, "explanation"),
        validated: c.note.is_some() && c.out.problem.is_none(),
        check: c.note.unwrap_or(Value::Null),
        problem: c.out.problem,
    })
}

// ── Suggest tests ───────────────────────────────────────────────────────────────────────────

const NO_VALUE_OPS: &[AssertOp] = &[AssertOp::Exists, AssertOp::NotExists, AssertOp::IsEmpty, AssertOp::IsNotEmpty];

/// Converts and validates suggested tests against the sample response.
pub fn validate_tests(v: &Value, sample: &ResponseContext) -> (Vec<SuggestedTest>, Vec<String>) {
    let body: Option<Value> = serde_json::from_str(&sample.body).ok();
    let mut ok: Vec<SuggestedTest> = Vec::new();
    let mut dropped = Vec::new();
    for t in v["tests"].as_array().cloned().unwrap_or_default() {
        let target_s = s(&t, "target");
        let selector = s(&t, "selector");
        let op_s = s(&t, "op");
        let value = t["value"].as_str().unwrap_or_default().trim().to_string();
        let label = format!("{target_s} {selector} {op_s} {value}").split_whitespace().collect::<Vec<_>>().join(" ");
        let Ok(op) = serde_json::from_value::<AssertOp>(json!(op_s)) else {
            dropped.push(format!("{label}: unknown operator"));
            continue;
        };
        let target = match target_s.as_str() {
            "status" => AssertTarget::Status,
            "header" if !selector.is_empty() => AssertTarget::Header { name: selector.clone() },
            "jsonPath" => {
                if let Err(e) = query_path(&selector, &json!({})) {
                    dropped.push(format!("{label}: {e}"));
                    continue;
                }
                AssertTarget::JsonPath { path: selector.clone() }
            }
            "body" => AssertTarget::Body,
            "responseTime" => AssertTarget::ResponseTime,
            "contentType" => AssertTarget::ContentType,
            "size" => AssertTarget::Size,
            _ => {
                dropped.push(format!("{label}: unknown target"));
                continue;
            }
        };
        let needs_presence = matches!(op, AssertOp::Exists | AssertOp::Eq | AssertOp::IsType | AssertOp::IsNotEmpty | AssertOp::Contains | AssertOp::OneOf);
        match &target {
            AssertTarget::JsonPath { path } if needs_presence => match &body {
                Some(b) => {
                    if query_path(path, b).map(|m| m.is_empty()).unwrap_or(true) {
                        dropped.push(format!("{label}: path not found in the sample response"));
                        continue;
                    }
                }
                None => {
                    dropped.push(format!("{label}: the sample body is not JSON"));
                    continue;
                }
            },
            AssertTarget::Header { name } if needs_presence && !sample.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) => {
                dropped.push(format!("{label}: header not present in the sample response"));
                continue;
            }
            _ => {}
        }
        if op == AssertOp::Matches && regex::Regex::new(&value).is_err() {
            dropped.push(format!("{label}: invalid regular expression"));
            continue;
        }
        if !NO_VALUE_OPS.contains(&op) && value.is_empty() && op != AssertOp::IsType {
            dropped.push(format!("{label}: missing value"));
            continue;
        }
        let assertion = Assertion { target, op, value: (!NO_VALUE_OPS.contains(&op) && !value.is_empty()).then_some(value), enabled: true };
        if ok.iter().any(|x| x.assertion == assertion) {
            continue;
        }
        ok.push(SuggestedTest { assertion, reason: s(&t, "reason") });
    }
    if (100..600).contains(&sample.status) && !ok.iter().any(|t| t.assertion.target == AssertTarget::Status) {
        ok.insert(
            0,
            SuggestedTest {
                assertion: Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some(sample.status.to_string()), enabled: true },
                reason: "Baseline: the status code of this response.".into(),
            },
        );
    }
    (ok, dropped)
}

pub async fn suggest_tests(env: &AiEnv<'_>, provider: &Provider, sample: &ResponseContext) -> Result<TestSuggestions> {
    let call = prepare(&FeatureInput::SuggestTests(sample.clone()), env, provider)?;
    let sample2 = sample.clone();
    let check = move |v: &Value| -> std::result::Result<(), String> {
        let (ok, dropped) = validate_tests(v, &sample2);
        if ok.len() <= 1 && !dropped.is_empty() { Err(format!("these tests don't fit the sample response: {}.", dropped.join("; "))) } else { Ok(()) }
    };
    let out = structured::generate(provider, &call.request, &check).await?;
    let (tests, dropped) = validate_tests(&out.value, sample);
    Ok(TestSuggestions { meta: meta(provider, &call, Some(&out), &out.model), tests, dropped })
}

// ── Notes and to-dos ────────────────────────────────────────────────────────────────────────

pub async fn summarize_notes(env: &AiEnv<'_>, provider: &Provider, paths: &[String]) -> Result<NotesSummary> {
    let call = prepare(&FeatureInput::SummarizeNotes { paths: paths.to_vec() }, env, provider)?;
    let out = structured::generate(provider, &call.request, &|_| Ok(())).await?;
    let v = &out.value;
    let summary = s(v, "summary");
    let key_points = strings(v, "keyPoints");
    let all_text = format!("{summary}\n{}", key_points.join("\n"));
    let citations = context::citations(&all_text, &call.sources);
    Ok(NotesSummary { meta: meta(provider, &call, Some(&out), &out.model), summary, key_points, open_questions: strings(v, "openQuestions"), citations, sources: call.sources })
}

/// Validates and normalises extracted to-dos (dedupe, due dates the to-do parser understands
/// relative to `ctx`, p1–p4, short labels, at most 15).
pub fn clean_todos(v: &Value, ctx: &DateCtx) -> Vec<TodoSuggestion> {
    let mut out: Vec<TodoSuggestion> = Vec::new();
    for t in v["todos"].as_array().cloned().unwrap_or_default() {
        let text = s(&t, "text").trim_start_matches(['-', '*', ' ']).trim_start_matches("[ ]").trim().to_string();
        if text.is_empty() || out.iter().any(|o| o.text.eq_ignore_ascii_case(&text)) {
            continue;
        }
        let raw_priority = t["priority"].as_u64().map(|n| n.to_string()).unwrap_or_else(|| s(&t, "priority").to_lowercase());
        let priority = match raw_priority.as_str() {
            "p1" | "1" | "urgent" | "critical" => Priority::P1,
            "p2" | "2" | "high" => Priority::P2,
            "p3" | "3" | "medium" => Priority::P3,
            _ => Priority::P4,
        };
        let raw_due = s(&t, "due");
        let (due, due_date) = match dates::parse_due(&raw_due, ctx) {
            Ok(Some(d)) => (Some(raw_due), Some(d.date.format("%Y-%m-%d").to_string())),
            _ => (None, None),
        };
        let mut labels: Vec<String> = Vec::new();
        for l in strings(&t, "labels") {
            let l = l.trim().trim_start_matches('@').to_lowercase().split_whitespace().collect::<Vec<_>>().join("-");
            if !l.is_empty() && l.chars().count() <= 30 && !labels.contains(&l) && labels.len() < 5 {
                labels.push(l);
            }
        }
        let links = strings(&t, "links").into_iter().filter(|l| !l.contains(char::is_whitespace)).take(5).collect();
        let description: String = s(&t, "description").chars().take(500).collect();
        out.push(TodoSuggestion { text: text.chars().take(300).collect(), description, priority, due, due_date, labels, links });
        if out.len() == 15 {
            break;
        }
    }
    out
}

pub async fn todos_from_text(env: &AiEnv<'_>, provider: &Provider, text: &str) -> Result<TodoSuggestions> {
    let call = prepare(&FeatureInput::TodosFromText { text: text.into() }, env, provider)?;
    let now = DateCtx::local_now();
    let ctx = chrono::NaiveDate::parse_from_str(&env.today, "%Y-%m-%d").map(|d| DateCtx::new(d, now.now)).unwrap_or(now);
    let check = |v: &Value| -> std::result::Result<(), String> { if clean_todos(v, &ctx).is_empty() { Err("no usable to-do items were returned.".into()) } else { Ok(()) } };
    let out = structured::generate(provider, &call.request, &check).await?;
    Ok(TodoSuggestions { meta: meta(provider, &call, Some(&out), &out.model), todos: clean_todos(&out.value, &ctx) })
}

/// Runs any feature and returns its JSON result (Ask runs non-streaming here).
pub async fn run(input: &FeatureInput, env: &AiEnv<'_>, provider: &Provider) -> Result<Value> {
    let to = |r: std::result::Result<Value, serde_json::Error>| r.map_err(|e| AiError::Protocol(e.to_string()));
    match input {
        FeatureInput::Ask { question } => to(serde_json::to_value(ask(env, provider, question).await?)),
        FeatureInput::NlRegex { description, positive, negative } => to(serde_json::to_value(nl_to_regex(env, provider, description, positive, negative).await?)),
        FeatureInput::NlJsonPath { description, sample_json } => to(serde_json::to_value(nl_to_jsonpath(env, provider, description, sample_json).await?)),
        FeatureInput::NlJq { description, sample_json } => to(serde_json::to_value(nl_to_jq(env, provider, description, sample_json).await?)),
        FeatureInput::NlRequest { description } => to(serde_json::to_value(nl_to_request(env, provider, description).await?)),
        FeatureInput::MockData { schema_or_sample, count } => to(serde_json::to_value(generate_mock_data(env, provider, schema_or_sample, *count).await?)),
        FeatureInput::ExplainResponse(r) => to(serde_json::to_value(explain_response(env, provider, r).await?)),
        FeatureInput::ExplainError { text } => to(serde_json::to_value(explain_error(env, provider, text).await?)),
        FeatureInput::ExplainFailure(fc) => to(serde_json::to_value(explain_failure(env, provider, fc).await?)),
        FeatureInput::OpenapiNames { spec } => to(serde_json::to_value(improve_openapi_names(env, provider, spec).await?)),
        // Without the host's live checks (the app supplies them in `ai_run`).
        FeatureInput::NlSql { .. } => to(serde_json::to_value(nl_to_sql(env, provider, input, &structured::NoCheck).await?)),
        FeatureInput::NlMongo { .. } => to(serde_json::to_value(nl_to_mongo(env, provider, input, &structured::NoCheck).await?)),
        FeatureInput::NlKubectl { .. } => to(serde_json::to_value(nl_to_kubectl(env, provider, input, &structured::NoCheck).await?)),
        FeatureInput::SuggestTests(r) => to(serde_json::to_value(suggest_tests(env, provider, r).await?)),
        FeatureInput::SummarizeNotes { paths } => to(serde_json::to_value(summarize_notes(env, provider, paths).await?)),
        FeatureInput::TodosFromText { text } => to(serde_json::to_value(todos_from_text(env, provider, text).await?)),
    }
}
