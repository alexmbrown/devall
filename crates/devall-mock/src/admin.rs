//! WireMock admin API (`/__admin/...`) plus /dev/all extensions under `/__admin/devall/`.
//!
//! Cross-origin access is limited to loopback and Tauri origins (plus configured extras) so a web
//! page the user happens to visit cannot drive the API; requests without an `Origin` header
//! (curl, the desktop app, the CLI) are always allowed.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use http::Response;
use http::request::Parts;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::chaos::{Chaos, ChaosConfig};
use crate::journal::ServeEvent;
use crate::matchers::ValuePattern;
use crate::pattern::RequestPattern;
use crate::recording;
use crate::request::MockRequest;
use crate::server::{RespBody, ServerState, build_response, full, lock, read, write};
use crate::stub::{DelayDistribution, Stub};
use crate::util::guess_content_type;
use crate::{VERSION, request};

type Resp = Response<RespBody>;

fn json_resp(status: u16, v: &Value) -> Resp {
    build_response(status, None, &[("Content-Type".into(), "application/json".into())], full(serde_json::to_vec(v).unwrap_or_default()))
}

fn ok_empty() -> Resp {
    build_response(200, None, &[], full(Bytes::new()))
}

fn error(status: u16, title: &str, detail: Option<&str>) -> Resp {
    let mut e = Map::new();
    e.insert("code".into(), json!(status));
    e.insert("title".into(), json!(title));
    if let Some(d) = detail {
        e.insert("detail".into(), json!(d));
    }
    json_resp(status, &json!({ "errors": [Value::Object(e)] }))
}

fn not_found(what: &str) -> Resp {
    error(404, &format!("{what} not found"), None)
}

fn parse_body(body: &Bytes) -> Result<Value, Box<Resp>> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(body).map_err(|e| Box::new(error(400, "Error parsing JSON", Some(&e.to_string()))))
}

fn query(parts: &Parts) -> Vec<(String, String)> {
    parts.uri.query().map(request::parse_form).unwrap_or_default()
}

fn q<'a>(qs: &'a [(String, String)], k: &str) -> Option<&'a str> {
    qs.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
}

fn is_local_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h == "::1" || h.starts_with("127.")
}

fn origin_allowed(origin: &str, extra: &[String]) -> bool {
    if extra.iter().any(|o| o == "*" || o.eq_ignore_ascii_case(origin)) {
        return true;
    }
    match url::Url::parse(origin) {
        Ok(u) => matches!(u.scheme(), "http" | "https" | "tauri") && u.host_str().is_some_and(is_local_host),
        Err(_) => false,
    }
}

fn with_cors(mut resp: Resp, origin: Option<&str>) -> Resp {
    if let Some(o) = origin
        && let Ok(v) = http::HeaderValue::from_str(o)
    {
        let h = resp.headers_mut();
        h.insert("access-control-allow-origin", v);
        h.insert("access-control-expose-headers", http::HeaderValue::from_static("Content-Type, Location"));
        h.append("vary", http::HeaderValue::from_static("Origin"));
    }
    resp
}

fn preflight(parts: &Parts, origin: &str) -> Resp {
    let allow_headers = parts
        .headers
        .get("access-control-request-headers")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Authorization, Content-Type")
        .to_string();
    let headers = vec![
        ("Access-Control-Allow-Methods".to_string(), "OPTIONS, GET, POST, PUT, PATCH, DELETE".to_string()),
        ("Access-Control-Allow-Headers".to_string(), allow_headers),
        ("Access-Control-Max-Age".to_string(), "600".to_string()),
    ];
    with_cors(build_response(200, None, &headers, full(Bytes::new())), Some(origin))
}

