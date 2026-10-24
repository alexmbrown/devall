//! Response snapshots: the expected body lives in a committed file (by default
//! `http/__snapshots__/<request path>.snap.json`) and the `snapshot` assertion compares every
//! response with it. JSON is compared semantically (key order and number formatting don't matter);
//! nodes selected by `ignore` JSONPaths (timestamps, generated ids) are masked on both sides and
//! stored masked, so the file doesn't churn in git.

use std::path::Path;

use devall_core::fsutil::{atomic_write, normalize_rel, safe_join};
use devall_core::project::{HTTP_DIR, REQUEST_EXT, SNAPSHOT_DIR, item_kind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Placeholder written in place of ignored nodes.
pub const IGNORED: &str = "<ignored>";
const SNAPSHOT_EXT: &str = ".snap.json";

/// What a `.snap.json` file holds: the body as JSON (`json`) or as text (`text`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFile {
    #[serde(default = "one")]
    pub version: u32,
    /// The request it was recorded from (informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Paths that were masked when recording (informational; the assertion's list is used).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

fn one() -> u32 {
    1
}

/// `users/list.req.yaml` → `http/__snapshots__/users/list.snap.json` (relative to the project root).
pub fn default_path(request_path: &str) -> String {
    let rel = normalize_rel(request_path);
    let stem = match item_kind(&rel) {
        Some((ext, _)) => rel.trim_end_matches(ext).to_string(),
        None => rel.trim_end_matches(REQUEST_EXT).to_string(),
    };
    let stem = if stem.is_empty() { "untitled".to_string() } else { stem };
    format!("{HTTP_DIR}/{SNAPSHOT_DIR}/{stem}{SNAPSHOT_EXT}")
}

/// Replaces every node selected by `ignore` with [`IGNORED`].
pub fn mask(v: &mut Value, ignore: &[String]) -> Result<(), String> {
    for path in ignore.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let p = serde_json_path::JsonPath::parse(path).map_err(|e| format!("invalid ignore path {path}: {e}"))?;
        let pointers: Vec<String> = p.query_located(v).iter().map(|n| n.location().to_json_pointer()).collect();
        for ptr in pointers {
            if let Some(node) = v.pointer_mut(&ptr) {
                *node = Value::String(IGNORED.into());
            }
        }
    }
    Ok(())
}

fn is_json_type(ct: Option<&str>) -> bool {
    ct.is_some_and(|c| {
        let c = c.to_ascii_lowercase();
        c.contains("json")
    })
}

/// Builds the snapshot for a response body. JSON bodies (by content type, or when the text
/// parses as an object/array) are stored as JSON with `ignore` applied; anything else as text.
pub fn make(body: &[u8], content_type: Option<&str>, request: Option<&str>, ignore: &[String]) -> Result<SnapshotFile, String> {
    let text = std::str::from_utf8(body).map_err(|_| "binary responses can't be snapshot-tested (the body is not UTF-8 text)".to_string())?;
    let parsed = serde_json::from_str::<Value>(text).ok().filter(|v| is_json_type(content_type) || v.is_object() || v.is_array());
    let (json, text) = match parsed {
        Some(mut v) => {
            mask(&mut v, ignore)?;
            (Some(v), None)
        }
        None => (None, Some(text.replace("\r\n", "\n"))),
    };
    Ok(SnapshotFile {
        version: 1,
        request: request.map(str::to_string),
        content_type: content_type.map(str::to_string),
        ignore: ignore.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        json,
        text,
    })
}

pub fn to_text(s: &SnapshotFile) -> String {
    let mut out = serde_json::to_string_pretty(s).unwrap_or_else(|_| "{}".into());
    out.push('\n');
    out
}

/// Writes `snap` to `file` (relative to `root`, `..` rejected).
pub fn write(root: &Path, file: &str, snap: &SnapshotFile) -> Result<(), String> {
    let path = safe_join(root, file).map_err(|e| e.to_string())?;
    atomic_write(&path, to_text(snap).as_bytes()).map_err(|e| e.to_string())
}

pub fn read(root: &Path, file: &str) -> Result<Option<SnapshotFile>, String> {
    let path = safe_join(root, file).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map(Some).map_err(|e| format!("{file} is not a snapshot file: {e}"))
}

