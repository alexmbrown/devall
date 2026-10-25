//! JSON-RPC tests over an in-memory duplex pipe (same framing as stdio) and over HTTP.

use std::time::Duration;

use devall_core::secrets::{ALL_ENVIRONMENTS, SecretScope};
use devall_core::{EnvironmentFile, HttpRequestFile, KeyValue, MemorySecretStore, Project, SecretStore};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::{DevallMcp, McpOptions, config_snippets, serve_http};

/// `/echo` answers with the Authorization header it received.
async fn echo_server() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let auth = req.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim().to_string())).unwrap_or_default();
                let body = json!({ "auth": auth, "ok": true }).to_string();
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: sid=abc123456789\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

struct Fixture {
    _dir: tempfile::TempDir,
    _data: tempfile::TempDir,
    root: std::path::PathBuf,
    data: std::path::PathBuf,
    store: MemorySecretStore,
}

async fn fixture() -> Fixture {
    let base = echo_server().await;
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "Shop API").unwrap();
    let mut env = EnvironmentFile::new("Local");
    env.variables.insert("baseUrl".into(), base.clone());
    env.secrets = vec!["API_TOKEN".into()];
    p.save_environment("local", &env).unwrap();
    let mut prod = EnvironmentFile::new("Prod");
    prod.protected = true;
    prod.variables.insert("baseUrl".into(), base);
    p.save_environment("prod", &prod).unwrap();
    let mut me = HttpRequestFile::new("Me");
    me.url = "{{baseUrl}}/echo".into();
    me.headers = vec![KeyValue::new("Authorization", "Bearer {{secret.API_TOKEN}}")];
    me.tests = vec![devall_core::Assertion { target: devall_core::AssertTarget::Status, op: devall_core::AssertOp::Eq, value: Some("200".into()), enabled: true }];
    p.create_request("", me).unwrap();
    let mut create = HttpRequestFile::new("Create order");
    create.method = "POST".into();
    create.url = "{{baseUrl}}/echo".into();
    p.create_request("", create).unwrap();
    let n = p.create_note("", "Payments").unwrap();
    p.save_note(&n, "# Payments\n\nRotate the token tok-live-998877 quarterly. Stripe key sk_live_abcdefghijklmnop1234.\n").unwrap();
    let mut todos = devall_core::todos::TodoStore::open(data.path()).unwrap();
    todos.add_text(Some(&devall_core::todos::ProjectRef::of(&p)), "Add idempotency keys to orders p1 @orders").unwrap();
    todos.add_text(None, "Private errand in the Inbox").unwrap();
    std::fs::create_dir_all(dir.path().join("mocks/mappings")).unwrap();
    std::fs::write(
        dir.path().join("mocks/mappings/orders.json"),
        r#"{"mappings":[{"id":"7c6b","name":"List orders","request":{"method":"GET","urlPath":"/orders"},"response":{"status":200}},{"request":{"method":"POST","url":"/orders"},"response":{"status":201}}]}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("mocks/mappings/broken.json"), "{ not json").unwrap();
    let store = MemorySecretStore::default();
    store.set(&SecretScope::new(p.id(), ALL_ENVIRONMENTS), "API_TOKEN", "tok-live-998877").unwrap();
    Fixture { root: dir.path().to_path_buf(), data: data.path().to_path_buf(), _dir: dir, _data: data, store }
}

fn server(f: &Fixture, allow_writes: bool) -> DevallMcp {
    let store = MemorySecretStore::default();
    let p = Project::open(&f.root).unwrap();
    if let Some(v) = f.store.get(&SecretScope::new(p.id(), ALL_ENVIRONMENTS), "API_TOKEN").unwrap() {
        store.set(&SecretScope::new(p.id(), ALL_ENVIRONMENTS), "API_TOKEN", &v).unwrap();
    }
    let opts = McpOptions { allow_writes, environment: Some("local".into()), app_data_dir: Some(f.data.clone()), no_keychain: true, ..Default::default() };
    DevallMcp::with_store(f.root.clone(), opts, Box::new(store))
}