pub(crate) async fn handle(st: &Arc<ServerState>, parts: &Parts, body: Bytes, _peer: SocketAddr) -> Resp {
    let origin = parts.headers.get(http::header::ORIGIN).and_then(|v| v.to_str().ok()).map(str::to_string);
    if let Some(o) = &origin
        && !origin_allowed(o, &st.config.admin_allowed_origins)
    {
        return error(403, "Origin not allowed", Some("Add it to the server's admin_allowed_origins to use the admin API from this page."));
    }
    // DNS-rebinding guard: a loopback-only server only answers to loopback host names.
    if !st.config.bind_all
        && let Some(host) = parts.headers.get(http::header::HOST).and_then(|v| v.to_str().ok())
    {
        let (name, _) = request::split_host_port(host);
        if !is_local_host(&name) {
            return error(403, "Host not allowed", None);
        }
    }
    if parts.method == http::Method::OPTIONS {
        return match &origin {
            Some(o) => preflight(parts, o),
            None => ok_empty(),
        };
    }
    let resp = route(st, parts, &body).await;
    with_cors(resp, origin.as_deref())
}

fn segments(parts: &Parts) -> Vec<String> {
    let path = parts.uri.path().trim_start_matches("/__admin");
    path.split('/').filter(|s| !s.is_empty()).map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()).collect()
}

