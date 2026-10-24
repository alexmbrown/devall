//! Publishing a project's WireMock stubs as a hosted mock (PROTOCOL.md §6.1).
//!
//! The edge implements a documented WireMock subset (`relay/worker/src/hosted-mock-subset.json`,
//! shared with this module so the two never disagree). [`analyze`] checks mappings against it
//! *before* upload and lists every stub using something the edge can't do (unsupported matchers
//! never match there, unsupported response features are ignored). [`build_payload`] adds the small
//! `__files` the stubs reference.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::api::MockWarning;
use crate::error::{RelayError, Result};
use crate::protocol::Limits;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Subset {
    mapping_keys: Vec<String>,
    request_keys: Vec<String>,
    response_keys: Vec<String>,
    matchers: Vec<String>,
    matcher_options: Vec<String>,
    delay_distributions: Vec<String>,
    helpers: Vec<String>,
    max_delay_ms: u64,
    max_upload_file_bytes: u64,
}

static SUBSET: LazyLock<Subset> = LazyLock::new(|| serde_json::from_str(include_str!("../../../relay/worker/src/hosted-mock-subset.json")).expect("hosted-mock-subset.json is valid"));

/// Template helpers hosted mocks implement.
pub fn supported_helpers() -> Vec<String> {
    SUBSET.helpers.clone()
}

/// Files above this size aren't uploaded (keep hosted mocks small; the stub then answers 500).
pub fn max_upload_file_bytes() -> u64 {
    SUBSET.max_upload_file_bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockSettings {
    /// Answer CORS preflights / add `access-control-allow-origin` (browser apps can call the mock).
    pub cors: bool,
    /// Template every response (WireMock's `--global-response-templating`).
    pub global_templating: bool,
}

impl Default for MockSettings {
    fn default() -> Self {
        Self { cors: true, global_templating: false }
    }
}

/// The `PUT /v1/mocks/<name>` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockPayload {
    pub mappings: Vec<Value>,
    /// Path under `mocks/__files` → base64 content.
    pub files: BTreeMap<String, String>,
    pub settings: MockSettings,
}

/// What would be uploaded, shown before publishing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishPlan {
    pub stubs: usize,
    /// Mappings + files.
    pub bytes: u64,
    pub files: Vec<String>,
    /// `(path, reason)` for referenced files that won't be uploaded.
    pub skipped_files: Vec<(String, String)>,
    pub warnings: Vec<MockWarning>,
    /// Plan limits that this upload breaks (publishing would be refused).
    pub blocking: Vec<String>,
}

fn label(m: &Map<String, Value>, i: usize) -> String {
    m.get("name").and_then(Value::as_str).or_else(|| m.get("id").and_then(Value::as_str)).map(str::to_string).unwrap_or_else(|| format!("stub #{}", i + 1))
}

fn check_pattern(p: &Value, where_: &str, stub: &str, out: &mut Vec<MockWarning>) {
    let Some(obj) = p.as_object() else {
        out.push(MockWarning { stub: stub.into(), feature: where_.into(), message: format!("{where_} must be a pattern object — this stub never matches on hosted mocks") });
        return;
    };
    let keys: Vec<&String> = obj.keys().filter(|k| !SUBSET.matcher_options.contains(k)).collect();
    if keys.len() != 1 {
        out.push(MockWarning { stub: stub.into(), feature: where_.into(), message: format!("{where_}: one matcher per pattern — this stub never matches on hosted mocks") });
        return;
    }
    let k = keys[0].as_str();
    if !SUBSET.matchers.iter().any(|m| m == k) {
        out.push(MockWarning { stub: stub.into(), feature: format!("{where_}.{k}"), message: format!("\"{k}\" isn't supported on hosted mocks — this stub never matches there") });
        return;
    }
    match (k, &obj[k]) {
        ("and" | "or", Value::Array(items)) => {
            for (i, it) in items.iter().enumerate() {
                check_pattern(it, &format!("{where_}.{k}[{i}]"), stub, out);
            }
        }
        ("not", inner) => check_pattern(inner, &format!("{where_}.not"), stub, out),
        ("matchesJsonPath", Value::Object(o)) => {
            let rest: Map<String, Value> = o.iter().filter(|(k, _)| k.as_str() != "expression").map(|(k, v)| (k.clone(), v.clone())).collect();
            if !rest.is_empty() {
                check_pattern(&Value::Object(rest), &format!("{where_}.matchesJsonPath"), stub, out);
            }
        }
        _ => {}
    }
}