fn store_for(f: &Fixture) -> MemorySecretStore {
    let store = MemorySecretStore::default();
    let p = Project::open(&f.root).unwrap();
    if let Some(v) = f.store.get(&SecretScope::new(p.id(), ALL_ENVIRONMENTS), "API_TOKEN").unwrap() {
        store.set(&SecretScope::new(p.id(), ALL_ENVIRONMENTS), "API_TOKEN", &v).unwrap();
    }
    store
}

/// A newline-delimited JSON-RPC client over a duplex pipe.
struct Client {
    w: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    r: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    next: u64,
}

impl Client {
    fn start(server: DevallMcp) -> Self {
        use rmcp::ServiceExt;
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server_io);
        tokio::spawn(async move {
            if let Ok(running) = server.serve((sr, sw)).await {
                let _ = running.waiting().await;
            }
        });
        let (cr, cw) = tokio::io::split(client_io);
        Client { w: cw, r: BufReader::new(cr), next: 1 }
    }

    async fn send(&mut self, msg: Value) {
        let mut line = msg.to_string();
        line.push('\n');
        self.w.write_all(line.as_bytes()).await.unwrap();
        self.w.flush().await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await;
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(Duration::from_secs(20), self.r.read_line(&mut line)).await.expect("response in time").unwrap();
            assert!(n > 0, "server closed the pipe");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    async fn call(&mut self, tool: &str, args: Value, meta: Option<Value>) -> Value {
        let mut params = json!({ "name": tool, "arguments": args });
        if let Some(m) = meta {
            params["_meta"] = m;
        }
        let v = self.request("tools/call", params).await;
        assert!(v.get("error").is_none(), "{v}");
        v["result"].clone()
    }
}

