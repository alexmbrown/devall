//! Validates JSON values against OpenAPI 3.0 / 3.1 schemas.
//!
//! Covers what real API schemas use: `type` (string or list), 3.0 `nullable`, `enum`, `const`,
//! numeric bounds (3.0 boolean and 3.1 numeric `exclusiveMinimum`/`exclusiveMaximum`),
//! `multipleOf`, string lengths, `pattern`, common `format`s (date, date-time, time, email, uuid,
//! uri, ipv4, ipv6, int32, int64, byte), arrays (`items`, `prefixItems`, `minItems`/`maxItems`,
//! `uniqueItems`, `contains`), objects (`properties`, `required`, `additionalProperties`,
//! `patternProperties`, `minProperties`/`maxProperties`), `allOf` / `anyOf` / `oneOf` (with
//! `discriminator`), `not`, and local `$ref`s. `readOnly` properties are not required in
//! requests and `writeOnly` ones not in responses. External `$ref`s and unknown keywords are
//! ignored (they never fail validation).
//!
//! The generic JSON Schema checker in `devall-ai` validates LLM output; OpenAPI's dialect differs
//! (nullable, boolean exclusive bounds, read/write-only), hence this one.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::spec::Spec;

const MAX_ERRORS: usize = 25;
const MAX_DEPTH: usize = 64;

/// Which side of the exchange the value is on (affects `readOnly` / `writeOnly`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Request,
    Response,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaError {
    /// `$.items[2].email`
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

pub struct Validator<'a> {
    spec: &'a Spec,
    dir: Direction,
    errors: Vec<SchemaError>,
}

/// Validates `value` against `schema` (which may be a `$ref`) from `spec`.
pub fn validate(spec: &Spec, schema: &Value, value: &Value, dir: Direction) -> Vec<SchemaError> {
    let mut v = Validator { spec, dir, errors: Vec::new() };
    v.check(schema, value, "$", 0);
    v.errors.truncate(MAX_ERRORS);
    v.errors
}

/// `true` when valid (no error collection; used for `anyOf` / `oneOf` branches).
fn is_valid(spec: &Spec, schema: &Value, value: &Value, dir: Direction, depth: usize) -> bool {
    let mut v = Validator { spec, dir, errors: Vec::new() };
    v.check(schema, value, "$", depth);
    v.errors.is_empty()
}

pub fn type_name(v: &Value) -> &'static str {
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

fn is_integer(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0 && f.is_finite()),
        _ => false,
    }
}

fn type_matches(v: &Value, t: &str) -> bool {
    match t {
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        "integer" => is_integer(v),
        "number" => v.is_number(),
        "string" => v.is_string(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 60 { format!("{}…", s.chars().take(59).collect::<String>()) } else { s }
}

/// JSON equality treating `1` and `1.0` as equal.
pub fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w))),
        _ => a == b,
    }
}

