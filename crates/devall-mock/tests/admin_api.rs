//! Admin API round trips (WireMock JSON shapes), persistence into the project, CORS and lifecycle.

mod common;

use common::{start, start_with, write_mapping};
use serde_json::json;

#[tokio::test]
async fn create_call_update_delete_stub() {
    let f = start().await;
    let created = f
        .stub(json!({
            "name": "Hello",
            "request": { "method": "GET", "urlPath": "/hello" },
            "response": { "status": 200, "body": "hi", "headers": { "Content-Type": "text/plain" } }
        }))
        .await;
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["uuid"], created["id"], "3.x uuid alias");
    assert!(created["insertionIndex"].is_number());

    let r = f.get("/hello").await;
    assert_eq!((r.status, r.text.as_str()), (200, "hi"));
    assert_eq!(r.header("content-type").as_deref(), Some("text/plain"));

    let all = f.admin("GET", "/mappings", None).await.json();
    assert_eq!(all["meta"]["total"], 1);
    assert_eq!(all["mappings"][0]["name"], "Hello");
    let one = f.admin("GET", &format!("/mappings/{id}"), None).await;
    assert_eq!(one.status, 200);
    assert_eq!(one.json()["request"]["urlPath"], "/hello");

    let updated = f
        .admin("PUT", &format!("/mappings/{id}"), Some(json!({ "request": { "method": "GET", "urlPath": "/hello" }, "response": { "body": "updated" } })))
        .await;
    assert_eq!(updated.status, 200, "{}", updated.text);
    assert_eq!(updated.json()["id"], id.as_str());
    assert_eq!(f.get("/hello").await.text, "updated");

    assert_eq!(f.admin("DELETE", &format!("/mappings/{id}"), None).await.status, 200);
    assert_eq!(f.get("/hello").await.status, 404);
    assert_eq!(f.admin("GET", &format!("/mappings/{id}"), None).await.status, 404);
    assert_eq!(f.admin("DELETE", &format!("/mappings/{id}"), None).await.status, 404);
    assert_eq!(f.admin("PUT", "/mappings/0b6fb5e1-0000-4000-8000-000000000000", Some(json!({"request": {}}))).await.status, 404);
}

#[tokio::test]
async fn invalid_stubs_are_rejected_with_wiremock_errors() {
    let f = start().await;
    let r = f.admin("POST", "/mappings", Some(json!({ "request": { "urlPattern": "(" } }))).await;
    assert_eq!(r.status, 422);
    let e = r.json();
    assert!(e["errors"][0]["detail"].as_str().unwrap().contains("regex"));
    let r = f.send(f.http.post(f.url("/__admin/mappings")).body("{not json")).await;
    assert_eq!(r.status, 400);
    assert!(r.json()["errors"][0]["title"].is_string());
}