fn key_path(base: &str, key: &str) -> String {
    let simple = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !key.starts_with(|c: char| c.is_ascii_digit());
    if simple { format!("{base}.{key}") } else { format!("{base}[{}]", serde_json::to_string(key).unwrap_or_default().replace('"', "'")) }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 80 { format!("{}…", s.chars().take(80).collect::<String>()) } else { s }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Semantic JSON differences, as `path: what` lines (at most `limit`, then a count of the rest).
pub fn diff(expected: &Value, actual: &Value, limit: usize) -> Vec<String> {
    fn walk(path: &str, e: &Value, a: &Value, out: &mut Vec<String>) {
        match (e, a) {
            (Value::Object(eo), Value::Object(ao)) => {
                for (k, ev) in eo {
                    match ao.get(k) {
                        Some(av) => walk(&key_path(path, k), ev, av, out),
                        None => out.push(format!("{}: missing (expected {})", key_path(path, k), short(ev))),
                    }
                }
                for (k, av) in ao {
                    if !eo.contains_key(k) {
                        out.push(format!("{}: unexpected field ({})", key_path(path, k), short(av)));
                    }
                }
            }
            (Value::Array(ea), Value::Array(aa)) => {
                if ea.len() != aa.len() {
                    out.push(format!("{path}: expected {} item(s), got {}", ea.len(), aa.len()));
                }
                for (i, (ev, av)) in ea.iter().zip(aa).enumerate() {
                    walk(&format!("{path}[{i}]"), ev, av, out);
                }
            }
            (Value::Number(en), Value::Number(an)) => {
                if en != an && en.as_f64() != an.as_f64() {
                    out.push(format!("{path}: expected {en}, got {an}"));
                }
            }
            _ if e == a => {}
            _ if kind(e) != kind(a) => out.push(format!("{path}: expected {} {}, got {} {}", kind(e), short(e), kind(a), short(a))),
            _ => out.push(format!("{path}: expected {}, got {}", short(e), short(a))),
        }
    }
    let mut out = Vec::new();
    walk("$", expected, actual, &mut out);
    if out.len() > limit {
        let rest = out.len() - limit;
        out.truncate(limit);
        out.push(format!("… and {rest} more difference(s)"));
    }
    out
}

fn text_diff(expected: &str, actual: &str) -> Vec<String> {
    let (e, a): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), actual.lines().collect());
    for i in 0..e.len().max(a.len()) {
        match (e.get(i), a.get(i)) {
            (Some(x), Some(y)) if x == y => {}
            (x, y) => {
                let show = |l: Option<&&str>| l.map_or_else(|| "(no line)".to_string(), |l| format!("{:?}", if l.chars().count() > 80 { format!("{}…", l.chars().take(80).collect::<String>()) } else { l.to_string() }));
                return vec![format!("line {}: expected {}, got {}", i + 1, show(x), show(y))];
            }
        }
    }
    Vec::new()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Match,
    /// Differences, most important first.
    Mismatch(Vec<String>),
    Missing,
    /// Written because it was missing or different and updating was requested.
    Written { created: bool },
}

