//! Record & snapshot: turn proxied (or, on request, any) journal entries into stubs.
//!
//! Unlike WireMock, JSON response bodies are always inlined as `jsonBody` (WireMock writes them
//! to `__files` and references them with `bodyFileName`, issue #1399). Text bodies are inlined up
//! to `extractBodyCriteria.textSizeThreshold` (default 1 MiB), binary ones as `base64Body` up to
//! `binarySizeThreshold` (default 64 KiB); larger bodies go to `mocks/__files/` when persisting.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::journal::ServeEvent;
use crate::pattern::RequestPattern;
use crate::server::{ServerState, lock};
use crate::stub::Stub;
use crate::util::{b64_encode, content_type_is_json, content_type_is_xml, looks_textual, parse_size, short_id};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RecStatus {
    #[default]
    NeverStarted,
    Recording,
    Stopped,
}

impl RecStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::NeverStarted => "NeverStarted",
            Self::Recording => "Recording",
            Self::Stopped => "Stopped",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct RecordingState {
    pub status: RecStatus,
    pub spec: Value,
    pub proxy_stub: Option<Uuid>,
    pub since_seq: u64,
}

struct Spec {
    filter: Option<RequestPattern>,
    ids: Option<Vec<Uuid>>,
    allow_non_proxied: bool,
    capture_headers: Vec<(String, bool)>,
    body_matcher: String,
    ignore_array_order: bool,
    ignore_extra: bool,
    body_case_insensitive: bool,
    text_threshold: u64,
    binary_threshold: u64,
    output_ids: bool,
    persist: bool,
    repeats_as_scenarios: bool,
    transformers: Option<Value>,
    transformer_parameters: Option<Value>,
}

fn parse_spec(v: &Value) -> Result<Spec, String> {
    let empty = Map::new();
    let obj = v.as_object().unwrap_or(&empty);
    let filters = obj.get("filters").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut pattern = filters.clone();
    pattern.remove("ids");
    pattern.remove("allowNonProxied");
    let filter = if pattern.is_empty() { None } else { Some(RequestPattern::from_json(&Value::Object(pattern)).map_err(|e| format!("filters: {e}"))?) };
    let ids = filters.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter_map(|s| Uuid::parse_str(s).ok()).collect());
    let capture_headers = obj
        .get("captureHeaders")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.get("caseInsensitive").and_then(Value::as_bool).unwrap_or(false))).collect())
        .unwrap_or_default();
    let rbp = obj.get("requestBodyPattern").and_then(Value::as_object).cloned().unwrap_or_default();
    let flag = |m: &Map<String, Value>, k: &str, d: bool| m.get(k).and_then(Value::as_bool).unwrap_or(d);
    let extract = obj.get("extractBodyCriteria").and_then(Value::as_object).cloned().unwrap_or_default();
    let threshold = |k: &str, d: u64| -> Result<u64, String> {
        match extract.get(k).filter(|v| !v.is_null()) {
            Some(Value::Number(n)) => Ok(n.as_u64().unwrap_or(d)),
            Some(Value::String(s)) => parse_size(s).ok_or_else(|| format!("invalid {k} {s:?}")),
            _ => Ok(d),
        }
    };
    Ok(Spec {
        filter,
        ids,
        allow_non_proxied: flag(&filters, "allowNonProxied", false),
        capture_headers,
        body_matcher: rbp.get("matcher").and_then(Value::as_str).unwrap_or("auto").to_string(),
        ignore_array_order: flag(&rbp, "ignoreArrayOrder", true),
        ignore_extra: flag(&rbp, "ignoreExtraElements", true),
        body_case_insensitive: flag(&rbp, "caseInsensitive", false),
        text_threshold: threshold("textSizeThreshold", 1024 * 1024)?,
        binary_threshold: threshold("binarySizeThreshold", 64 * 1024)?,
        output_ids: obj.get("outputFormat").and_then(Value::as_str).is_some_and(|f| f.eq_ignore_ascii_case("IDS")),
        persist: flag(obj, "persist", true),
        repeats_as_scenarios: flag(obj, "repeatsAsScenarios", true),
        transformers: obj.get("transformers").filter(|v| !v.is_null()).cloned(),
        transformer_parameters: obj.get("transformerParameters").filter(|v| !v.is_null()).cloned(),
    })
}

const SKIP_RESPONSE_HEADERS: &[&str] = &["content-length", "content-encoding", "transfer-encoding", "connection", "keep-alive", "date"];

