//! Model Context Protocol server: exposes one /dev/all project to coding agents (Claude Code,
//! Cursor, VS Code, Codex…).
//!
//! Built on the official Rust SDK (`rmcp` 3.6), which speaks both protocol eras: the stateless
//! 2026-07-28 revision (`server/discover`, per-request `_meta`) and the `initialize`-based
//! 2025-11-25 / 2025-06-18 / 2025-03-26 revisions most clients still use.
//!
//! **Tools** (deterministic order): `get_context`, `list_environments`, `list_mocks`,
//! `list_repos`, `list_requests`, `list_todos`, `read_note`, `read_request`, `run_utility`, `search_notes`,
//! `search_project` (semantic search with the project's local embedding index), `send_request`.
//! **Resources**: every note as `devall://notes/<path>`.
//!
//! Safety: everything is read-only except `send_request`, which only sends GET/HEAD/OPTIONS unless
//! the server was started with `allow_writes` (`devall mcp --allow-writes`) or `confirm_writes`
//! (each other method waits for the user's approval through a [`Confirmer`]: the desktop app's
//! HTTP endpoint). Requests in an environment marked `protected` always need an approval, so the
//! stdio CLI (no UI to ask) never sends there. See [`approval`]. All output is redacted (keychain/.env secret values and
//! credential patterns). Transports: stdio (primary) and optional Streamable HTTP bound to
//! 127.0.0.1 with a random bearer token, Host/Origin validation (DNS-rebinding protection).
//!
//! Connect Claude Code: `claude mcp add devall -- devall mcp --project /path/to/project`.

pub mod approval;
mod http;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use devall_core::{KeychainSecretStore, MemorySecretStore, SecretStore};
use devall_http::ClientPool;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub use approval::{ApprovalDecision, ApprovalFuture, ApprovalRequest, Approvals, Confirmer, WriteMode};
pub use devall_ai::embed::{ConfigEmbedderSource, EmbedderSource};
pub use http::{HttpInfo, HttpServer, serve_http, serve_http_with, serve_server};

