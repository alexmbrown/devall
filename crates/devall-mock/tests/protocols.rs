//! GraphQL, WebSocket, gRPC and MQTT mocks end to end, with real clients: reqwest (GraphQL),
//! tokio-tungstenite (WebSocket), /dev/all's own gRPC client over server reflection, and
//! /dev/all's MQTT client (rumqttc).

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const SDL: &str = r#"
type Query { user(id: ID!): User, users(first: Int): [User!]! }
type Mutation { createUser(name: String!): User! }
type User { id: ID! name: String! email: String! posts: [Post!]! }
type Post { id: ID! title: String! }
"#;

fn post_json(url: String, v: &Value) -> reqwest::RequestBuilder {
    reqwest::Client::new().post(url).header("content-type", "application/json").body(v.to_string())
}

async fn gql(f: &Fixture, body: Value) -> (u16, Value) {
    let r = f.send(post_json(f.url("/graphql"), &body)).await;
    (r.status, r.json())
}

#[tokio::test]
async fn graphql_stubs_automock_and_validation() {
    let f = start_with(
        |root| {
            write(root, "mocks/graphql/schema.graphql", SDL);
            write_mapping(
                root,
                "get-ada.json",
                &json!({
                    "name": "Ada",
                    "request": { "method": "POST", "urlPath": "/graphql", "graphql": { "operationName": "GetUser", "variables": { "id": "1" } } },
                    "response": { "status": 200, "transformers": ["graphql-automock"], "jsonBody": { "data": { "user": { "name": "Ada Lovelace", "posts": [{ "title": "Notes on the Analytical Engine" }] } } } }
                }),
            );
            write_mapping(
                root,
                "create-fails.json",
                &json!({
                    "request": { "method": "POST", "urlPath": "/graphql", "customMatcher": { "name": "gql-matcher", "parameters": { "query": "mutation { createUser(name: \"x\") { id } }" } } },
                    "response": { "status": 200, "jsonBody": { "errors": [{ "message": "name taken" }] } }
                }),
            );
        },
        |_| {},
    )
    .await;

    // Stub: overrides on top of generated data.
    let (s, v) = gql(&f, json!({ "query": "query GetUser($id: ID!) { user(id: $id) { id name email posts { title } } }", "variables": { "id": "1" }, "operationName": "GetUser" })).await;
    assert_eq!(s, 200);
    assert_eq!(v["data"]["user"]["name"], "Ada Lovelace");
    assert_eq!(v["data"]["user"]["id"], "1");
    assert!(v["data"]["user"]["email"].as_str().unwrap().contains('@'));
    assert_eq!(v["data"]["user"]["posts"].as_array().unwrap().len(), 1);

    // gql-matcher stub (normalised query, different whitespace).
    let (_, v) = gql(&f, json!({ "query": "mutation {\n  createUser(name: \"x\") {\n    id\n  }\n}" })).await;
    assert_eq!(v["errors"][0]["message"], "name taken");

    // No stub: auto-mocked from the schema, journaled as served.
    let (s, v) = gql(&f, json!({ "query": "{ users(first: 3) { id name } }" })).await;
    assert_eq!(s, 200);
    assert_eq!(v["data"]["users"].as_array().unwrap().len(), 3);
    // Validation.
    let (_, v) = gql(&f, json!({ "query": "{ users { nope } }" })).await;
    assert_eq!(v["errors"][0]["message"], "Cannot query field \"nope\" on type \"User\".");
    assert_eq!(v["errors"][0]["extensions"]["code"], "GRAPHQL_VALIDATION_FAILED");

    let reqs = f.get("/__admin/requests").await.json();
    assert!(reqs["requests"].as_array().unwrap().iter().all(|r| r["wasMatched"] == true), "auto-mocked requests are not near misses");
    let info = f.get("/__admin/devall/graphql").await.json();
    assert_eq!(info["schemas"][0]["endpoint"], "/graphql");
    let exec = f.send(post_json(f.url("/__admin/devall/graphql/execute"), &json!({ "query": "{ user(id: 7) { id } }" }))).await.json();
    assert_eq!(exec["data"]["user"]["id"], "7");
    f.handle.stop().await;
}