fn request_json(e: &ServeEvent, spec: &Spec) -> Value {
    let req = &e.request;
    let mut m = Map::new();
    m.insert("url".into(), json!(req.url));
    m.insert("method".into(), json!(req.method));
    let mut headers = Map::new();
    for (name, ci) in &spec.capture_headers {
        if let Some(v) = req.header(name) {
            let mut p = Map::new();
            p.insert("equalTo".into(), json!(v));
            if *ci {
                p.insert("caseInsensitive".into(), json!(true));
            }
            headers.insert(name.clone(), Value::Object(p));
        }
    }
    if !headers.is_empty() {
        m.insert("headers".into(), Value::Object(headers));
    }
    if !req.body.is_empty() {
        let ct = req.content_type().unwrap_or("");
        let text = std::str::from_utf8(&req.body).ok();
        let pattern = match (spec.body_matcher.as_str(), text) {
            (_, None) => json!({ "binaryEqualTo": b64_encode(&req.body) }),
            ("equalTo", Some(t)) => json!({ "equalTo": t, "caseInsensitive": spec.body_case_insensitive }),
            ("equalToXml", Some(t)) => json!({ "equalToXml": t }),
            ("equalToJson", Some(t)) => json!({ "equalToJson": t, "ignoreArrayOrder": spec.ignore_array_order, "ignoreExtraElements": spec.ignore_extra }),
            (_, Some(t)) => {
                if content_type_is_json(ct) || serde_json::from_str::<Value>(t).is_ok_and(|v| v.is_object() || v.is_array()) {
                    json!({ "equalToJson": t, "ignoreArrayOrder": spec.ignore_array_order, "ignoreExtraElements": spec.ignore_extra })
                } else if content_type_is_xml(ct) {
                    json!({ "equalToXml": t })
                } else {
                    json!({ "equalTo": t })
                }
            }
        };
        m.insert("bodyPatterns".into(), json!([pattern]));
    }
    Value::Object(m)
}

fn ext_for(ct: &str, textual: bool) -> &'static str {
    let ct = ct.to_ascii_lowercase();
    if content_type_is_json(&ct) {
        "json"
    } else if content_type_is_xml(&ct) {
        "xml"
    } else if ct.contains("html") {
        "html"
    } else if ct.starts_with("image/png") {
        "png"
    } else if ct.starts_with("image/jpeg") {
        "jpg"
    } else if textual {
        "txt"
    } else {
        "bin"
    }
}

fn response_json(st: &ServerState, e: &ServeEvent, spec: &Spec, id: &Uuid) -> Result<Value, String> {
    let r = &e.response;
    let mut m = Map::new();
    m.insert("status".into(), json!(r.status));
    let mut headers = Map::new();
    for (k, v) in &r.headers {
        if SKIP_RESPONSE_HEADERS.contains(&k.to_ascii_lowercase().as_str()) {
            continue;
        }
        let key = crate::util::canonical_header_name(k);
        match headers.get_mut(&key) {
            None => {
                headers.insert(key, json!(v));
            }
            Some(Value::Array(a)) => a.push(json!(v)),
            Some(existing) => {
                let first = existing.take();
                *existing = json!([first, v]);
            }
        }
    }
    let ct = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map_or("", |(_, v)| v.as_str()).to_string();
    if !r.body.is_empty() {
        let textual = looks_textual(&r.body, Some(&ct));
        let json_body = if textual && (content_type_is_json(&ct) || ct.is_empty()) {
            serde_json::from_slice::<Value>(&r.body).ok().filter(|v| content_type_is_json(&ct) || v.is_object() || v.is_array())
        } else {
            None
        };
        let size = r.body.len() as u64;
        if let Some(j) = json_body {
            m.insert("jsonBody".into(), j);
        } else if textual && size <= spec.text_threshold {
            m.insert("body".into(), json!(String::from_utf8_lossy(&r.body)));
        } else if !textual && size <= spec.binary_threshold {
            m.insert("base64Body".into(), json!(b64_encode(&r.body)));
        } else if spec.persist {
            let stem = devall_core::fsutil::slugify(&format!("{} {}", e.request.method, e.request.path));
            let name = format!("{stem}-{}.{}", short_id(id), ext_for(&ct, textual));
            st.files.write_file(&name, &r.body).map_err(|e| e.to_string())?;
            m.insert("bodyFileName".into(), json!(name));
        } else if textual {
            m.insert("body".into(), json!(String::from_utf8_lossy(&r.body)));
        } else {
            m.insert("base64Body".into(), json!(b64_encode(&r.body)));
        }
    }
    if !headers.is_empty() {
        m.insert("headers".into(), Value::Object(headers));
    }
    if let Some(t) = &spec.transformers {
        m.insert("transformers".into(), t.clone());
    }
    if let Some(p) = &spec.transformer_parameters {
        m.insert("transformerParameters".into(), p.clone());
    }
    Ok(Value::Object(m))
}