#[tokio::test]
async fn pagination_import_metadata_and_unmatched() {
    let f = start().await;
    let r = f
        .admin(
            "POST",
            "/mappings/import",
            Some(json!({ "mappings": [
                { "id": "11111111-1111-4111-8111-111111111111", "request": { "urlPath": "/a" }, "response": { "body": "a" }, "metadata": { "team": "blue", "n": 1 } },
                { "id": "22222222-2222-4222-8222-222222222222", "request": { "urlPath": "/b" }, "response": { "body": "b" }, "metadata": { "team": "red" } },
                { "id": "33333333-3333-4333-8333-333333333333", "request": { "urlPath": "/c" }, "response": { "body": "c" } }
            ] })),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text);
    let page = f.admin("GET", "/mappings?limit=2&offset=1", None).await.json();
    assert_eq!(page["meta"]["total"], 3);
    assert_eq!(page["mappings"].as_array().unwrap().len(), 2);

    // IGNORE keeps the existing stub
    f.admin(
        "POST",
        "/mappings/import",
        Some(json!({ "mappings": [{ "id": "11111111-1111-4111-8111-111111111111", "request": { "urlPath": "/a" }, "response": { "body": "changed" } }], "importOptions": { "duplicatePolicy": "IGNORE" } })),
    )
    .await;
    assert_eq!(f.get("/a").await.text, "a");

    let found = f.admin("POST", "/mappings/find-by-metadata", Some(json!({ "matchesJsonPath": "$[?(@.team == 'blue')]" }))).await.json();
    assert_eq!(found["mappings"].as_array().unwrap().len(), 1);
    assert_eq!(found["mappings"][0]["id"], "11111111-1111-4111-8111-111111111111");
    let found = f.admin("POST", "/mappings/find-by-metadata", Some(json!({ "matchesJsonPath": { "expression": "$.team", "equalTo": "red" } }))).await.json();
    assert_eq!(found["mappings"][0]["id"], "22222222-2222-4222-8222-222222222222");

    // unmatched stubs = never served
    f.get("/a").await;
    let unmatched = f.admin("GET", "/mappings/unmatched", None).await.json();
    assert_eq!(unmatched["meta"]["total"], 2);

    f.admin("POST", "/mappings/remove-by-metadata", Some(json!({ "matchesJsonPath": "$.team" }))).await;
    assert_eq!(f.handle.stub_count(), 1);
    f.admin("DELETE", "/mappings/unmatched", None).await;
    assert_eq!(f.handle.stub_count(), 0);

    // deleteAllNotInImport
    f.stub(json!({ "request": { "urlPath": "/x" } })).await;
    f.admin("POST", "/mappings/import", Some(json!({ "mappings": [{ "request": { "urlPath": "/y" } }], "importOptions": { "deleteAllNotInImport": true } }))).await;
    assert_eq!(f.handle.stub_count(), 1);
    assert_eq!(f.get("/x").await.status, 404);
}

#[tokio::test]
async fn save_writes_project_files_and_reset_reloads_them() {
    let f = start().await;
    f.stub(json!({ "name": "List users", "request": { "method": "GET", "urlPath": "/users" }, "response": { "jsonBody": [1, 2] } })).await;
    f.stub(json!({ "request": { "method": "POST", "urlPath": "/orders/new" }, "response": { "status": 201 } })).await;
    assert!(f.mapping_files().is_empty(), "nothing is written before save");

    assert_eq!(f.admin("POST", "/mappings/save", None).await.status, 200);
    let files = f.mapping_files();
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files.iter().any(|n| n.starts_with("list-users-")), "{files:?}");
    assert!(files.iter().any(|n| n.starts_with("post-orders-new-")), "{files:?}");
    let text = std::fs::read_to_string(f.root().join("mocks/mappings").join(files.iter().find(|n| n.starts_with("list")).unwrap())).unwrap();
    let keys: Vec<String> = serde_json::from_str::<serde_json::Value>(&text).unwrap().as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["id", "name", "request", "response", "persistent"]);
    assert!(text.ends_with("}\n") && text.contains("\n  \"name\": \"List users\""));

    let all = f.admin("GET", "/mappings", None).await.json();
    assert!(all["mappings"].as_array().unwrap().iter().all(|m| m["persistent"] == true));

    // a non-persistent stub disappears on reset, saved ones come back from disk
    f.stub(json!({ "request": { "urlPath": "/temp" } })).await;
    assert_eq!(f.handle.stub_count(), 3);
    assert_eq!(f.admin("POST", "/mappings/reset", None).await.status, 200);
    assert_eq!(f.handle.stub_count(), 2);
    assert_eq!(f.get("/users").await.json(), json!([1, 2]));

    // editing a persistent stub rewrites its file; deleting removes it
    let users = all["mappings"].as_array().unwrap().iter().find(|m| m["name"] == "List users").unwrap().clone();
    let id = users["id"].as_str().unwrap();
    f.admin("PUT", &format!("/mappings/{id}"), Some(json!({ "name": "List users", "request": { "method": "GET", "urlPath": "/users" }, "response": { "jsonBody": [3] } }))).await;
    assert_eq!(f.mapping_files().len(), 2);
    f.admin("POST", "/mappings/reset", None).await;
    assert_eq!(f.get("/users").await.json(), json!([3]));
    f.admin("DELETE", &format!("/mappings/{id}"), None).await;
    assert_eq!(f.mapping_files().len(), 1);

    // persistent: true on create writes immediately
    f.stub(json!({ "persistent": true, "request": { "urlPath": "/now" } })).await;
    assert_eq!(f.mapping_files().len(), 2);

    // DELETE /mappings removes everything, files included
    f.admin("DELETE", "/mappings", None).await;
    assert_eq!(f.handle.stub_count(), 0);
    assert!(f.mapping_files().is_empty());
}