fn text_of(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn legacy_initialize_handshake_and_tools() {
    let f = fixture().await;
    let mut c = Client::start(server(&f, false));
    let init = c
        .request("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }))
        .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "devall", "{init}");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert!(init["result"]["instructions"].as_str().unwrap().contains("Shop API"));
    c.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;

    let tools = c.request("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "get_context",
            "list_environments",
            "list_mocks",
            "list_repos",
            "list_requests",
            "list_todos",
            "read_note",
            "read_request",
            "run_utility",
            "search_notes",
            "search_project",
            "send_request",
            "write_note",
            "append_to_note",
            "edit_note",
            "add_todo",
            "update_todo",
            "complete_todo",
            "comment_todo"
        ]
    );
    let send = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "send_request").unwrap();
    assert_eq!(send["annotations"]["readOnlyHint"], true);

    let reqs = c.call("list_requests", json!({}), None).await;
    let paths: Vec<&str> = reqs["structuredContent"]["requests"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"me.req.yaml") && paths.contains(&"create-order.req.yaml"), "{paths:?}");

    // Read-only by default: POST is refused with a tool-level error.
    let post = c.call("send_request", json!({ "path": "create-order.req.yaml" }), None).await;
    assert_eq!(post["isError"], true);
    assert!(text_of(&post).contains("--allow-writes"));

    // Protected environments are never used.
    let prod = c.call("send_request", json!({ "path": "me.req.yaml", "environment": "prod" }), None).await;
    assert_eq!(prod["isError"], true);
    assert!(text_of(&prod).contains("protected"));

    // GET works; the secret used in the request is masked in everything returned.
    let get = c.call("send_request", json!({ "path": "me.req.yaml" }), None).await;
    assert_eq!(get["isError"], false, "{get}");
    let s = &get["structuredContent"];
    assert_eq!(s["status"], 200);
    assert!(!get.to_string().contains("tok-live-998877"), "{get}");
    assert!(s["body"].as_str().unwrap().contains("Bearer ••••••"));
    assert_eq!(s["tests"][0]["passed"], true);
    assert!(s["headers"].as_array().unwrap().iter().any(|h| h[0].as_str().unwrap().eq_ignore_ascii_case("set-cookie") && h[1] == "••••••"));

    let note = c.call("read_note", json!({ "path": "payments.md" }), None).await;
    let t = text_of(&note);
    assert!(t.contains("Rotate the token ••••••") && !t.contains("sk_live_abcdefghijklmnop1234"), "{t}");

    let envs = c.call("list_environments", json!({}), None).await;
    let e = &envs["structuredContent"]["environments"];
    assert_eq!(e[0]["secrets"][0], json!({ "name": "API_TOKEN", "set": true }));
    assert!(!envs.to_string().contains("tok-live-998877"));

    let todos = c.call("list_todos", json!({}), None).await;
    let t = text_of(&todos);
    assert!(t.contains("Add idempotency keys to orders"), "{t}");
    assert_eq!(todos["structuredContent"]["items"][0]["priority"], "p1");
    assert!(!t.contains("Private errand"), "other lists stay private");
    let p1 = c.call("list_todos", json!({ "filter": "p1 & @orders" }), None).await;
    assert_eq!(p1["structuredContent"]["items"].as_array().map(Vec::len), Some(1));
    let none = c.call("list_todos", json!({ "filter": "p2" }), None).await;
    assert_eq!(none["structuredContent"]["items"].as_array().map(Vec::len), Some(0));

    let ctx = c.call("get_context", json!({ "query": "what's left on orders?" }), None).await;
    assert_eq!(ctx["structuredContent"]["documents"][0]["kind"], "todos");

    let mocks = c.call("list_mocks", json!({}), None).await;
    let m = &mocks["structuredContent"];
    assert_eq!(m["mappings"].as_array().unwrap().len(), 2);
    assert_eq!(m["mappings"][0]["url"], "/orders (urlPath)");
    assert_eq!(m["errors"][0]["file"], "mocks/mappings/broken.json");

    // A nested repo shows up as discovered (not yet listed in devall.yaml), with its branch.
    std::fs::create_dir_all(f.root.join("web/.git")).unwrap();
    std::fs::write(f.root.join("web/.git/HEAD"), "ref: refs/heads/feature/x").unwrap();
    let repos = c.call("list_repos", json!({}), None).await;
    let r = &repos["structuredContent"]["repos"][0];
    assert_eq!((r["name"].as_str(), r["branch"].as_str(), r["listed"].as_bool(), r["origin"].as_str()), (Some("web"), Some("feature/x"), Some(false), Some("discovered")), "{repos}");

    let util = c.call("run_utility", json!({ "op": "base64.decode", "input": "aGVsbG8=" }), None).await;
    assert_eq!(text_of(&util), "hello");

    let hits = c.call("search_notes", json!({ "query": "stripe" }), None).await;
    assert!(text_of(&hits).contains("payments.md"));

    // No index yet: keyword results with a hint; secrets in notes stay masked.
    let sp = c.call("search_project", json!({ "query": "rotate token" }), None).await;
    let s = &sp["structuredContent"];
    assert_eq!(s["mode"], "keyword");
    assert!(s["note"].as_str().unwrap().contains("No semantic index"));
    assert_eq!(s["hits"][0]["source"], "notes/payments.md", "{sp}");
    assert!(!sp.to_string().contains("tok-live-998877"));

    let rr = c.call("read_request", json!({ "path": "me.req.yaml" }), None).await;
    assert!(rr["structuredContent"]["file"].as_str().unwrap().contains("{{secret.API_TOKEN}}"), "template names are kept");

    let res = c.request("resources/list", json!({})).await;
    assert_eq!(res["result"]["resources"][0]["uri"], "devall://notes/payments.md");
    let read = c.request("resources/read", json!({ "uri": "devall://notes/payments.md" })).await;
    assert!(read["result"]["contents"][0]["text"].as_str().unwrap().contains("# Payments"));

    let unknown = c.request("tools/call", json!({ "name": "rm_rf", "arguments": {} })).await;
    assert!(unknown["error"].is_object());
}