/// Builds stubs from serve events (oldest first) according to a record spec.
fn generate(st: &ServerState, events: &[Arc<ServeEvent>], spec: &Spec, proxy_stub: Option<Uuid>) -> Result<Vec<Stub>, String> {
    let selected: Vec<&Arc<ServeEvent>> = events
        .iter()
        .filter(|e| spec.allow_non_proxied || e.proxied_from.is_some())
        .filter(|e| proxy_stub.is_none_or(|id| e.stub.as_ref().is_some_and(|s| s.id == id)))
        .filter(|e| spec.filter.as_ref().is_none_or(|f| f.is_match(&e.request)))
        .filter(|e| spec.ids.as_ref().is_none_or(|ids| ids.contains(&e.id)))
        .filter(|e| e.response.fault.is_none())
        .collect();
    // Group identical requests, keeping first-seen order.
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, (Value, Vec<&Arc<ServeEvent>>)> = HashMap::new();
    for e in selected {
        let rj = request_json(e, spec);
        let key = rj.to_string();
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_insert_with(|| (rj, Vec::new())).1.push(e);
    }
    let mut stubs = Vec::new();
    let mut scenario_counter = 0;
    for key in order {
        let Some((rj, evs)) = groups.remove(&key) else { continue };
        let evs = if spec.repeats_as_scenarios { evs } else { evs.into_iter().take(1).collect() };
        let scenario = if evs.len() > 1 {
            scenario_counter += 1;
            let slug = devall_core::fsutil::slugify(&evs[0].request.path);
            Some(format!("scenario-{scenario_counter}-{slug}"))
        } else {
            None
        };
        let n = evs.len();
        for (i, e) in evs.into_iter().enumerate() {
            let id = Uuid::new_v4();
            let mut m = Map::new();
            m.insert("id".into(), json!(id.to_string()));
            m.insert("name".into(), json!(format!("{} {}", e.request.method, e.request.path)));
            m.insert("request".into(), rj.clone());
            m.insert("response".into(), response_json(st, e, spec, &id)?);
            if spec.persist {
                m.insert("persistent".into(), json!(true));
            }
            if let Some(name) = &scenario {
                m.insert("scenarioName".into(), json!(name));
                let state = |k: usize| if k == 0 { "Started".to_string() } else { format!("{name}-{}", k + 1) };
                m.insert("requiredScenarioState".into(), json!(state(i)));
                if i + 1 < n {
                    m.insert("newScenarioState".into(), json!(state(i + 1)));
                }
            }
            stubs.push(Stub::from_json(&Value::Object(m), Uuid::new_v4)?);
        }
    }
    Ok(stubs)
}

fn install(st: &ServerState, stubs: Vec<Stub>, spec: &Spec) -> Result<Value, String> {
    let mut out = Vec::new();
    for s in stubs {
        let arc = st.add_stub(s).map_err(|e| e.to_string())?;
        out.push(arc);
    }
    Ok(if spec.output_ids {
        json!({ "ids": out.iter().map(|s| s.id.to_string()).collect::<Vec<_>>() })
    } else {
        json!({ "mappings": out.iter().map(|s| s.to_json()).collect::<Vec<_>>() })
    })
}

pub(crate) fn start(st: &ServerState, spec: Value) -> Result<(), String> {
    let target = spec.get("targetBaseUrl").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("targetBaseUrl is required")?.to_string();
    url::Url::parse(&target).map_err(|e| format!("invalid targetBaseUrl: {e}"))?;
    parse_spec(&spec)?;
    let mut rec = lock(&st.recording);
    if rec.status == RecStatus::Recording {
        return Err("Recording is already in progress".into());
    }
    let proxy = json!({
        "request": { "method": "ANY", "urlPattern": ".*" },
        "response": { "proxyBaseUrl": target },
        "metadata": { "devall": { "recordingProxy": true } }
    });
    let stub = Stub::from_json(&proxy, Uuid::new_v4)?;
    let id = st.add_stub(stub).map_err(|e| e.to_string())?.id;
    rec.status = RecStatus::Recording;
    rec.spec = spec;
    rec.proxy_stub = Some(id);
    rec.since_seq = lock(&st.journal).next_seq();
    Ok(())
}

pub(crate) fn stop(st: &ServerState) -> Result<Value, String> {
    let (spec_v, proxy_id, since) = {
        let mut rec = lock(&st.recording);
        if rec.status != RecStatus::Recording {
            return Err("Recording has not been started".into());
        }
        rec.status = RecStatus::Stopped;
        (rec.spec.clone(), rec.proxy_stub.take(), rec.since_seq)
    };
    if let Some(id) = proxy_id {
        st.remove_stub(&id).map_err(|e| e.to_string())?;
    }
    let spec = parse_spec(&spec_v)?;
    let events: Vec<Arc<ServeEvent>> = lock(&st.journal).events().into_iter().filter(|e| e.seq >= since).collect();
    let stubs = generate(st, &events, &spec, proxy_id)?;
    install(st, stubs, &spec)
}

pub(crate) fn snapshot(st: &ServerState, spec_v: &Value) -> Result<Value, String> {
    let spec = parse_spec(spec_v)?;
    let events = lock(&st.journal).events();
    let stubs = generate(st, &events, &spec, None)?;
    install(st, stubs, &spec)
}

pub(crate) fn status(st: &ServerState) -> Value {
    json!({ "status": lock(&st.recording).status.name() })
}