pub const NOTE_URI_PREFIX: &str = "devall://notes/";

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("{0}")]
    Project(String),
    #[error("MCP transport: {0}")]
    Transport(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOptions {
    /// Allow `send_request` to send methods other than GET/HEAD/OPTIONS.
    pub allow_writes: bool,
    /// Ask the user (through the host's [`Confirmer`]) before each method other than
    /// GET/HEAD/OPTIONS. Ignored without a confirmer (then writes stay disabled).
    #[serde(default)]
    pub confirm_writes: bool,
    /// Default environment id for `send_request`.
    pub environment: Option<String>,
    /// Don't offer the note and to-do writing tools (by default agents may create and edit shared
    /// Markdown notes and the project's personal to-do list — local files the user can undo).
    #[serde(default)]
    pub read_only_local: bool,
    /// App data directory (personal to-dos); defaults to the desktop app's.
    pub app_data_dir: Option<PathBuf>,
    /// Use an in-memory secret store instead of the OS keychain (tests, CI).
    pub no_keychain: bool,
}

impl McpOptions {
    pub fn write_mode(&self) -> WriteMode {
        if self.allow_writes {
            WriteMode::Allow
        } else if self.confirm_writes {
            WriteMode::Confirm
        } else {
            WriteMode::ReadOnly
        }
    }
}

/// The desktop app's data directory (`dev.devall.app` under the platform data dir), so the MCP
/// server sees the same personal to-dos and local history.
pub fn default_app_data_dir() -> Option<PathBuf> {
    devall_core::local::app_data_dir()
}

struct Inner {
    root: PathBuf,
    options: McpOptions,
    store: Box<dyn SecretStore>,
    pool: ClientPool,
    approvals: Option<Approvals>,
    /// Embeds `search_project` queries (the desktop app's managed sidecar, or `ai.json`).
    embedder: std::sync::RwLock<Option<Arc<dyn EmbedderSource>>>,
}

/// The MCP server for one project. Cheap to clone (shared state).
#[derive(Clone)]
pub struct DevallMcp {
    inner: Arc<Inner>,
}

impl DevallMcp {
    pub fn new(project_root: impl AsRef<Path>, options: McpOptions) -> Result<Self, McpError> {
        Self::new_with_confirmer(project_root, options, None)
    }

    pub fn with_store(root: PathBuf, options: McpOptions, store: Box<dyn SecretStore>) -> Self {
        Self { inner: Arc::new(Inner { root, options, store, pool: ClientPool::new(), approvals: None, embedder: Default::default() }) }
    }

    /// Like [`with_store`](Self::with_store), with a host that can ask the user to approve sends.
    pub fn with_approvals(root: PathBuf, options: McpOptions, store: Box<dyn SecretStore>, approvals: Approvals) -> Self {
        Self { inner: Arc::new(Inner { root, options, store, pool: ClientPool::new(), approvals: Some(approvals), embedder: Default::default() }) }
    }

    /// Where `search_project` gets its query embedder. Without one, it ranks by keywords.
    pub fn set_embedder_source(&self, source: Option<Arc<dyn EmbedderSource>>) {
        *self.inner.embedder.write().unwrap() = source;
    }

    /// Opens the project like [`new`](Self::new), with an optional confirmer for approvals.
    pub fn new_with_confirmer(project_root: impl AsRef<Path>, options: McpOptions, confirmer: Option<Arc<dyn Confirmer>>) -> Result<Self, McpError> {
        let root = project_root.as_ref().to_path_buf();
        devall_core::Project::open(&root).map_err(|e| McpError::Project(format!("{}: {e}", root.display())))?;
        let keychain = !options.no_keychain && KeychainSecretStore::available();
        let store: Box<dyn SecretStore> = if keychain { Box::new(KeychainSecretStore::new()) } else { Box::new(MemorySecretStore::default()) };
        let data_dir = options.app_data_dir.clone().or_else(default_app_data_dir);
        let server = match confirmer {
            Some(c) => Self::with_approvals(root, options, store, Approvals::new(c)),
            None => Self::with_store(root, options, store),
        };
        // The embedding model configured in the app (`ai.json`); local unless the user allowed cloud context.
        if let Some(data_dir) = data_dir {
            let store: Arc<dyn SecretStore> = if keychain { Arc::new(KeychainSecretStore::new()) } else { Arc::new(MemorySecretStore::default()) };
            server.set_embedder_source(Some(Arc::new(ConfigEmbedderSource { data_dir, store })));
        }
        Ok(server)
    }

    pub fn options(&self) -> &McpOptions {
        &self.inner.options
    }

    fn ctx(&self) -> Result<tools::Ctx<'_>, String> {
        tools::Ctx::open(&self.inner.root, self.inner.store.as_ref(), &self.inner.options)
    }

    /// Tool definitions in a stable order.
    pub fn tools(&self) -> Vec<Tool> {
        let schema = |v: Value| -> Map<String, Value> { v.as_object().cloned().unwrap_or_default() };
        let ro = |title: &str| ToolAnnotations::with_title(title).read_only(true).destructive(false).idempotent(true).open_world(false);
        let can_ask = self.inner.approvals.is_some();
        let mode = self.inner.options.write_mode();
        let writes = mode == WriteMode::Allow || (mode == WriteMode::Confirm && can_ask);
        let protected = if can_ask {
            "Requests in environments marked protected wait for the user's approval in the /dev/all app."
        } else {
            "Never sends in environments marked protected."
        };
        let send_desc = match (mode, can_ask) {
            (WriteMode::Allow, _) => format!(
                "Send a saved request (by path from list_requests) and return status, headers, body and assertion results. Any method is allowed. {protected} Secrets are resolved from the OS keychain and masked in the output."
            ),
            (WriteMode::Confirm, true) => format!(
                "Send a saved request (by path from list_requests) and return status, headers, body and assertion results. GET/HEAD/OPTIONS are sent directly; other methods wait (up to {} s) for the user to approve the call in the /dev/all app. A denial is final: don't retry it. {protected} Secrets are resolved from the OS keychain and masked in the output.",
                approval::APPROVAL_TIMEOUT.as_secs()
            ),
            _ => format!(
                "Send a saved request (by path from list_requests) and return status, headers, body and assertion results. READ-ONLY: only GET, HEAD and OPTIONS requests are sent. {protected} Secrets are resolved from the OS keychain and masked in the output."
            ),
        };
        let empty = || schema(json!({ "type": "object", "properties": {}, "additionalProperties": false }));
        // Local writes: reversible edits to files on this machine, never the network.
        let local = |title: &str, idempotent: bool| ToolAnnotations::with_title(title).read_only(false).destructive(false).idempotent(idempotent).open_world(false);
        let mut tools = vec![
            Tool::new("get_context", "Project knowledge ranked by relevance to an optional query: shared notes, the user's personal to-do list, the saved-request outline and environments (secret names only). Use it first to orient yourself.", schema(json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "What you want to know about, e.g. \"payments webhooks\"" }, "limit": { "type": "integer", "description": "Max documents (default 8)" } },
                "additionalProperties": false
            }))).annotate(ro("Get project context")),
            Tool::new("list_environments", "Environments with their non-secret variables, the names of their secrets (values are never returned) and whether they are protected.", empty()).annotate(ro("List environments")),
            Tool::new("list_mocks", "WireMock-compatible mock stubs in mocks/mappings: method, URL matcher, response status, priority and scenario.", empty()).annotate(ro("List mocks")),
            Tool::new("list_repos", "The project's git repositories (a multi-repo project shares its requests, environments and notes across several frontend/backend repos): name, group, checked-out branch, local path, remote, state (found / missing / notARepo) and whether it is listed in devall.yaml. Files in a repo are referenced as repo:<name>/<path>.", empty()).annotate(ro("List repositories")),
            Tool::new("list_requests", "Saved requests in the project (path, name, method, protocol, folder). Paths are what read_request and send_request take.", empty()).annotate(ro("List saved requests")),
            Tool::new("list_todos", "The user's personal to-do list for this project (stored outside the repo): tasks with priority (p1 most urgent), due date/time and recurrence, deadline, labels, section, sub-tasks, links, attachment names (files kept on the user's machine) and comments.", schema(json!({
                "type": "object",
                "properties": {
                    "includeDone": { "type": "boolean", "description": "Also return completed items" },
                    "filter": { "type": "string", "description": "Optional filter, e.g. \"today | overdue\", \"p1 & @bug\", \"due before: next week\", \"search: refunds\"" }
                },
                "additionalProperties": false
            }))).annotate(ro("List personal to-dos")),
            Tool::new("read_note", "Read a shared project note (Markdown under notes/).", schema(json!({
                "type": "object", "properties": { "path": { "type": "string", "description": "Path relative to notes/, e.g. design/auth.md" } }, "required": ["path"], "additionalProperties": false
            }))).annotate(ro("Read a note")),
            Tool::new("read_request", "Read a saved request file (YAML) plus its effective method/URL after folder inheritance and the {{variables}} it references.", schema(json!({
                "type": "object", "properties": { "path": { "type": "string", "description": "Request path from list_requests, e.g. users/list.req.yaml" } }, "required": ["path"], "additionalProperties": false
            }))).annotate(ro("Read a saved request")),
            Tool::new("run_utility", "Run a /dev/all developer utility: encoders (base64, url, hex), JWT decode, hashes/HMAC, UUID/ULID, timestamps, JSON/YAML formatting, regex, JSONPath/jq, diff, cron… Call with op \"list\" to see every operation.", schema(json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "description": "Operation id (e.g. jwt.decode, base64.decode) or \"list\"" },
                    "input": { "type": "string" },
                    "args": { "type": "object", "description": "Operation arguments (strings)", "additionalProperties": true }
                },
                "required": ["op"], "additionalProperties": false
            }))).annotate(ro("Run a utility")),
            Tool::new("search_notes", "Full-text search over the project's shared notes.", schema(json!({
                "type": "object", "properties": { "query": { "type": "string" }, "limit": { "type": "integer" } }, "required": ["query"], "additionalProperties": false
            }))).annotate(ro("Search notes")),
            Tool::new("search_project", "Search the whole project by meaning: note sections, saved requests (name, method, URL, docs), mock stubs and environments (names only). Uses the local embedding index built in /dev/all (AI → Search); without it, results are keyword-ranked and `note` says why. Follow up with read_note / read_request.", schema(json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "What you're looking for, in plain words" }, "limit": { "type": "integer", "description": "Max hits (default 10)" } },
                "required": ["query"], "additionalProperties": false
            }))).annotate(ro("Search the project")),
            Tool::new("send_request", send_desc, schema(json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Request path from list_requests" },
                    "environment": { "type": "string", "description": "Environment id from list_environments (optional)" }
                },
                "required": ["path"], "additionalProperties": false
            })))
            .annotate(ToolAnnotations::with_title("Send a saved request").read_only(!writes).destructive(writes).idempotent(!writes).open_world(true)),
        ];
        if !self.inner.options.read_only_local {
            let id_prop = json!({ "type": "string", "description": "Task id from list_todos (a unique prefix or suffix of 4+ characters works)" });
            tools.extend([
                Tool::new("write_note", "Create a shared project note (Markdown under notes/, committed with the repo), or replace one with overwrite: true. Prefer edit_note / append_to_note for changes to an existing note.", schema(json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to notes/, e.g. design/auth.md (.md is added when missing; folders are created)" },
                        "content": { "type": "string", "description": "Full Markdown content; start with a # heading (it becomes the note's title)" },
                        "overwrite": { "type": "boolean", "description": "Replace the note when it exists (default false)" }
                    },
                    "required": ["path", "content"], "additionalProperties": false
                }))).annotate(local("Write a note", true)),
                Tool::new("append_to_note", "Append a paragraph (or any Markdown) to the end of a shared note, creating the note when missing — e.g. a decision log or findings.", schema(json!({
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Path relative to notes/" }, "text": { "type": "string", "description": "Markdown to append" } },
                    "required": ["path", "text"], "additionalProperties": false
                }))).annotate(local("Append to a note", false)),
                Tool::new("edit_note", "Replace an exact piece of text in a shared note (copy oldText from read_note; it must match once unless replaceAll).", schema(json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path relative to notes/" },
                        "oldText": { "type": "string", "description": "Exact text to replace" },
                        "newText": { "type": "string", "description": "Replacement (may be empty to delete)" },
                        "replaceAll": { "type": "boolean", "description": "Replace every occurrence (default false)" }
                    },
                    "required": ["path", "oldText", "newText"], "additionalProperties": false
                }))).annotate(local("Edit a note", false)),
                Tool::new("add_todo", "Add a task to the user's personal to-do list for this project (stored on their machine, outside the repo). `text` uses quick-add syntax: dates (\"tomorrow 5pm\", \"every mon\", \"in 3 days\"), priority p1–p4, @label, {deadline}.", schema(json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "e.g. \"Fix refund webhook retries tomorrow p1 @payments\"" },
                        "description": { "type": "string", "description": "Optional details (Markdown)" },
                        "parentId": { "type": "string", "description": "Make it a sub-task of this task id" },
                        "links": { "type": "array", "items": { "type": "string" }, "description": "Project paths or URLs, e.g. http/payments/refund.req.yaml" },
                        "parse": { "type": "boolean", "description": "Set false to keep the text literally (no date/priority parsing)" }
                    },
                    "required": ["text"], "additionalProperties": false
                }))).annotate(local("Add a to-do", false)),
                Tool::new("update_todo", "Change a task in this project's to-do list. Only the fields given change.", schema(json!({
                    "type": "object",
                    "properties": {
                        "id": id_prop,
                        "content": { "type": "string" },
                        "description": { "type": "string" },
                        "priority": { "type": "string", "enum": ["p1", "p2", "p3", "p4"] },
                        "due": { "type": "string", "description": "Natural language (\"fri 9am\", \"every weekday\"); empty string clears" },
                        "deadline": { "type": ["string", "null"], "description": "YYYY-MM-DD or natural language; empty/null clears" },
                        "labels": { "type": "array", "items": { "type": "string" }, "description": "Replace all labels" },
                        "addLabels": { "type": "array", "items": { "type": "string" } },
                        "removeLabels": { "type": "array", "items": { "type": "string" } },
                        "addLinks": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["id"], "additionalProperties": false
                }))).annotate(local("Update a to-do", true)),
                Tool::new("complete_todo", "Mark a task in this project's to-do list done (a recurring task moves to its next date), or reopen it with done: false.", schema(json!({
                    "type": "object",
                    "properties": { "id": id_prop, "done": { "type": "boolean", "description": "Default true" } },
                    "required": ["id"], "additionalProperties": false
                }))).annotate(local("Complete a to-do", false)),
                Tool::new("comment_todo", "Add a comment to a task in this project's to-do list (e.g. what you did or found).", schema(json!({
                    "type": "object",
                    "properties": { "id": id_prop, "text": { "type": "string" } },
                    "required": ["id", "text"], "additionalProperties": false
                }))).annotate(local("Comment on a to-do", false)),
            ]);
        }
        tools
    }

    fn instructions(&self) -> String {
        let name = devall_core::Project::open(&self.inner.root).map(|p| p.manifest.name).unwrap_or_else(|_| "project".into());
        format!(
            "/dev/all project \"{name}\": saved HTTP requests, environments, mocks, shared notes and the user's personal to-dos. Start with get_context. \
             {}send_request {}. Secret values are masked as ••••••; refer to secrets as {{{{secret.NAME}}}}.",
            if self.inner.options.read_only_local {
                "All tools are read-only except "
            } else {
                "You may keep shared notes (write_note, append_to_note, edit_note) and the user's to-dos for this project (add_todo, update_todo, complete_todo, comment_todo) up to date; other tools are read-only except "
            },
            match (self.inner.options.write_mode(), self.inner.approvals.is_some()) {
                (WriteMode::Allow, true) => "may send any method; protected environments need the user's approval",
                (WriteMode::Allow, false) => "may send any method but never runs in protected environments",
                (WriteMode::Confirm, true) => "sends GET/HEAD/OPTIONS directly and asks the user to approve other methods and anything in protected environments",
                (_, true) => "only sends GET/HEAD/OPTIONS; protected environments need the user's approval",
                (_, false) => "only sends GET/HEAD/OPTIONS and never runs in protected environments",
            }
        )
    }

    /// Runs a tool by name. `Ok` carries JSON or text; `Err` is a tool-level error message.
    pub async fn call(&self, name: &str, args: &Map<String, Value>) -> Result<Value, String> {
        let str_arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
        let req_str = |k: &str| str_arg(k).filter(|s| !s.trim().is_empty()).ok_or_else(|| format!("`{k}` is required"));
        let num = |k: &str, d: usize| args.get(k).and_then(Value::as_u64).map(|n| n as usize).unwrap_or(d);
        match name {
            "get_context" => tools::get_context(&self.ctx()?, str_arg("query").as_deref(), num("limit", 8)),
            "list_environments" => tools::list_environments(&self.ctx()?),
            "list_mocks" => tools::list_mocks(&self.ctx()?),
            "list_repos" => tools::list_repos(&self.ctx()?),
            "list_requests" => tools::list_requests(&self.ctx()?),
            "list_todos" => tools::list_todos(&self.ctx()?, args.get("includeDone").and_then(Value::as_bool).unwrap_or(false), str_arg("filter").as_deref()),
            "read_note" => tools::read_note(&self.ctx()?, &req_str("path")?).map(Value::String),
            "read_request" => tools::read_request(&self.ctx()?, &req_str("path")?),
            "run_utility" => {
                let op = req_str("op")?;
                let input = str_arg("input").unwrap_or_default();
                let a = args.get("args").cloned().unwrap_or_else(|| json!({}));
                let is_net = devall_utils::info(&op).is_some_and(|o| o.network);
                let out = if is_net {
                    tokio::task::spawn_blocking(move || tools::run_utility(&op, &input, &a)).await.map_err(|e| e.to_string())?
                } else {
                    tools::run_utility(&op, &input, &a)
                }?;
                let ctx = self.ctx()?;
                Ok(Value::String(ctx.clean(&out)))
            }
            "search_notes" => tools::search_notes(&self.ctx()?, &req_str("query")?, num("limit", 20)),
            "search_project" => {
                let source = self.inner.embedder.read().unwrap().clone();
                let embedder = match source {
                    Some(s) => s.embedder().await,
                    None => None,
                };
                tools::search_project(&self.ctx()?, &req_str("query")?, num("limit", 10), embedder).await
            }
            "send_request" => {
                let ctx = self.ctx()?;
                tools::send_request(&ctx, &self.inner.pool, &req_str("path")?, str_arg("environment").as_deref(), self.inner.approvals.as_ref()).await
            }
            "write_note" | "append_to_note" | "edit_note" | "add_todo" | "update_todo" | "complete_todo" | "comment_todo" if self.inner.options.read_only_local => {
                Err(format!("`{name}` is turned off: this /dev/all MCP server gives agents read-only access to notes and to-dos"))
            }
            "write_note" => {
                let content = str_arg("content").ok_or("`content` is required")?;
                tools::write_note(&self.ctx()?, &req_str("path")?, &content, args.get("overwrite").and_then(Value::as_bool).unwrap_or(false))
            }
            "append_to_note" => tools::append_to_note(&self.ctx()?, &req_str("path")?, &req_str("text")?),
            "edit_note" => {
                let new_text = str_arg("newText").ok_or("`newText` is required")?;
                tools::edit_note(&self.ctx()?, &req_str("path")?, &req_str("oldText")?, &new_text, args.get("replaceAll").and_then(Value::as_bool).unwrap_or(false))
            }
            "add_todo" => {
                let links = args.get("links").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
                let parse = args.get("parse").and_then(Value::as_bool).unwrap_or(true);
                tools::add_todo(&self.ctx()?, &req_str("text")?, str_arg("description").as_deref(), str_arg("parentId").as_deref(), links, parse)
            }
            "update_todo" => tools::update_todo(&self.ctx()?, &req_str("id")?, args),
            "complete_todo" => tools::complete_todo(&self.ctx()?, &req_str("id")?, args.get("done").and_then(Value::as_bool).unwrap_or(true)),
            "comment_todo" => tools::comment_todo(&self.ctx()?, &req_str("id")?, &req_str("text")?),
            other => Err(format!("unknown tool `{other}`")),
        }
    }
}