/// Compares a response body with the snapshot `file`. With `update`, a missing or different
/// snapshot is (re)written instead of failing.
pub fn check(root: &Path, file: &str, body: &[u8], content_type: Option<&str>, request: Option<&str>, ignore: &[String], update: bool) -> Result<Outcome, String> {
    let actual = make(body, content_type, request, ignore)?;
    let existing = read(root, file)?;
    let differences = match &existing {
        None => None,
        Some(expected) => Some(match (&expected.json, &actual.json, &expected.text, &actual.text) {
            (Some(e), Some(a), _, _) => {
                // Re-mask the expectation too: the ignore list may have grown since it was recorded.
                let mut e = e.clone();
                mask(&mut e, ignore)?;
                diff(&e, a, 8)
            }
            (_, _, Some(e), Some(a)) => text_diff(&e.replace("\r\n", "\n"), a),
            (Some(_), None, _, _) => vec!["the snapshot holds JSON but the response is not JSON".into()],
            _ => vec!["the snapshot holds text but the response is JSON".into()],
        }),
    };
    match differences {
        Some(d) if d.is_empty() => Ok(Outcome::Match),
        other if update => {
            write(root, file, &actual)?;
            Ok(Outcome::Written { created: other.is_none() })
        }
        Some(d) => Ok(Outcome::Mismatch(d)),
        None => Ok(Outcome::Missing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_paths() {
        assert_eq!(default_path("users/list.req.yaml"), "http/__snapshots__/users/list.snap.json");
        assert_eq!(default_path("ping.req.yaml"), "http/__snapshots__/ping.snap.json");
    }

    #[test]
    fn semantic_json_diff() {
        let e = json!({ "id": 1, "name": "Ada", "tags": ["a", "b"], "n": 1.0, "nested": { "x": true } });
        let same = json!({ "nested": { "x": true }, "n": 1, "tags": ["a", "b"], "name": "Ada", "id": 1 });
        assert!(diff(&e, &same, 10).is_empty(), "key order and 1 vs 1.0 don't matter");
        let other = json!({ "id": "1", "name": "Bob", "tags": ["a"], "n": 1, "nested": {}, "extra": 5 });
        let d = diff(&e, &other, 10);
        assert!(d.contains(&"$.id: expected number 1, got string \"1\"".to_string()), "{d:?}");
        assert!(d.contains(&"$.name: expected \"Ada\", got \"Bob\"".to_string()), "{d:?}");
        assert!(d.contains(&"$.tags: expected 2 item(s), got 1".to_string()), "{d:?}");
        assert!(d.contains(&"$.nested.x: missing (expected true)".to_string()), "{d:?}");
        assert!(d.contains(&"$.extra: unexpected field (5)".to_string()), "{d:?}");
        assert_eq!(diff(&e, &other, 2).last().unwrap(), "… and 3 more difference(s)");
        assert_eq!(diff(&json!({ "a b": 1 }), &json!({ "a b": 2 }), 5), vec!["$['a b']: expected 1, got 2"]);
    }

    #[test]
    fn record_compare_update() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = default_path("users/get.req.yaml");
        let ignore = vec!["$.createdAt".to_string(), "$.items[*].id".to_string()];
        let body = br#"{"name":"Ada","createdAt":"2026-10-22T10:00:00Z","items":[{"id":"x1","q":1},{"id":"x2","q":2}]}"#;
        let ct = Some("application/json");

        assert_eq!(check(root, &file, body, ct, None, &ignore, false).unwrap(), Outcome::Missing);
        assert_eq!(check(root, &file, body, ct, Some("users/get.req.yaml"), &ignore, true).unwrap(), Outcome::Written { created: true });
        let stored = std::fs::read_to_string(root.join(&file)).unwrap();
        assert!(stored.contains(IGNORED) && !stored.contains("x1") && !stored.contains("2026"), "{stored}");
        assert!(stored.contains("\"request\": \"users/get.req.yaml\""), "{stored}");

        // Volatile values change → still a match.
        let later = br#"{"items":[{"q":1,"id":"y9"},{"id":"y8","q":2}],"createdAt":"2027-01-01T00:00:00Z","name":"Ada"}"#;
        assert_eq!(check(root, &file, later, ct, None, &ignore, false).unwrap(), Outcome::Match);
        // A real change → mismatch with a readable path.
        let changed = br#"{"name":"Ada","createdAt":"x","items":[{"id":"y","q":1},{"id":"z","q":3}]}"#;
        let Outcome::Mismatch(d) = check(root, &file, changed, ct, None, &ignore, false).unwrap() else { panic!() };
        assert_eq!(d, vec!["$.items[1].q: expected 2, got 3"]);
        assert_eq!(check(root, &file, changed, ct, None, &ignore, true).unwrap(), Outcome::Written { created: false });
        assert_eq!(check(root, &file, changed, ct, None, &ignore, false).unwrap(), Outcome::Match);

        // Text bodies compare line by line.
        let tfile = "http/__snapshots__/hello.snap.json";
        check(root, tfile, b"hello\nworld\n", Some("text/plain"), None, &[], true).unwrap();
        let Outcome::Mismatch(d) = check(root, tfile, b"hello\nthere\n", Some("text/plain"), None, &[], false).unwrap() else { panic!() };
        assert_eq!(d, vec!["line 2: expected \"world\", got \"there\""]);
        assert!(check(root, "../outside.snap.json", b"{}", ct, None, &[], true).is_err());
        assert!(make(&[0xff, 0xfe], None, None, &[]).is_err());
    }
}