#[tokio::test]
async fn modern_stateless_requests_with_discover() {
    let f = fixture().await;
    let mut c = Client::start(server(&f, true));
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" }
    });
    let disc = c.request("server/discover", json!({ "_meta": meta })).await;
    let versions = disc["result"]["supportedVersions"].as_array().unwrap();
    assert!(versions.contains(&json!("2026-07-28")) && versions.contains(&json!("2025-11-25")), "{disc}");
    assert!(disc["result"]["capabilities"]["tools"].is_object());

    let tools = c.request("tools/list", json!({ "_meta": meta })).await;
    assert_eq!(tools["result"]["resultType"], "complete", "{tools}");
    assert!(tools["result"]["ttlMs"].as_u64().is_some());
    let send = tools["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "send_request").unwrap();
    assert_eq!(send["annotations"]["readOnlyHint"], false, "--allow-writes changes the annotation");

    // With --allow-writes, POST is sent.
    let post = c.call("send_request", json!({ "path": "create-order.req.yaml" }), Some(meta.clone())).await;
    assert_eq!(post["isError"], false, "{post}");
    assert_eq!(post["structuredContent"]["request"]["method"], "POST");

    let bad = c.request("tools/list", json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": "1900-01-01", "io.modelcontextprotocol/clientCapabilities": {} } })).await;
    assert!(bad["error"].is_object(), "{bad}");
}

#[tokio::test]
async fn http_transport_requires_token_and_rejects_foreign_origins() {
    let f = fixture().await;
    let opts = McpOptions { app_data_dir: Some(f.data.clone()), no_keychain: true, ..Default::default() };
    let srv = serve_http(&f.root, opts, 0).await.unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } });
    let post = |token: Option<&str>, origin: Option<&str>, body: &Value| {
        let mut rb = client
            .post(&srv.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(body.to_string());
        if let Some(t) = token {
            rb = rb.header("Authorization", format!("Bearer {t}"));
        }
        if let Some(o) = origin {
            rb = rb.header("Origin", o);
        }
        rb.send()
    };
    assert_eq!(post(None, None, &init).await.unwrap().status(), 401);
    assert_eq!(post(Some("wrong"), None, &init).await.unwrap().status(), 401);
    assert_eq!(post(Some(&srv.token), Some("https://evil.example"), &init).await.unwrap().status(), 403);
    let ok = post(Some(&srv.token), None, &init).await.unwrap();
    assert_eq!(ok.status(), 200);
    let text = ok.text().await.unwrap();
    assert!(text.contains("\"devall\""), "{text}");

    // Modern stateless request with the mirrored headers.
    let call = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
        "name": "list_requests", "arguments": {},
        "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {} }
    }});
    let resp = client
        .post(&srv.url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {}", srv.token))
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "list_requests")
        .body(call.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("me.req.yaml"), "{body}");

    let wrong_path = client.post(srv.url.replace("/mcp", "/other")).header("Authorization", format!("Bearer {}", srv.token)).body("{}").send().await.unwrap();
    assert_eq!(wrong_path.status(), 404);
    srv.stop();
}

#[test]
fn config_snippets_for_clients() {
    let s = config_snippets(std::path::Path::new("/home/me/My Project"), None, false, true);
    assert_eq!(s.claude_code, "claude mcp add devall -- devall mcp --project \"/home/me/My Project\"");
    let v: Value = serde_json::from_str(&s.mcp_servers_json).unwrap();
    assert_eq!(v["mcpServers"]["devall"]["args"], json!(["mcp", "--project", "/home/me/My Project"]));
    let w = config_snippets(std::path::Path::new("/p"), Some("/usr/local/bin/devall"), true, true);
    assert!(w.claude_code.ends_with("--project /p --allow-writes"));
    let vs: Value = serde_json::from_str(&w.vscode_json).unwrap();
    assert_eq!(vs["servers"]["devall"]["type"], "stdio");
    let ro = config_snippets(std::path::Path::new("/p"), None, false, false);
    assert!(ro.claude_code.ends_with("--project /p --read-only-notes"));
}