async fn route(st: &Arc<ServerState>, parts: &Parts, body: &Bytes) -> Resp {
    let segs = segments(parts);
    let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
    let method = parts.method.as_str();
    let qs = query(parts);
    macro_rules! body_json {
        () => {
            match parse_body(body) {
                Ok(v) => v,
                Err(r) => return *r,
            }
        };
    }
    match (method, segs.as_slice()) {
        ("GET", [] | ["health"]) => health(st),
        ("GET", ["version"]) => json_resp(200, &json!({ "version": VERSION })),

        // ---- mappings
        ("GET", ["mappings"]) => {
            let all: Vec<Value> = read(&st.store).all().iter().map(|s| s.to_json()).collect();
            json_resp(200, &paged(all, &qs, "mappings"))
        }
        ("POST", ["mappings"]) => create_stub(st, body_json!()),
        ("DELETE", ["mappings"]) => {
            let ids: Vec<Uuid> = read(&st.store).all().iter().map(|s| s.id).collect();
            for id in ids {
                if let Err(e) = st.remove_stub(&id) {
                    return error(500, "Could not delete mapping file", Some(&e.to_string()));
                }
            }
            ok_empty()
        }
        ("POST", ["mappings", "import"]) => import(st, body_json!()),
        ("POST", ["mappings", "reset"]) => {
            st.reload();
            ok_empty()
        }
        ("POST", ["mappings", "save"]) => match st.save_all() {
            Ok(_) => ok_empty(),
            Err(e) => error(500, "Could not save mappings", Some(&e.to_string())),
        },
        ("GET", ["mappings", "unmatched"]) => {
            let list: Vec<Value> = unmatched_stubs(st).iter().map(|s| s.to_json()).collect();
            json_resp(200, &json!({ "mappings": list, "meta": { "total": list.len() } }))
        }
        ("DELETE", ["mappings", "unmatched"]) => {
            for s in unmatched_stubs(st) {
                if let Err(e) = st.remove_stub(&s.id) {
                    return error(500, "Could not delete mapping file", Some(&e.to_string()));
                }
            }
            ok_empty()
        }
        ("POST", ["mappings", "find-by-metadata"]) => match by_metadata(st, &body_json!()) {
            Ok(list) => {
                let list: Vec<Value> = list.iter().map(|s| s.to_json()).collect();
                json_resp(200, &json!({ "mappings": list, "meta": { "total": list.len() } }))
            }
            Err(e) => error(422, "Invalid metadata pattern", Some(&e)),
        },
        ("POST", ["mappings", "remove-by-metadata"]) => match by_metadata(st, &body_json!()) {
            Ok(list) => {
                for s in list {
                    if let Err(e) = st.remove_stub(&s.id) {
                        return error(500, "Could not delete mapping file", Some(&e.to_string()));
                    }
                }
                ok_empty()
            }
            Err(e) => error(422, "Invalid metadata pattern", Some(&e)),
        },
        ("GET", ["mappings", id]) => match Uuid::parse_str(id).ok().and_then(|id| read(&st.store).get(&id)) {
            Some(s) => json_resp(200, &s.to_json()),
            None => not_found("Stub mapping"),
        },
        ("PUT", ["mappings", id]) => update_stub(st, id, body_json!()),
        ("DELETE", ["mappings", id]) => match Uuid::parse_str(id) {
            Ok(id) => match st.remove_stub(&id) {
                Ok(Some(_)) => ok_empty(),
                Ok(None) => not_found("Stub mapping"),
                Err(e) => error(500, "Could not delete mapping file", Some(&e.to_string())),
            },
            Err(_) => not_found("Stub mapping"),
        },

        // ---- request journal
        ("GET", ["requests"]) => requests(st, &qs),
        ("DELETE", ["requests"]) | ("POST", ["requests", "reset"]) => {
            st.clear_journal();
            ok_empty()
        }
        ("POST", ["requests", "count"]) => match pattern_of(&body_json!()) {
            Ok(p) => {
                let count = lock(&st.journal).events().iter().filter(|e| p.is_match(&e.request)).count();
                json_resp(200, &json!({ "count": count }))
            }
            Err(r) => *r,
        },
        ("POST", ["requests", "find"]) => match pattern_of(&body_json!()) {
            Ok(p) => {
                let list: Vec<Value> = lock(&st.journal).events().iter().filter(|e| p.is_match(&e.request)).map(|e| e.request_json()).collect();
                json_resp(200, &json!({ "requests": list, "requestJournalDisabled": false }))
            }
            Err(r) => *r,
        },
        ("POST", ["requests", "remove"]) => match pattern_of(&body_json!()) {
            Ok(p) => {
                let removed = lock(&st.journal).retain(|e| !p.is_match(&e.request));
                json_resp(200, &json!({ "requests": removed.iter().map(|e| e.to_json()).collect::<Vec<_>>() }))
            }
            Err(r) => *r,
        },
        ("POST", ["requests", "remove-by-metadata"]) => {
            let pattern = match ValuePattern::from_json(&body_json!()) {
                Ok(p) => p,
                Err(e) => return error(422, "Invalid metadata pattern", Some(&e)),
            };
            let removed = lock(&st.journal).retain(|e| !e.stub.as_ref().and_then(|s| s.metadata.as_ref()).is_some_and(|m| pattern.is_match(Some(&m.to_string()))));
            json_resp(200, &json!({ "requests": removed.iter().map(|e| e.to_json()).collect::<Vec<_>>() }))
        }
        ("GET", ["requests", "unmatched"]) => {
            let list: Vec<Value> = lock(&st.journal).events().iter().filter(|e| !e.was_matched).map(|e| e.request_json()).collect();
            json_resp(200, &json!({ "requests": list, "requestJournalDisabled": false }))
        }
        ("GET", ["requests", "unmatched", "near-misses"]) => {
            let unmatched: Vec<Arc<ServeEvent>> = lock(&st.journal).events().into_iter().filter(|e| !e.was_matched).collect();
            let skip = unmatched.len().saturating_sub(200);
            let mut out = Vec::new();
            for e in unmatched.iter().skip(skip) {
                out.extend(near_misses_for_request(st, &e.request, Some(e.request_json())));
            }
            json_resp(200, &json!({ "nearMisses": out }))
        }
        ("GET", ["requests", id]) => match Uuid::parse_str(id).ok().and_then(|id| lock(&st.journal).get(&id)) {
            Some(e) => json_resp(200, &e.to_json()),
            None => not_found("Request"),
        },
        ("DELETE", ["requests", id]) => match Uuid::parse_str(id) {
            Ok(id) if lock(&st.journal).remove(&id) => ok_empty(),
            _ => not_found("Request"),
        },
        ("POST", ["near-misses", "request"]) => match MockRequest::from_json(&body_json!()) {
            Ok(req) => json_resp(200, &json!({ "nearMisses": near_misses_for_request(st, &req, None) })),
            Err(e) => error(422, "Invalid request", Some(&e)),
        },
        ("POST", ["near-misses", "request-pattern"]) => {
            let raw = body_json!();
            match RequestPattern::from_json(&raw) {
                Ok(p) => json_resp(200, &json!({ "nearMisses": near_misses_for_pattern(st, &p, &raw) })),
                Err(e) => error(422, "Invalid request pattern", Some(&e)),
            }
        }

        // ---- scenarios
        ("GET", ["scenarios"]) => json_resp(200, &read(&st.store).scenarios_json()),
        ("POST", ["scenarios", "reset"]) => {
            write(&st.store).reset_scenarios();
            ok_empty()
        }
        ("PUT", ["scenarios", name, "state"]) => {
            let v = body_json!();
            let state = v.get("state").and_then(Value::as_str).map(str::to_string);
            if write(&st.store).set_scenario_state(name, state) { ok_empty() } else { not_found(&format!("Scenario {name}")) }
        }

        // ---- global
        ("POST", ["reset"]) => {
            st.reload();
            st.clear_journal();
            write(&st.store).reset_scenarios();
            ok_empty()
        }
        ("GET", ["settings"]) => json_resp(200, &json!({ "settings": Value::Object(read(&st.settings).clone()) })),
        ("POST" | "PUT", ["settings"]) => set_settings(st, body_json!(), false),
        ("PATCH", ["settings"]) => set_settings(st, body_json!(), true),
        ("POST", ["shutdown"]) => {
            let st2 = st.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = st2.shutdown.send_replace(true);
            });
            ok_empty()
        }

        // ---- recording
        ("POST", ["recordings", "start"]) => match recording::start(st, body_json!()) {
            Ok(()) => ok_empty(),
            Err(e) => error(400, "Could not start recording", Some(&e)),
        },
        ("POST", ["recordings", "stop"]) => match recording::stop(st) {
            Ok(v) => json_resp(200, &v),
            Err(e) => error(400, "Could not stop recording", Some(&e)),
        },
        ("GET", ["recordings", "status"]) => json_resp(200, &recording::status(st)),
        ("POST", ["recordings", "snapshot"]) => match recording::snapshot(st, &body_json!()) {
            Ok(v) => json_resp(200, &v),
            Err(e) => error(400, "Could not take snapshot", Some(&e)),
        },

        // ---- __files
        ("GET", ["files"]) => json_resp(200, &json!(st.files.list_files())),
        ("GET", ["files", rest @ ..]) => {
            let rel = rest.join("/");
            match st.files.read_body_file(&rel) {
                Ok(bytes) => build_response(200, None, &[("Content-Type".into(), guess_content_type(&rel).into())], full(bytes)),
                Err(_) => not_found("File"),
            }
        }
        ("PUT" | "POST", ["files", rest @ ..]) if !rest.is_empty() => match st.files.write_file(&rest.join("/"), body) {
            Ok(()) => ok_empty(),
            Err(e) => error(400, "Could not write file", Some(&e.to_string())),
        },
        ("DELETE", ["files", rest @ ..]) if !rest.is_empty() => match st.files.delete_file(&rest.join("/")) {
            Ok(true) => ok_empty(),
            Ok(false) => not_found("File"),
            Err(e) => error(400, "Could not delete file", Some(&e.to_string())),
        },

        // ---- /dev/all extensions
        ("GET", ["devall", "chaos"]) => json_resp(200, &serde_json::to_value(&read(&st.chaos).config).unwrap_or(Value::Null)),
        ("PUT" | "POST", ["devall", "chaos"]) => {
            let v = body_json!();
            let config: ChaosConfig = match serde_json::from_value(v) {
                Ok(c) => c,
                Err(e) => return error(422, "Invalid chaos configuration", Some(&e.to_string())),
            };
            match Chaos::new(config) {
                Ok(c) => {
                    let out = serde_json::to_value(&c.config).unwrap_or(Value::Null);
                    *write(&st.chaos) = c;
                    json_resp(200, &out)
                }
                Err(e) => error(422, "Invalid chaos configuration", Some(&e)),
            }
        }
        ("GET", ["devall", "info"]) => json_resp(
            200,
            &json!({
                "engine": "devall-mock",
                "version": VERSION,
                "port": st.port,
                "projectRoot": st.files.root.display().to_string(),
                "stubCount": st.stub_count(),
                "running": st.running.load(Ordering::SeqCst),
                "globalTemplating": st.config.global_templating,
                "loadErrors": read(&st.load_errors).clone(),
            }),
        ),
        ("POST", ["devall", "reload"]) => {
            let errors = st.reload();
            json_resp(200, &json!({ "stubCount": st.stub_count(), "loadErrors": errors }))
        }

        // ---- other protocols
        ("GET", ["devall", "protocols"]) => json_resp(200, &protocols_json(st)),
        ("GET", ["devall", "graphql"]) => {
            let g = read(&st.graphql).clone();
            json_resp(200, &json!({ "schemas": g.info(), "errors": g.errors }))
        }
        ("POST", ["devall", "graphql", "execute"]) => graphql_execute(st, &body_json!()),
        ("GET", ["devall", "websocket"]) => json_resp(200, &json!({ "stubs": st.ws.stubs_json(), "connections": st.ws.connections(), "errors": read(&st.ws.errors).clone() })),
        ("POST", ["devall", "websocket", "send"]) => {
            let v = body_json!();
            let msg = match crate::ws::MessageDef::from_json(&v) {
                Ok(m) => m,
                Err(e) => return error(422, "Invalid message", Some(&e)),
            };
            let out = msg.render(st, &json!({}));
            let sent = st.ws.send(v.get("connectionId").and_then(Value::as_str), v.get("path").and_then(Value::as_str), &out);
            json_resp(200, &json!({ "sent": sent }))
        }
        ("POST", ["devall", "websocket", "close"]) => {
            let v = body_json!();
            let code = v.get("code").and_then(Value::as_u64).map(|c| c as u16).unwrap_or(1000);
            let reason = v.get("reason").and_then(Value::as_str).unwrap_or_default().to_string();
            let closed = st.ws.send(v.get("connectionId").and_then(Value::as_str), v.get("path").and_then(Value::as_str), &crate::ws::Out::Close(code, reason));
            json_resp(200, &json!({ "closed": closed }))
        }
        ("GET", ["devall", "grpc"]) => json_resp(
            200,
            &json!({
                "running": st.grpc.port().is_some(),
                "port": st.grpc.port(),
                "hasProtos": st.grpc.has_protos(),
                "methods": st.grpc.methods(),
                "stubs": st.grpc.stubs_json(),
                "errors": read(&st.grpc.errors).clone(),
            }),
        ),
        ("POST", ["devall", "grpc", "start"]) => {
            let port = parse_body(body).ok().and_then(|v| v.get("port").and_then(Value::as_u64)).unwrap_or(0) as u16;
            match crate::grpc::start(st, port).await {
                Ok(p) => json_resp(200, &json!({ "port": p })),
                Err(e) => error(409, "Could not start the gRPC listener", Some(&e.to_string())),
            }
        }
        ("POST", ["devall", "grpc", "stop"]) => {
            crate::grpc::stop(st).await;
            ok_empty()
        }
        ("GET", ["devall", "mqtt"]) => json_resp(
            200,
            &json!({
                "running": st.mqtt.port().is_some(),
                "port": st.mqtt.port(),
                "hasBehaviours": st.mqtt.has_behaviours(),
                "clients": st.mqtt.clients(),
                "retained": st.mqtt.retained(),
                "behaviours": st.mqtt.behaviours_json(),
                "errors": read(&st.mqtt.errors).clone(),
            }),
        ),
        ("POST", ["devall", "mqtt", "start"]) => {
            let port = parse_body(body).ok().and_then(|v| v.get("port").and_then(Value::as_u64)).unwrap_or(0) as u16;
            match crate::mqtt::start(st, port).await {
                Ok(p) => json_resp(200, &json!({ "port": p })),
                Err(e) => error(409, "Could not start the MQTT broker", Some(&e.to_string())),
            }
        }
        ("POST", ["devall", "mqtt", "stop"]) => {
            crate::mqtt::stop(st).await;
            ok_empty()
        }
        ("POST", ["devall", "mqtt", "publish"]) => {
            let v = body_json!();
            let Some(topic) = v.get("topic").and_then(Value::as_str).filter(|t| !t.is_empty() && !t.contains(['+', '#'])) else {
                return error(422, "Invalid message", Some("topic is required (no wildcards)"));
            };
            if st.mqtt.port().is_none() {
                return error(409, "The MQTT broker is not running", None);
            }
            let payload = match (v.get("payload"), v.get("json")) {
                (Some(Value::String(s)), _) => s.clone(),
                (Some(other), _) => other.to_string(),
                (None, Some(j)) => j.to_string(),
                _ => String::new(),
            };
            let msg = crate::mqtt::Msg {
                topic: topic.to_string(),
                payload: payload.into_bytes(),
                qos: v.get("qos").and_then(Value::as_u64).unwrap_or(0).min(2) as u8,
                retain: v.get("retain").and_then(Value::as_bool).unwrap_or(false),
            };
            let delivered = crate::mqtt::publish(st, msg, Some("admin"));
            json_resp(200, &json!({ "delivered": delivered }))
        }
        ("GET", ["devall", "messages"]) => {
            let protocol = q(&qs, "protocol").and_then(crate::messages::Protocol::parse);
            let limit = q(&qs, "limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(500);
            json_resp(200, &json!({ "messages": st.messages.list(protocol, limit) }))
        }
        ("DELETE", ["devall", "messages"]) => {
            st.messages.clear(q(&qs, "protocol").and_then(crate::messages::Protocol::parse));
            ok_empty()
        }
        _ => error(404, "Not found", Some(&format!("{method} {}", parts.uri.path()))),
    }
}

