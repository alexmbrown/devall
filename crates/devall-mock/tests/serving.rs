//! Serving stubs over real HTTP: matching, priorities, scenarios, templating, body files,
//! delays, dribbling, faults, chaos and CORS.

mod common;

use std::time::{Duration, Instant};

use common::{Fixture, start, start_with, write_body_file, write_mapping};
use devall_mock::{ChaosConfig, Fault, JournalEvent};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn raw(f: &Fixture, request: &str) -> std::io::Result<Vec<u8>> {
    let mut s = tokio::net::TcpStream::connect(f.handle.local_addr()).await?;
    s.write_all(request.as_bytes()).await?;
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await.expect("read timed out")?;
    Ok(buf)
}

fn get_close(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
}

#[tokio::test]
async fn request_matching_over_http() {
    let f = start().await;
    f.stub(json!({
        "request": {
            "method": "POST",
            "urlPath": "/search",
            "queryParameters": { "page": { "matches": "\\d+" }, "debug": { "absent": true } },
            "headers": { "Accept": { "contains": "json" } },
            "cookies": { "session": { "equalTo": "s1" } },
            "bodyPatterns": [{ "matchesJsonPath": "$.terms[?(@ == 'rust')]" }, { "equalToJson": { "terms": ["wiremock", "rust"] }, "ignoreArrayOrder": true, "ignoreExtraElements": true }]
        },
        "response": { "status": 200, "body": "found" }
    }))
    .await;
    f.stub(json!({
        "request": { "method": "POST", "url": "/login", "basicAuthCredentials": { "username": "ann", "password": "pw" }, "formParameters": { "remember": { "equalTo": "yes" } } },
        "response": { "status": 204 }
    }))
    .await;
    let ok = f
        .send(
            f.http
                .post(f.url("/search?page=2"))
                .header("Accept", "application/json")
                .header("Cookie", "session=s1")
                .body(r#"{"terms":["rust","wiremock"],"limit":5}"#),
        )
        .await;
    assert_eq!((ok.status, ok.text.as_str()), (200, "found"));
    let wrong_cookie = f.send(f.http.post(f.url("/search?page=2")).header("Accept", "application/json").header("Cookie", "session=s2").body(r#"{"terms":["rust","wiremock"]}"#)).await;
    assert_eq!(wrong_cookie.status, 404);
    let debug = f.send(f.http.post(f.url("/search?page=2&debug=1")).header("Accept", "application/json").header("Cookie", "session=s1").body(r#"{"terms":["rust","wiremock"]}"#)).await;
    assert_eq!(debug.status, 404);

    let login = f
        .send(f.http.post(f.url("/login")).basic_auth("ann", Some("pw")).header("Content-Type", "application/x-www-form-urlencoded").body("user=ann&remember=yes"))
        .await;
    assert_eq!(login.status, 204);
    let bad = f.send(f.http.post(f.url("/login")).basic_auth("ann", Some("nope")).header("Content-Type", "application/x-www-form-urlencoded").body("remember=yes")).await;
    assert_eq!(bad.status, 404);
}

#[tokio::test]
async fn priorities_and_recency() {
    let f = start().await;
    f.stub(json!({ "priority": 10, "request": { "urlPattern": ".*" }, "response": { "status": 418, "body": "fallback" } })).await;
    f.stub(json!({ "request": { "method": "GET", "urlPath": "/thing" }, "response": { "body": "first" } })).await;
    f.stub(json!({ "request": { "method": "GET", "urlPath": "/thing" }, "response": { "body": "second (newest)" } })).await;
    f.stub(json!({ "priority": 1, "request": { "method": "GET", "urlPathPattern": "/thing.*" }, "response": { "body": "top" } })).await;
    assert_eq!(f.get("/thing").await.text, "top");
    assert_eq!(f.get("/things").await.text, "top");
    assert_eq!(f.get("/other").await.status, 418);
    let all = f.admin("GET", "/mappings", None).await.json();
    assert_eq!(all["mappings"][0]["priority"], 1, "admin listing is in match order");
}

#[tokio::test]
async fn scenarios_flow() {
    let f = start().await;
    f.stub(json!({ "scenarioName": "To do list", "requiredScenarioState": "Started", "request": { "method": "GET", "url": "/todo/items" }, "response": { "jsonBody": ["Buy milk"] } })).await;
    f.stub(json!({ "scenarioName": "To do list", "requiredScenarioState": "Started", "newScenarioState": "Cancel newspaper item added", "request": { "method": "POST", "url": "/todo/items", "bodyPatterns": [{ "contains": "Cancel newspaper" }] }, "response": { "status": 201 } })).await;
    f.stub(json!({ "scenarioName": "To do list", "requiredScenarioState": "Cancel newspaper item added", "request": { "method": "GET", "url": "/todo/items" }, "response": { "jsonBody": ["Buy milk", "Cancel newspaper subscription"] } })).await;

    assert_eq!(f.get("/todo/items").await.json(), json!(["Buy milk"]));
    assert_eq!(f.request("POST", "/todo/items", Some("Cancel newspaper subscription")).await.status, 201);
    assert_eq!(f.get("/todo/items").await.json(), json!(["Buy milk", "Cancel newspaper subscription"]));
    assert_eq!(f.request("POST", "/todo/items", Some("Cancel newspaper subscription")).await.status, 404, "POST only valid in Started");

    let sc = f.admin("GET", "/scenarios", None).await.json();
    assert_eq!(sc["scenarios"][0]["name"], "To do list");
    assert_eq!(sc["scenarios"][0]["state"], "Cancel newspaper item added");
    assert_eq!(sc["scenarios"][0]["possibleStates"], json!(["Started", "Cancel newspaper item added"]));
    assert_eq!(sc["scenarios"][0]["mappings"].as_array().unwrap().len(), 3);

    f.admin("POST", "/scenarios/reset", None).await;
    assert_eq!(f.get("/todo/items").await.json(), json!(["Buy milk"]));
    assert_eq!(f.admin("PUT", "/scenarios/To%20do%20list/state", Some(json!({ "state": "Cancel newspaper item added" }))).await.status, 200);
    assert_eq!(f.get("/todo/items").await.json().as_array().unwrap().len(), 2);
    assert_eq!(f.admin("PUT", "/scenarios/To%20do%20list/state", None).await.status, 200, "no body resets to Started");
    assert_eq!(f.get("/todo/items").await.json().as_array().unwrap().len(), 1);
    assert_eq!(f.admin("PUT", "/scenarios/nope/state", Some(json!({ "state": "x" }))).await.status, 404);
}

#[tokio::test]
async fn response_templating() {
    let f = start().await;
    f.stub(json!({
        "request": { "method": "POST", "urlPathTemplate": "/users/{id}", "pathParameters": { "id": { "matches": "\\d+" } } },
        "response": {
            "status": 200,
            "headers": { "Content-Type": "application/json", "X-User": "{{request.path.id}}" },
            "jsonBody": { "id": "{{request.path.id}}", "q": "{{request.query.q}}", "name": "{{jsonPath request.body '$.name'}}", "method": "{{request.method}}", "agent": "{{request.headers.User-Agent}}", "host": "{{parameters.host}}" },
            "transformers": ["response-template"],
            "transformerParameters": { "host": "example" }
        }
    }))
    .await;
    f.stub(json!({ "request": { "urlPath": "/raw" }, "response": { "body": "{{request.path}}" } })).await;
    let r = f.send(f.http.post(f.url("/users/42?q=find%20me")).header("User-Agent", "tester/1").body(r#"{"name":"Ann"}"#)).await;
    assert_eq!(r.status, 200, "{}", r.text);
    assert_eq!(r.header("x-user").as_deref(), Some("42"));
    assert_eq!(r.json(), json!({ "id": "42", "q": "find me", "name": "Ann", "method": "POST", "agent": "tester/1", "host": "example" }));
    assert_eq!(f.get("/raw").await.text, "{{request.path}}", "no templating without the transformer");
    assert_eq!(f.request("POST", "/users/abc", None).await.status, 404);

    // global templating
    let g = start_with(|root| write_mapping(root, "t.json", &json!({ "request": { "urlPath": "/now" }, "response": { "body": "{{now format='yyyy'}}|{{randomValue length=4 type='NUMERIC'}}|{{upper request.query.x}}" } })), |c| c.global_templating = true).await;
    let body = g.get("/now?x=abc").await.text;
    let parts: Vec<&str> = body.split('|').collect();
    assert_eq!(parts[0], chrono::Utc::now().format("%Y").to_string());
    assert_eq!(parts[1].len(), 4);
    assert_eq!(parts[2], "ABC");

    // template errors become 500s
    f.stub(json!({ "request": { "urlPath": "/broken" }, "response": { "body": "{{#if}}", "transformers": ["response-template"] } })).await;
    let r = f.get("/broken").await;
    assert_eq!(r.status, 500);
    assert!(r.text.contains("template error"));
}

#[tokio::test]
async fn body_sources() {
    let f = start_with(
        |root| {
            write_body_file(root, "user.json", br#"{"id":"{{request.pathSegments.[1]}}"}"#);
            write_body_file(root, "items/7.txt", b"seven");
            write_body_file(root, "pixel.bin", &[0, 159, 146, 150]);
            std::fs::write(root.join("secret.txt"), "top secret").unwrap();
        },
        |_| {},
    )
    .await;
    f.stub(json!({ "request": { "urlPathPattern": "/users/.*" }, "response": { "bodyFileName": "user.json", "transformers": ["response-template"], "headers": { "Content-Type": "application/json" } } })).await;
    f.stub(json!({ "request": { "urlPath": "/items" }, "response": { "bodyFileName": "items/{{request.query.n}}.txt", "transformers": ["response-template"] } })).await;
    f.stub(json!({ "request": { "urlPath": "/bin" }, "response": { "bodyFileName": "pixel.bin" } })).await;
    f.stub(json!({ "request": { "urlPath": "/b64" }, "response": { "base64Body": "AQIDBA==" } })).await;
    f.stub(json!({ "request": { "urlPath": "/escape" }, "response": { "bodyFileName": "../../secret.txt" } })).await;
    f.stub(json!({ "request": { "urlPath": "/missing" }, "response": { "bodyFileName": "nope.json" } })).await;
    f.stub(json!({ "request": { "urlPath": "/status" }, "response": { "status": 299, "statusMessage": "Totally Fine", "body": "x" } })).await;

    assert_eq!(f.get("/users/99").await.json(), json!({ "id": "99" }));
    assert_eq!(f.get("/items?n=7").await.text, "seven");
    let bin = f.http.get(f.url("/bin")).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(bin.as_ref(), &[0, 159, 146, 150]);
    let b64 = f.http.get(f.url("/b64")).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(b64.as_ref(), &[1, 2, 3, 4]);
    let esc = f.get("/escape").await;
    assert_eq!(esc.status, 500);
    assert!(!esc.text.contains("top secret"));
    assert_eq!(f.get("/missing").await.status, 500);
    let raw_resp = String::from_utf8(raw(&f, &get_close("/status")).await.unwrap()).unwrap();
    assert!(raw_resp.starts_with("HTTP/1.1 299 Totally Fine\r\n"), "{raw_resp}");
}

#[tokio::test]
async fn delays_and_dribble() {
    let f = start().await;
    f.stub(json!({ "request": { "urlPath": "/fixed" }, "response": { "fixedDelayMilliseconds": 300 } })).await;
    f.stub(json!({ "request": { "urlPath": "/uniform" }, "response": { "delayDistribution": { "type": "uniform", "lower": 150, "upper": 200 } } })).await;
    f.stub(json!({ "request": { "urlPath": "/lognormal" }, "response": { "delayDistribution": { "type": "lognormal", "median": 20, "sigma": 0.1, "maxValue": 50 } } })).await;
    f.stub(json!({ "request": { "urlPath": "/dribble" }, "response": { "body": "0123456789", "chunkedDribbleDelay": { "numberOfChunks": 5, "totalDuration": 400 } } })).await;
    f.stub(json!({ "request": { "urlPath": "/plain" }, "response": { "body": "quick" } })).await;

    let t = Instant::now();
    assert_eq!(f.get("/fixed").await.status, 200);
    assert!(t.elapsed() >= Duration::from_millis(300), "{:?}", t.elapsed());
    let t = Instant::now();
    f.get("/uniform").await;
    assert!(t.elapsed() >= Duration::from_millis(150));
    let t = Instant::now();
    f.get("/lognormal").await;
    assert!(t.elapsed() < Duration::from_millis(1000));
    let t = Instant::now();
    let r = f.get("/dribble").await;
    assert_eq!(r.text, "0123456789");
    assert!(t.elapsed() >= Duration::from_millis(350), "{:?}", t.elapsed());

    let ev = f.admin("GET", "/requests?limit=1", None).await.json();
    assert_eq!(ev["requests"][0]["request"]["url"], "/dribble");

    // global delay from settings applies to stubs without their own
    f.admin("POST", "/settings", Some(json!({ "fixedDelay": 250 }))).await;
    let t = Instant::now();
    assert_eq!(f.get("/plain").await.text, "quick");
    assert!(t.elapsed() >= Duration::from_millis(250));
    let timing = f.admin("GET", "/requests?limit=1", None).await.json();
    assert!(timing["requests"][0]["timing"]["addedDelay"].as_u64().unwrap() >= 250);
}

#[tokio::test]
async fn faults() {
    let f = start().await;
    for fault in ["EMPTY_RESPONSE", "CONNECTION_RESET_BY_PEER", "MALFORMED_RESPONSE_CHUNK", "RANDOM_DATA_THEN_CLOSE"] {
        f.stub(json!({ "request": { "urlPath": format!("/fault/{fault}") }, "response": { "fault": fault } })).await;
    }
    let empty = raw(&f, &get_close("/fault/EMPTY_RESPONSE")).await.unwrap();
    assert!(empty.is_empty(), "{empty:?}");

    match raw(&f, &get_close("/fault/CONNECTION_RESET_BY_PEER")).await {
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset, "{e}"),
        Ok(bytes) => assert!(bytes.is_empty(), "reset must not deliver data: {bytes:?}"),
    }

    let malformed = raw(&f, &get_close("/fault/MALFORMED_RESPONSE_CHUNK")).await.unwrap();
    assert!(String::from_utf8_lossy(&malformed).starts_with("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked"));
    let body = match f.http.get(f.url("/fault/MALFORMED_RESPONSE_CHUNK")).send().await {
        Ok(r) => r.bytes().await.is_err(),
        Err(_) => true,
    };
    assert!(body, "client must fail reading a malformed chunk");

    let random = raw(&f, &get_close("/fault/RANDOM_DATA_THEN_CLOSE")).await.unwrap();
    assert!(!random.is_empty());
    assert!(f.http.get(f.url("/fault/RANDOM_DATA_THEN_CLOSE")).send().await.is_err());
    assert!(f.http.get(f.url("/fault/EMPTY_RESPONSE")).send().await.is_err());

    // the journal records faults, and the server keeps serving
    let ev = f.admin("GET", "/requests?limit=1", None).await.json();
    assert_eq!(ev["requests"][0]["response"]["fault"], "EMPTY_RESPONSE");
    f.stub(json!({ "request": { "urlPath": "/fine" }, "response": { "body": "ok" } })).await;
    assert_eq!(f.get("/fine").await.text, "ok");
}

#[tokio::test]
async fn chaos_injection() {
    let f = start().await;
    f.stub(json!({ "request": { "urlPathPattern": "/api/.*" }, "response": { "body": "real" } })).await;
    assert_eq!(f.admin("GET", "/devall/chaos", None).await.json()["enabled"], false);

    let r = f.admin("PUT", "/devall/chaos", Some(json!({ "enabled": true, "errorRate": 1.0, "errorStatuses": [503] }))).await;
    assert_eq!(r.status, 200, "{}", r.text);
    let r = f.get("/api/x").await;
    assert_eq!(r.status, 503);
    assert_eq!(r.header("x-devall-chaos").as_deref(), Some("error"));
    let ev = f.admin("GET", "/requests?limit=1", None).await.json();
    assert_eq!(ev["requests"][0]["devallChaos"]["errorStatus"], 503);
    assert_eq!(ev["requests"][0]["wasMatched"], true);

    f.admin("PUT", "/devall/chaos", Some(json!({ "enabled": true, "latencyMs": 200, "jitterMs": 50 }))).await;
    let t = Instant::now();
    assert_eq!(f.get("/api/x").await.text, "real");
    assert!(t.elapsed() >= Duration::from_millis(200));

    f.admin("PUT", "/devall/chaos", Some(json!({ "enabled": true, "faultRate": 1.0, "faults": ["EMPTY_RESPONSE"], "urlPattern": "/api/flaky.*" }))).await;
    assert!(f.http.get(f.url("/api/flaky")).send().await.is_err());
    assert_eq!(f.get("/api/stable").await.text, "real", "urlPattern limits chaos");

    assert_eq!(f.admin("PUT", "/devall/chaos", Some(json!({ "errorRate": 2 }))).await.status, 422);
    f.handle.set_chaos(ChaosConfig::default()).unwrap();
    assert_eq!(f.get("/api/flaky").await.text, "real");
    let mut cfg = ChaosConfig { enabled: true, error_rate: 1.0, error_statuses: vec![500], ..Default::default() };
    f.handle.set_chaos(cfg.clone()).unwrap();
    assert_eq!(f.get("/api/x").await.status, 500);
    cfg.faults = vec![Fault::ConnectionResetByPeer];
    cfg.error_rate = 0.0;
    cfg.fault_rate = 1.0;
    f.handle.set_chaos(cfg).unwrap();
    assert!(f.http.get(f.url("/api/x")).send().await.is_err());
    assert_eq!(f.handle.chaos().faults, vec![Fault::ConnectionResetByPeer]);
}

#[tokio::test]
async fn unmatched_requests_get_a_diff() {
    let f = start().await;
    let empty = f.get("/anything").await;
    assert_eq!(empty.status, 404);
    assert!(empty.text.contains("Request was not matched"));
    f.stub(json!({ "name": "Users", "request": { "method": "GET", "urlPath": "/api/users", "headers": { "X-Key": { "equalTo": "k" } } } })).await;
    let r = f.get("/api/userz").await;
    assert_eq!(r.status, 404);
    assert_eq!(r.header("content-type").as_deref(), Some("text/plain"));
    assert!(r.text.contains("Closest stub: Users"), "{}", r.text);
    assert!(r.text.contains("<<<<< does not match"));
}

#[tokio::test]
async fn stub_cors_option() {
    let f = start_with(|_| {}, |c| c.stub_cors = true).await;
    f.stub(json!({ "request": { "method": "GET", "urlPath": "/data" }, "response": { "body": "d" } })).await;
    let pre = f
        .send(f.http.request(reqwest::Method::OPTIONS, f.url("/data")).header("Origin", "http://app.test").header("Access-Control-Request-Method", "GET"))
        .await;
    assert_eq!(pre.status, 200);
    assert_eq!(pre.header("access-control-allow-origin").as_deref(), Some("http://app.test"));
    let r = f.send(f.http.get(f.url("/data")).header("Origin", "http://app.test")).await;
    assert_eq!(r.header("access-control-allow-origin").as_deref(), Some("http://app.test"));
    let plain = start().await;
    plain.stub(json!({ "request": { "urlPath": "/data" } })).await;
    assert!(plain.send(plain.http.get(plain.url("/data")).header("Origin", "http://app.test")).await.header("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn subscribers_see_requests_and_mapping_changes() {
    let f = start().await;
    let mut rx = f.handle.subscribe();
    f.stub(json!({ "name": "Ping", "request": { "urlPath": "/ping" }, "response": { "body": "pong" } })).await;
    f.get("/ping").await;
    let mut saw_change = false;
    let mut summary = None;
    while summary.is_none() {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event").unwrap() {
            JournalEvent::MappingsChanged { stub_count } => saw_change |= stub_count == 1,
            JournalEvent::Request(s) => summary = Some(s),
            _ => {}
        }
    }
    let s = summary.unwrap();
    assert!(saw_change);
    assert_eq!((s.method.as_str(), s.url.as_str(), s.status, s.was_matched), ("GET", "/ping", 200, true));
    assert_eq!(s.stub_name.as_deref(), Some("Ping"));
    let json = serde_json::to_value(JournalEvent::Request(s)).unwrap();
    assert_eq!(json["type"], "request");
    assert_eq!(json["wasMatched"], true);
    f.admin("DELETE", "/requests", None).await;
    loop {
        if let JournalEvent::JournalCleared = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap() {
            break;
        }
    }
    assert_eq!(f.handle.journal_len(), 0);
}