fn tool_result(out: Result<Value, String>) -> CallToolResult {
    match out {
        Ok(Value::String(s)) => CallToolResult::success(vec![ContentBlock::text(s)]),
        Ok(v) => {
            let mut r = CallToolResult::success(vec![ContentBlock::text(serde_json::to_string_pretty(&v).unwrap_or_default())]);
            if v.is_object() {
                r.structured_content = Some(v);
            }
            r
        }
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
    }
}

impl ServerHandler for DevallMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
            .with_server_info(Implementation::new("devall", env!("CARGO_PKG_VERSION")))
            .with_instructions(self.instructions())
    }

    async fn list_tools(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools()).with_ttl_ms(300_000).with_cache_scope(CacheScope::Private))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools().into_iter().find(|t| t.name == name)
    }

    async fn call_tool(&self, request: CallToolRequestParams, _context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        if self.get_tool(&request.name).is_none() {
            return Err(ErrorData::invalid_params(format!("unknown tool `{}`", request.name), None));
        }
        let args = request.arguments.unwrap_or_default();
        Ok(tool_result(self.call(&request.name, &args).await).into())
    }

    async fn list_resources(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListResourcesResult, ErrorData> {
        let ctx = self.ctx().map_err(|e| ErrorData::internal_error(e, None))?;
        let notes = tools::list_notes(&ctx).map_err(|e| ErrorData::internal_error(e, None))?;
        let resources = notes
            .into_iter()
            .map(|(title, path)| Resource::new(format!("{NOTE_URI_PREFIX}{path}"), path.clone()).with_title(title).with_description(format!("Project note notes/{path}")).with_mime_type("text/markdown"))
            .collect();
        Ok(ListResourcesResult::with_all_items(resources).with_ttl_ms(30_000).with_cache_scope(CacheScope::Private))
    }

    async fn read_resource(&self, request: ReadResourceRequestParams, _context: RequestContext<RoleServer>) -> Result<ReadResourceResponse, ErrorData> {
        let Some(path) = request.uri.strip_prefix(NOTE_URI_PREFIX) else {
            return Err(ErrorData::resource_not_found(format!("unknown resource {}", request.uri), None));
        };
        let ctx = self.ctx().map_err(|e| ErrorData::internal_error(e, None))?;
        let text = tools::read_note(&ctx, path).map_err(|e| ErrorData::resource_not_found(e, None))?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri.clone()).with_mime_type("text/markdown")]).with_ttl_ms(30_000).with_cache_scope(CacheScope::Private).into())
    }
}