/// Status of everything beyond HTTP stubs (for the Mock server page's protocol tabs).
fn protocols_json(st: &ServerState) -> Value {
    let g = read(&st.graphql).clone();
    json!({
        "graphql": { "schemas": g.info(), "errors": g.errors },
        "websocket": { "stubs": st.ws.stubs_json().len(), "connections": st.ws.connections().len(), "errors": read(&st.ws.errors).clone() },
        "grpc": { "running": st.grpc.port().is_some(), "port": st.grpc.port(), "hasProtos": st.grpc.has_protos(), "methods": st.grpc.methods().len(), "errors": read(&st.grpc.errors).clone() },
        "mqtt": { "running": st.mqtt.port().is_some(), "port": st.mqtt.port(), "hasBehaviours": st.mqtt.has_behaviours(), "clients": st.mqtt.clients().len(), "errors": read(&st.mqtt.errors).clone() },
    })
}

/// `{"schema"?, "query", "variables"?, "operationName"?}` → the auto-mocked result (no stubs).
fn graphql_execute(st: &ServerState, v: &Value) -> Resp {
    let g = read(&st.graphql).clone();
    let entry = match v.get("schema").and_then(Value::as_str) {
        Some(name) => g.by_name(name),
        None => g.schemas.first(),
    };
    let Some(entry) = entry else { return error(404, "GraphQL schema not found", Some("add a schema to mocks/graphql/")) };
    let Some(query) = v.get("query").and_then(Value::as_str) else { return error(422, "query is required", None) };
    let payload = crate::graphql::Payload {
        query: query.to_string(),
        variables: v.get("variables").and_then(Value::as_object).cloned().unwrap_or_default(),
        operation_name: v.get("operationName").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
    };
    json_resp(200, &crate::graphql::execute_payload(entry, &payload, None, &[], false))
}

