//! Request journal and near-miss endpoints.

mod common;

use common::{start, start_with};
use serde_json::json;

#[tokio::test]
async fn journal_shapes_and_queries() {
    let f = start().await;
    let stub = f.stub(json!({ "name": "Users", "request": { "method": "GET", "urlPath": "/api/users" }, "response": { "status": 200, "body": "[]", "headers": { "Content-Type": "application/json" } } })).await;
    let stub_id = stub["id"].as_str().unwrap().to_string();
    f.send(f.http.get(f.url("/api/users?page=1&page=2")).header("Cookie", "a=1; b=2").header("X-Trace", "t1")).await;
    f.send(f.http.post(f.url("/api/orders")).body("order-body")).await;
    f.get("/api/users").await;

    let list = f.admin("GET", "/requests", None).await.json();
    assert_eq!(list["meta"]["total"], 3);
    assert_eq!(list["requestJournalDisabled"], false);
    let reqs = list["requests"].as_array().unwrap();
    assert_eq!(reqs[0]["request"]["url"], "/api/users", "newest first");
    assert_eq!(reqs[1]["request"]["url"], "/api/orders");

    let first = &reqs[2];
    let r = &first["request"];
    assert_eq!(r["method"], "GET");
    assert_eq!(r["url"], "/api/users?page=1&page=2");
    assert_eq!(r["absoluteUrl"], format!("{}/api/users?page=1&page=2", f.handle.url()));
    assert_eq!(r["clientIp"], "127.0.0.1");
    assert_eq!(r["headers"]["X-Trace"], "t1");
    assert_eq!(r["cookies"]["b"], "2");
    assert_eq!(r["queryParams"]["page"]["values"], json!(["1", "2"]));
    assert!(r["loggedDate"].as_i64().unwrap() > 1_600_000_000_000);
    assert!(r["loggedDateString"].as_str().unwrap().ends_with('Z'));
    assert_eq!(first["wasMatched"], true);
    assert_eq!(first["stubMapping"]["id"], stub_id.as_str());
    assert_eq!(first["responseDefinition"]["fromConfiguredStub"], true);
    assert_eq!(first["response"]["status"], 200);
    assert_eq!(first["response"]["body"], "[]");
    assert_eq!(first["response"]["headers"]["Content-Type"], "application/json");
    assert!(first["timing"]["totalTime"].is_number());

    let orders = &reqs[1];
    assert_eq!(orders["wasMatched"], false);
    assert_eq!(orders["request"]["body"], "order-body");
    assert_eq!(orders["request"]["bodyAsBase64"], "b3JkZXItYm9keQ==");
    assert_eq!(orders["response"]["status"], 404);
    assert_eq!(orders["responseDefinition"]["fromConfiguredStub"], false);

    let id = first["id"].as_str().unwrap();
    assert_eq!(f.admin("GET", &format!("/requests/{id}"), None).await.json()["id"], id);
    assert_eq!(f.admin("GET", "/requests?limit=1", None).await.json()["requests"].as_array().unwrap().len(), 1);
    assert_eq!(f.admin("GET", "/requests?unmatched=true", None).await.json()["requests"].as_array().unwrap().len(), 1);
    assert_eq!(f.admin("GET", &format!("/requests?matchingStub={stub_id}"), None).await.json()["requests"].as_array().unwrap().len(), 2);
    let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let since = f.admin("GET", &format!("/requests?since={}", urlencode(&future)), None).await.json();
    assert_eq!(since["requests"].as_array().unwrap().len(), 0);

    assert_eq!(f.admin("POST", "/requests/count", Some(json!({ "method": "GET", "urlPathPattern": "/api/.*" }))).await.json()["count"], 2);
    let found = f.admin("POST", "/requests/find", Some(json!({ "method": "POST" }))).await.json();
    assert_eq!(found["requests"][0]["url"], "/api/orders");
    let unmatched = f.admin("GET", "/requests/unmatched", None).await.json();
    assert_eq!(unmatched["requests"].as_array().unwrap().len(), 1);
    assert_eq!(unmatched["requests"][0]["method"], "POST");

    assert_eq!(f.admin("DELETE", &format!("/requests/{id}"), None).await.status, 200);
    assert_eq!(f.admin("GET", &format!("/requests/{id}"), None).await.status, 404);
    let removed = f.admin("POST", "/requests/remove", Some(json!({ "method": "POST" }))).await.json();
    assert_eq!(removed["requests"].as_array().unwrap().len(), 1);
    assert_eq!(f.handle.journal_len(), 1);
    assert_eq!(f.admin("DELETE", "/requests", None).await.status, 200);
    assert_eq!(f.admin("GET", "/requests", None).await.json()["meta"]["total"], 0);
    f.get("/api/users").await;
    f.admin("POST", "/requests/reset", None).await;
    assert_eq!(f.handle.journal_len(), 0);
}

fn urlencode(s: &str) -> String {
    s.replace('+', "%2B").replace(':', "%3A")
}

#[tokio::test]
async fn journal_is_a_ring_buffer() {
    let f = start_with(|_| {}, |c| c.journal_size = 3).await;
    for i in 0..5 {
        f.get(&format!("/r/{i}")).await;
    }
    let list = f.admin("GET", "/requests", None).await.json();
    assert_eq!(list["meta"]["total"], 3);
    let urls: Vec<&str> = list["requests"].as_array().unwrap().iter().map(|e| e["request"]["url"].as_str().unwrap()).collect();
    assert_eq!(urls, ["/r/4", "/r/3", "/r/2"]);
}

