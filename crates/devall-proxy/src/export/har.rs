//! HAR 1.2 export and import. Exports redact `Authorization`, `Cookie`, `Set-Cookie` (and the
//! other [`SENSITIVE_HEADERS`](super::SENSITIVE_HEADERS)) unless sensitive values are requested.
//! WebSocket frames use Chrome's `_webSocketMessages` extension.

use base64::Engine;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{REDACTED, decoded_body, is_sensitive_header, query_pairs};
use crate::error::{ProxyError, Result};
use crate::model::{CapturedBody, CapturedResponse, Exchange, ExchangeState, HeaderList, Timing, WsDirection, WsFrame};
use crate::util::{header_value, mime_essence};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HarOptions {
    /// Keep `Authorization`, cookies and API-key headers as captured.
    pub include_sensitive: bool,
}

fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn headers_json(headers: &HeaderList, opts: &HarOptions) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(k, v)| {
                let value = if !opts.include_sensitive && is_sensitive_header(k) { REDACTED } else { v.as_str() };
                json!({ "name": k, "value": value })
            })
            .collect(),
    )
}

fn request_cookies(headers: &HeaderList, opts: &HarOptions) -> Value {
    let mut out = Vec::new();
    for (k, v) in headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("cookie")) {
        let _ = k;
        for pair in v.split(';') {
            if let Some((n, val)) = pair.trim().split_once('=') {
                out.push(json!({ "name": n.trim(), "value": if opts.include_sensitive { val.trim() } else { REDACTED } }));
            }
        }
    }
    Value::Array(out)
}

fn response_cookies(headers: &HeaderList, opts: &HarOptions) -> Value {
    let mut out = Vec::new();
    for (_, v) in headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")) {
        let first = v.split(';').next().unwrap_or("");
        if let Some((n, val)) = first.split_once('=') {
            out.push(json!({ "name": n.trim(), "value": if opts.include_sensitive { val.trim() } else { REDACTED } }));
        }
    }
    Value::Array(out)
}

fn opt_ms(v: Option<f64>) -> Value {
    match v {
        Some(x) => json!((x * 1000.0).round() / 1000.0),
        None => json!(-1),
    }
}