#[tokio::test]
async fn agents_keep_notes_and_todos() {
    let f = fixture().await;
    let srv = server(&f, false);
    let a = |v: Value| v.as_object().cloned().unwrap();
    let names: Vec<String> = srv.tools().iter().map(|t| t.name.to_string()).collect();
    for t in ["write_note", "append_to_note", "edit_note", "add_todo", "update_todo", "complete_todo", "comment_todo"] {
        assert!(names.iter().any(|n| n == t), "{t} missing from {names:?}");
    }

    // Notes: create (folders and .md added), no silent overwrite, append, exact edits.
    let w = srv.call("write_note", &a(json!({ "path": "agents/plan", "content": "# Plan\n\nStep 1\n" }))).await.unwrap();
    assert_eq!((w["path"].as_str(), w["created"].as_bool()), (Some("agents/plan.md"), Some(true)));
    assert!(srv.call("write_note", &a(json!({ "path": "notes/agents/plan.md", "content": "x" }))).await.unwrap_err().contains("already exists"));
    srv.call("append_to_note", &a(json!({ "path": "agents/plan.md", "text": "Step 2" }))).await.unwrap();
    srv.call("edit_note", &a(json!({ "path": "agents/plan.md", "oldText": "Step 1", "newText": "Step one" }))).await.unwrap();
    let p = Project::open(&f.root).unwrap();
    assert_eq!(p.read_note("agents/plan.md").unwrap(), "# Plan\n\nStep one\n\nStep 2\n");
    assert!(srv.call("edit_note", &a(json!({ "path": "agents/plan.md", "oldText": "Step", "newText": "S" }))).await.unwrap_err().contains("2 times"));
    let ow = srv.call("write_note", &a(json!({ "path": "agents/plan.md", "content": "# Plan v2\n", "overwrite": true }))).await.unwrap();
    assert_eq!(ow["created"], false);
    assert!(srv.call("write_note", &a(json!({ "path": "../escape.md", "content": "x" }))).await.is_err());
    assert!(srv.call("write_note", &a(json!({ "path": "run.ipynb", "content": "{}" }))).await.unwrap_err().contains("notebooks"));
    // The agent only saw this note masked: no wholesale replace, and masked text never matches.
    let masked = srv.call("write_note", &a(json!({ "path": "payments.md", "content": "# Payments\n", "overwrite": true }))).await.unwrap_err();
    assert!(masked.contains("masked"), "{masked}");
    let e = srv.call("edit_note", &a(json!({ "path": "payments.md", "oldText": "Rotate the token ••••••", "newText": "x" }))).await.unwrap_err();
    assert!(e.contains("masked"), "{e}");
    assert!(p.read_note("payments.md").unwrap().contains("tok-live-998877"));

    // To-dos: quick-add syntax, short ids, only this project's list.
    let added = srv.call("add_todo", &a(json!({ "text": "Write refund tests tomorrow p2 @payments", "description": "From the agent", "links": ["http/me.req.yaml"] }))).await.unwrap();
    let t = &added["task"];
    assert_eq!((t["content"].as_str(), t["priority"].as_str()), (Some("Write refund tests"), Some("p2")));
    assert_eq!(t["labels"], json!(["payments"]));
    assert!(t["due"]["date"].is_string());
    let id = t["id"].as_str().unwrap().to_string();
    let short = &id[id.len() - 6..];
    let up = srv.call("update_todo", &a(json!({ "id": short, "priority": "p1", "addLabels": ["@tests"], "due": "" }))).await.unwrap();
    assert_eq!(up["task"]["priority"], "p1");
    assert_eq!(up["task"]["labels"], json!(["payments", "tests"]));
    assert!(up["task"]["due"].is_null());
    assert!(srv.call("update_todo", &a(json!({ "id": short, "priority": "urgent" }))).await.is_err());
    assert_eq!(srv.call("comment_todo", &a(json!({ "id": short, "text": "Covered partial refunds" }))).await.unwrap()["comments"], 1);
    assert_eq!(srv.call("complete_todo", &a(json!({ "id": short }))).await.unwrap()["task"]["done"], true);
    // Attachments (added in the app) are listed by name.
    let mut st = devall_core::todos::TodoStore::open(&f.data).unwrap();
    st.attach_bytes(&id, "refunds.png", Some("image/png"), b"png", None, &devall_core::todos::Now::local()).unwrap();
    let listed = srv.call("list_todos", &a(json!({ "includeDone": true }))).await.unwrap();
    assert!(listed["items"].as_array().unwrap().iter().any(|i| i["id"] == id.as_str() && i["done"] == true && i["attachments"] == json!(["refunds.png"])));
    let data = devall_core::todos::TodoStore::read(&f.data).unwrap();
    let private = data.tasks.iter().find(|t| t.content == "Private errand in the Inbox").unwrap().id.clone();
    assert!(srv.call("complete_todo", &a(json!({ "id": private }))).await.unwrap_err().contains("not found"));

    // Read-only mode: the tools are not offered and calls are refused.
    let opts = McpOptions { read_only_local: true, app_data_dir: Some(f.data.clone()), no_keychain: true, ..Default::default() };
    let ro = DevallMcp::with_store(f.root.clone(), opts, Box::new(MemorySecretStore::default()));
    assert!(!ro.tools().iter().any(|t| t.name == "add_todo" || t.name == "write_note"));
    assert!(ro.call("add_todo", &a(json!({ "text": "nope" }))).await.unwrap_err().contains("turned off"));
}