/// Helper-ish names used in `{{…}}` expressions (the first word and words after `(`).
fn template_words(t: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = t;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let inner = after[..end].trim_start_matches(['{', '~', '#', '^', ' ']).trim_end_matches(['}', '~', ' ']);
        rest = &after[end + 2..];
        if inner.starts_with('!') || inner.starts_with('/') || inner == "else" {
            continue;
        }
        let inner = inner.strip_prefix("else ").unwrap_or(inner);
        let mut first = true;
        for (i, part) in inner.split('(').enumerate() {
            if i > 0 || first {
                let w: String = part.trim_start().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if !w.is_empty() && !w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    // A path like `request.path` isn't a helper; only bare words are.
                    let next = part.trim_start()[w.len()..].chars().next();
                    if !matches!(next, Some('.') | Some('[') | Some('/')) && !["request", "parameters", "this", "true", "false", "null", "undefined"].contains(&w.as_str()) {
                        out.insert(w);
                    }
                }
            }
            first = false;
        }
    }
    out
}

fn check_templates(stub: &str, texts: &[(&str, String)], out: &mut Vec<MockWarning>) {
    for (where_, t) in texts {
        for w in template_words(t) {
            if !SUBSET.helpers.contains(&w) {
                out.push(MockWarning { stub: stub.into(), feature: (*where_).into(), message: format!("template helper \"{w}\" isn't available on hosted mocks (the stub answers 500)") });
            }
        }
    }
}