fn entry(ex: &Exchange, opts: &HarOptions) -> Value {
    let req = &ex.request;
    let mut request = Map::new();
    request.insert("method".into(), json!(req.method));
    request.insert("url".into(), json!(req.url));
    request.insert("httpVersion".into(), json!(req.http_version));
    request.insert("cookies".into(), request_cookies(&req.headers, opts));
    request.insert("headers".into(), headers_json(&req.headers, opts));
    request.insert("queryString".into(), Value::Array(query_pairs(&req.url).into_iter().map(|(n, v)| json!({ "name": n, "value": v })).collect()));
    if req.body.size > 0 {
        let mime = header_value(&req.headers, "content-type").unwrap_or("application/octet-stream").to_string();
        let mut post = Map::new();
        post.insert("mimeType".into(), json!(mime));
        match decoded_body(&req.body, &req.headers) {
            Some(b) => match String::from_utf8(b) {
                Ok(t) => {
                    post.insert("text".into(), json!(t));
                }
                Err(e) => {
                    post.insert("text".into(), json!(base64::engine::general_purpose::STANDARD.encode(e.as_bytes())));
                    post.insert("_encoding".into(), json!("base64"));
                }
            },
            None => {
                post.insert("text".into(), json!(""));
                post.insert("comment".into(), json!("body not captured"));
            }
        }
        request.insert("postData".into(), Value::Object(post));
    }
    request.insert("headersSize".into(), json!(-1));
    request.insert("bodySize".into(), json!(req.body.size));

    let mut response = Map::new();
    match &ex.response {
        Some(r) => {
            response.insert("status".into(), json!(r.status));
            response.insert("statusText".into(), json!(r.reason));
            response.insert("httpVersion".into(), json!(r.http_version));
            response.insert("cookies".into(), response_cookies(&r.headers, opts));
            response.insert("headers".into(), headers_json(&r.headers, opts));
            let mime = header_value(&r.headers, "content-type").unwrap_or("").to_string();
            let mut content = Map::new();
            match decoded_body(&r.body, &r.headers) {
                Some(b) => {
                    content.insert("size".into(), json!(b.len()));
                    if (b.len() as u64) > r.body.size {
                        content.insert("compression".into(), json!(b.len() as u64 - r.body.size));
                    }
                    content.insert("mimeType".into(), json!(mime));
                    match String::from_utf8(b) {
                        Ok(t) => {
                            content.insert("text".into(), json!(t));
                        }
                        Err(e) => {
                            content.insert("text".into(), json!(base64::engine::general_purpose::STANDARD.encode(e.as_bytes())));
                            content.insert("encoding".into(), json!("base64"));
                        }
                    }
                }
                None => {
                    content.insert("size".into(), json!(r.body.size));
                    content.insert("mimeType".into(), json!(mime));
                    content.insert("comment".into(), json!("body not captured"));
                }
            }
            if r.body.truncated {
                content.insert("comment".into(), json!("body truncated by the capture limit"));
            }
            response.insert("content".into(), Value::Object(content));
            response.insert("redirectURL".into(), json!(header_value(&r.headers, "location").unwrap_or("")));
            response.insert("headersSize".into(), json!(-1));
            response.insert("bodySize".into(), json!(r.body.size));
        }
        None => {
            response.insert("status".into(), json!(0));
            response.insert("statusText".into(), json!(""));
            response.insert("httpVersion".into(), json!(req.http_version));
            response.insert("cookies".into(), json!([]));
            response.insert("headers".into(), json!([]));
            response.insert("content".into(), json!({ "size": 0, "mimeType": "" }));
            response.insert("redirectURL".into(), json!(""));
            response.insert("headersSize".into(), json!(-1));
            response.insert("bodySize".into(), json!(-1));
            if let Some(err) = &ex.error {
                response.insert("_error".into(), json!(err));
            }
        }
    }

    let t = &ex.timing;
    let connect = match (t.connect_ms, t.tls_ms) {
        (Some(c), tls) => Some(c + tls.unwrap_or(0.0)),
        _ => None,
    };
    let timings = json!({
        "blocked": opt_ms(t.blocked_ms),
        "dns": opt_ms(t.dns_ms),
        "connect": opt_ms(connect),
        "send": 0,
        "wait": t.wait_ms.map_or(json!(0), |w| json!((w * 1000.0).round() / 1000.0)),
        "receive": t.receive_ms.map_or(json!(0), |w| json!((w * 1000.0).round() / 1000.0)),
        "ssl": opt_ms(t.tls_ms),
    });
    let mut e = Map::new();
    e.insert("startedDateTime".into(), json!(iso(ex.started_at)));
    e.insert("time".into(), json!(t.total_ms.map_or(0.0, |x| (x * 1000.0).round() / 1000.0)));
    e.insert("request".into(), Value::Object(request));
    e.insert("response".into(), Value::Object(response));
    e.insert("cache".into(), json!({}));
    e.insert("timings".into(), timings);
    if let Some(addr) = &ex.server_addr {
        let ip = addr.parse::<std::net::SocketAddr>().map(|a| a.ip().to_string()).unwrap_or_else(|_| addr.clone());
        e.insert("serverIPAddress".into(), json!(ip));
    }
    if !ex.ws_frames.is_empty() {
        let start = ex.started_at as f64 / 1000.0;
        let msgs: Vec<Value> = ex
            .ws_frames
            .iter()
            .map(|f| {
                let opcode = match f.opcode.as_str() {
                    "text" => 1,
                    "binary" => 2,
                    "close" => 8,
                    "ping" => 9,
                    "pong" => 10,
                    _ => 0,
                };
                json!({
                    "type": if f.direction == WsDirection::Send { "send" } else { "receive" },
                    "time": start + f.at_ms / 1000.0,
                    "opcode": opcode,
                    "data": f.text.clone().or_else(|| f.base64.clone()).unwrap_or_default(),
                })
            })
            .collect();
        e.insert("_resourceType".into(), json!("websocket"));
        e.insert("_webSocketMessages".into(), Value::Array(msgs));
    }
    if !ex.tags.is_empty() {
        e.insert("comment".into(), json!(ex.tags.join(", ")));
    }
    Value::Object(e)
}

/// HAR 1.2 for the given exchanges (tunnels and TLS failures are skipped: they are not HTTP).
pub fn to_har(exchanges: &[Exchange], opts: &HarOptions) -> Value {
    let entries: Vec<Value> = exchanges.iter().filter(|e| e.state != ExchangeState::Tunnel && e.request.method != "CONNECT").map(|e| entry(e, opts)).collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "devall", "version": env!("CARGO_PKG_VERSION") },
            "pages": [],
            "entries": entries,
        }
    })
}

pub fn to_har_string(exchanges: &[Exchange], opts: &HarOptions) -> String {
    let mut s = serde_json::to_string_pretty(&to_har(exchanges, opts)).unwrap_or_default();
    s.push('\n');
    s
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn headers_of(v: &Value) -> HeaderList {
    v.get("headers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter().filter(|h| !str_of(h, "name").starts_with(':')).map(|h| (str_of(h, "name").to_ascii_lowercase(), str_of(h, "value").to_string())).collect()
        })
        .unwrap_or_default()
}

