//! A small JSON Schema toolkit: validate model output in Rust (backends only *try* to follow a
//! schema — llama.cpp ignores unsupported keywords, prompt-only fallbacks can drift), adapt a
//! schema to what each backend accepts, and infer a schema from a sample document.
//!
//! Supported keywords: `type` (string or list), `enum`, `const`, `properties`, `required`,
//! `additionalProperties` (bool or schema), `items`, `minItems`/`maxItems`, `minLength`/`maxLength`,
//! `minimum`/`maximum`/`exclusiveMinimum`/`exclusiveMaximum`, `pattern`, `format` (date,
//! date-time, email, uuid, uri), `anyOf`/`oneOf`/`allOf`, local `$ref` (`#/$defs/…`,
//! `#/definitions/…`). Unknown keywords are ignored.

use serde_json::{Map, Value, json};

use crate::provider::ProviderKind;

const MAX_ERRORS: usize = 20;
const MAX_DEPTH: usize = 32;

/// Validates `value` against `schema`; returns human-readable errors (`$.items[2].email: …`).
pub fn validate(value: &Value, schema: &Value) -> Vec<String> {
    let mut errs = Vec::new();
    check(value, schema, schema, "$", 0, &mut errs);
    errs.truncate(MAX_ERRORS);
    errs
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(v: &Value, t: &str) -> bool {
    match t {
        "number" => v.is_number(),
        "integer" => v.as_f64().is_some_and(|f| f.fract() == 0.0) && v.is_number(),
        other => type_name(v) == other,
    }
}

fn resolve<'a>(root: &'a Value, r: &str) -> Option<&'a Value> {
    let path = r.strip_prefix("#/")?;
    let mut cur = root;
    for part in path.split('/') {
        cur = cur.get(part.replace("~1", "/").replace("~0", "~"))?;
    }
    Some(cur)
}

fn check(v: &Value, schema: &Value, root: &Value, at: &str, depth: usize, errs: &mut Vec<String>) {
    if errs.len() >= MAX_ERRORS || depth > MAX_DEPTH {
        return;
    }
    let Some(s) = schema.as_object() else {
        if schema == &Value::Bool(false) {
            errs.push(format!("{at}: not allowed"));
        }
        return;
    };
    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        match resolve(root, r) {
            Some(target) => check(v, target, root, at, depth + 1, errs),
            None => errs.push(format!("{at}: unresolvable $ref {r}")),
        }
    }
    if let Some(t) = s.get("type") {
        let types: Vec<&str> = match t {
            Value::String(t) => vec![t.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if !types.is_empty() && !types.iter().any(|t| type_matches(v, t)) {
            errs.push(format!("{at}: expected {}, got {}", types.join(" or "), type_name(v)));
            return;
        }
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        let opts: Vec<String> = e.iter().take(10).map(Value::to_string).collect();
        errs.push(format!("{at}: must be one of {}", opts.join(", ")));
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        errs.push(format!("{at}: must equal {c}"));
    }
    for (kw, any) in [("anyOf", true), ("oneOf", true)] {
        if let Some(list) = s.get(kw).and_then(Value::as_array) {
            let ok = list.iter().any(|sub| {
                let mut e = Vec::new();
                check(v, sub, root, at, depth + 1, &mut e);
                e.is_empty()
            });
            if any && !ok {
                errs.push(format!("{at}: does not match any allowed shape"));
            }
        }
    }
    if let Some(list) = s.get("allOf").and_then(Value::as_array) {
        for sub in list {
            check(v, sub, root, at, depth + 1, errs);
        }
    }
    match v {
        Value::Object(o) => {
            let props = s.get("properties").and_then(Value::as_object);
            if let Some(req) = s.get("required").and_then(Value::as_array) {
                for r in req.iter().filter_map(Value::as_str) {
                    if !o.contains_key(r) {
                        errs.push(format!("{at}: missing required property `{r}`"));
                    }
                }
            }
            for (k, val) in o {
                let child = format!("{at}.{k}");
                match props.and_then(|p| p.get(k)) {
                    Some(ps) => check(val, ps, root, &child, depth + 1, errs),
                    None => match s.get("additionalProperties") {
                        Some(Value::Bool(false)) => errs.push(format!("{at}: unexpected property `{k}`")),
                        Some(extra @ Value::Object(_)) => check(val, extra, root, &child, depth + 1, errs),
                        _ => {}
                    },
                }
            }
        }
        Value::Array(a) => {
            if let Some(n) = s.get("minItems").and_then(Value::as_u64)
                && (a.len() as u64) < n
            {
                errs.push(format!("{at}: needs at least {n} items, got {}", a.len()));
            }
            if let Some(n) = s.get("maxItems").and_then(Value::as_u64)
                && (a.len() as u64) > n
            {
                errs.push(format!("{at}: at most {n} items, got {}", a.len()));
            }
            if let Some(items) = s.get("items").filter(|i| i.is_object() || i.is_boolean()) {
                for (i, item) in a.iter().enumerate() {
                    check(item, items, root, &format!("{at}[{i}]"), depth + 1, errs);
                }
            }
        }
        Value::String(st) => {
            let len = st.chars().count() as u64;
            if let Some(n) = s.get("minLength").and_then(Value::as_u64)
                && len < n
            {
                errs.push(format!("{at}: shorter than {n} characters"));
            }
            if let Some(n) = s.get("maxLength").and_then(Value::as_u64)
                && len > n
            {
                errs.push(format!("{at}: longer than {n} characters"));
            }
            if let Some(p) = s.get("pattern").and_then(Value::as_str)
                && let Ok(re) = regex::Regex::new(p)
                && !re.is_match(st)
            {
                errs.push(format!("{at}: does not match pattern {p}"));
            }
            if let Some(f) = s.get("format").and_then(Value::as_str)
                && !format_ok(f, st)
            {
                errs.push(format!("{at}: not a valid {f}"));
            }
        }
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or(0.0);
            let num = |k: &str| s.get(k).and_then(Value::as_f64);
            if num("minimum").is_some_and(|m| x < m) || num("exclusiveMinimum").is_some_and(|m| x <= m) {
                errs.push(format!("{at}: {x} is below the minimum"));
            }
            if num("maximum").is_some_and(|m| x > m) || num("exclusiveMaximum").is_some_and(|m| x >= m) {
                errs.push(format!("{at}: {x} is above the maximum"));
            }
        }
        _ => {}
    }
}