/// Lists what the edge won't do for these mappings. `global_templating` mirrors the mock setting.
pub fn analyze(mappings: &[Value], global_templating: bool) -> Vec<MockWarning> {
    let mut out = Vec::new();
    for (i, m) in mappings.iter().enumerate() {
        let Some(obj) = m.as_object() else {
            out.push(MockWarning { stub: format!("stub #{}", i + 1), feature: "mapping".into(), message: "not a JSON object".into() });
            continue;
        };
        let stub = label(obj, i);
        let warn = |feature: String, message: String, out: &mut Vec<MockWarning>| out.push(MockWarning { stub: stub.clone(), feature, message });
        for k in obj.keys() {
            if !SUBSET.mapping_keys.contains(k) {
                warn(k.clone(), format!("\"{k}\" is ignored on hosted mocks"), &mut out);
            }
        }
        if obj.get("postServeActions").is_some_and(|v| !v.is_null()) || obj.get("serveEventListeners").is_some_and(|v| !v.is_null()) {
            warn("serveEventListeners".into(), "webhooks / post-serve actions don't run on hosted mocks".into(), &mut out);
        }
        let empty = Map::new();
        let req = obj.get("request").and_then(Value::as_object).unwrap_or(&empty);
        for k in req.keys() {
            if !SUBSET.request_keys.contains(k) {
                warn(format!("request.{k}"), format!("request.{k} isn't supported — this stub never matches on hosted mocks"), &mut out);
            }
        }
        for field in ["queryParameters", "headers", "cookies", "formParameters"] {
            if let Some(map) = req.get(field).and_then(Value::as_object) {
                for (name, p) in map {
                    check_pattern(p, &format!("request.{field}.{name}"), &stub, &mut out);
                }
            }
        }
        if let Some(h) = req.get("host") {
            check_pattern(h, "request.host", &stub, &mut out);
        }
        if let Some(bps) = req.get("bodyPatterns").and_then(Value::as_array) {
            for (i, bp) in bps.iter().enumerate() {
                check_pattern(bp, &format!("request.bodyPatterns[{i}]"), &stub, &mut out);
            }
        }
        let resp = obj.get("response").and_then(Value::as_object).unwrap_or(&empty);
        for k in resp.keys() {
            if SUBSET.response_keys.contains(k) {
                continue;
            }
            let msg = match k.as_str() {
                "proxyBaseUrl" => "proxying isn't available on hosted mocks (the stub answers 501)".to_string(),
                "fault" => "faults can't be simulated at the edge (the stub answers 502)".to_string(),
                "chunkedDribbleDelay" => "chunked dribble delays are ignored on hosted mocks".to_string(),
                _ => format!("response.{k} is ignored on hosted mocks"),
            };
            warn(format!("response.{k}"), msg, &mut out);
        }
        let transformers: Vec<&str> = resp.get("transformers").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        for t in &transformers {
            if *t != "response-template" {
                warn("response.transformers".into(), format!("transformer \"{t}\" isn't available on hosted mocks"), &mut out);
            }
        }
        if resp.get("transformerParameters").and_then(Value::as_object).is_some_and(|p| p.contains_key("dataSource")) {
            warn("response.transformerParameters.dataSource".into(), "data sources (mocks/data) aren't uploaded to hosted mocks".into(), &mut out);
        }
        if let Some(t) = resp.get("delayDistribution").and_then(|d| d.get("type")).and_then(Value::as_str)
            && !SUBSET.delay_distributions.iter().any(|d| d == t)
        {
            warn("response.delayDistribution".into(), format!("delay distribution \"{t}\" is ignored"), &mut out);
        }
        if resp.get("fixedDelayMilliseconds").and_then(Value::as_u64).is_some_and(|d| d > SUBSET.max_delay_ms) {
            warn("response.fixedDelayMilliseconds".into(), format!("delays are capped at {} ms on hosted mocks", SUBSET.max_delay_ms), &mut out);
        }
        if global_templating || transformers.contains(&"response-template") {
            let mut texts: Vec<(&str, String)> = Vec::new();
            if let Some(b) = resp.get("body").and_then(Value::as_str) {
                texts.push(("response.body", b.to_string()));
            }
            if let Some(j) = resp.get("jsonBody").filter(|v| !v.is_null()) {
                texts.push(("response.jsonBody", j.to_string()));
            }
            if let Some(f) = resp.get("bodyFileName").and_then(Value::as_str) {
                texts.push(("response.bodyFileName", f.to_string()));
            }
            if let Some(hs) = resp.get("headers").and_then(Value::as_object) {
                for v in hs.values() {
                    texts.push(("response.headers", v.to_string()));
                }
            }
            check_templates(&stub, &texts, &mut out);
        }
    }
    out
}