fn ms_of(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|x| *x >= 0.0)
}

/// Parses a HAR file into exchanges (bodies are stored decoded, so `Content-Encoding` is
/// dropped). Each gets the tag `imported`.
pub fn from_har(text: &str) -> Result<Vec<Exchange>> {
    let v: Value = serde_json::from_str(text)?;
    let entries = v.pointer("/log/entries").and_then(Value::as_array).ok_or_else(|| ProxyError::Invalid("not a HAR file (no log.entries)".into()))?;
    let mut out = Vec::new();
    for e in entries {
        let req = e.get("request").cloned().unwrap_or(Value::Null);
        let url = str_of(&req, "url");
        if url.is_empty() {
            continue;
        }
        let mut ex = Exchange::new(str_of(&req, "method").to_ascii_uppercase(), url, str_of(&req, "httpVersion"), headers_of(&req));
        ex.request.headers.retain(|(k, _)| k != "content-encoding");
        if let Some(t) = e.get("startedDateTime").and_then(Value::as_str).and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) {
            ex.started_at = t.timestamp_millis();
        }
        if let Some(text) = req.pointer("/postData/text").and_then(Value::as_str) {
            let bytes = if req.pointer("/postData/_encoding").and_then(Value::as_str) == Some("base64") {
                base64::engine::general_purpose::STANDARD.decode(text).unwrap_or_default()
            } else {
                text.as_bytes().to_vec()
            };
            ex.request.body = CapturedBody::new(Bytes::from(bytes), usize::MAX);
        }
        let resp = e.get("response").cloned().unwrap_or(Value::Null);
        let status = resp.get("status").and_then(Value::as_u64).unwrap_or(0) as u16;
        if status > 0 {
            let mut headers = headers_of(&resp);
            headers.retain(|(k, _)| k != "content-encoding");
            let content = resp.get("content").cloned().unwrap_or(Value::Null);
            let text = str_of(&content, "text");
            let bytes = if str_of(&content, "encoding") == "base64" {
                base64::engine::general_purpose::STANDARD.decode(text).unwrap_or_default()
            } else {
                text.as_bytes().to_vec()
            };
            if header_value(&headers, "content-type").is_none() && !str_of(&content, "mimeType").is_empty() {
                headers.push(("content-type".into(), mime_essence(str_of(&content, "mimeType"))));
            }
            ex.response = Some(CapturedResponse {
                status,
                reason: str_of(&resp, "statusText").to_string(),
                http_version: str_of(&resp, "httpVersion").to_string(),
                headers,
                body: CapturedBody::new(Bytes::from(bytes), usize::MAX),
            });
            ex.state = ExchangeState::Complete;
        } else {
            ex.state = ExchangeState::Failed;
            ex.error = resp.get("_error").and_then(Value::as_str).map(str::to_string).or(Some("no response".into()));
        }
        let t = e.get("timings").cloned().unwrap_or(Value::Null);
        let ssl = ms_of(&t, "ssl");
        ex.timing = Timing {
            blocked_ms: ms_of(&t, "blocked"),
            dns_ms: ms_of(&t, "dns"),
            connect_ms: ms_of(&t, "connect").map(|c| (c - ssl.unwrap_or(0.0)).max(0.0)),
            tls_ms: ssl,
            wait_ms: ms_of(&t, "wait"),
            receive_ms: ms_of(&t, "receive"),
            total_ms: e.get("time").and_then(Value::as_f64),
            connection_reused: None,
        };
        ex.server_addr = e.get("serverIPAddress").and_then(Value::as_str).map(str::to_string);
        if let Some(msgs) = e.get("_webSocketMessages").and_then(Value::as_array) {
            let start = ex.started_at as f64 / 1000.0;
            for m in msgs {
                let data = str_of(m, "data").to_string();
                let opcode = match m.get("opcode").and_then(Value::as_u64).unwrap_or(1) {
                    2 => "binary",
                    8 => "close",
                    9 => "ping",
                    10 => "pong",
                    _ => "text",
                };
                ex.ws_frames.push(WsFrame {
                    direction: if str_of(m, "type") == "send" { WsDirection::Send } else { WsDirection::Receive },
                    at_ms: ((m.get("time").and_then(Value::as_f64).unwrap_or(start) - start) * 1000.0).max(0.0),
                    opcode: opcode.into(),
                    size: data.len(),
                    text: (opcode != "binary").then(|| data.clone()),
                    base64: (opcode == "binary").then_some(data),
                    truncated: false,
                });
            }
            ex.ws_frame_count = ex.ws_frames.len();
            if !ex.ws_frames.is_empty() {
                ex.state = ExchangeState::WebSocket;
            }
        }
        ex.tag("imported");
        out.push(ex);
    }
    Ok(out)
}