impl Validator<'_> {
    fn err(&mut self, path: &str, message: impl Into<String>) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(SchemaError { path: path.to_string(), message: message.into() });
        }
    }

    fn check(&mut self, schema: &Value, v: &Value, path: &str, depth: usize) {
        if depth > MAX_DEPTH || self.errors.len() >= MAX_ERRORS {
            return;
        }
        let schema = match schema {
            Value::Bool(true) => return,
            Value::Bool(false) => return self.err(path, "no value is allowed here"),
            Value::Object(_) => schema,
            _ => return,
        };
        // $ref: in 3.0 siblings are ignored; in 3.1 they apply too.
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            match self.spec.pointer(r) {
                Some(target) => self.check(target, v, path, depth + 1),
                None if r.starts_with('#') => self.err(path, format!("$ref {r} does not resolve")),
                None => {} // external: not checked
            }
            if self.spec.version == crate::spec::SpecVersion::V30 {
                return;
            }
        }
        let s = schema.as_object().expect("checked above");

        let nullable = s.get("nullable").and_then(Value::as_bool).unwrap_or(false);
        if v.is_null() && nullable {
            return;
        }

        // ----- type -----
        let types: Vec<&str> = match s.get("type") {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty() && !types.iter().any(|t| type_matches(v, t)) {
            let expected = types.join(" or ");
            return self.err(path, format!("expected {expected}{}, got {} {}", if nullable { " or null" } else { "" }, type_name(v), short(v)));
        }

        if let Some(Value::Array(options)) = s.get("enum")
            && !options.iter().any(|o| json_eq(o, v))
        {
            let list: Vec<String> = options.iter().take(8).map(short).collect();
            self.err(path, format!("{} is not one of {}{}", short(v), list.join(", "), if options.len() > 8 { ", …" } else { "" }));
        }
        if let Some(c) = s.get("const")
            && !json_eq(c, v)
        {
            self.err(path, format!("expected {}, got {}", short(c), short(v)));
        }

        match v {
            Value::Number(n) => self.number(s, n.as_f64().unwrap_or(0.0), v, path),
            Value::String(text) => self.string(s, text, path),
            Value::Array(items) => self.array(s, items, path, depth),
            Value::Object(obj) => self.object(s, obj, path, depth),
            _ => {}
        }

        // ----- combinators -----
        if let Some(Value::Array(all)) = s.get("allOf") {
            for sub in all {
                self.check(sub, v, path, depth + 1);
            }
        }
        if let Some(Value::Array(any)) = s.get("anyOf")
            && !any.iter().any(|sub| is_valid(self.spec, sub, v, self.dir, depth + 1))
        {
            self.best_branch(any, v, path, depth, "anyOf");
        }
        if let Some(Value::Array(one)) = s.get("oneOf") {
            if let Some(target) = Self::discriminated(s, one, v) {
                self.check(target, v, path, depth + 1);
            } else {
                let passing = one.iter().filter(|sub| is_valid(self.spec, sub, v, self.dir, depth + 1)).count();
                match passing {
                    1 => {}
                    0 => self.best_branch(one, v, path, depth, "oneOf"),
                    n => self.err(path, format!("matches {n} oneOf alternatives; exactly one must match")),
                }
            }
        }
        if let Some(not) = s.get("not")
            && is_valid(self.spec, not, v, self.dir, depth + 1)
        {
            self.err(path, "matches a schema it must not match (`not`)");
        }
    }

    /// Reports the errors of the alternative that came closest: one whose `type` fits the value,
    /// then the fewest errors.
    fn best_branch(&mut self, branches: &[Value], v: &Value, path: &str, depth: usize, kw: &str) {
        let mut best: Option<((bool, usize), Vec<SchemaError>)> = None;
        for b in branches {
            let mut sub = Validator { spec: self.spec, dir: self.dir, errors: Vec::new() };
            sub.check(b, v, path, depth + 1);
            let resolved = self.spec.resolve(b);
            let type_fits = match resolved.get("type") {
                Some(Value::String(t)) => type_matches(v, t),
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).any(|t| type_matches(v, t)),
                _ => true,
            };
            let rank = (!type_fits, sub.errors.len());
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, sub.errors));
            }
        }
        let best = best.map(|(_, e)| e);
        match best {
            Some(errs) if errs.len() == 1 => {
                let e = &errs[0];
                self.err(&e.path, format!("{} (no {kw} alternative matches)", e.message));
            }
            _ => self.err(path, format!("matches none of the {} {kw} alternatives", branches.len())),
        }
    }

    /// `discriminator.propertyName` (+ `mapping`) picks the `oneOf` branch directly.
    fn discriminated<'s>(s: &'s Map<String, Value>, branches: &'s [Value], v: &Value) -> Option<&'s Value> {
        let d = s.get("discriminator")?;
        let prop = d.get("propertyName")?.as_str()?;
        let tag = v.get(prop)?.as_str()?;
        if let Some(r) = d.get("mapping").and_then(|m| m.get(tag)).and_then(Value::as_str) {
            let r = if r.starts_with('#') { r.to_string() } else { format!("#/components/schemas/{r}") };
            return branches.iter().find(|b| b.get("$ref").and_then(Value::as_str) == Some(r.as_str()));
        }
        branches.iter().find(|b| b.get("$ref").and_then(Value::as_str).is_some_and(|r| r.rsplit('/').next() == Some(tag)))
    }

    fn number(&mut self, s: &Map<String, Value>, n: f64, v: &Value, path: &str) {
        let num = |k: &str| s.get(k).and_then(Value::as_f64);
        let excl_bool = |k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
        if let Some(min) = num("minimum") {
            if excl_bool("exclusiveMinimum") && n <= min {
                self.err(path, format!("{n} must be greater than {min}"));
            } else if n < min {
                self.err(path, format!("{n} is less than the minimum {min}"));
            }
        }
        if let Some(max) = num("maximum") {
            if excl_bool("exclusiveMaximum") && n >= max {
                self.err(path, format!("{n} must be less than {max}"));
            } else if n > max {
                self.err(path, format!("{n} is greater than the maximum {max}"));
            }
        }
        if let Some(min) = num("exclusiveMinimum")
            && n <= min
        {
            self.err(path, format!("{n} must be greater than {min}"));
        }
        if let Some(max) = num("exclusiveMaximum")
            && n >= max
        {
            self.err(path, format!("{n} must be less than {max}"));
        }
        if let Some(m) = num("multipleOf")
            && m > 0.0
        {
            let q = n / m;
            if (q - q.round()).abs() > 1e-9 {
                self.err(path, format!("{n} is not a multiple of {m}"));
            }
        }
        match s.get("format").and_then(Value::as_str) {
            Some("int32") if !is_integer(v) || !(i32::MIN as f64..=i32::MAX as f64).contains(&n) => self.err(path, format!("{n} is not an int32")),
            Some("int64") if !is_integer(v) => self.err(path, format!("{n} is not an int64")),
            _ => {}
        }
    }

    fn string(&mut self, s: &Map<String, Value>, text: &str, path: &str) {
        let len = text.chars().count() as u64;
        if let Some(min) = s.get("minLength").and_then(Value::as_u64)
            && len < min
        {
            self.err(path, format!("is {len} characters, shorter than minLength {min}"));
        }
        if let Some(max) = s.get("maxLength").and_then(Value::as_u64)
            && len > max
        {
            self.err(path, format!("is {len} characters, longer than maxLength {max}"));
        }
        if let Some(p) = s.get("pattern").and_then(Value::as_str)
            && let Ok(re) = regex::Regex::new(p)
            && !re.is_match(text)
        {
            self.err(path, format!("{} does not match pattern {p}", short(&Value::String(text.to_string()))));
        }
        if let Some(f) = s.get("format").and_then(Value::as_str)
            && !format_ok(f, text)
        {
            self.err(path, format!("{} is not a valid {f}", short(&Value::String(text.to_string()))));
        }
    }

    fn array(&mut self, s: &Map<String, Value>, items: &[Value], path: &str, depth: usize) {
        let n = items.len() as u64;
        if let Some(min) = s.get("minItems").and_then(Value::as_u64)
            && n < min
        {
            self.err(path, format!("has {n} items, fewer than minItems {min}"));
        }
        if let Some(max) = s.get("maxItems").and_then(Value::as_u64)
            && n > max
        {
            self.err(path, format!("has {n} items, more than maxItems {max}"));
        }
        if s.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
            'outer: for i in 0..items.len() {
                for j in i + 1..items.len() {
                    if json_eq(&items[i], &items[j]) {
                        self.err(path, format!("items {i} and {j} are equal but uniqueItems is set"));
                        break 'outer;
                    }
                }
            }
        }
        let prefix = s.get("prefixItems").and_then(Value::as_array);
        let skip = prefix.map_or(0, |p| p.len());
        if let Some(p) = prefix {
            for (i, (sub, item)) in p.iter().zip(items).enumerate() {
                self.check(sub, item, &format!("{path}[{i}]"), depth + 1);
            }
        }
        if let Some(item_schema) = s.get("items") {
            for (i, item) in items.iter().enumerate().skip(skip) {
                self.check(item_schema, item, &format!("{path}[{i}]"), depth + 1);
            }
        }
        if let Some(c) = s.get("contains")
            && !items.iter().any(|i| is_valid(self.spec, c, i, self.dir, depth + 1))
        {
            self.err(path, "no item matches `contains`");
        }
    }

    fn object(&mut self, s: &Map<String, Value>, obj: &Map<String, Value>, path: &str, depth: usize) {
        let props = s.get("properties").and_then(Value::as_object);
        if let Some(Value::Array(req)) = s.get("required") {
            for r in req.iter().filter_map(Value::as_str) {
                if obj.contains_key(r) {
                    continue;
                }
                // readOnly props aren't sent by clients; writeOnly ones aren't returned.
                let prop = props.and_then(|p| p.get(r)).map(|p| self.spec.resolve(p));
                let skip = prop.is_some_and(|p| match self.dir {
                    Direction::Request => p.get("readOnly").and_then(Value::as_bool) == Some(true),
                    Direction::Response => p.get("writeOnly").and_then(Value::as_bool) == Some(true),
                });
                if !skip {
                    self.err(path, format!("missing required property `{r}`"));
                }
            }
        }
        let n = obj.len() as u64;
        if let Some(min) = s.get("minProperties").and_then(Value::as_u64)
            && n < min
        {
            self.err(path, format!("has {n} properties, fewer than minProperties {min}"));
        }
        if let Some(max) = s.get("maxProperties").and_then(Value::as_u64)
            && n > max
        {
            self.err(path, format!("has {n} properties, more than maxProperties {max}"));
        }
        let patterns: Vec<(regex::Regex, &Value)> = s
            .get("patternProperties")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(p, sch)| regex::Regex::new(p).ok().map(|re| (re, sch))).collect())
            .unwrap_or_default();
        let mut seen: HashSet<&str> = HashSet::new();
        for (k, val) in obj {
            let child = child_path(path, k);
            if let Some(sub) = props.and_then(|p| p.get(k)) {
                seen.insert(k);
                self.check(sub, val, &child, depth + 1);
            }
            for (re, sub) in &patterns {
                if re.is_match(k) {
                    seen.insert(k);
                    self.check(sub, val, &child, depth + 1);
                }
            }
            if seen.contains(k.as_str()) {
                continue;
            }
            match s.get("additionalProperties") {
                Some(Value::Bool(false)) => self.err(&child, "is not allowed (additionalProperties: false)"),
                Some(sub @ Value::Object(_)) => self.check(sub, val, &child, depth + 1),
                _ => {}
            }
        }
    }
}