fn format_ok(format: &str, s: &str) -> bool {
    use std::sync::OnceLock;
    static RES: OnceLock<[(&'static str, regex::Regex); 5]> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [
            ("date", regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap()),
            ("date-time", regex::Regex::new(r"^\d{4}-\d{2}-\d{2}[Tt ]\d{2}:\d{2}(:\d{2}(\.\d+)?)?([Zz]|[+-]\d{2}:?\d{2})?$").unwrap()),
            ("email", regex::Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").unwrap()),
            ("uuid", regex::Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$").unwrap()),
            ("uri", regex::Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*:\S+$").unwrap()),
        ]
    });
    res.iter().find(|(f, _)| *f == format).is_none_or(|(_, re)| re.is_match(s))
}

const ANTHROPIC_UNSUPPORTED: &[&str] = &[
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
];
const ANTHROPIC_FORMATS: &[&str] = &["date-time", "time", "date", "duration", "email", "hostname", "uri", "ipv4", "ipv6", "uuid"];

/// Adapts a schema to a backend: Anthropic needs closed objects and no numeric/string/array
/// constraints; OpenAI strict mode needs closed objects with every property required. Other
/// backends get the schema unchanged. Validation always uses the original schema.
pub fn for_backend(schema: &Value, kind: ProviderKind, openai_strict: bool) -> Value {
    let mut s = schema.clone();
    match kind {
        ProviderKind::Anthropic => adapt(&mut s, &|o| {
            for k in ANTHROPIC_UNSUPPORTED {
                o.remove(*k);
            }
            if o.get("format").and_then(Value::as_str).is_some_and(|f| !ANTHROPIC_FORMATS.contains(&f)) {
                o.remove("format");
            }
            close_object(o, false);
        }),
        ProviderKind::OpenAiCompatible if openai_strict => adapt(&mut s, &|o| close_object(o, true)),
        _ => {}
    }
    if let Some(o) = s.as_object_mut() {
        o.remove("$schema");
    }
    s
}

fn close_object(o: &mut Map<String, Value>, require_all: bool) {
    let is_object = o.get("type").is_some_and(|t| t == "object" || t.as_array().is_some_and(|a| a.iter().any(|x| x == "object"))) || o.contains_key("properties");
    if !is_object {
        return;
    }
    if !matches!(o.get("additionalProperties"), Some(Value::Bool(false))) {
        o.insert("additionalProperties".into(), Value::Bool(false));
    }
    if require_all && let Some(props) = o.get("properties").and_then(Value::as_object) {
        let keys: Vec<Value> = props.keys().cloned().map(Value::String).collect();
        o.insert("required".into(), Value::Array(keys));
    }
}

fn adapt(v: &mut Value, f: &dyn Fn(&mut Map<String, Value>)) {
    match v {
        Value::Object(o) => {
            f(o);
            for (_, child) in o.iter_mut() {
                adapt(child, f);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|c| adapt(c, f)),
        _ => {}
    }
}

/// Infers a schema from a sample document (all observed keys required; formats detected).
pub fn infer(sample: &Value) -> Value {
    match sample {
        Value::Null => json!({}),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Number(n) if n.is_i64() || n.is_u64() => json!({ "type": "integer" }),
        Value::Number(_) => json!({ "type": "number" }),
        Value::String(s) => {
            let fmt = ["uuid", "date-time", "date", "email"].into_iter().find(|f| format_ok(f, s) && !s.is_empty());
            match fmt {
                Some(f) => json!({ "type": "string", "format": f }),
                None => json!({ "type": "string" }),
            }
        }
        Value::Array(a) => match a.first() {
            Some(first) => json!({ "type": "array", "items": infer(first) }),
            None => json!({ "type": "array" }),
        },
        Value::Object(o) => {
            let props: Map<String, Value> = o.iter().map(|(k, v)| (k.clone(), infer(v))).collect();
            let required: Vec<Value> = o.iter().filter(|(_, v)| !v.is_null()).map(|(k, _)| Value::String(k.clone())).collect();
            json!({ "type": "object", "properties": props, "required": required })
        }
    }
}

/// Heuristic: does this JSON look like a JSON Schema (rather than a sample document)?
pub fn looks_like_schema(v: &Value) -> bool {
    let Some(o) = v.as_object() else { return false };
    o.contains_key("$schema")
        || (o.get("type").is_some_and(|t| t.is_string() || t.is_array()) && (o.contains_key("properties") || o.contains_key("items") || o.len() <= 3))
        || o.contains_key("$defs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_common_keywords() {
        let schema = json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "minimum": 1 },
                "email": { "type": "string", "format": "email" },
                "role": { "enum": ["admin", "user"] },
                "tags": { "type": "array", "items": { "type": "string" }, "maxItems": 2 },
                "ref": { "$ref": "#/$defs/code" }
            },
            "required": ["id", "email"],
            "additionalProperties": false,
            "$defs": { "code": { "type": "string", "pattern": "^[A-Z]{3}$" } }
        });
        assert!(validate(&json!({"id": 3, "email": "a@b.co", "role": "user", "tags": ["x"], "ref": "ABC"}), &schema).is_empty());
        let errs = validate(&json!({"id": 0, "email": "nope", "role": "root", "tags": ["a","b","c"], "ref": "abc", "extra": 1}), &schema);
        let joined = errs.join("\n");
        for needle in ["below the minimum", "not a valid email", "must be one of", "at most 2 items", "does not match pattern", "unexpected property `extra`"] {
            assert!(joined.contains(needle), "missing `{needle}` in:\n{joined}");
        }
        assert!(validate(&json!({"email": "a@b.co"}), &schema)[0].contains("missing required property `id`"));
        assert!(validate(&json!(1.5), &json!({"type": "integer"}))[0].contains("expected integer"));
        assert!(validate(&json!(2.0), &json!({"type": "integer"})).is_empty());
    }

    #[test]
    fn adapts_for_backends() {
        let s = json!({"$schema": "x", "type": "object", "properties": {"n": {"type": "integer", "minimum": 0}, "s": {"type": "string", "format": "color"}}, "required": ["n"]});
        let a = for_backend(&s, ProviderKind::Anthropic, false);
        assert_eq!(a["additionalProperties"], false);
        assert!(a["properties"]["n"].get("minimum").is_none());
        assert!(a["properties"]["s"].get("format").is_none());
        assert!(a.get("$schema").is_none());
        let o = for_backend(&s, ProviderKind::OpenAiCompatible, true);
        assert_eq!(o["required"], json!(["n", "s"]));
        let l = for_backend(&s, ProviderKind::OpenAiCompatible, false);
        assert_eq!(l["properties"]["n"]["minimum"], 0);
    }

    #[test]
    fn infers_from_samples() {
        let sample = json!({"id": 1, "price": 9.5, "email": "a@b.co", "created": "2026-10-21T10:00:00Z", "tags": ["x"], "note": null});
        let s = infer(&sample);
        assert_eq!(s["properties"]["id"]["type"], "integer");
        assert_eq!(s["properties"]["price"]["type"], "number");
        assert_eq!(s["properties"]["email"]["format"], "email");
        assert_eq!(s["properties"]["created"]["format"], "date-time");
        assert_eq!(s["properties"]["tags"]["items"]["type"], "string");
        assert!(!s["required"].as_array().unwrap().contains(&json!("note")));
        assert!(validate(&sample, &s).is_empty());
        assert!(looks_like_schema(&json!({"type": "object", "properties": {}})));
        assert!(!looks_like_schema(&sample));
    }
}