/// Builds the upload: the mappings, the `__files` they reference (≤ [`max_upload_file_bytes`] each;
/// all small files when a `bodyFileName` is templated) and a plan with warnings and limit problems.
pub fn build_payload(mappings: Vec<Value>, files_dir: &Path, settings: MockSettings, limits: Option<&Limits>) -> Result<(MockPayload, PublishPlan)> {
    let mut warnings = analyze(&mappings, settings.global_templating);
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut templated_files = false;
    for m in &mappings {
        if let Some(f) = m.pointer("/response/bodyFileName").and_then(Value::as_str) {
            if f.contains("{{") {
                templated_files = true;
            } else {
                referenced.insert(f.replace('\\', "/").trim_start_matches('/').to_string());
            }
        }
    }
    let mut candidates: BTreeSet<String> = referenced.clone();
    if templated_files && files_dir.is_dir() {
        let mut stack = vec![files_dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(rel) = p.strip_prefix(files_dir) {
                    candidates.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    let max_file = SUBSET.max_upload_file_bytes;
    let mut files = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut bytes: u64 = mappings.iter().map(|m| serde_json::to_vec(m).map(|v| v.len() as u64).unwrap_or(0)).sum();
    for rel in candidates {
        if rel.split('/').any(|s| s == ".." || s.is_empty()) {
            skipped.push((rel, "invalid path".into()));
            continue;
        }
        let path = files_dir.join(&rel);
        match std::fs::metadata(&path) {
            Ok(md) if md.len() > max_file => {
                if referenced.contains(&rel) {
                    skipped.push((rel.clone(), format!("{} bytes is over the {max_file}-byte limit for hosted mocks", md.len())));
                }
            }
            Ok(_) => {
                let data = std::fs::read(&path)?;
                bytes += data.len() as u64;
                files.insert(rel, base64::engine::general_purpose::STANDARD.encode(&data));
            }
            Err(_) => {
                if referenced.contains(&rel) {
                    skipped.push((rel, "file not found in mocks/__files".into()));
                }
            }
        }
    }
    for (f, why) in &skipped {
        warnings.push(MockWarning { stub: "__files".into(), feature: f.clone(), message: format!("not uploaded: {why} (stubs using it answer 500)") });
    }
    let mut blocking = Vec::new();
    if let Some(l) = limits {
        if l.hosted_mocks == Some(0) {
            blocking.push("Hosted mocks aren't included in your plan.".into());
        }
        if let Some(max) = l.mock_max_stubs
            && mappings.len() as u64 > max as u64
        {
            blocking.push(format!("{} stubs is over your plan's {max} per hosted mock.", mappings.len()));
        }
        if let Some(max) = l.mock_max_bytes
            && bytes > max
        {
            blocking.push(format!("{bytes} bytes of stubs and files is over your plan's {max}."));
        }
    }
    let plan = PublishPlan { stubs: mappings.len(), bytes, files: files.keys().cloned().collect(), skipped_files: skipped, warnings, blocking };
    Ok((MockPayload { mappings, files, settings }, plan))
}

/// A hosted mock name from free text: lower-case `[a-z0-9-]`, 3–40 chars.
pub fn suggest_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let mut out = out.trim_matches('-').chars().take(40).collect::<String>().trim_end_matches('-').to_string();
    while out.len() < 3 {
        out.push_str("-mock");
        out = out.trim_start_matches('-').to_string();
    }
    out
}

/// Validates a hosted mock name like the relay does (reserved names are checked there).
pub fn check_name(name: &str) -> Result<()> {
    let ok = (3..=40).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--");
    if ok { Ok(()) } else { Err(RelayError::Config("mock names use 3–40 characters: lower-case letters, digits and inner hyphens".into())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn features(w: &[MockWarning]) -> Vec<String> {
        w.iter().map(|w| format!("{}:{}", w.stub, w.feature)).collect()
    }

    #[test]
    fn supported_mappings_pass_cleanly() {
        let m = vec![
            json!({"name": "list", "request": {"method": "GET", "urlPath": "/pets", "queryParameters": {"limit": {"matches": "\\d+"}}, "headers": {"Accept": {"contains": "json", "caseInsensitive": true}}},
                   "response": {"status": 200, "jsonBody": [{"id": 1}], "headers": {"X-Count": "{{size (jsonPath request.body '$.items')}}"}, "transformers": ["response-template"]}, "priority": 1}),
            json!({"name": "one", "request": {"urlPathTemplate": "/pets/{id}", "bodyPatterns": [{"matchesJsonPath": {"expression": "$.a", "equalTo": "1"}}, {"or": [{"contains": "x"}, {"absent": true}]}]},
                   "response": {"body": "{{#each (range 1 3)}}{{this}}{{/each}} {{now format='yyyy'}} {{randomValue length=4}} {{random 'Name.firstName'}}", "transformers": ["response-template"], "delayDistribution": {"type": "uniform", "lower": 1, "upper": 2}}}),
        ];
        assert_eq!(analyze(&m, false), vec![]);
    }

    #[test]
    fn reports_unsupported_features() {
        let m = vec![
            json!({"name": "xml", "request": {"bodyPatterns": [{"equalToXml": "<a/>"}], "multipartPatterns": []}, "response": {"status": 200}}),
            json!({"name": "proxy", "request": {}, "response": {"proxyBaseUrl": "https://x", "fault": "CONNECTION_RESET_BY_PEER", "transformers": ["response-template", "custom"], "body": "{{xPath request.body '/a'}} {{request.path}}", "fixedDelayMilliseconds": 60000}}),
            json!({"name": "data", "request": {}, "response": {"transformerParameters": {"dataSource": "users"}}, "serveEventListeners": [{"name": "webhook"}]}),
        ];
        let f = features(&analyze(&m, false));
        for want in [
            "xml:request.bodyPatterns[0].equalToXml",
            "xml:request.multipartPatterns",
            "proxy:response.proxyBaseUrl",
            "proxy:response.fault",
            "proxy:response.transformers",
            "proxy:response.body",
            "proxy:response.fixedDelayMilliseconds",
            "data:response.transformerParameters.dataSource",
            "data:serveEventListeners",
        ] {
            assert!(f.contains(&want.to_string()), "missing {want} in {f:?}");
        }
        // `request.path` is a path, not a helper; only xPath is reported.
        assert_eq!(analyze(&m, false).iter().filter(|w| w.feature == "response.body").count(), 1);
    }

    #[test]
    fn finds_template_helpers() {
        let w = template_words("{{#if (eq request.method 'GET')}}{{jsonPath request.body '$.a'}}{{else}}{{{upper x}}}{{/if}} {{! c}} {{~now~}} {{this}} {{@index}}");
        assert_eq!(w.into_iter().collect::<Vec<_>>(), vec!["eq", "if", "jsonPath", "now", "upper"]);
        assert!(supported_helpers().contains(&"jsonPath".to_string()));
    }

    #[test]
    fn builds_the_upload_with_referenced_small_files() {
        let dir = std::env::temp_dir().join(format!("devall-hosted-mock-{}", std::process::id()));
        let files = dir.join("__files");
        std::fs::create_dir_all(files.join("sub")).unwrap();
        std::fs::write(files.join("sub/ok.json"), b"{\"ok\":true}").unwrap();
        std::fs::write(files.join("big.bin"), vec![0u8; (max_upload_file_bytes() + 1) as usize]).unwrap();
        std::fs::write(files.join("unused.txt"), b"x").unwrap();
        let m = vec![
            json!({"name": "a", "request": {}, "response": {"bodyFileName": "sub/ok.json"}}),
            json!({"name": "b", "request": {}, "response": {"bodyFileName": "big.bin"}}),
            json!({"name": "c", "request": {}, "response": {"bodyFileName": "missing.json"}}),
        ];
        let limits = Limits { mock_max_stubs: Some(2), hosted_mocks: Some(1), mock_max_bytes: Some(1 << 20), ..Default::default() };
        let (payload, plan) = build_payload(m, &files, MockSettings::default(), Some(&limits)).unwrap();
        assert_eq!(payload.files.keys().collect::<Vec<_>>(), vec!["sub/ok.json"]);
        assert_eq!(plan.skipped_files.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(), vec!["big.bin", "missing.json"]);
        assert_eq!(plan.blocking.len(), 1, "3 stubs > 2");
        assert!(plan.warnings.iter().any(|w| w.feature == "big.bin"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names() {
        assert_eq!(suggest_name("My Pet Store API!"), "my-pet-store-api");
        assert_eq!(suggest_name("x"), "x-mock");
        assert!(check_name("pets-api").is_ok());
        assert!(check_name("Pets").is_err());
        assert!(check_name("a--b").is_err());
    }
}
