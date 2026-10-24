//! Proxying to a local upstream, recording and snapshots.

mod common;

use common::{Fixture, start, start_with};
use serde_json::{Value, json};

/// An upstream that echoes what it received, plus a few fixed resources.
async fn upstream() -> Fixture {
    let f = start().await;
    f.stub(json!({
        "request": { "urlPathPattern": "/echo/.*" },
        "response": {
            "headers": { "Content-Type": "application/json" },
            "jsonBody": { "path": "{{request.path}}", "query": "{{request.query.q}}", "added": "{{request.headers.X-Added}}", "secret": "{{request.headers.X-Secret}}", "method": "{{request.method}}", "body": "{{request.body}}" },
            "transformers": ["response-template"]
        }
    }))
    .await;
    f.stub(json!({ "request": { "method": "GET", "urlPath": "/api/users" }, "response": { "headers": { "Content-Type": "application/json", "X-Upstream": "1" }, "jsonBody": [{ "id": 1, "name": "Ann" }] } })).await;
    f.stub(json!({ "scenarioName": "counter", "requiredScenarioState": "Started", "newScenarioState": "two", "request": { "urlPath": "/api/counter" }, "response": { "headers": { "Content-Type": "application/json" }, "jsonBody": { "n": 1 } } })).await;
    f.stub(json!({ "scenarioName": "counter", "requiredScenarioState": "two", "request": { "urlPath": "/api/counter" }, "response": { "headers": { "Content-Type": "application/json" }, "jsonBody": { "n": 2 } } })).await;
    f.stub(json!({ "request": { "urlPath": "/api/text" }, "response": { "headers": { "Content-Type": "text/plain" }, "body": "hello text" } })).await;
    f.stub(json!({ "request": { "method": "POST", "urlPath": "/api/users", "bodyPatterns": [{ "equalToJson": { "name": "Bob" } }] }, "response": { "status": 201, "headers": { "Content-Type": "application/json" }, "jsonBody": { "id": 2, "name": "Bob" } } })).await;
    f
}

#[tokio::test]
async fn proxies_with_header_rules() {
    let up = upstream().await;
    let f = start().await;
    f.stub(json!({
        "request": { "urlPathPattern": "/proxy/.*" },
        "response": {
            "proxyBaseUrl": up.handle.url(),
            "proxyUrlPrefixToRemove": "/proxy",
            "additionalProxyRequestHeaders": { "X-Added": "yes" },
            "removeProxyRequestHeaders": ["X-Secret"],
            "headers": { "X-Proxied-By": "devall" }
        }
    }))
    .await;
    let r = f.send(f.http.post(f.url("/proxy/echo/items?q=a%20b")).header("X-Secret", "s3cr3t").body("payload")).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.header("x-proxied-by").as_deref(), Some("devall"));
    assert_eq!(r.json(), json!({ "path": "/echo/items", "query": "a b", "added": "yes", "secret": "", "method": "POST", "body": "payload" }));
    let ev = f.admin("GET", "/requests?limit=1", None).await.json();
    assert_eq!(ev["requests"][0]["responseDefinition"]["proxyBaseUrl"], up.handle.url());
    assert_eq!(ev["requests"][0]["response"]["status"], 200);

    // templated proxy target
    f.stub(json!({ "request": { "urlPath": "/dyn" }, "response": { "proxyBaseUrl": "{{request.headers.X-Target}}", "transformers": ["response-template"] } })).await;
    let r = f.send(f.http.get(f.url("/dyn")).header("X-Target", format!("{}/api/users", up.handle.url()))).await;
    assert_eq!(r.status, 404, "upstream sees /api/users/dyn: {}", r.text);

    // dead upstream
    up.handle.stop().await;
    let r = f.get("/proxy/echo/x").await;
    assert_eq!(r.status, 500);
    assert!(r.text.contains("Network failure"), "{}", r.text);
}

fn by_url<'a>(mappings: &'a [Value], url: &str) -> Vec<&'a Value> {
    mappings.iter().filter(|m| m["request"]["url"] == url).collect()
}

