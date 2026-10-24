//! Format auto-detection from content (and, for folders and some files, from the path).

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::{str_at, yaml_to_json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    /// Postman Collection v2.0 / v2.1 (JSON).
    Postman,
    /// Postman environment or globals export (JSON).
    PostmanEnvironment,
    /// Insomnia v4 export (JSON) or v5 collection (YAML).
    Insomnia,
    /// Bruno collection folder (`.bru` files) or a single `.bru` request.
    Bruno,
    /// OpenCollection YAML (Bruno ≥ 3.1): a bundled file or a collection folder.
    OpenCollection,
    /// OpenAPI 3.0 / 3.1 (JSON or YAML).
    OpenApi,
    /// Swagger 2.0 (JSON or YAML).
    Swagger,
    /// HTTP Archive 1.2 (browser dev tools, proxies).
    Har,
    /// `.http` / `.rest` files (JetBrains HTTP Client, VS Code REST Client).
    HttpFile,
    /// Hurl files.
    Hurl,
    /// One or more cURL commands.
    Curl,
    /// A WebAssembly plugin's import format (parsed by the app / CLI through `devall-plugins`;
    /// never returned by [`detect`]).
    Plugin,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Postman => "Postman collection",
            Format::PostmanEnvironment => "Postman environment",
            Format::Insomnia => "Insomnia",
            Format::Bruno => "Bruno",
            Format::OpenCollection => "OpenCollection",
            Format::OpenApi => "OpenAPI",
            Format::Swagger => "Swagger 2.0",
            Format::Har => "HAR",
            Format::HttpFile => ".http file",
            Format::Hurl => "Hurl",
            Format::Curl => "cURL",
            Format::Plugin => "Plugin format",
        }
    }
}

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT", "QUERY"];

/// Detects the format of `text`. Structured formats are recognized by their markers, text
/// formats (.http, Hurl, cURL, .bru) by their syntax.
pub fn detect(text: &str) -> Option<Format> {
    let t = text.trim_start_matches('\u{feff}').trim();
    if t.is_empty() {
        return None;
    }
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return detect_value(&v);
    }
    if let Some(f) = detect_text(t) {
        return Some(f);
    }
    // YAML last: almost any text is valid YAML (a plain scalar), so only mappings count.
    if let Ok(y) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(t)
        && y.is_mapping()
    {
        return detect_value(&yaml_to_json(y));
    }
    None
}

/// Detects the format of a file or folder. Folders are Bruno or OpenCollection collections.
pub fn detect_path(path: &Path) -> Option<Format> {
    if path.is_dir() {
        if ["opencollection.yml", "opencollection.yaml"].iter().any(|f| path.join(f).is_file()) {
            return Some(Format::OpenCollection);
        }
        if path.join("bruno.json").is_file() || path.join("collection.bru").is_file() || has_ext_below(path, "bru", 3) {
            return Some(Format::Bruno);
        }
        if has_ext_below(path, "yml", 1) || has_ext_below(path, "yaml", 1) {
            return Some(Format::OpenCollection);
        }
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "bru" => return Some(Format::Bruno),
        "har" => return Some(Format::Har),
        "http" | "rest" => return Some(Format::HttpFile),
        "hurl" => return Some(Format::Hurl),
        _ => {}
    }
    let text = std::fs::read_to_string(path).ok()?;
    detect(&text)
}

fn has_ext_below(dir: &Path, ext: &str, depth: usize) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 && !e.file_name().to_string_lossy().starts_with('.') && has_ext_below(&p, ext, depth - 1) {
                return true;
            }
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
            return true;
        }
    }
    false
}

pub(crate) fn detect_value(v: &Value) -> Option<Format> {
    let o = v.as_object()?;
    if o.get("openapi").is_some_and(|x| x.is_string()) {
        return Some(Format::OpenApi);
    }
    if o.contains_key("swagger") {
        return Some(Format::Swagger);
    }
    if let Some(info) = o.get("info") {
        let schema = str_at(info, "schema");
        if schema.contains("getpostman.com") || schema.contains("postman") || info.get("_postman_id").is_some() {
            return Some(Format::Postman);
        }
    }
    if o.contains_key("_postman_variable_scope") || (o.contains_key("values") && o.contains_key("name") && !o.contains_key("info")) {
        return Some(Format::PostmanEnvironment);
    }
    if o.get("_type").and_then(Value::as_str) == Some("export") || str_at(v, "type").contains("insomnia.rest") {
        return Some(Format::Insomnia);
    }
    if o.contains_key("opencollection") || (o.contains_key("info") && (o.contains_key("http") || o.contains_key("graphql"))) {
        return Some(Format::OpenCollection);
    }
    if o.get("log").and_then(|l| l.get("entries")).is_some() {
        return Some(Format::Har);
    }
    if o.contains_key("info") && o.get("item").is_some_and(Value::is_array) {
        return Some(Format::Postman);
    }
    if str_at(v, "type") == "collection" && o.contains_key("name") && o.contains_key("version") {
        return Some(Format::Bruno); // bruno.json — the folder is what gets imported
    }
    None
}

