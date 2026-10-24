//! `{{template}}` resolution shared by the app and the CLI.
//!
//! | Syntax              | Meaning                                                    |
//! |---------------------|------------------------------------------------------------|
//! | `{{name}}`          | variable layers (environment → folders → project)          |
//! | `{{secret.NAME}}`   | secret lookup (keychain, then `.env`)                      |
//! | `{{$env.NAME}}`     | OS environment variable                                    |
//! | `{{$uuid}}` …       | dynamic values (see [`dynamic`])                           |
//!
//! Unresolved references are left in place and reported; every secret value that was used is
//! remembered so callers can redact it from history, logs, exports and AI prompts.

use std::collections::BTreeSet;

use indexmap::IndexMap;

const MAX_DEPTH: usize = 8;
const REDACTED: &str = "••••••";

pub trait SecretLookup: Send + Sync {
    fn lookup(&self, name: &str) -> Option<String>;
}

/// No secrets available (e.g. previews that must never reveal values).
pub struct NoSecrets;
impl SecretLookup for NoSecrets {
    fn lookup(&self, _: &str) -> Option<String> {
        None
    }
}

pub struct Resolver<'a> {
    /// Highest precedence first.
    layers: Vec<IndexMap<String, String>>,
    secrets: &'a dyn SecretLookup,
    /// References that could not be resolved, without braces (`baseUrl`, `secret.TOKEN`).
    pub unresolved: BTreeSet<String>,
    /// Secret values used during resolution, for redaction.
    pub secret_values: BTreeSet<String>,
}

impl<'a> Resolver<'a> {
    pub fn new(secrets: &'a dyn SecretLookup) -> Self {
        Self { layers: Vec::new(), secrets, unresolved: BTreeSet::new(), secret_values: BTreeSet::new() }
    }

    /// Adds a variable layer with *lower* precedence than the ones already added.
    pub fn push_layer(&mut self, vars: IndexMap<String, String>) -> &mut Self {
        self.layers.push(vars);
        self
    }

    pub fn resolve(&mut self, input: &str) -> String {
        self.resolve_depth(input, 0)
    }

    fn resolve_depth(&mut self, input: &str, depth: usize) -> String {
        if !input.contains("{{") {
            return input.to_string();
        }
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                out.push_str(&rest[start..]);
                return out;
            };
            let key = after[..end].trim();
            match self.lookup(key, depth) {
                Some(v) => out.push_str(&v),
                None => {
                    if !key.is_empty() {
                        self.unresolved.insert(key.to_string());
                    }
                    out.push_str(&rest[start..start + 2 + end + 2]);
                }
            }
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        out
    }

    fn lookup(&mut self, key: &str, depth: usize) -> Option<String> {
        if key.is_empty() {
            return None;
        }
        if let Some(name) = key.strip_prefix("secret.") {
            let v = self.secrets.lookup(name.trim())?;
            if !v.is_empty() {
                self.secret_values.insert(v.clone());
            }
            return Some(v);
        }
        if let Some(name) = key.strip_prefix("$env.") {
            return std::env::var(name.trim()).ok();
        }
        if key.starts_with('$') {
            return dynamic(key);
        }
        let raw = self.layers.iter().find_map(|l| l.get(key)).cloned()?;
        if depth >= MAX_DEPTH {
            return Some(raw);
        }
        Some(self.resolve_depth(&raw, depth + 1))
    }

    /// Replaces every secret value used so far with a mask. Very short values are skipped to
    /// avoid masking common substrings.
    pub fn redact(&self, text: &str) -> String {
        redact_values(text, &self.secret_values)
    }
}

pub fn redact_values(text: &str, values: &BTreeSet<String>) -> String {
    let mut out = text.to_string();
    // Longest first so a secret that contains another is masked whole.
    let mut vals: Vec<&String> = values.iter().filter(|v| v.len() >= 4).collect();
    vals.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for v in vals {
        if out.contains(v.as_str()) {
            out = out.replace(v.as_str(), REDACTED);
        }
    }
    out
}

/// Built-in dynamic values. Unknown names return `None` (reported as unresolved).
pub fn dynamic(key: &str) -> Option<String> {
    let now = chrono::Utc::now();
    Some(match key {
        "$uuid" | "$guid" | "$randomUUID" => uuid::Uuid::new_v4().to_string(),
        "$uuidv7" => uuid::Uuid::now_v7().to_string(),
        "$ulid" => ulid::Ulid::generate().to_string(),
        "$timestamp" => now.timestamp().to_string(),
        "$timestampMs" => now.timestamp_millis().to_string(),
        "$isoTimestamp" => now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "$randomInt" => fastrand::u32(0..1000).to_string(),
        "$randomBool" => fastrand::bool().to_string(),
        _ => return None,
    })
}

/// Names of the built-in dynamic values, for autocomplete.
pub const DYNAMIC_NAMES: &[&str] =
    &["$uuid", "$uuidv7", "$ulid", "$timestamp", "$timestampMs", "$isoTimestamp", "$randomInt", "$randomBool"];

/// Every `{{reference}}` in `input`, in order of appearance (deduplicated).
pub fn references(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let key = after[..end].trim().to_string();
        if !key.is_empty() && !out.contains(&key) {
            out.push(key);
        }
        rest = &after[end + 2..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct S;
    impl SecretLookup for S {
        fn lookup(&self, name: &str) -> Option<String> {
            (name == "TOKEN").then(|| "s3cr3t-value".to_string())
        }
    }

    fn map(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn layers_precedence_and_nesting() {
        let s = S;
        let mut r = Resolver::new(&s);
        r.push_layer(map(&[("host", "env.example.com")]));
        r.push_layer(map(&[("host", "project.example.com"), ("baseUrl", "https://{{host}}/api")]));
        assert_eq!(r.resolve("{{ baseUrl }}/users"), "https://env.example.com/api/users");
        assert!(r.unresolved.is_empty());
    }

    #[test]
    fn secrets_are_tracked_and_redacted() {
        let s = S;
        let mut r = Resolver::new(&s);
        let out = r.resolve("Bearer {{secret.TOKEN}}");
        assert_eq!(out, "Bearer s3cr3t-value");
        assert_eq!(r.redact(&out), "Bearer ••••••");
        r.resolve("{{secret.MISSING}}");
        assert!(r.unresolved.contains("secret.MISSING"));
    }

    #[test]
    fn unresolved_left_in_place() {
        let mut r = Resolver::new(&NoSecrets);
        assert_eq!(r.resolve("a {{nope}} b {{ }} c {{unterminated"), "a {{nope}} b {{ }} c {{unterminated");
        assert_eq!(r.unresolved.iter().collect::<Vec<_>>(), vec!["nope"]);
    }

    #[test]
    fn cycles_terminate() {
        let mut r = Resolver::new(&NoSecrets);
        r.push_layer(map(&[("a", "{{b}}"), ("b", "{{a}}")]));
        let out = r.resolve("{{a}}");
        assert!(out.contains("{{"));
    }

    #[test]
    fn dynamic_values() {
        let mut r = Resolver::new(&NoSecrets);
        assert_eq!(r.resolve("{{$uuid}}").len(), 36);
        assert_eq!(r.resolve("{{$ulid}}").len(), 26);
        assert!(r.resolve("{{$timestamp}}").parse::<i64>().is_ok());
        r.resolve("{{$nope}}");
        assert!(r.unresolved.contains("$nope"));
    }

    #[test]
    fn finds_references() {
        assert_eq!(references("{{a}}/{{ secret.B }}/{{a}}"), vec!["a", "secret.B"]);
    }
}