async fn ws_connect(port: u16, path: &str) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (ws, resp) = tokio_tungstenite::client_async(format!("ws://127.0.0.1:{port}{path}"), stream).await.unwrap();
    assert_eq!(resp.status().as_u16(), 101);
    ws
}

async fn next_text(ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> String {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("timed out").expect("closed").unwrap() {
            Message::Text(t) => return t.to_string(),
            Message::Close(c) => return format!("CLOSE {}", c.map(|c| u16::from(c.code)).unwrap_or(0)),
            _ => continue,
        }
    }
}

#[tokio::test]
async fn websocket_message_stubs() {
    let f = start_with(
        |root| {
            write(
                root,
                "mocks/websocket/chat.json",
                &json!({
                    "name": "Chat",
                    "request": { "urlPath": "/ws/chat" },
                    "onConnect": [ { "text": "welcome {{request.query.user}}" } ],
                    "messages": [
                        { "match": "ping", "reply": [ { "text": "pong" } ] },
                        { "match": { "matchesJsonPath": "$[?(@.type == 'say')]" }, "reply": [ { "json": { "from": "{{request.query.user}}", "said": "{{jsonPath message.text '$.text'}}" } } ], "broadcast": true },
                        { "match": "bye", "reply": [ { "text": "see you" } ], "close": { "code": 4000, "reason": "bye" } }
                    ],
                    "periodic": [ { "intervalMs": 50, "count": 2, "message": { "text": "tick" } } ]
                })
                .to_string(),
            );
        },
        |_| {},
    )
    .await;
    let port = f.handle.port();
    let mut a = ws_connect(port, "/ws/chat?user=ada").await;
    assert_eq!(next_text(&mut a).await, "welcome ada");
    a.send(Message::text("ping")).await.unwrap();
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(next_text(&mut a).await);
    }
    assert!(got.contains(&"pong".to_string()), "{got:?}");
    assert_eq!(got.iter().filter(|t| *t == "tick").count(), 2, "{got:?}");

    // Broadcast reaches every connection of the stub.
    let mut b = ws_connect(port, "/ws/chat?user=grace").await;
    assert_eq!(next_text(&mut b).await, "welcome grace");
    a.send(Message::text(r#"{"type":"say","text":"hi all"}"#)).await.unwrap();
    let expected = json!({ "from": "ada", "said": "hi all" });
    for ws in [&mut a, &mut b] {
        let mut t = next_text(ws).await;
        while t == "tick" {
            t = next_text(ws).await;
        }
        assert_eq!(serde_json::from_str::<Value>(&t).unwrap(), expected);
    }

    // Admin push to one connection, then the close rule.
    let conns = f.get("/__admin/devall/websocket").await.json();
    assert_eq!(conns["connections"].as_array().unwrap().len(), 2);
    let sent = f.send(post_json(f.url("/__admin/devall/websocket/send"), &json!({ "path": "/ws/chat", "text": "server says hi" }))).await.json();
    assert_eq!(sent["sent"], 2);
    let mut t = next_text(&mut b).await;
    while t == "tick" {
        t = next_text(&mut b).await;
    }
    assert_eq!(t, "server says hi");
    b.send(Message::text("bye")).await.unwrap();
    let mut t = next_text(&mut b).await;
    while t != "see you" {
        t = next_text(&mut b).await;
    }
    assert_eq!(next_text(&mut b).await, "CLOSE 4000");

    // Unknown paths are not upgraded.
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    assert!(tokio_tungstenite::client_async(format!("ws://127.0.0.1:{port}/nope"), stream).await.is_err());

    // Journal.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let msgs = f.get("/__admin/devall/messages?protocol=websocket").await.json();
    let list = msgs["messages"].as_array().unwrap();
    assert!(list.iter().any(|m| m["direction"] == "in" && m["body"] == "ping"));
    assert!(list.iter().any(|m| m["direction"] == "out" && m["body"] == "pong" && m["stub"] == "Chat"));
    assert!(list.iter().any(|m| m["meta"]["event"] == "close" && m["meta"]["code"] == 4000));
    f.handle.stop().await;
}

const PROTO: &str = r#"
syntax = "proto3";
package shop.v1;
service Catalog {
  rpc GetItem (GetItemRequest) returns (Item);
  rpc ListItems (ListRequest) returns (stream Item);
  rpc Upload (stream Item) returns (UploadReply);
}
message GetItemRequest { string id = 1; }
message ListRequest { int32 limit = 1; }
message Item { string id = 1; string name = 2; double price = 3; }
message UploadReply { int32 count = 1; }
"#;

fn grpc_opts(port: u16, method: &str, message: &str) -> devall_proto::grpc::GrpcOptions {
    devall_proto::grpc::GrpcOptions {
        target: devall_proto::grpc::GrpcTarget::parse(&format!("grpc://127.0.0.1:{port}"), true, Duration::from_secs(5)).unwrap(),
        service: "shop.v1.Catalog".into(),
        method: method.into(),
        message: message.into(),
        metadata: vec![("x-tenant".into(), "acme".into())],
        deadline: None,
        source: devall_proto::grpc::SchemaSource::Reflection,
    }
}

async fn call(port: u16, method: &str, message: &str) -> (devall_proto::grpc::GrpcStatus, Vec<Value>) {
    use devall_proto::{Direction, EventSink, ProtoEvent};
    let (sink, mut rx) = EventSink::channel();
    let (_h, fut) = devall_proto::grpc::start_call(&grpc_opts(port, method, message), sink, false).await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), fut).await.unwrap();
    let mut msgs = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let ProtoEvent::Message { direction: Direction::In, body, .. } = e {
            msgs.push(serde_json::from_str(&body).unwrap());
        }
    }
    (status, msgs)
}