fn child_path(path: &str, key: &str) -> String {
    if key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !key.is_empty() { format!("{path}.{key}") } else { format!("{path}[{}]", serde_json::to_string(key).unwrap_or_default()) }
}

fn all_digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

fn date_ok(s: &str) -> bool {
    let p: Vec<&str> = s.split('-').collect();
    if p.len() != 3 || !all_digits(p[0], 4) || !all_digits(p[1], 2) || !all_digits(p[2], 2) {
        return false;
    }
    let (m, d): (u32, u32) = (p[1].parse().unwrap_or(0), p[2].parse().unwrap_or(0));
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

fn time_ok(s: &str) -> bool {
    // HH:MM:SS(.frac)?(Z|±HH:MM)? — offset optional to accept common sloppy values.
    let (clock, _) = match s.find(['Z', 'z', '+']).or_else(|| s.rfind('-').filter(|i| *i > 5)) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let clock = clock.split('.').next().unwrap_or("");
    let p: Vec<&str> = clock.split(':').collect();
    p.len() == 3 && p.iter().all(|x| all_digits(x, 2)) && p[0] < "24" && p[1] < "60" && p[2] <= "60"
}

pub(crate) fn format_ok(format: &str, s: &str) -> bool {
    match format {
        "date" => date_ok(s),
        "date-time" => match s.find(['T', 't', ' ']) {
            Some(i) => date_ok(&s[..i]) && time_ok(&s[i + 1..]),
            None => false,
        },
        "time" => time_ok(s),
        "email" => {
            let mut parts = s.splitn(2, '@');
            let (local, domain) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !s.contains(char::is_whitespace)
        }
        "uuid" => {
            let b = s.as_bytes();
            s.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_hexdigit() })
        }
        "uri" | "url" => url::Url::parse(s).is_ok(),
        "ipv4" => s.parse::<std::net::Ipv4Addr>().is_ok(),
        "ipv6" => s.parse::<std::net::Ipv6Addr>().is_ok(),
        "byte" => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(s.trim()).is_ok()
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(v: &str) -> Spec {
        Spec::parse(&format!(
            r##"{{"openapi":"{v}","info":{{"title":"t","version":"1"}},"paths":{{}},"components":{{"schemas":{{
            "User":{{"type":"object","required":["id","email","password"],"additionalProperties":false,"properties":{{
                "id":{{"type":"integer","format":"int64","readOnly":true,"minimum":1}},
                "email":{{"type":"string","format":"email"}},
                "password":{{"type":"string","writeOnly":true,"minLength":8}},
                "nick":{{"type":"string","nullable":true,"maxLength":5}},
                "role":{{"enum":["admin","user"]}},
                "tags":{{"type":"array","items":{{"type":"string"}},"uniqueItems":true,"maxItems":3}},
                "born":{{"type":"string","format":"date"}}
            }}}},
            "Pet":{{"oneOf":[{{"$ref":"#/components/schemas/Cat"}},{{"$ref":"#/components/schemas/Dog"}}],"discriminator":{{"propertyName":"kind"}}}},
            "Cat":{{"type":"object","required":["kind","lives"],"properties":{{"kind":{{"const":"Cat"}},"lives":{{"type":"integer","maximum":9}}}}}},
            "Dog":{{"type":"object","required":["kind"],"properties":{{"kind":{{"const":"Dog"}},"good":{{"type":"boolean"}}}}}}
        }}}}}}"##
        ))
        .unwrap()
    }

    fn errs(s: &Spec, schema: &str, v: Value, dir: Direction) -> Vec<String> {
        validate(s, &json!({ "$ref": format!("#/components/schemas/{schema}") }), &v, dir).into_iter().map(|e| e.to_string()).collect()
    }

    #[test]
    fn objects_and_read_write_only() {
        let s = spec("3.0.3");
        assert!(errs(&s, "User", json!({"id": 1, "email": "a@b.co"}), Direction::Response).is_empty(), "password is writeOnly");
        assert!(errs(&s, "User", json!({"email": "a@b.co", "password": "longenough"}), Direction::Request).is_empty(), "id is readOnly");
        let e = errs(&s, "User", json!({"id": 0, "email": "nope", "nick": "toolong", "role": "root", "tags": ["a", "a"], "born": "2024-13-01", "x": 1}), Direction::Response);
        let joined = e.join("\n");
        for needle in ["$.id: 0 is less than the minimum 1", "$.email: \"nope\" is not a valid email", "$.nick: is 7 characters", "$.role: \"root\" is not one of", "$.tags: items 0 and 1 are equal", "$.born: \"2024-13-01\" is not a valid date", "$.x: is not allowed"] {
            assert!(joined.contains(needle), "missing {needle} in\n{joined}");
        }
        assert!(errs(&s, "User", json!({"id": 2, "email": "a@b.co", "nick": null}), Direction::Response).is_empty(), "nullable");
        assert_eq!(errs(&s, "User", json!([]), Direction::Response), vec!["$: expected object, got array []".to_string()]);
        assert_eq!(errs(&s, "User", json!({"id": 1.0, "email": "a@b.co"}), Direction::Response), Vec::<String>::new(), "1.0 is an integer");
    }

    #[test]
    fn one_of_with_discriminator_and_31_types() {
        let s = spec("3.0.3");
        assert!(errs(&s, "Pet", json!({"kind": "Cat", "lives": 9}), Direction::Response).is_empty());
        assert_eq!(errs(&s, "Pet", json!({"kind": "Cat", "lives": 10}), Direction::Response), vec!["$.lives: 10 is greater than the maximum 9".to_string()]);
        assert!(errs(&s, "Pet", json!({"kind": "Dog"}), Direction::Response).is_empty());
        let s31 = spec("3.1.0");
        let schema = json!({ "type": ["string", "null"], "exclusiveMinimum": 3 });
        assert!(validate(&s31, &schema, &Value::Null, Direction::Response).is_empty());
        assert!(!validate(&s31, &schema, &json!(5), Direction::Response).is_empty());
        assert!(!validate(&s31, &json!({ "type": "number", "exclusiveMinimum": 3 }), &json!(3), Direction::Response).is_empty());
        assert!(validate(&s31, &json!({ "type": "number", "exclusiveMinimum": 3 }), &json!(3.5), Direction::Response).is_empty());
        let any = json!({ "anyOf": [{ "type": "string" }, { "type": "integer", "minimum": 10 }] });
        assert!(validate(&s31, &any, &json!(12), Direction::Response).is_empty());
        assert_eq!(validate(&s31, &any, &json!(5), Direction::Response)[0].message, "5 is less than the minimum 10 (no anyOf alternative matches)");
        let dangling = json!({ "$ref": "#/components/schemas/Nope" });
        assert!(validate(&s31, &dangling, &json!(1), Direction::Response)[0].message.contains("does not resolve"));
        assert!(validate(&s31, &json!({ "$ref": "other.yaml#/X" }), &json!(1), Direction::Response).is_empty());
    }

    #[test]
    fn formats() {
        for (f, ok, bad) in [
            ("date-time", "2024-03-05T07:08:09.123Z", "2024-03-05"),
            ("date-time", "2024-03-05T07:08:09+02:00", "yesterday"),
            ("uuid", "123e4567-e89b-12d3-a456-426614174000", "123e4567e89b12d3a456426614174000"),
            ("uri", "https://example.com/x", "not a uri"),
            ("ipv4", "10.0.0.1", "10.0.0.256"),
            ("time", "23:59:60", "24:00:00"),
        ] {
            assert!(format_ok(f, ok), "{f} {ok}");
            assert!(!format_ok(f, bad), "{f} {bad}");
        }
    }
}