#[tokio::test]
async fn record_then_replay_without_upstream() {
    let up = upstream().await;
    let f = start().await;
    assert_eq!(f.admin("GET", "/recordings/status", None).await.json()["status"], "NeverStarted");
    let r = f.admin("POST", "/recordings/start", Some(json!({ "targetBaseUrl": up.handle.url(), "captureHeaders": { "Accept": {} } }))).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(f.admin("GET", "/recordings/status", None).await.json()["status"], "Recording");
    assert_eq!(f.admin("POST", "/recordings/start", Some(json!({ "targetBaseUrl": up.handle.url() }))).await.status, 400);

    assert_eq!(f.send(f.http.get(f.url("/api/users")).header("Accept", "application/json")).await.json(), json!([{ "id": 1, "name": "Ann" }]));
    assert_eq!(f.get("/api/counter").await.json(), json!({ "n": 1 }));
    assert_eq!(f.get("/api/counter").await.json(), json!({ "n": 2 }));
    assert_eq!(f.get("/api/text").await.text, "hello text");
    assert_eq!(f.send(f.http.post(f.url("/api/users")).header("Content-Type", "application/json").body(r#"{"name":"Bob"}"#)).await.status, 201);

    let stopped = f.admin("POST", "/recordings/stop", None).await;
    assert_eq!(stopped.status, 200, "{}", stopped.text);
    let mappings = stopped.json()["mappings"].as_array().unwrap().clone();
    assert_eq!(mappings.len(), 5, "{mappings:#?}");
    let users = by_url(&mappings, "/api/users").into_iter().find(|m| m["request"]["method"] == "GET").unwrap();
    assert_eq!(users["response"]["jsonBody"], json!([{ "id": 1, "name": "Ann" }]), "JSON is inlined, not a bodyFileName");
    assert!(users["response"].get("bodyFileName").is_none());
    assert_eq!(users["response"]["headers"]["X-Upstream"], "1");
    assert!(users["response"]["headers"].get("Content-Length").is_none());
    assert_eq!(users["request"]["headers"]["Accept"]["equalTo"], "application/json");
    assert_eq!(users["persistent"], true);
    let post = by_url(&mappings, "/api/users").into_iter().find(|m| m["request"]["method"] == "POST").unwrap();
    assert_eq!(post["request"]["bodyPatterns"][0]["equalToJson"], r#"{"name":"Bob"}"#);
    assert_eq!(post["response"]["status"], 201);
    let counters = by_url(&mappings, "/api/counter");
    assert_eq!(counters.len(), 2);
    assert_eq!(counters[0]["scenarioName"], counters[1]["scenarioName"]);
    assert_eq!(counters[0]["requiredScenarioState"], "Started");
    assert_eq!(counters[0]["newScenarioState"], counters[1]["requiredScenarioState"]);
    assert!(counters[1].get("newScenarioState").is_none());
    assert_eq!(by_url(&mappings, "/api/text")[0]["response"]["body"], "hello text");

    assert_eq!(f.admin("GET", "/recordings/status", None).await.json()["status"], "Stopped");
    let all = f.admin("GET", "/mappings", None).await.json();
    assert!(all["mappings"].as_array().unwrap().iter().all(|m| m["response"].get("proxyBaseUrl").is_none()), "recording proxy removed");
    assert_eq!(f.mapping_files().len(), 5, "persisted by default");

    up.handle.stop().await;
    assert_eq!(f.get("/api/users").await.status, 404, "Accept header was captured");
    assert_eq!(f.send(f.http.get(f.url("/api/users")).header("Accept", "application/json")).await.json(), json!([{ "id": 1, "name": "Ann" }]));
    assert_eq!(f.get("/api/counter").await.json(), json!({ "n": 1 }));
    assert_eq!(f.get("/api/counter").await.json(), json!({ "n": 2 }));
    f.admin("POST", "/mappings/reset", None).await;
    assert_eq!(f.handle.stub_count(), 5, "recorded stubs reload from the project");
    assert_eq!(f.admin("POST", "/recordings/stop", None).await.status, 400);
}

#[tokio::test]
async fn snapshot_options() {
    let up = upstream().await;
    let f = start().await;
    f.stub(json!({ "request": { "urlPathPattern": "/api/.*" }, "response": { "proxyBaseUrl": up.handle.url() } })).await;
    f.stub(json!({ "request": { "urlPath": "/local" }, "response": { "body": "not proxied" } })).await;
    f.get("/api/users").await;
    f.get("/api/users").await;
    f.get("/api/text").await;
    f.get("/local").await;

    let snap = f.admin("POST", "/recordings/snapshot", Some(json!({ "persist": false, "repeatsAsScenarios": false, "filters": { "urlPathPattern": "/api/users.*" } }))).await;
    assert_eq!(snap.status, 200, "{}", snap.text);
    let m = snap.json()["mappings"].as_array().unwrap().clone();
    assert_eq!(m.len(), 1, "duplicates collapse without repeatsAsScenarios");
    assert_eq!(m[0]["response"]["jsonBody"][0]["name"], "Ann");
    assert!(m[0].get("persistent").is_none());
    assert!(f.mapping_files().is_empty());

    let ids = f.admin("POST", "/recordings/snapshot", Some(json!({ "persist": false, "outputFormat": "IDS", "filters": { "allowNonProxied": true, "urlPath": "/local" } }))).await.json();
    assert_eq!(ids["ids"].as_array().unwrap().len(), 1);

    // large text bodies go to __files when persisting
    let big = f.admin("POST", "/recordings/snapshot", Some(json!({ "filters": { "urlPath": "/api/text" }, "extractBodyCriteria": { "textSizeThreshold": "4" } }))).await.json();
    let file = big["mappings"][0]["response"]["bodyFileName"].as_str().unwrap().to_string();
    assert_eq!(std::fs::read_to_string(f.root().join("mocks/__files").join(&file)).unwrap(), "hello text");
    assert_eq!(f.mapping_files().len(), 1);

    let bad = f.admin("POST", "/recordings/snapshot", Some(json!({ "filters": { "urlPattern": "(" } }))).await;
    assert_eq!(bad.status, 400);
}

#[tokio::test]
async fn proxy_trust_and_bind_options_start() {
    let f = start_with(|_| {}, |c| {
        c.proxy_trust_all_certs = true;
        c.bind_all = true;
    })
    .await;
    assert!(f.handle.local_addr().ip().is_unspecified());
    assert_eq!(f.get("/__admin/health").await.status, 200);
}