fn detect_text(t: &str) -> Option<Format> {
    let lines: Vec<&str> = t.lines().collect();
    let first = lines.iter().map(|l| l.trim()).find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))?;
    let lower = first.to_ascii_lowercase();
    if lower.starts_with("curl ") || lower.starts_with("curl.exe ") || lower == "curl" {
        return Some(Format::Curl);
    }
    let is_bru_method = |l: &str| {
        let l = l.trim_end();
        l.strip_suffix('{').map(str::trim_end).is_some_and(|m| {
            ["get", "post", "put", "delete", "patch", "options", "head", "connect", "trace", "http", "graphql"].contains(&m)
        })
    };
    if lines.iter().any(|l| l.trim_end() == "meta {") && lines.iter().any(|l| is_bru_method(l)) {
        return Some(Format::Bruno);
    }
    let mut request_line = false;
    let mut http_markers = false;
    for l in &lines {
        let l = l.trim_end();
        if is_request_line(l) {
            request_line = true;
        }
        if is_hurl_marker(l) {
            return Some(Format::Hurl);
        }
        if l.starts_with("###") || (l.starts_with('@') && l.contains('=')) || l.trim_start().starts_with("# @name") || l.trim_start().starts_with("// @name") {
            http_markers = true;
        }
    }
    if request_line || http_markers {
        return Some(Format::HttpFile);
    }
    None
}

pub(crate) fn is_request_line(l: &str) -> bool {
    let Some((m, rest)) = l.split_once(' ') else { return false };
    let rest = rest.trim();
    // `{` alone is JSON; `{{baseUrl}}/x` is a templated URL.
    METHODS.contains(&m) && !rest.is_empty() && (!rest.starts_with('{') || rest.starts_with("{{"))
}

pub(crate) fn is_hurl_marker(l: &str) -> bool {
    const SECTIONS: &[&str] = &[
        "[Asserts]",
        "[Captures]",
        "[QueryStringParams]",
        "[Query]",
        "[FormParams]",
        "[Form]",
        "[MultipartFormData]",
        "[Multipart]",
        "[Cookies]",
        "[BasicAuth]",
        "[Options]",
    ];
    if SECTIONS.contains(&l.trim()) {
        return true;
    }
    // Response line: `HTTP 200`, `HTTP/1.1 404`, `HTTP *`.
    let Some(rest) = l.strip_prefix("HTTP") else { return false };
    let rest = rest.strip_prefix("/1.0").or_else(|| rest.strip_prefix("/1.1")).or_else(|| rest.strip_prefix("/2")).or_else(|| rest.strip_prefix("/3")).unwrap_or(rest);
    let Some(code) = rest.strip_prefix(' ') else { return false };
    let code = code.trim();
    code == "*" || (code.len() == 3 && code.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_structured_formats() {
        assert_eq!(detect(r#"{"info":{"name":"x","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[]}"#), Some(Format::Postman));
        assert_eq!(detect(r#"{"name":"Dev","values":[],"_postman_variable_scope":"environment"}"#), Some(Format::PostmanEnvironment));
        assert_eq!(detect(r#"{"_type":"export","__export_format":4,"resources":[]}"#), Some(Format::Insomnia));
        assert_eq!(detect("type: collection.insomnia.rest/5.0\nname: x\ncollection: []\n"), Some(Format::Insomnia));
        assert_eq!(detect("openapi: 3.1.0\ninfo:\n  title: x\n  version: '1'\npaths: {}\n"), Some(Format::OpenApi));
        assert_eq!(detect(r#"{"swagger":"2.0","info":{},"paths":{}}"#), Some(Format::Swagger));
        assert_eq!(detect(r#"{"log":{"version":"1.2","entries":[]}}"#), Some(Format::Har));
        assert_eq!(detect("opencollection: 1.0.0\ninfo:\n  name: x\nitems: []\n"), Some(Format::OpenCollection));
    }

    #[test]
    fn detects_text_formats() {
        assert_eq!(detect("curl https://x.io -H 'A: b'"), Some(Format::Curl));
        assert_eq!(detect("### Get\nGET https://x.io/a\nAccept: */*\n"), Some(Format::HttpFile));
        assert_eq!(detect("@host = x.io\n\nGET https://{{host}}/a\n"), Some(Format::HttpFile));
        assert_eq!(detect("GET https://x.io/a\nHTTP 200\n[Asserts]\njsonpath \"$.a\" == 1\n"), Some(Format::Hurl));
        assert_eq!(detect("meta {\n  name: A\n  type: http\n  seq: 1\n}\n\nget {\n  url: https://x.io\n}\n"), Some(Format::Bruno));
        assert_eq!(detect("just some prose"), None);
        assert_eq!(detect(""), None);
    }
}