#[tokio::test]
async fn project_files_load_with_per_file_errors() {
    let f = start_with(
        |root| {
            write_mapping(root, "ok.json", &json!({ "request": { "urlPath": "/ok" }, "response": { "body": "fine" } }));
            write_mapping(root, "many.json", &json!({ "mappings": [{ "request": { "urlPath": "/m1" } }, { "request": { "urlPath": "/m2" }, "response": { "status": 202 } }] }));
            std::fs::write(root.join("mocks/mappings/broken.json"), "{ \"request\": ").unwrap();
            write_mapping(root, "invalid.json", &json!({ "request": { "method": 5 } }));
        },
        |_| {},
    )
    .await;
    assert_eq!(f.handle.stub_count(), 3);
    let errors = f.handle.load_errors();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors.iter().any(|e| e.file == "mocks/mappings/broken.json" && e.error.contains("invalid JSON")));
    assert!(errors.iter().any(|e| e.file == "mocks/mappings/invalid.json"));
    assert_eq!(f.get("/ok").await.text, "fine");
    assert_eq!(f.get("/m2").await.status, 202);
    let all = f.admin("GET", "/mappings", None).await.json();
    assert!(all["mappings"].as_array().unwrap().iter().all(|m| m["persistent"] == true));
    let info = f.admin("GET", "/devall/info", None).await.json();
    assert_eq!(info["loadErrors"].as_array().unwrap().len(), 2);

    // editing a stub that lives in a multi-mapping file rewrites just that entry
    let m1 = all["mappings"].as_array().unwrap().iter().find(|m| m["request"]["urlPath"] == "/m1").unwrap().clone();
    let id = m1["id"].as_str().unwrap();
    f.admin("PUT", &format!("/mappings/{id}"), Some(json!({ "request": { "urlPath": "/m1" }, "response": { "body": "edited" } }))).await;
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f.root().join("mocks/mappings/many.json")).unwrap()).unwrap();
    assert_eq!(doc["mappings"].as_array().unwrap().len(), 2);
    assert_eq!(doc["mappings"][0]["response"]["body"], "edited");
    assert_eq!(doc["mappings"][0]["id"], id);
    f.admin("POST", "/mappings/reset", None).await;
    assert_eq!(f.get("/m1").await.text, "edited");
}

#[tokio::test]
async fn files_api() {
    let f = start().await;
    let r = f.send(f.http.put(f.url("/__admin/files/users/1.json")).body("{\"id\":1}")).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(f.admin("GET", "/files", None).await.json(), json!(["users/1.json"]));
    let r = f.get("/__admin/files/users/1.json").await;
    assert_eq!(r.text, "{\"id\":1}");
    assert_eq!(r.header("content-type").as_deref(), Some("application/json"));
    assert_eq!(f.get("/__admin/files/..%2Fsecret").await.status, 404);
    assert_eq!(f.send(f.http.put(f.url("/__admin/files/..%2Fevil.txt")).body("x")).await.status, 400);
    assert!(!f.root().join("mocks/evil.txt").exists());
    assert_eq!(f.admin("DELETE", "/files/users/1.json", None).await.status, 200);
    assert_eq!(f.admin("DELETE", "/files/users/1.json", None).await.status, 404);
}