fn health(st: &ServerState) -> Resp {
    json_resp(
        200,
        &json!({
            "status": "healthy",
            "message": "Wiremock is ok",
            "version": VERSION,
            "uptimeInSeconds": st.started.elapsed().as_secs(),
            "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        }),
    )
}

fn paged(all: Vec<Value>, qs: &[(String, String)], key: &str) -> Value {
    let total = all.len();
    let offset = q(qs, "offset").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let limit = q(qs, "limit").and_then(|v| v.parse::<usize>().ok()).filter(|l| *l > 0).unwrap_or(usize::MAX);
    let page: Vec<Value> = all.into_iter().skip(offset).take(limit).collect();
    json!({ key: page, "meta": { "total": total } })
}

fn stub_error(e: &str) -> Resp {
    error(422, "Invalid stub mapping", Some(e))
}

fn create_stub(st: &ServerState, v: Value) -> Resp {
    match Stub::from_json(&v, Uuid::new_v4) {
        Ok(stub) => match st.add_stub(stub) {
            Ok(s) => json_resp(201, &s.to_json()),
            Err(e) => error(500, "Could not save stub mapping", Some(&e.to_string())),
        },
        Err(e) => stub_error(&e),
    }
}

fn update_stub(st: &ServerState, id: &str, mut v: Value) -> Resp {
    let Ok(id) = Uuid::parse_str(id) else { return not_found("Stub mapping") };
    let Some(old) = read(&st.store).get(&id) else { return not_found("Stub mapping") };
    let explicit_persistent = v.get("persistent").is_some();
    if let Value::Object(o) = &mut v {
        o.insert("id".into(), json!(id.to_string()));
        o.remove("uuid");
    }
    match Stub::from_json(&v, || id) {
        Ok(mut stub) => {
            if !explicit_persistent && old.persistent {
                stub.set_persistent(true);
            }
            match st.add_stub(stub) {
                Ok(s) => json_resp(200, &s.to_json()),
                Err(e) => error(500, "Could not save stub mapping", Some(&e.to_string())),
            }
        }
        Err(e) => stub_error(&e),
    }
}

fn import(st: &ServerState, v: Value) -> Resp {
    let entries = v.get("mappings").and_then(Value::as_array).cloned().unwrap_or_default();
    let opts = v.get("importOptions").cloned().unwrap_or_else(|| json!({}));
    let ignore_dupes = opts.get("duplicatePolicy").and_then(Value::as_str).is_some_and(|p| p.eq_ignore_ascii_case("IGNORE"));
    let delete_others = opts.get("deleteAllNotInImport").and_then(Value::as_bool).unwrap_or(false);
    let mut stubs = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        match Stub::from_json(e, Uuid::new_v4) {
            Ok(s) => stubs.push(s),
            Err(err) => return stub_error(&format!("mapping #{}: {err}", i + 1)),
        }
    }
    let ids: HashSet<Uuid> = stubs.iter().map(|s| s.id).collect();
    if delete_others {
        let others: Vec<Uuid> = read(&st.store).all().iter().map(|s| s.id).filter(|id| !ids.contains(id)).collect();
        for id in others {
            if let Err(e) = st.remove_stub(&id) {
                return error(500, "Could not delete mapping file", Some(&e.to_string()));
            }
        }
    }
    for s in stubs {
        if ignore_dupes && read(&st.store).get(&s.id).is_some() {
            continue;
        }
        if let Err(e) = st.add_stub(s) {
            return error(500, "Could not save stub mapping", Some(&e.to_string()));
        }
    }
    ok_empty()
}

fn unmatched_stubs(st: &ServerState) -> Vec<Arc<Stub>> {
    let used: HashSet<Uuid> = lock(&st.journal).events().iter().filter_map(|e| e.stub.as_ref().map(|s| s.id)).collect();
    read(&st.store).all().iter().filter(|s| !used.contains(&s.id)).cloned().collect()
}

fn by_metadata(st: &ServerState, pattern: &Value) -> Result<Vec<Arc<Stub>>, String> {
    let p = ValuePattern::from_json(pattern)?;
    Ok(read(&st.store).all().iter().filter(|s| s.metadata.as_ref().is_some_and(|m| p.is_match(Some(&m.to_string())))).cloned().collect())
}

fn pattern_of(v: &Value) -> Result<RequestPattern, Box<Resp>> {
    RequestPattern::from_json(v).map_err(|e| Box::new(error(422, "Invalid request pattern", Some(&e))))
}

fn requests(st: &ServerState, qs: &[(String, String)]) -> Resp {
    let events = lock(&st.journal).events();
    let total = events.len();
    let since: Option<DateTime<Utc>> = q(qs, "since").and_then(crate::dates::parse_flexible);
    let unmatched = q(qs, "unmatched").is_some_and(|v| v == "true");
    let stub_filter = q(qs, "matchingStub").and_then(|s| Uuid::parse_str(s).ok());
    let limit = q(qs, "limit").and_then(|v| v.parse::<usize>().ok()).filter(|l| *l > 0).unwrap_or(usize::MAX);
    let list: Vec<Value> = events
        .iter()
        .rev()
        .filter(|e| since.is_none_or(|s| e.request.logged_at > s))
        .filter(|e| !unmatched || !e.was_matched)
        .filter(|e| stub_filter.is_none_or(|id| e.stub.as_ref().is_some_and(|s| s.id == id)))
        .take(limit)
        .map(|e| e.to_json())
        .collect();
    json_resp(200, &json!({ "requests": list, "meta": { "total": total }, "requestJournalDisabled": false }))
}

fn match_result_json(distance: f64, fields: &[crate::pattern::FieldResult]) -> Value {
    json!({ "distance": distance, "fields": fields })
}

/// Up to three stubs closest to a request, with per-field results.
pub(crate) fn near_misses_for_request(st: &ServerState, req: &MockRequest, req_json: Option<Value>) -> Vec<Value> {
    let req_json = req_json.unwrap_or_else(|| req.to_json());
    let store = read(&st.store);
    let mut scored: Vec<(f64, Arc<Stub>, Vec<crate::pattern::FieldResult>)> = store
        .all()
        .iter()
        .map(|s| {
            let mut r = s.request.evaluate(req);
            if let (Some(name), Some(required)) = (&s.scenario_name, &s.required_state) {
                let current = store.scenario_state(name).unwrap_or("Started");
                if r.matched && current != required {
                    r.fields.push(crate::pattern::FieldResult {
                        field: format!("Scenario {name}"),
                        expected: required.clone(),
                        actual: Some(current.to_string()),
                        matched: false,
                        distance: 1.0,
                        note: Some("The stub requires a different scenario state.".into()),
                    });
                }
            }
            (r.distance, s.clone(), r.fields)
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(3)
        .map(|(d, s, fields)| json!({ "request": req_json, "stubMapping": s.to_json(), "matchResult": match_result_json(d, &fields) }))
        .collect()
}

/// Up to three journal requests closest to a pattern.
fn near_misses_for_pattern(st: &ServerState, p: &RequestPattern, raw: &Value) -> Vec<Value> {
    let events = lock(&st.journal).events();
    let mut scored: Vec<(f64, Arc<ServeEvent>, Vec<crate::pattern::FieldResult>)> = events
        .into_iter()
        .map(|e| {
            let r = p.evaluate(&e.request);
            (r.distance, e, r.fields)
        })
        .collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(3)
        .map(|(d, e, fields)| json!({ "request": e.request_json(), "requestPattern": raw, "matchResult": match_result_json(d, &fields) }))
        .collect()
}

fn set_settings(st: &ServerState, v: Value, merge: bool) -> Resp {
    let v = match v {
        Value::Object(mut o) if o.len() == 1 && o.get("settings").is_some_and(Value::is_object) => o.remove("settings").unwrap_or_default(),
        other => other,
    };
    let Value::Object(new) = v else { return error(422, "Invalid settings", Some("settings must be an object")) };
    if let Some(d) = new.get("delayDistribution").filter(|d| !d.is_null())
        && let Err(e) = serde_json::from_value::<DelayDistribution>(d.clone())
    {
        return error(422, "Invalid settings", Some(&format!("delayDistribution: {e}")));
    }
    if let Some(f) = new.get("fixedDelay").filter(|d| !d.is_null())
        && f.as_u64().is_none()
    {
        return error(422, "Invalid settings", Some("fixedDelay must be a non-negative number of milliseconds"));
    }
    let mut settings = write(&st.settings);
    if !merge {
        settings.clear();
    }
    for (k, v) in new {
        if v.is_null() {
            settings.remove(&k);
        } else if merge && k == "extended" && v.is_object() && settings.get("extended").is_some_and(Value::is_object) {
            if let (Some(Value::Object(dst)), Value::Object(src)) = (settings.get_mut("extended"), v) {
                dst.extend(src);
            }
        } else {
            settings.insert(k, v);
        }
    }
    json_resp(200, &json!({ "settings": Value::Object(settings.clone()) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert!(origin_allowed("http://localhost:5173", &[]));
        assert!(origin_allowed("http://127.0.0.1:3000", &[]));
        assert!(origin_allowed("tauri://localhost", &[]));
        assert!(origin_allowed("http://tauri.localhost", &[]));
        assert!(origin_allowed("http://[::1]:8080", &[]));
        assert!(!origin_allowed("https://evil.example", &[]));
        assert!(!origin_allowed("null", &[]));
        assert!(origin_allowed("https://ui.example", &["https://ui.example".into()]));
        assert!(origin_allowed("https://anything", &["*".into()]));
    }
}
