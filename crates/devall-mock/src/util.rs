//! Small shared helpers: Java-compatible regexes, string distance, header names, stable ids.

use base64::Engine as _;
use serde_json::Value;
use uuid::Uuid;

pub(crate) const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub(crate) fn b64_encode(data: &[u8]) -> String {
    B64.encode(data)
}

/// Lenient base64 decode: standard or URL-safe alphabet, padding optional, whitespace ignored.
pub(crate) fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let cleaned = cleaned.replace('-', "+").replace('_', "/");
    let trimmed = cleaned.trim_end_matches('=');
    base64::engine::general_purpose::STANDARD_NO_PAD.decode(trimmed).ok()
}

/// Compiles a WireMock (Java) regex: whole-string match, DOTALL, with lookaround and
/// backreference support via `fancy-regex`.
pub(crate) fn java_regex(pattern: &str) -> Result<fancy_regex::Regex, String> {
    fancy_regex::Regex::new(&format!("(?s)^(?:{pattern})$")).map_err(|e| format!("invalid regex {pattern:?}: {e}"))
}

/// Unanchored regex (templating helpers such as `regexExtract` and `matches`).
pub(crate) fn find_regex(pattern: &str) -> Result<fancy_regex::Regex, String> {
    fancy_regex::Regex::new(pattern).map_err(|e| format!("invalid regex {pattern:?}: {e}"))
}

pub(crate) fn regex_matches(re: &fancy_regex::Regex, text: &str) -> bool {
    re.is_match(text).unwrap_or(false)
}

/// Levenshtein distance normalised to 0..=1 (0 = identical). Very long inputs fall back to a
/// cheap prefix/suffix estimate so near-miss ranking never becomes quadratic on large bodies.
pub(crate) fn normalized_distance(a: &str, b: &str) -> f64 {
    if a == b {
        return 0.0;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let max = a.len().max(b.len());
    if max == 0 {
        return 0.0;
    }
    if a.len().saturating_mul(b.len()) > 4_000_000 {
        let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let suffix = a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count();
        let same = (prefix + suffix).min(max);
        return (max - same) as f64 / max as f64;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()] as f64 / max as f64
}

/// `content-type` → `Content-Type` (hyper lower-cases names; journals read better in title case).
pub(crate) fn canonical_header_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = true;
    for ch in name.chars() {
        if upper {
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }
        upper = ch == '-';
    }
    out
}

/// Deterministic UUID (version 8) from a seed: stubs without an `id` in their file keep the same
/// id across reloads without us having to rewrite the user's file.
pub(crate) fn stable_uuid(seed: &str) -> Uuid {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013B;
    let mut hash = OFFSET;
    for byte in seed.bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    uuid::Builder::from_custom_bytes(hash.to_be_bytes()).into_uuid()
}

pub(crate) fn short_id(id: &Uuid) -> String {
    id.simple().to_string()[..8].to_string()
}

/// Text form of a JSON value used when a string matcher is applied to it.
pub(crate) fn json_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn content_type_is_json(ct: &str) -> bool {
    let ct = ct.to_ascii_lowercase();
    ct.contains("/json") || ct.contains("+json")
}

pub(crate) fn content_type_is_xml(ct: &str) -> bool {
    let ct = ct.to_ascii_lowercase();
    ct.contains("/xml") || ct.contains("+xml")
}

/// Whether a body should be treated as text: valid UTF-8 without NUL bytes, and not declared as
/// an obviously binary media type.
pub(crate) fn looks_textual(body: &[u8], content_type: Option<&str>) -> bool {
    if let Some(ct) = content_type {
        let ct = ct.to_ascii_lowercase();
        let binary = ["image/", "audio/", "video/", "application/octet-stream", "application/pdf", "application/zip", "font/"];
        if binary.iter().any(|b| ct.starts_with(b)) {
            return false;
        }
    }
    std::str::from_utf8(body).is_ok_and(|s| !s.contains('\0'))
}

/// Parses sizes like `1Kb`, `2.5 MB`, `12345` into bytes.
pub(crate) fn parse_size(text: &str) -> Option<u64> {
    let t = text.trim().to_ascii_lowercase();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: f64 = num.parse().ok()?;
    let mult = match unit.trim() {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// Minimal file-extension → media type map for body files and `/__admin/files`.
pub(crate) fn guess_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "txt" | "text" => "text/plain",
        "csv" => "text/csv",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "yaml" | "yml" => "application/yaml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance() {
        assert_eq!(normalized_distance("abc", "abc"), 0.0);
        assert!((normalized_distance("abc", "abd") - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(normalized_distance("", "abc"), 1.0);
        let big_a = "a".repeat(5000);
        let big_b = format!("{}b", "a".repeat(4999));
        assert!(normalized_distance(&big_a, &big_b) < 0.01);
    }

    #[test]
    fn regexes_are_anchored_and_dotall() {
        let re = java_regex("a.c").unwrap();
        assert!(regex_matches(&re, "a\nc"));
        assert!(!regex_matches(&re, "xa_c"));
        let look = java_regex("(?!admin).*").unwrap();
        assert!(regex_matches(&look, "user"));
        assert!(!regex_matches(&look, "admin"));
        assert!(java_regex("(").is_err());
    }

    #[test]
    fn misc() {
        assert_eq!(canonical_header_name("x-request-id"), "X-Request-Id");
        assert_eq!(stable_uuid("a"), stable_uuid("a"));
        assert_ne!(stable_uuid("a"), stable_uuid("b"));
        assert_eq!(parse_size("1Kb"), Some(1024));
        assert_eq!(parse_size("2 mb"), Some(2 * 1024 * 1024));
        assert_eq!(b64_decode("aGVsbG8"), Some(b"hello".to_vec()));
        assert!(looks_textual(b"hi", Some("text/plain")));
        assert!(!looks_textual(b"hi", Some("image/png")));
    }
}