#[test]
fn opening_a_missing_project_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    assert!(DevallMcp::new(dir.path(), McpOptions { no_keychain: true, ..Default::default() }).is_err());
}

/// Answers approvals from a queue (`None` = never answers) and records what it was asked.
struct FakeConfirmer {
    answers: std::sync::Mutex<std::collections::VecDeque<Option<crate::ApprovalDecision>>>,
    seen: std::sync::Mutex<Vec<crate::ApprovalRequest>>,
}

impl crate::Confirmer for FakeConfirmer {
    fn confirm(&self, request: crate::ApprovalRequest) -> crate::ApprovalFuture<'_> {
        self.seen.lock().unwrap().push(request);
        let answer = self.answers.lock().unwrap().pop_front().flatten();
        Box::pin(async move {
            match answer {
                Some(d) => d,
                None => std::future::pending().await,
            }
        })
    }
}

#[tokio::test]
async fn writes_wait_for_in_app_approval() {
    use crate::ApprovalDecision::*;
    let f = fixture().await;
    let fake = std::sync::Arc::new(FakeConfirmer { answers: Default::default(), seen: Default::default() });
    fake.answers.lock().unwrap().extend([Some(AllowOnce), Some(AllowSession), Some(Deny), Some(AllowOnce), None]);
    let opts = McpOptions { confirm_writes: true, environment: Some("local".into()), app_data_dir: Some(f.data.clone()), no_keychain: true, ..Default::default() };
    let approvals = crate::Approvals::new(fake.clone()).with_timeout(Duration::from_millis(300));
    let srv = DevallMcp::with_approvals(f.root.clone(), opts, Box::new(store_for(&f)), approvals);
    let args = |path: &str, env: Option<&str>| {
        let mut m = serde_json::Map::new();
        m.insert("path".into(), json!(path));
        if let Some(e) = env {
            m.insert("environment".into(), json!(e));
        }
        m
    };
    let send = srv.tools().into_iter().find(|t| t.name == "send_request").unwrap();
    assert!(send.description.as_deref().unwrap_or_default().contains("approve"), "{:?}", send.description);

    // GET: no question asked.
    assert!(srv.call("send_request", &args("me.req.yaml", None)).await.is_ok());
    assert!(fake.seen.lock().unwrap().is_empty());

    // POST: asked; allowed once → sent; asked again next time.
    let r = srv.call("send_request", &args("create-order.req.yaml", None)).await.unwrap();
    assert_eq!(r["request"]["method"], "POST");
    srv.call("send_request", &args("create-order.req.yaml", None)).await.unwrap();
    assert_eq!(fake.seen.lock().unwrap().len(), 2);
    // …the second answer was "for this session": not asked a third time.
    srv.call("send_request", &args("create-order.req.yaml", None)).await.unwrap();
    assert_eq!(fake.seen.lock().unwrap().len(), 2);
    {
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].request_path, "create-order.req.yaml");
        assert_eq!(seen[0].environment.as_deref(), Some("Local"));
        assert!(!seen[0].protected);
        assert!(seen[0].url.ends_with("/echo"));
    }

    // Protected environment: even a GET is asked; denied → a tool error, nothing sent.
    let denied = srv.call("send_request", &args("me.req.yaml", Some("prod"))).await.unwrap_err();
    assert!(denied.contains("denied"), "{denied}");
    {
        let seen = fake.seen.lock().unwrap();
        let last = seen.last().unwrap();
        assert!(last.protected && last.reason.contains("protected"));
        let shown = serde_json::to_string(last).unwrap();
        assert!(!shown.contains("tok-live-998877"), "secrets are masked in the prompt: {shown}");
        assert!(last.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "••••••"));
    }
    srv.call("send_request", &args("me.req.yaml", Some("prod"))).await.unwrap();

    // No answer in time = denied.
    let started = std::time::Instant::now();
    let timeout = srv.call("send_request", &args("me.req.yaml", Some("prod"))).await.unwrap_err();
    assert!(timeout.contains("denied") && started.elapsed() < Duration::from_secs(5), "{timeout}");

    // Without a confirmer (the stdio CLI), the flag behaviour stays: refused, never asked.
    let plain = server(&f, false);
    assert!(plain.call("send_request", &args("create-order.req.yaml", None)).await.unwrap_err().contains("--allow-writes"));
    assert!(plain.call("send_request", &args("me.req.yaml", Some("prod"))).await.unwrap_err().contains("protected"));
    let allow = server(&f, true);
    assert!(allow.call("send_request", &args("me.req.yaml", Some("prod"))).await.unwrap_err().contains("protected"), "allow-writes never covers protected envs");
}