/// Serves the project over stdio until the client disconnects (`devall mcp`).
pub async fn serve_stdio(project_root: impl AsRef<Path>, options: McpOptions) -> Result<(), McpError> {
    let server = DevallMcp::new(project_root, options)?;
    let running = server.serve(rmcp::transport::stdio()).await.map_err(|e| McpError::Transport(e.to_string()))?;
    running.waiting().await.map_err(|e| McpError::Transport(e.to_string()))?;
    Ok(())
}

/// Ready-to-paste client configuration.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnippets {
    /// `claude mcp add devall -- devall mcp --project <path>`
    pub claude_code: String,
    /// `.cursor/mcp.json` / Claude Desktop style `{ "mcpServers": { … } }`.
    pub mcp_servers_json: String,
    /// VS Code `.vscode/mcp.json` (`{ "servers": { … } }`).
    pub vscode_json: String,
    /// The command line the client runs.
    pub command: Vec<String>,
}

fn quote_arg(s: &str) -> String {
    if s.chars().any(|c| c.is_whitespace() || "\"'$`\\&|;<>()".contains(c)) { format!("\"{}\"", s.replace('"', "\\\"")) } else { s.to_string() }
}

/// Snippets for connecting agents to this project. `exe` is the `devall` CLI path (default
/// `devall` on PATH); `local_writes: false` adds `--read-only-notes`.
pub fn config_snippets(project_root: &Path, exe: Option<&str>, allow_writes: bool, local_writes: bool) -> ConfigSnippets {
    let exe = exe.unwrap_or("devall").to_string();
    let root = project_root.to_string_lossy().to_string();
    let mut args = vec!["mcp".to_string(), "--project".to_string(), root.clone()];
    if allow_writes {
        args.push("--allow-writes".into());
    }
    if !local_writes {
        args.push("--read-only-notes".into());
    }
    let mut command = vec![exe.clone()];
    command.extend(args.iter().cloned());
    let claude_code = format!("claude mcp add devall -- {}", command.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" "));
    let server = json!({ "command": exe, "args": args });
    ConfigSnippets {
        claude_code,
        mcp_servers_json: serde_json::to_string_pretty(&json!({ "mcpServers": { "devall": server } })).unwrap_or_default(),
        vscode_json: serde_json::to_string_pretty(&json!({ "servers": { "devall": { "type": "stdio", "command": server["command"], "args": server["args"] } } })).unwrap_or_default(),
        command,
    }
}

#[cfg(test)]
mod tests;
