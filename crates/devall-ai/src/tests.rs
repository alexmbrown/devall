//! End-to-end tests against fake local model servers (no real model, no remote API).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devall_core::todos::{ProjectRef, TodoStore};
use devall_core::{EnvironmentFile, Project};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::features::{self, AiEnv, FeatureInput, ResponseContext};
use crate::provider::{Provider, ProviderConfig, ProviderKind};
use crate::redact::Redactor;
use crate::{AiError, prompts};

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

/// What the fake server answers: status, content type, body chunks (streamed with small gaps).
type Reply = (u16, &'static str, Vec<String>);
type Handler = Arc<dyn Fn(&Seen) -> Reply + Send + Sync>;

async fn fake_server(handler: Handler) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let handler = handler.clone();
            let seen = seen2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let header_end = loop {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let mut lines = head.lines();
                let path = lines.next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/").to_string();
                let headers: Vec<(String, String)> =
                    lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
                let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
                while buf.len() < header_end + len {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body = serde_json::from_slice(&buf[header_end..]).unwrap_or(Value::Null);
                let req = Seen { path, headers, body };
                let (status, ct, chunks) = handler(&req);
                seen.lock().unwrap().push(req);
                let head = format!("HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nConnection: close\r\n\r\n");
                let _ = s.write_all(head.as_bytes()).await;
                for c in chunks {
                    let _ = s.write_all(c.as_bytes()).await;
                    let _ = s.flush().await;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let _ = s.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

fn json_reply(v: Value) -> Reply {
    (200, "application/json", vec![v.to_string()])
}

fn openai_message(content: &str) -> Value {
    json!({ "model": "fake-qwen3.5", "choices": [{ "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }], "usage": { "prompt_tokens": 10, "completion_tokens": 5 } })
}

fn openai_sse(deltas: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = deltas.iter().map(|d| format!("data: {}\n\n", json!({ "model": "fake", "choices": [{ "delta": { "content": d } }] }))).collect();
    out.push(format!("data: {}\n\n", json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] })));
    out.push("data: [DONE]\n\n".into());
    out
}

/// A project with a note (containing a secret value), to-dos and a staging environment.
fn project_with_knowledge() -> (tempfile::TempDir, tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "Shop").unwrap();
    let n = p.create_note("", "Payments").unwrap();
    p.save_note(&n, "# Payments\n\nStripe webhooks are verified with the signing secret whsec_5tKqA9yRm2Lp8XwZ3bN7cV1d.\nRefunds use POST /refunds.\n").unwrap();
    let mut env = EnvironmentFile::new("Staging");
    env.variables.insert("baseUrl".into(), "https://api.staging.example.com".into());
    env.secrets = vec!["STRIPE_KEY".into()];
    p.save_environment("staging", &env).unwrap();
    let mut l = TodoStore::open(base.path()).unwrap();
    l.add_text(Some(&ProjectRef::of(&p)), "Re-sign stripe webhook replays").unwrap();
    l.add_text(Some(&ProjectRef::of(&p)), "Add idempotency keys to payment charges").unwrap();
    (dir, base, p)
}

fn local_provider(base: &str) -> Provider {
    Provider::new(ProviderConfig::new("local", "llama.cpp", ProviderKind::OpenAiCompatible, format!("{base}/v1"), "qwen3.5-4b"), None).unwrap()
}

#[tokio::test]
async fn ask_streams_tokens_cites_todos_and_redacts_secrets() {
    let (_d, base_dir, p) = project_with_knowledge();
    let handler: Handler = Arc::new(|_req: &Seen| {
        (200, "text/event-stream", openai_sse(&["<think>\n\n</think>\n\n", "Two items ", "remain [S1]: re-sign ", "webhook replays and add idempotency keys [S1]. ", "Refunds: see [S2]."]))
    });
    let (url, seen) = fake_server(handler).await;
    let provider = local_provider(&url);
    let redactor = Redactor::new(["whsec_5tKqA9yRm2Lp8XwZ3bN7cV1d".to_string()].into());
    let env = AiEnv::new(Some(&p), Some(base_dir.path()), redactor, false);

    let preview = features::preview(&FeatureInput::Ask { question: "What's left on payments?".into() }, &env, &provider).unwrap();
    let mut tokens = Vec::new();
    let answer = features::ask_stream(&env, &provider, "What's left on payments?", |t| tokens.push(t.to_string())).await.unwrap();

    assert_eq!(tokens.concat(), answer.answer, "streamed tokens equal the final answer (think block hidden)");
    assert!(answer.answer.starts_with("Two items remain"));
    assert_eq!(answer.citations[0].kind, "todos");
    assert_eq!(answer.citations[1].source, "notes/payments.md");
    assert!(!answer.meta.remote);

    let sent = &seen.lock().unwrap()[0];
    assert_eq!(sent.path, "/v1/chat/completions");
    assert_eq!(sent.body["stream"], true);
    assert_eq!(sent.body["chat_template_kwargs"]["enable_thinking"], false, "thinking off for local Qwen");
    let prompt = sent.body["messages"][1]["content"].as_str().unwrap();
    assert!(prompt.contains("- [ ] Add idempotency keys to payment charges"));
    assert!(!prompt.contains("whsec_5tKqA9yRm2Lp8XwZ3bN7cV1d"), "keychain secret values never leave, even to local models");
    assert!(prompt.contains("••••••"));
    assert_eq!(preview.payload.body, sent.body, "preview shows exactly what was sent");
    assert_eq!(preview.redactions, 1);
}

#[tokio::test]
async fn nl_regex_validates_against_samples_and_repairs_once() {
    let replies = Arc::new(Mutex::new(VecDeque::from([
        openai_message(r#"{"pattern": "\\d+", "flags": "", "explanation": "digits"}"#),
        openai_message("```json\n{\"pattern\": \"^ORD-\\\\d{4}$\", \"flags\": \"\", \"explanation\": \"order ids\"}\n```"),
    ])));
    let r2 = replies.clone();
    let (url, seen) = fake_server(Arc::new(move |_: &Seen| json_reply(r2.lock().unwrap().pop_front().unwrap()))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let out = features::nl_to_regex(&env, &provider, "order ids like ORD-1234", &["ORD-1234".into(), "ORD-0001".into()], &["ORD-12".into(), "1234".into()]).await.unwrap();
    assert_eq!(out.pattern, r"^ORD-\d{4}$");
    assert!(out.valid && out.compiles && out.meta.repaired);
    assert!(out.checks.iter().all(|c| c.matched == c.should_match));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].body["response_format"]["type"], "json_schema");
    let repair = seen[1].body["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(repair.contains("matches negative sample `1234`"), "{repair}");
}

#[tokio::test]
async fn falls_back_to_prompt_only_json_when_response_format_is_rejected() {
    let (url, seen) = fake_server(Arc::new(|req: &Seen| {
        if req.body.get("response_format").is_some() {
            (400, "application/json", vec![r#"{"error":{"message":"response_format is not supported"}}"#.to_string()])
        } else {
            json_reply(openai_message(r#"Here: {"path": "$.items[*].id", "explanation": "all ids"}"#))
        }
    }))
    .await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let sample = r#"{"items":[{"id":1},{"id":2}]}"#;
    let out = features::nl_to_jsonpath(&env, &provider, "all item ids", sample).await.unwrap();
    assert_eq!(out.path, "$.items[*].id");
    assert_eq!(out.matches, vec![json!(1), json!(2)]);
    assert!(out.valid && !out.meta.schema_enforced);
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn nl_jq_runs_the_filter_on_the_sample_and_repairs_once() {
    let replies = Arc::new(Mutex::new(VecDeque::from([
        // Wrong field name: runs, but yields only nulls → repaired.
        openai_message(r#"{"filter": ".item[0].id", "explanation": "first id"}"#),
        openai_message(r#"{"filter": "[.items[] | select(.active) | .id]", "explanation": "ids of active items"}"#),
    ])));
    let r2 = replies.clone();
    let (url, seen) = fake_server(Arc::new(move |_: &Seen| json_reply(r2.lock().unwrap().pop_front().unwrap()))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let sample = r#"{"items":[{"id":1,"active":true},{"id":2,"active":false},{"id":3,"active":true}]}"#;
    let out = features::nl_to_jq(&env, &provider, "ids of the active items", sample).await.unwrap();
    assert_eq!(out.filter, "[.items[] | select(.active) | .id]");
    assert!(out.valid && out.meta.repaired, "{out:?}");
    assert_eq!(serde_json::from_str::<Value>(&out.output).unwrap(), json!([1, 3]));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let repair = seen[1].body["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(repair.contains("returns null"), "{repair}");
    assert!(features::run_jq("not valid jq (", sample).is_err());
}

#[tokio::test]
async fn invalid_json_after_repair_is_an_error() {
    let (url, _) = fake_server(Arc::new(|_: &Seen| json_reply(openai_message("I can't do that in JSON, sorry.")))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let err = features::explain_error(&env, &provider, "panicked at src/main.rs:3:5").await.unwrap_err();
    assert!(matches!(err, AiError::InvalidOutput { .. }), "{err}");
}

#[tokio::test]
async fn anthropic_messages_api_json_and_streaming() {
    let (url, seen) = fake_server(Arc::new(|req: &Seen| {
        if req.body["stream"] == true {
            let events = [
                json!({"type":"message_start","message":{"model":"claude-opus-5-5","usage":{"input_tokens":12}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Root cause: "}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"a nil map [S1]."}}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}),
                json!({"type":"message_stop"}),
            ];
            let chunks = events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect();
            (200, "text/event-stream", chunks)
        } else {
            let text = json!({"summary":"Index out of range","rootCause":"Vec is empty","suggestedFixes":[{"title":"Check len","detail":"Guard before indexing"}],"relevantLines":["src/main.rs:3"]}).to_string();
            json_reply(json!({"model":"claude-opus-5-5","content":[{"type":"thinking","thinking":""},{"type":"text","text": text}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":9}}))
        }
    }))
    .await;
    let mut cfg = ProviderConfig::new("anthropic", "Anthropic", ProviderKind::Anthropic, url.clone(), "claude-opus-5-5");
    cfg.trusted_local = false;
    let provider = Provider::new(cfg, Some("sk-ant-test-key-0123456789abcdef".into())).unwrap();
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let out = features::explain_error(&env, &provider, "thread 'main' panicked at src/main.rs:3: index out of bounds\nAuthorization: Bearer abcdefghijklmnop").await.unwrap();
    assert_eq!(out.root_cause, "Vec is empty");
    assert_eq!(out.suggested_fixes[0].title, "Check len");
    {
        let seen = seen.lock().unwrap();
        let req = &seen[0];
        assert_eq!(req.path, "/v1/messages");
        assert!(req.headers.iter().any(|(k, v)| k == "x-api-key" && v == "sk-ant-test-key-0123456789abcdef"));
        assert!(req.headers.iter().any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
        assert_eq!(req.body["output_config"]["format"]["type"], "json_schema");
        assert!(req.body.get("temperature").is_none());
        // 127.0.0.1 counts as local; the bearer token is only pattern-redacted for remote hosts.
        assert!(req.body["messages"][0]["content"].as_str().unwrap().contains("index out of bounds"));
    }

    let mut tokens = String::new();
    let resp = provider.chat_stream(&crate::ChatRequest { messages: vec![crate::ChatMessage::user("why?")], ..Default::default() }, |t| tokens.push_str(t)).await.unwrap();
    assert_eq!(resp.text, "Root cause: a nil map [S1].");
    assert_eq!(tokens, resp.text);
    assert_eq!(resp.usage.output_tokens, Some(7));
    assert_eq!(resp.model, "claude-opus-5-5");
}

#[tokio::test]
async fn ollama_native_streaming_with_think_off() {
    let (url, seen) = fake_server(Arc::new(|req: &Seen| {
        if req.body.get("think").is_some() && req.body["model"] == "old-model" {
            return (400, "application/json", vec![r#"{"error":"\"old-model\" does not support thinking"}"#.to_string()]);
        }
        let lines = vec![
            format!("{}\n", json!({"model":"qwen3.5:4b","message":{"role":"assistant","content":"Hel"},"done":false})),
            format!("{}\n", json!({"model":"qwen3.5:4b","message":{"role":"assistant","content":"lo"},"done":false})),
            format!("{}\n", json!({"model":"qwen3.5:4b","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":3,"eval_count":2})),
        ];
        (200, "application/x-ndjson", lines)
    }))
    .await;
    let provider = Provider::new(ProviderConfig::new("ollama", "Ollama", ProviderKind::Ollama, url.clone(), "qwen3.5:4b"), None).unwrap();
    let mut got = String::new();
    let resp = provider.chat_stream(&crate::ChatRequest { messages: vec![crate::ChatMessage::user("hi")], ..Default::default() }, |t| got.push_str(t)).await.unwrap();
    assert_eq!((got.as_str(), resp.text.as_str()), ("Hello", "Hello"));
    assert_eq!(resp.usage.output_tokens, Some(2));
    assert_eq!(seen.lock().unwrap()[0].path, "/api/chat");
    assert_eq!(seen.lock().unwrap()[0].body["think"], false);

    // A model without a thinking toggle: retried without `think`.
    let old = Provider::new(ProviderConfig::new("ollama", "Ollama", ProviderKind::Ollama, url, "old-model"), None).unwrap();
    let resp = old.chat_stream(&crate::ChatRequest { messages: vec![crate::ChatMessage::user("hi")], ..Default::default() }, |_| {}).await.unwrap();
    assert_eq!(resp.text, "Hello");
    assert!(seen.lock().unwrap().last().unwrap().body.get("think").is_none());
}

#[tokio::test]
async fn cloud_providers_get_no_notes_unless_allowed_and_full_redaction() {
    let (_d, base_dir, p) = project_with_knowledge();
    let remote = Provider::new(ProviderConfig::new("or", "OpenRouter", ProviderKind::OpenAiCompatible, "https://openrouter.ai/api/v1", "qwen/qwen3.8-27b"), Some("sk-or-v1-abc".into())).unwrap();
    let env = AiEnv::new(Some(&p), Some(base_dir.path()), Redactor::default(), false);
    let err = features::preview(&FeatureInput::Ask { question: "what's left?".into() }, &env, &remote).unwrap_err();
    assert!(matches!(err, AiError::RemoteContextBlocked(_)));

    // Explicit input is fine, with credential patterns masked for the cloud.
    let ctx = ResponseContext {
        method: "GET".into(),
        url: "https://api.example.com/me?api_key=live-abcdef123".into(),
        request_headers: vec![("Authorization".into(), "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl".into())],
        status: 401,
        status_text: "Unauthorized".into(),
        headers: vec![("WWW-Authenticate".into(), "Bearer error=\"invalid_token\"".into())],
        body: r#"{"error":"token expired"}"#.into(),
        ..Default::default()
    };
    let pv = features::preview(&FeatureInput::ExplainResponse(ctx), &env, &remote).unwrap();
    let sent = pv.payload.body.to_string();
    assert!(pv.remote && pv.redactions >= 2, "{}", pv.redactions);
    assert!(!sent.contains("live-abcdef123") && !sent.contains("eyJhbGciOiJIUzI1NiJ9"), "{sent}");
    assert!(pv.payload.headers.iter().all(|(_, v)| !v.contains("sk-or-v1-abc")));
    assert!(pv.notice.contains("openrouter.ai"));

    let allowed = AiEnv::new(Some(&p), Some(base_dir.path()), Redactor::default(), true);
    let pv = features::preview(&FeatureInput::Ask { question: "what's left?".into() }, &allowed, &remote).unwrap();
    let prompt = pv.payload.body["messages"][1]["content"].as_str().unwrap();
    assert!(!prompt.contains("whsec_5tKqA9yRm2Lp8XwZ3bN7cV1d"), "credential-looking strings masked for cloud providers");
}

#[tokio::test]
async fn mock_data_from_a_sample_is_validated() {
    let items = json!({"items": [
        {"id": 1, "email": "ada@example.com", "active": true},
        {"id": 2, "email": "bob@example.org", "active": false},
        {"id": 3, "email": "cy@example.com", "active": true}
    ]});
    let reply = openai_message(&items.to_string());
    let (url, seen) = fake_server(Arc::new(move |_: &Seen| json_reply(reply.clone()))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let out = features::generate_mock_data(&env, &provider, r#"[{"id": 7, "email": "x@example.com", "active": true}]"#, 3).await.unwrap();
    assert!(out.inferred_schema && out.valid, "{:?}", out.errors);
    assert_eq!(out.items.len(), 3);
    assert_eq!(out.schema["properties"]["email"]["format"], "email");
    let sent = seen.lock().unwrap()[0].body.clone();
    assert_eq!(sent["response_format"]["json_schema"]["strict"], false);
    assert_eq!(sent["response_format"]["json_schema"]["schema"]["properties"]["items"]["type"], "array");

    let err = features::generate_mock_data(&env, &provider, "not json", 3).await.unwrap_err();
    assert!(matches!(err, AiError::Invalid(_)));
}

#[tokio::test]
async fn request_drafts_use_project_variables() {
    let (_d, _b, p) = project_with_knowledge();
    let draft = json!({
        "name": "Create refund", "method": "post", "url": "{{baseUrl}}/refunds",
        "query": [], "headers": [{"name": "Idempotency-Key", "value": "{{$uuid}}"}],
        "bodyType": "json", "body": "{\"charge\": \"{{chargeId}}\", \"amount\": 500}",
        "authType": "bearer", "authValue": "{{secret.STRIPE_KEY}}", "explanation": "POST to refunds"
    });
    let reply = openai_message(&draft.to_string());
    let (url, seen) = fake_server(Arc::new(move |_: &Seen| json_reply(reply.clone()))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(Some(&p), None, Redactor::default(), false);
    let out = features::nl_to_request(&env, &provider, "create a refund for a charge").await.unwrap();
    assert_eq!(out.request.method, "POST");
    assert_eq!(out.request.url, "{{baseUrl}}/refunds");
    assert!(matches!(&out.request.auth, devall_core::Auth::Bearer { token, .. } if token == "{{secret.STRIPE_KEY}}"));
    assert!(matches!(&out.request.body, devall_core::Body::Json { content } if content.contains("\"amount\": 500")));
    assert_eq!(out.unknown_variables, vec!["chargeId".to_string()]);
    let prompt = seen.lock().unwrap()[0].body["messages"][1]["content"].as_str().unwrap().to_string();
    assert!(prompt.contains("{{baseUrl}}") && prompt.contains("{{secret.STRIPE_KEY}}"));
    assert!(!prompt.contains("api.staging.example.com"), "variable values are not sent, only names");
}

#[test]
fn suggested_tests_are_validated_against_the_sample() {
    let sample = ResponseContext {
        status: 200,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: r#"{"data":[{"id":1,"name":"Ada"}],"total":1}"#.into(),
        ..Default::default()
    };
    let model = json!({"tests": [
        {"target":"jsonPath","selector":"$.data[0].id","op":"exists","value":"","reason":"id present"},
        {"target":"jsonPath","selector":"$.data[0].missing","op":"exists","value":"","reason":"bogus"},
        {"target":"jsonPath","selector":"$.total","op":"isType","value":"number","reason":"typed"},
        {"target":"header","selector":"X-Nope","op":"exists","value":"","reason":"absent"},
        {"target":"contentType","selector":"","op":"contains","value":"json","reason":"json"},
        {"target":"status","selector":"","op":"wobble","value":"200","reason":"bad op"},
        {"target":"jsonPath","selector":"$.data[0].id","op":"exists","value":"","reason":"duplicate"}
    ]});
    let (ok, dropped) = features::validate_tests(&model, &sample);
    let labels: Vec<String> = ok.iter().map(|t| serde_json::to_string(&t.assertion).unwrap()).collect();
    assert_eq!(ok.len(), 4, "{labels:?}");
    assert_eq!(ok[0].assertion.target, devall_core::AssertTarget::Status, "baseline status test added first");
    assert_eq!(dropped.len(), 3, "{dropped:?}");
    assert!(ok.iter().all(|t| t.assertion.enabled));
}

#[test]
fn todos_are_cleaned() {
    use devall_core::todos::{Priority, dates::DateCtx};
    let v = json!({"todos": [
        {"text": "- Ship the refunds endpoint", "priority": "p1", "due": "2026-11-03", "labels": ["@API", "Payments team", "api"], "links": ["http/payments/refund.req.yaml", "not a link"], "description": "Finance asked twice"},
        {"text": "ship the refunds endpoint", "priority": "p4", "due": "", "links": []},
        {"text": "Email finance", "priority": "urgent", "due": "next friday 5pm", "links": []},
        {"text": "Water the plants", "priority": 3, "due": "every monday", "links": []},
        {"text": "Plan offsite", "priority": "high", "due": "sometime soon", "links": []},
        {"text": "   ", "priority": "p2", "due": "", "links": []}
    ]});
    // Friday 2026-10-23.
    let ctx = DateCtx::new(chrono::NaiveDate::from_ymd_opt(2026, 10, 23).unwrap(), chrono::NaiveTime::from_hms_opt(14, 30, 0).unwrap());
    let t = features::clean_todos(&v, &ctx);
    assert_eq!(t.len(), 4);
    assert_eq!(t[0].text, "Ship the refunds endpoint");
    assert_eq!((t[0].priority, t[0].due.as_deref(), t[0].due_date.as_deref()), (Priority::P1, Some("2026-11-03"), Some("2026-11-03")));
    assert_eq!(t[0].labels, ["api", "payments-team"]);
    assert_eq!(t[0].links, vec!["http/payments/refund.req.yaml"]);
    assert_eq!(t[0].description, "Finance asked twice");
    assert_eq!((t[1].priority, t[1].due.as_deref(), t[1].due_date.as_deref()), (Priority::P1, Some("next friday 5pm"), Some("2026-10-30")));
    assert_eq!((t[2].priority, t[2].due_date.as_deref()), (Priority::P3, Some("2026-10-26")));
    assert_eq!((t[3].priority, t[3].due.as_deref()), (Priority::P2, None), "unparseable dates are dropped");
}

#[test]
fn feature_inputs_parse_from_ui_json() {
    let f = FeatureInput::from_parts("nlRegex", json!({"description": "x", "positive": ["a"]})).unwrap();
    assert!(matches!(f, FeatureInput::NlRegex { ref negative, .. } if negative.is_empty()));
    let f = FeatureInput::from_parts("mockData", json!({"schemaOrSample": "{}"})).unwrap();
    assert!(matches!(f, FeatureInput::MockData { count: 10, .. }));
    let f = FeatureInput::from_parts("suggestTests", json!({"status": 200, "body": "{}"})).unwrap();
    assert_eq!(f.name(), "suggestTests");
    assert!(FeatureInput::from_parts("nope", json!({})).is_err());
    assert!(prompts::TODOS_SYSTEM.contains("{today}"));
}

fn failure_context() -> features::FailureContext {
    features::FailureContext {
        exchange: ResponseContext {
            method: "POST".into(),
            url: "http://127.0.0.1:18080/orders".into(),
            status: 500,
            status_text: "Server Error".into(),
            body: r#"{"error":"db unavailable"}"#.into(),
            ..Default::default()
        },
        error: None,
        sources: vec![
            features::FailureSource { kind: "mockNearMiss".into(), source: "mocks/mappings/orders.json#7c6b".into(), title: "Closest mock stub".into(), text: "POST /orders requires $.sku".into() },
            features::FailureSource { kind: "podEvents".into(), source: "pod:kind/shop/orders-7d9".into(), title: "Pod orders-7d9 events".into(), text: "Warning BackOff: Back-off restarting failed container (x5)".into() },
        ],
    }
}

#[tokio::test]
async fn explain_failure_cites_gathered_sources() {
    let answer = json!({
        "summary": "The orders pod is crash-looping [S3], so the forward returns 500 [S1].",
        "likelyCauses": ["The database connection fails on start [S3]"],
        "suggestedFixes": [{ "title": "Check the pod logs", "detail": "See the BackOff events [S3]" }],
        "severity": "error"
    });
    let reply = openai_message(&answer.to_string());
    let (url, seen) = fake_server(Arc::new(move |_: &Seen| json_reply(reply.clone()))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let out = features::explain_failure(&env, &provider, &failure_context()).await.unwrap();
    assert_eq!(out.sources.len(), 3);
    assert_eq!(out.sources[0].id, "S1");
    assert_eq!(out.sources[2].source, "pod:kind/shop/orders-7d9");
    let cited: Vec<&str> = out.citations.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(cited, ["S3", "S1"]);
    assert!(out.omitted.is_empty());
    let prompt = seen.lock().unwrap()[0].body["messages"][1]["content"].as_str().unwrap().to_string();
    assert!(prompt.contains("<source id=\"S2\" kind=\"mockNearMiss\""), "{prompt}");
    assert!(prompt.contains("BackOff"));
}

#[tokio::test]
async fn explain_failure_keeps_project_context_local_unless_allowed() {
    let remote = Provider::new(ProviderConfig::new("or", "OpenRouter", ProviderKind::OpenAiCompatible, "https://openrouter.ai/api/v1", "qwen/qwen3.8-27b"), Some("sk-or-v1-abc".into())).unwrap();
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let pv = features::preview(&FeatureInput::ExplainFailure(failure_context()), &env, &remote).unwrap();
    let sent = pv.payload.body.to_string();
    assert!(sent.contains("db unavailable"), "the user's own exchange is always sent");
    assert!(!sent.contains("BackOff") && !sent.contains("requires $.sku"), "{sent}");
    assert_eq!(pv.sources.len(), 1);
    assert!(pv.notice.contains("Not sent: Closest mock stub, Pod orders-7d9 events"), "{}", pv.notice);
    let allowed = AiEnv::new(None, None, Redactor::default(), true);
    let pv = features::preview(&FeatureInput::ExplainFailure(failure_context()), &allowed, &remote).unwrap();
    assert_eq!(pv.sources.len(), 3);

    // A transport error (no response) is explained too.
    let mut fc = failure_context();
    fc.exchange.status = 0;
    fc.error = Some("connection refused (os error 10061)".into());
    let pv = features::preview(&FeatureInput::ExplainFailure(fc), &allowed, &remote).unwrap();
    assert!(pv.payload.body.to_string().contains("No response"));
    let parsed = FeatureInput::from_parts("explainFailure", json!({ "status": 0, "url": "http://x", "error": "refused", "sources": [] })).unwrap();
    assert_eq!(parsed.name(), "explainFailure");
}

#[tokio::test]
async fn openapi_names_are_validated_repaired_and_applied() {
    let spec = "openapi: 3.1.0\ninfo: { title: shop API, version: 0.1.0 }\npaths:\n  /orders:\n    get:\n      operationId: getOrders\n      summary: GET /orders\n      responses:\n        '200':\n          description: OK\n          content: { application/json: { schema: { type: object, properties: { id: { type: integer }, total: { type: number } } } } }\n    post:\n      operationId: postOrders\n      summary: POST /orders\n      responses: { '201': { description: Created } }\n";
    let replies = Arc::new(Mutex::new(VecDeque::from([
        // Duplicate operationId → repaired.
        openai_message(&json!({ "operations": [
            { "key": "GET /orders", "operationId": "orders", "summary": "List orders", "tags": ["Orders"] },
            { "key": "POST /orders", "operationId": "orders", "summary": "Create an order", "tags": ["Orders"] }
        ], "schemas": [{ "body": "B1", "name": "Order" }], "tags": [] }).to_string()),
        openai_message(&json!({ "operations": [
            { "key": "GET /orders", "operationId": "listOrders", "summary": "List orders", "tags": ["Orders"] },
            { "key": "POST /orders", "operationId": "createOrder", "summary": "Create an order", "tags": ["Orders"] }
        ], "schemas": [{ "body": "B1", "name": "Order" }], "tags": [{ "name": "Orders", "description": "Purchases" }] }).to_string()),
    ])));
    let r2 = replies.clone();
    let (url, seen) = fake_server(Arc::new(move |_: &Seen| json_reply(r2.lock().unwrap().pop_front().unwrap()))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let out = features::improve_openapi_names(&env, &provider, spec).await.unwrap();
    assert!(out.meta.repaired && out.problem.is_none(), "{:?}", out.problem);
    assert_eq!(out.spec["paths"]["/orders"]["post"]["operationId"], "createOrder");
    assert_eq!(out.spec["paths"]["/orders"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["$ref"], "#/components/schemas/Order");
    assert_eq!(out.proposal.tags[0].description, "Purchases");
    let seen = seen.lock().unwrap();
    let repair = seen[1].body["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(repair.contains("used twice"), "{repair}");
    assert_eq!(seen[0].body["response_format"]["json_schema"]["name"], "openapi_names");
    let pv = features::preview(&FeatureInput::OpenapiNames { spec: spec.into() }, &env, &provider).unwrap();
    assert!(pv.payload.body.to_string().contains("POST /orders"));
}

/// Accepts candidates whose `field` contains `ok`, recording every candidate it saw.
struct FakeChecker {
    field: &'static str,
    ok: &'static str,
    seen: Mutex<Vec<String>>,
}

impl crate::structured::Checker for FakeChecker {
    fn check<'a>(&'a self, candidate: &'a Value) -> crate::structured::CheckFuture<'a> {
        let text = candidate[self.field].as_str().unwrap_or_default().to_string();
        self.seen.lock().unwrap().push(text.clone());
        Box::pin(async move { if text.contains(self.ok) { Ok(json!({ "plan": "SCAN users" })) } else { Err(format!("EXPLAIN failed: no such table in `{text}`")) } })
    }
}

#[tokio::test]
async fn nl_sql_is_read_only_and_checked_with_explain() {
    let replies = Arc::new(Mutex::new(VecDeque::from([
        openai_message(r#"{"sql": "DELETE FROM users", "explanation": "x"}"#),
        openai_message(r#"{"sql": "SELECT id, email FROM users LIMIT 100;", "explanation": "users"}"#),
        openai_message(r#"{"sql": "SELECT * FROM userz", "explanation": "typo"}"#),
        openai_message(r#"{"sql": "SELECT * FROM users", "explanation": "fixed"}"#),
    ])));
    let r2 = replies.clone();
    let (url, seen) = fake_server(Arc::new(move |_: &Seen| json_reply(r2.lock().unwrap().pop_front().unwrap()))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let input = FeatureInput::NlSql { question: "emails of all users".into(), dialect: "sqlite".into(), schema: "users(id INTEGER PK, email TEXT)".into(), allow_writes: false, session: None };
    let checker = FakeChecker { field: "sql", ok: "FROM users", seen: Mutex::new(Vec::new()) };
    // A write is refused before the database sees it, then the repaired SELECT is EXPLAINed.
    let out = features::nl_to_sql(&env, &provider, &input, &checker).await.unwrap();
    assert_eq!(out.sql, "SELECT id, email FROM users LIMIT 100");
    assert!(out.validated && out.meta.repaired);
    assert_eq!(out.check["plan"], "SCAN users");
    assert_eq!(checker.seen.lock().unwrap().len(), 1, "the DELETE never reached the checker");
    let first = seen.lock().unwrap()[0].body["messages"][1]["content"].as_str().unwrap().to_string();
    assert!(first.contains("users(id INTEGER PK, email TEXT)"), "{first}");
    let sys = seen.lock().unwrap()[0].body["messages"][0]["content"].as_str().unwrap().to_string();
    assert!(sys.contains("MUST be read-only"), "{sys}");
    // An EXPLAIN failure goes back to the model once.
    let out = features::nl_to_sql(&env, &provider, &input, &checker).await.unwrap();
    assert!(out.validated && out.sql == "SELECT * FROM users");
    let repair = seen.lock().unwrap()[3].body["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(repair.contains("EXPLAIN failed"), "{repair}");
}

#[tokio::test]
async fn nl_mongo_and_kubectl_are_shape_checked_then_validated() {
    let replies = Arc::new(Mutex::new(VecDeque::from([
        openai_message(&json!({"type": "aggregate", "filter": "", "projection": "", "sort": "", "limit": 0, "pipeline": "[{\"$match\": {\"status\": \"paid\"}}, {\"$group\": {\"_id\": \"$userId\", \"n\": {\"$sum\": 1}}}]", "explanation": "per user"}).to_string()),
        openai_message(r#"{"command": "kubectl get pods -n shop | grep api", "explanation": "x"}"#),
        openai_message(r#"{"command": "kubectl scale deployment/api --replicas=3 -n shop", "explanation": "scale"}"#),
    ])));
    let r2 = replies.clone();
    let (url, _) = fake_server(Arc::new(move |_: &Seen| json_reply(r2.lock().unwrap().pop_front().unwrap()))).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::default(), false);
    let mongo = FeatureInput::NlMongo { question: "paid orders per user".into(), database: "shop".into(), collection: "orders".into(), fields: "status: string\nuserId: objectId".into(), session: None };
    let checker = FakeChecker { field: "pipeline", ok: "$group", seen: Mutex::new(Vec::new()) };
    let m = features::nl_to_mongo(&env, &provider, &mongo, &checker).await.unwrap();
    assert_eq!(m.kind, "aggregate");
    assert!(m.validated && m.pipeline.contains("\"$group\""));
    let kube = FeatureInput::NlKubectl { question: "scale the api to 3".into(), context: "kind-dev".into(), namespace: "shop".into(), kinds: String::new() };
    let checker = FakeChecker { field: "command", ok: "scale", seen: Mutex::new(Vec::new()) };
    let k = features::nl_to_kubectl(&env, &provider, &kube, &checker).await.unwrap();
    assert_eq!(k.command, "kubectl scale deployment/api --replicas=3 -n shop");
    assert!(k.validated && k.meta.repaired, "the piped command was refused first");
    assert_eq!(FeatureInput::from_parts("nlSql", json!({"question": "q", "schema": "s"})).unwrap().name(), "nlSql");
}

#[tokio::test]
async fn commit_message_masks_credentials_even_locally() {
    let handler: Handler = Arc::new(|_req: &Seen| json_reply(openai_message(r#"{"subject": "Add the login form.", "body": "Validates the e-mail first."}"#)));
    let (url, seen) = fake_server(handler).await;
    let provider = local_provider(&url);
    let env = AiEnv::new(None, None, Redactor::new(["hunter2-project-secret".to_string()].into_iter().collect()), false);
    let diff = "diff --git a/src/login.ts b/src/login.ts\n+const key = 'sk-ant-abcdefghijklmnopqrstuvwxyz0123';\n+const pw = 'hunter2-project-secret';\n+export function login() {}\n";
    let input = FeatureInput::from_parts("commitMessage", json!({ "diff": diff, "branch": "feature/login", "recent": ["feat: add signup"] })).unwrap();
    assert_eq!(input.name(), "commitMessage");
    let preview = features::preview(&input, &env, &provider).unwrap();
    assert!(preview.redactions >= 2, "{}", preview.redactions);
    let out = features::commit_message(&env, &provider, &input).await.unwrap();
    assert_eq!(out.subject, "Add the login form");
    assert_eq!(out.message, "Add the login form\n\nValidates the e-mail first.");
    assert!(out.meta.redactions >= 2);
    let sent = seen.lock().unwrap()[0].body.to_string();
    assert!(!sent.contains("sk-ant-abcdefghijklmnop"), "credential pattern masked for a local model");
    assert!(!sent.contains("hunter2-project-secret"), "project secret masked");
    assert!(sent.contains("feature/login") && sent.contains("feat: add signup"));
    let empty = FeatureInput::from_parts("commitMessage", json!({ "diff": "  " })).unwrap();
    assert!(features::commit_message(&env, &provider, &empty).await.is_err());
    assert_eq!(features::clean_subject("\"Fix a very long subject line that keeps going and going well beyond the seventy-two character limit.\""), "Fix a very long subject line that keeps going and going well beyond the");
}