struct FakeSource(std::sync::Arc<devall_ai::embed::FakeEmbedder>);

impl devall_ai::embed::EmbedderSource for FakeSource {
    fn embedder(&self) -> devall_ai::embed::EmbedderFuture<'_> {
        let e: std::sync::Arc<dyn devall_ai::embed::Embed> = self.0.clone();
        Box::pin(async move { Some(e) })
    }
}

#[tokio::test]
async fn search_project_uses_the_embedding_index() {
    let f = fixture().await;
    let srv = server(&f, false);
    let fake = std::sync::Arc::new(devall_ai::embed::FakeEmbedder::new(64));
    let p = Project::open(&f.root).unwrap();
    let chunks = devall_ai::semantic::collect_chunks(&p, &devall_ai::Redactor::default()).unwrap();
    devall_ai::semantic::build(&f.root, chunks, fake.as_ref(), &Default::default(), |_| {}).await.unwrap();
    srv.set_embedder_source(Some(std::sync::Arc::new(FakeSource(fake.clone()))));
    let args = json!({ "query": "list the orders", "limit": 3 }).as_object().unwrap().clone();
    let out = srv.call("search_project", &args).await.unwrap();
    assert_eq!(out["mode"], "hybrid", "{out}");
    assert!(out["hits"].as_array().unwrap().iter().any(|h| h["kind"] == "mock" && h["title"] == "List orders"), "{out}");
    // Without an embedder the same index is searched by keywords.
    srv.set_embedder_source(None);
    let out = srv.call("search_project", &args).await.unwrap();
    assert_eq!(out["mode"], "keyword");
}
