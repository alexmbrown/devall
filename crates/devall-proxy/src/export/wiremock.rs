//! Captured exchanges → WireMock stub mappings (`<project>/mocks/mappings/<slug>.json`), in the
//! format `crates/devall-mock` loads: `urlPath` (or `urlPathPattern` with numeric / UUID / hex /
//! ULID / date / token segments generalized, as wiremock-ui's "create stub from request" does),
//! `queryParameters` (`equalTo`, `hasExactly` for repeated keys), optional header matchers and a
//! body pattern; the real response with `jsonBody` for JSON. Keys are written in a fixed order.

use std::path::Path;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{canonical_header, decoded_body, is_sensitive_header, query_pairs};
use crate::error::Result;
use crate::model::Exchange;
use crate::pathgen::{SegKind, classify, path_pattern};
use crate::util::{header_value, is_json_type, is_text_type};

/// Response bodies up to this size are inlined as `base64Body`; larger binary ones go to
/// `mocks/__files/`.
const INLINE_BINARY_MAX: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BodyMatch {
    None,
    #[default]
    EqualToJson,
    EqualTo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StubOptions {
    pub match_query: bool,
    /// Request header names to match exactly (sensitive ones only with `includeSensitive`).
    pub match_headers: Vec<String>,
    pub body: BodyMatch,
    pub ignore_extra_elements: bool,
    /// Turn id-like path segments into regexes (`urlPathPattern`).
    pub generalize: bool,
    pub include_sensitive: bool,
    /// Sub-folder of `mocks/mappings/` (default: none).
    pub folder: String,
}

impl Default for StubOptions {
    fn default() -> Self {
        Self {
            match_query: true,
            match_headers: Vec::new(),
            body: BodyMatch::EqualToJson,
            ignore_extra_elements: true,
            generalize: true,
            include_sensitive: false,
            folder: String::new(),
        }
    }
}

const SKIP_RESPONSE_HEADERS: &[&str] =
    &["content-length", "transfer-encoding", "date", "connection", "keep-alive", "server", "vary", "content-encoding", "matched-stub-id", "alt-svc", "via"];

/// Readable path for names and file slugs: id-like segments become `{id}`.
pub(crate) fn display_path(path: &str, generalize: bool) -> String {
    if !generalize {
        return path.to_string();
    }
    path.split('/').map(|s| if classify(s) == SegKind::Literal { s.to_string() } else { "{id}".to_string() }).collect::<Vec<_>>().join("/")
}

/// Where a body that is too big to inline goes: (`bodyFileName`, bytes).
pub type BodyFile = (String, Vec<u8>);

/// The stub mapping for one exchange, plus a body file when the response must live in
/// `__files/`. `None` for exchanges without a usable response.
pub fn to_stub(ex: &Exchange, opts: &StubOptions) -> Option<(Value, Option<BodyFile>)> {
    let resp = ex.response.as_ref()?;
    if resp.status < 100 || resp.status == 101 {
        return None;
    }
    let url = url::Url::parse(&ex.request.url).ok()?;
    let path = url.path().to_string();
    let mut request = Map::new();
    request.insert("method".into(), json!(ex.request.method));
    match path_pattern(&path).filter(|_| opts.generalize) {
        Some(p) => request.insert("urlPathPattern".into(), json!(p)),
        None => request.insert("urlPath".into(), json!(path)),
    };
    if opts.match_query {
        let pairs = query_pairs(&ex.request.url);
        let mut q = Map::new();
        let mut names: Vec<&str> = Vec::new();
        for (k, _) in &pairs {
            if !names.contains(&k.as_str()) {
                names.push(k);
            }
        }
        for name in names {
            let vals: Vec<&str> = pairs.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).collect();
            let m = if vals.len() > 1 {
                json!({ "hasExactly": vals.iter().map(|v| json!({ "equalTo": v })).collect::<Vec<_>>() })
            } else {
                json!({ "equalTo": vals[0] })
            };
            q.insert(name.to_string(), m);
        }
        if !q.is_empty() {
            request.insert("queryParameters".into(), Value::Object(q));
        }
    }
    let mut hm = Map::new();
    for name in &opts.match_headers {
        if is_sensitive_header(name) && !opts.include_sensitive {
            continue;
        }
        if let Some(v) = header_value(&ex.request.headers, name) {
            hm.insert(canonical_header(name), json!({ "equalTo": v }));
        }
    }
    if !hm.is_empty() {
        request.insert("headers".into(), Value::Object(hm));
    }
    if opts.body != BodyMatch::None
        && ex.request.body.size > 0
        && !ex.request.body.truncated
        && let Some(bytes) = decoded_body(&ex.request.body, &ex.request.headers)
        && let Ok(text) = String::from_utf8(bytes)
    {
        let json_val = serde_json::from_str::<Value>(&text).ok().filter(|v| v.is_object() || v.is_array());
        let pattern = match (opts.body, json_val) {
            (BodyMatch::EqualToJson, Some(v)) => {
                let mut p = Map::new();
                p.insert("equalToJson".into(), v);
                if opts.ignore_extra_elements {
                    p.insert("ignoreExtraElements".into(), json!(true));
                }
                Value::Object(p)
            }
            _ => json!({ "equalTo": text }),
        };
        request.insert("bodyPatterns".into(), json!([pattern]));
    }

    let mut response = Map::new();
    response.insert("status".into(), json!(resp.status));
    let mut headers = Map::new();
    for (k, v) in &resp.headers {
        let lk = k.to_ascii_lowercase();
        if SKIP_RESPONSE_HEADERS.contains(&lk.as_str()) || (is_sensitive_header(&lk) && !opts.include_sensitive) {
            continue;
        }
        let key = canonical_header(k);
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
    if !headers.is_empty() {
        response.insert("headers".into(), Value::Object(headers));
    }
    let ct = header_value(&resp.headers, "content-type").unwrap_or("").to_string();
    let mut file = None;
    if let Some(body) = decoded_body(&resp.body, &resp.headers).filter(|b| !b.is_empty()) {
        let json_body = if is_json_type(&ct) || ct.is_empty() {
            serde_json::from_slice::<Value>(&body).ok().filter(|v| is_json_type(&ct) || v.is_object() || v.is_array())
        } else {
            None
        };
        if let Some(v) = json_body {
            response.insert("jsonBody".into(), v);
        } else if let Some(text) = std::str::from_utf8(&body).ok().filter(|t| is_text_type(&ct) || ct.is_empty() || !t.contains('\0')) {
            response.insert("body".into(), json!(text));
        } else if body.len() <= INLINE_BINARY_MAX {
            response.insert("base64Body".into(), json!(base64::engine::general_purpose::STANDARD.encode(&body)));
        } else {
            let ext = match ct.split(';').next().unwrap_or("").trim() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/gif" => "gif",
                "image/webp" => "webp",
                "application/pdf" => "pdf",
                _ => "bin",
            };
            let slug = devall_core::fsutil::slugify(&format!("{} {}", ex.request.method, display_path(&path, opts.generalize)));
            let name = format!("{slug}.{ext}");
            response.insert("bodyFileName".into(), json!(name));
            file = Some((name, body));
        }
    }
    let mut stub = Map::new();
    stub.insert("name".into(), json!(format!("{} {}", ex.request.method, display_path(&path, opts.generalize))));
    stub.insert("request".into(), Value::Object(request));
    stub.insert("response".into(), Value::Object(response));
    Some((Value::Object(stub), file))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenStub {
    /// Project-relative path, `/`-separated.
    pub file: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StubsResult {
    pub written: Vec<WrittenStub>,
    /// Exchanges that produced no stub, and why.
    pub skipped: Vec<String>,
}

/// Writes one mapping file per exchange (identical request matchers in one batch are written
/// once). Existing files are never overwritten: names get `-2`, `-3`, … suffixes.
pub fn write_stubs(root: &Path, exchanges: &[Exchange], opts: &StubOptions) -> Result<StubsResult> {
    let folder = devall_core::fsutil::normalize_rel(&opts.folder);
    let mappings = devall_core::fsutil::safe_join(&root.join("mocks").join("mappings"), &folder)?;
    let files_dir = root.join("mocks").join("__files");
    let mut out = StubsResult::default();
    let mut seen: Vec<String> = Vec::new();
    for ex in exchanges {
        let label = format!("{} {}", ex.request.method, ex.request.url);
        let Some((stub, body_file)) = to_stub(ex, opts) else {
            out.skipped.push(format!("{label}: no response to stub"));
            continue;
        };
        let key = stub.get("request").map(Value::to_string).unwrap_or_default();
        if seen.contains(&key) {
            out.skipped.push(format!("{label}: same request matcher as an earlier stub"));
            continue;
        }
        seen.push(key);
        if ex.response.as_ref().is_some_and(|r| r.body.truncated) {
            out.skipped.push(format!("{label}: response body was truncated by the capture limit; the stub has the captured part only"));
        }
        let name = stub.get("name").and_then(Value::as_str).unwrap_or("stub").to_string();
        let mut stub = stub;
        if let Some((file_name, bytes)) = body_file {
            std::fs::create_dir_all(&files_dir).map_err(|e| crate::error::ProxyError::IoAt { path: files_dir.clone(), source: e })?;
            let unique = devall_core::fsutil::unique_name(
                &files_dir,
                file_name.rsplit_once('.').map_or(file_name.as_str(), |(s, _)| s),
                &format!(".{}", file_name.rsplit_once('.').map_or("bin", |(_, e)| e)),
            );
            devall_core::fsutil::atomic_write(&files_dir.join(&unique), &bytes)?;
            if let Some(r) = stub.get_mut("response").and_then(Value::as_object_mut) {
                r.insert("bodyFileName".into(), json!(unique));
            }
        }
        std::fs::create_dir_all(&mappings).map_err(|e| crate::error::ProxyError::IoAt { path: mappings.clone(), source: e })?;
        let file = devall_core::fsutil::unique_name(&mappings, &devall_core::fsutil::slugify(&name), ".json");
        let mut text = serde_json::to_string_pretty(&stub)?;
        text.push('\n');
        devall_core::fsutil::atomic_write(&mappings.join(&file), text.as_bytes())?;
        let rel = if folder.is_empty() { format!("mocks/mappings/{file}") } else { format!("mocks/mappings/{folder}/{file}") };
        out.written.push(WrittenStub { file: rel, name });
    }
    Ok(out)
}