#[tokio::test]
async fn grpc_unary_streaming_status_and_reflection() {
    let f = start_with(
        |root| {
            write(root, "mocks/grpc/catalog.proto", PROTO);
            write(
                root,
                "mocks/grpc/stubs.json",
                &json!({ "stubs": [
                    { "name": "Known item", "method": "shop.v1.Catalog/GetItem", "request": { "body": { "id": "42" }, "metadata": { "x-tenant": { "equalTo": "acme" } } },
                      "response": { "message": { "id": "{{jsonPath request.body '$.id'}}", "name": "Teapot", "price": 9.5 }, "headers": { "x-mock": "yes" }, "trailers": { "x-trace": "t-1" } } },
                    { "name": "Missing item", "method": "shop.v1.Catalog/GetItem", "priority": 10, "response": { "status": "NOT_FOUND", "statusMessage": "no such item" } },
                    { "name": "List", "method": "shop.v1.Catalog/ListItems", "response": { "messages": [ { "id": "a" }, { "id": "b" }, { "id": "c" } ], "streamIntervalMs": 20 } },
                    { "name": "Upload", "method": "shop.v1.Catalog/Upload", "request": { "body": { "matchesJsonPath": "$[1]" } }, "response": { "message": { "count": 2 } } }
                ]})
                .to_string(),
            );
        },
        |cfg| cfg.grpc_port = Some(0),
    )
    .await;
    let port = f.handle.grpc_port().expect("gRPC listener started");

    let (st, msgs) = call(port, "GetItem", r#"{"id":"42"}"#).await;
    assert!(st.is_ok(), "{st:?}");
    assert_eq!(msgs[0], json!({ "id": "42", "name": "Teapot", "price": 9.5 }));
    assert!(st.headers.iter().any(|(k, v)| k == "x-mock" && v == "yes"));
    assert!(st.trailers.iter().any(|(k, v)| k == "x-trace" && v == "t-1"));

    let (st, msgs) = call(port, "GetItem", r#"{"id":"7"}"#).await;
    assert_eq!((st.code, st.message.as_str()), (5, "no such item"));
    assert!(msgs.is_empty());

    let (st, msgs) = call(port, "ListItems", r#"{"limit":3}"#).await;
    assert!(st.is_ok(), "{st:?}");
    assert_eq!(msgs.iter().map(|m| m["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["a", "b", "c"]);

    let (st, msgs) = call(port, "Upload", r#"[{"id":"x"},{"id":"y"}]"#).await;
    assert!(st.is_ok(), "{st:?}");
    assert_eq!(msgs[0]["count"], 2);

    let info = f.get("/__admin/devall/grpc").await.json();
    assert_eq!(info["port"], port);
    assert_eq!(info["methods"].as_array().unwrap().len(), 3);
    let msgs = f.get("/__admin/devall/messages?protocol=grpc").await.json();
    assert!(msgs["messages"].as_array().unwrap().iter().any(|m| m["meta"]["statusName"] == "NOT_FOUND"));

    // Stop / restart on another port through the admin API.
    f.send(f.http.post(f.url("/__admin/devall/grpc/stop"))).await;
    assert!(f.handle.grpc_port().is_none());
    let p = f.send(post_json(f.url("/__admin/devall/grpc/start"), &json!({}))).await.json();
    let port2 = p["port"].as_u64().unwrap() as u16;
    assert!(call(port2, "GetItem", r#"{"id":"42"}"#).await.0.is_ok());
    f.handle.stop().await;
    assert!(f.handle.grpc_port().is_none());
}

async fn next_msg(rx: &mut tokio::sync::mpsc::UnboundedReceiver<devall_proto::ProtoEvent>) -> (String, String) {
    use devall_proto::{Direction, ProtoEvent};
    loop {
        let e = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("timed out waiting for an MQTT message").expect("sink closed");
        if let ProtoEvent::Message { direction: Direction::In, label, body, .. } = e {
            return (label.unwrap_or_default(), body);
        }
    }
}

#[tokio::test]
async fn mqtt_broker_behaviours() {
    use devall_proto::mqtt::{MqttOptions, MqttSession, parse_broker_url};
    let f = start_with(
        |root| {
            write(
                root,
                "mocks/mqtt/devices.json",
                &json!({
                    "name": "Devices",
                    "retained": [ { "topic": "devices/1/state", "payload": "on", "qos": 1 } ],
                    "onSubscribe": [ { "topic": "devices/+/config", "publish": [ { "topic": "devices/1/config", "json": { "rate": 5 } } ] } ],
                    "rules": [ { "topic": "rpc/request/+", "payload": { "matchesJsonPath": "$.id" },
                                 "publish": [ { "topic": "rpc/response/{{topicSegments.[2]}}", "json": { "ok": true, "id": "{{jsonPath message.payload '$.id'}}" }, "delayMs": 20 } ] } ],
                    "periodic": [ { "intervalMs": 50, "count": 2, "publish": { "topic": "sensors/temp", "payload": "21" } } ]
                })
                .to_string(),
            );
        },
        |cfg| cfg.mqtt_port = Some(0),
    )
    .await;
    let port = f.handle.mqtt_port().expect("broker started");
    let opts = |subs: &[(&str, u8)]| MqttOptions {
        broker: parse_broker_url(&format!("mqtt://127.0.0.1:{port}")).unwrap(),
        client_id: format!("t-{:08x}", fastrand::u32(..)),
        username: None,
        password: None,
        keep_alive: Duration::from_secs(30),
        clean_session: true,
        version: devall_proto::MqttVersion::V311,
        last_will: None,
        verify_tls: true,
        connect_timeout: Duration::from_secs(5),
        subscriptions: subs.iter().map(|(t, q)| (t.to_string(), *q)).collect(),
    };

    // Retained message on subscribe, publish-on-subscribe, periodic publishes.
    let (sink, mut rx) = devall_proto::EventSink::channel();
    let s = MqttSession::connect(opts(&[("devices/+/state", 1), ("devices/+/config", 0), ("sensors/#", 0)]), sink).await.unwrap();
    let mut got = Vec::new();
    for _ in 0..4 {
        got.push(next_msg(&mut rx).await);
    }
    assert!(got.contains(&("devices/1/state".into(), "on".into())), "{got:?}");
    assert!(got.contains(&("devices/1/config".into(), "{\"rate\":5}".into())), "{got:?}");
    assert_eq!(got.iter().filter(|(t, _)| t == "sensors/temp").count(), 2, "{got:?}");

    // Request / response rule, and plain routing between clients.
    s.subscribe("rpc/response/#", 0).await.unwrap();
    let (sink2, _rx2) = devall_proto::EventSink::channel();
    let other = MqttSession::connect(opts(&[]), sink2).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    other.publish("rpc/request/42", br#"{"id":"r-1"}"#.to_vec(), 1, false).await.unwrap();
    let (topic, body) = next_msg(&mut rx).await;
    assert_eq!(topic, "rpc/response/42");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({ "ok": true, "id": "r-1" }));
    s.subscribe("chat/#", 0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    other.publish("chat/room", b"hello".to_vec(), 0, false).await.unwrap();
    assert_eq!(next_msg(&mut rx).await, ("chat/room".into(), "hello".into()));

    // Admin publish + status + journal.
    let r = f.send(post_json(f.url("/__admin/devall/mqtt/publish"), &json!({ "topic": "chat/room", "payload": "from admin" }))).await.json();
    assert_eq!(r["delivered"], 1);
    assert_eq!(next_msg(&mut rx).await, ("chat/room".into(), "from admin".into()));
    let info = f.get("/__admin/devall/mqtt").await.json();
    assert_eq!(info["clients"].as_array().unwrap().len(), 2);
    assert_eq!(info["retained"][0]["topic"], "devices/1/state");
    let msgs = f.get("/__admin/devall/messages?protocol=mqtt").await.json();
    let list = msgs["messages"].as_array().unwrap();
    assert!(list.iter().any(|m| m["direction"] == "in" && m["channel"] == "rpc/request/42"));
    assert!(list.iter().any(|m| m["direction"] == "out" && m["channel"] == "rpc/response/42" && m["stub"] == "Devices"));
    assert!(list.iter().any(|m| m["meta"]["event"] == "subscribe"));

    s.disconnect().await;
    other.disconnect().await;
    f.handle.stop().await;
    assert!(f.handle.mqtt_port().is_none());
}

#[tokio::test]
async fn load_errors_and_protocol_folders() {
    let f = start_with(
        |root| {
            write(root, "mocks/websocket/bad.json", r#"{ "name": "no request" }"#);
            write(root, "mocks/grpc/broken.proto", "syntax = \"proto3\"; message {");
            write(root, "mocks/mqtt/bad.json", r#"{ "rules": [ { "topic": "a/#/b" } ] }"#);
            write(root, "mocks/graphql/schema.graphql", "type Query { a: Nope }");
        },
        |_| {},
    )
    .await;
    let errors = f.handle.load_errors();
    let files: Vec<&str> = errors.iter().map(|e| e.file.as_str()).collect();
    assert!(files.contains(&"mocks/websocket/bad.json"), "{errors:?}");
    assert!(files.contains(&"mocks/grpc"), "{errors:?}");
    assert!(files.contains(&"mocks/mqtt/bad.json"), "{errors:?}");
    assert!(files.contains(&"mocks/graphql/schema.graphql"), "{errors:?}");
    let folders = devall_mock::protocol_folders(f.root());
    assert!(folders.graphql && folders.grpc && folders.mqtt);
    let p = f.get("/__admin/devall/protocols").await.json();
    assert_eq!(p["grpc"]["running"], false);
    assert!(!p["mqtt"]["errors"].as_array().unwrap().is_empty());
    f.handle.stop().await;
}