#[tokio::test]
async fn health_version_settings_and_unknown_routes() {
    let f = start().await;
    let h = f.admin("GET", "/health", None).await.json();
    assert_eq!(h["status"], "healthy");
    assert_eq!(h["version"], "3.13.0-devall");
    assert_eq!(f.admin("GET", "/version", None).await.json()["version"], "3.13.0-devall");
    assert_eq!(f.admin("GET", "/settings", None).await.json(), json!({ "settings": {} }));
    let r = f.admin("POST", "/settings", Some(json!({ "fixedDelay": 5, "extended": { "a": 1 } }))).await;
    assert_eq!(r.status, 200);
    let r = f.admin("PATCH", "/settings", Some(json!({ "extended": { "b": 2 } }))).await;
    assert_eq!(r.json()["settings"], json!({ "fixedDelay": 5, "extended": { "a": 1, "b": 2 } }));
    let r = f.admin("POST", "/settings", Some(json!({ "delayDistribution": { "type": "nope" } }))).await;
    assert_eq!(r.status, 422);
    f.admin("POST", "/settings", Some(json!({}))).await;
    assert_eq!(f.admin("GET", "/settings", None).await.json(), json!({ "settings": {} }));
    assert_eq!(f.admin("GET", "/nope", None).await.status, 404);
}

#[tokio::test]
async fn cors_and_origin_guard() {
    let f = start().await;
    let r = f
        .send(
            f.http
                .request(reqwest::Method::OPTIONS, f.url("/__admin/mappings"))
                .header("Origin", "http://localhost:5173")
                .header("Access-Control-Request-Method", "POST")
                .header("Access-Control-Request-Headers", "authorization, content-type"),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("access-control-allow-origin").as_deref(), Some("http://localhost:5173"));
    assert_eq!(r.header("access-control-allow-headers").as_deref(), Some("authorization, content-type"));
    assert!(r.header("access-control-allow-methods").unwrap().contains("DELETE"));

    let r = f.send(f.http.get(f.url("/__admin/mappings")).header("Origin", "tauri://localhost")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("access-control-allow-origin").as_deref(), Some("tauri://localhost"));

    let r = f.send(f.http.post(f.url("/__admin/shutdown")).header("Origin", "https://evil.example").body("{}")).await;
    assert_eq!(r.status, 403);
    assert!(f.handle.is_running());

    let r = f.send(f.http.get(f.url("/__admin/mappings")).header("Host", "attacker.example:80")).await;
    assert_eq!(r.status, 403, "DNS-rebinding guard");

    let g = start_with(|_| {}, |c| c.admin_allowed_origins = vec!["https://ui.example".into()]).await;
    let r = g.send(g.http.get(g.url("/__admin/health")).header("Origin", "https://ui.example")).await;
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn shutdown_stops_only_that_server() {
    let a = start().await;
    let b = start().await;
    assert_ne!(a.handle.port(), b.handle.port());
    let mut events = a.handle.subscribe();
    assert_eq!(a.admin("POST", "/shutdown", None).await.status, 200);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(devall_mock::JournalEvent::Stopped) = events.recv().await {
                break;
            }
        }
    })
    .await
    .expect("stopped event");
    assert!(!a.handle.is_running());
    assert!(a.http.get(a.url("/__admin/health")).send().await.is_err());
    assert!(b.handle.is_running());
    assert_eq!(b.admin("GET", "/health", None).await.status, 200);
    a.handle.stop().await;
    b.handle.stop().await;
    b.handle.stop().await;
    assert!(!b.handle.is_running());
}

#[tokio::test]
async fn fixed_port_and_rebind_after_stop() {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let f = start_with(|_| {}, |c| c.port = port).await;
    assert_eq!(f.handle.port(), port);
    assert_eq!(f.handle.url(), format!("http://127.0.0.1:{port}"));
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = devall_mock::MockConfig::new(dir.path());
    cfg.port = port;
    assert!(devall_mock::MockServer::start(cfg.clone()).await.is_err(), "port in use");
    f.handle.stop().await;
    let again = devall_mock::MockServer::start(cfg).await.expect("port is free after stop");
    again.stop().await;
}

#[tokio::test]
async fn stop_right_after_start() {
    let dir = tempfile::tempdir().unwrap();
    let h = devall_mock::MockServer::start(devall_mock::MockConfig::new(dir.path())).await.unwrap();
    assert!(h.is_running());
    tokio::time::timeout(std::time::Duration::from_secs(5), h.stop()).await.expect("stop must not hang");
    assert!(!h.is_running());
}