#[tokio::test]
async fn near_misses() {
    let f = start().await;
    f.stub(json!({ "name": "users", "request": { "method": "GET", "urlPath": "/api/users", "headers": { "Accept": { "equalTo": "application/json" } } } })).await;
    f.stub(json!({ "name": "orders", "request": { "method": "POST", "urlPath": "/api/orders" } })).await;
    f.stub(json!({ "name": "health", "request": { "method": "DELETE", "urlPath": "/totally/unrelated/endpoint" } })).await;
    f.stub(json!({ "name": "extra", "request": { "method": "PUT", "urlPath": "/zzz" } })).await;
    f.send(f.http.get(f.url("/api/user")).header("Accept", "application/json")).await;

    let nm = f.admin("GET", "/requests/unmatched/near-misses", None).await.json();
    let list = nm["nearMisses"].as_array().unwrap();
    assert_eq!(list.len(), 3, "top three per unmatched request");
    assert_eq!(list[0]["stubMapping"]["name"], "users");
    assert_eq!(list[0]["request"]["url"], "/api/user");
    let d0 = list[0]["matchResult"]["distance"].as_f64().unwrap();
    let d1 = list[1]["matchResult"]["distance"].as_f64().unwrap();
    assert!(d0 > 0.0 && d0 < d1, "{d0} vs {d1}");
    let fields = list[0]["matchResult"]["fields"].as_array().unwrap();
    let url_field = fields.iter().find(|x| x["field"] == "URL path").unwrap();
    assert_eq!(url_field["matched"], false);
    assert_eq!(url_field["expected"], "/api/users");
    assert_eq!(url_field["actual"], "/api/user");
    assert!(fields.iter().any(|x| x["field"] == "Header Accept" && x["matched"] == true));

    // near misses for an arbitrary logged request
    let nm = f.admin("POST", "/near-misses/request", Some(json!({ "url": "/api/orders", "method": "GET" }))).await.json();
    assert_eq!(nm["nearMisses"][0]["stubMapping"]["name"], "orders");
    assert!(nm["nearMisses"][0]["matchResult"]["fields"].as_array().unwrap().iter().any(|x| x["field"] == "Method" && x["matched"] == false));

    // near misses for a pattern against the journal
    f.get("/api/orders").await;
    let nm = f.admin("POST", "/near-misses/request-pattern", Some(json!({ "method": "POST", "urlPath": "/api/orders" }))).await.json();
    let list = nm["nearMisses"].as_array().unwrap();
    assert_eq!(list[0]["request"]["url"], "/api/orders");
    assert_eq!(list[0]["requestPattern"]["urlPath"], "/api/orders");
    assert!(list[0]["matchResult"]["distance"].as_f64().unwrap() > 0.0);
    assert_eq!(f.admin("POST", "/near-misses/request-pattern", Some(json!({ "urlPattern": "(" }))).await.status, 422);
}

#[tokio::test]
async fn scenario_state_shows_up_in_near_misses_and_reset_restores() {
    let f = start().await;
    f.stub(json!({ "scenarioName": "s", "requiredScenarioState": "later", "request": { "urlPath": "/gated" } })).await;
    assert_eq!(f.get("/gated").await.status, 404);
    let nm = f.admin("GET", "/requests/unmatched/near-misses", None).await.json();
    let fields = nm["nearMisses"][0]["matchResult"]["fields"].as_array().unwrap().clone();
    let sc = fields.iter().find(|x| x["field"] == "Scenario s").expect("scenario field");
    assert_eq!(sc["expected"], "later");
    assert_eq!(sc["actual"], "Started");

    f.admin("PUT", "/scenarios/s/state", Some(json!({ "state": "later" }))).await;
    assert_eq!(f.get("/gated").await.status, 200);
    f.stub(json!({ "request": { "urlPath": "/temp" } })).await;
    assert_eq!(f.admin("POST", "/reset", None).await.status, 200);
    assert_eq!(f.handle.journal_len(), 0);
    assert_eq!(f.handle.stub_count(), 0, "reset drops non-persistent stubs");
}

#[tokio::test]
async fn explain_request_finds_the_stub_or_the_near_misses() {
    let f = start().await;
    f.stub(json!({ "name": "Users", "request": { "method": "GET", "urlPath": "/api/users" }, "response": { "status": 200 } })).await;
    f.stub(json!({ "name": "Create order", "request": { "method": "POST", "urlPath": "/api/orders", "bodyPatterns": [{ "matchesJsonPath": "$.sku" }] }, "response": { "status": 201 } })).await;
    f.get("/api/users?page=2").await;
    f.send(f.http.post(f.url("/api/orders")).body("{}")).await;

    let hit = f.handle.explain_request("get", "/api/users?page=2");
    assert!(hit.matched && hit.event.is_some());
    assert_eq!(hit.stub_name.as_deref(), Some("Users"));
    assert!(hit.near_misses.is_empty());

    let miss = f.handle.explain_request("POST", "/api/orders");
    assert!(!miss.matched && miss.stub.is_none());
    assert_eq!(miss.near_misses[0]["stubMapping"]["name"], "Create order", "{:?}", miss.near_misses);

    // Not in the journal: what would have matched.
    let never = f.handle.explain_request("DELETE", "/api/users");
    assert!(never.event.is_none() && !never.near_misses.is_empty());
}
