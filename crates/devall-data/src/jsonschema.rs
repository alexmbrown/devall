//! JSON Schema / OpenAPI → generator schema (json-schema-faker semantics, the common subset):
//! `type`, `format`, `enum`, `const`, `minimum`/`maximum` (and exclusive forms), `minLength`/
//! `maxLength`, `pattern`, `items`, `minItems`/`maxItems`, `properties`, `required`,
//! `nullable`, `allOf` (merged), `oneOf`/`anyOf` (first branch), and local `$ref`s.
//!
//! An OpenAPI (or Swagger 2) document becomes one entity per `components.schemas`
//! (`definitions`) object schema; a plain schema becomes one entity.

use serde_json::{Map, Value};

use crate::error::{DataError, Result};
use crate::infer::{infer_kind, norm_key, string_kind_for_key};
use crate::model::{Entity, Field, FieldKind, Schema};

const DEFAULT_COUNT: u64 = 10;
const MAX_REF_DEPTH: usize = 12;

/// Parses JSON or YAML text and converts it (see [`from_json_schema`]).
pub fn from_json_schema_text(text: &str) -> Result<Schema> {
    let doc: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(json_err) => serde_yaml_ng::from_str(text).map_err(|yaml_err| DataError::Parse(format!("not JSON ({json_err}) or YAML ({yaml_err})")))?,
    };
    from_json_schema(&doc)
}

pub fn from_json_schema(doc: &Value) -> Result<Schema> {
    let obj = doc.as_object().ok_or_else(|| DataError::Parse("a JSON Schema must be an object".into()))?;
    let mut schema = Schema::new(obj.get("title").or_else(|| obj.pointer_get("info", "title")).and_then(Value::as_str).unwrap_or("Imported schema"));
    let cv = Converter { root: doc };

    let named = obj
        .get("components")
        .and_then(|c| c.get("schemas"))
        .or_else(|| if obj.contains_key("swagger") || obj.contains_key("openapi") { obj.get("definitions") } else { None })
        .and_then(Value::as_object);
    if obj.contains_key("openapi") || obj.contains_key("swagger") {
        let defs = named.ok_or_else(|| DataError::Parse("the OpenAPI document has no components.schemas".into()))?;
        for (name, s) in defs {
            let resolved = cv.resolve(s, 0);
            if is_object_schema(&resolved) {
                schema.entities.push(Entity::new(name, DEFAULT_COUNT, cv.fields(&resolved, 0)));
            }
        }
        if schema.entities.is_empty() {
            return Err(DataError::Parse("no object schemas found in components.schemas".into()));
        }
        return Ok(schema);
    }

    let root = cv.resolve(doc, 0);
    let entity_name = obj.get("title").and_then(Value::as_str).map(entity_name).unwrap_or_else(|| "items".into());
    if root.get("type").and_then(Value::as_str) == Some("array") || (root.get("items").is_some() && root.get("properties").is_none()) {
        let items = root.get("items").map(|i| cv.resolve(i, 0)).unwrap_or(Value::Null);
        let count = root.get("minItems").and_then(Value::as_u64).unwrap_or(DEFAULT_COUNT).max(1);
        let fields = if is_object_schema(&items) { cv.fields(&items, 0) } else { vec![cv.field("value", &items, 0)] };
        schema.entities.push(Entity::new(entity_name, count, fields));
    } else if is_object_schema(&root) {
        schema.entities.push(Entity::new(entity_name, DEFAULT_COUNT, cv.fields(&root, 0)));
    } else if let Some(defs) = obj.get("$defs").or_else(|| obj.get("definitions")).and_then(Value::as_object) {
        for (name, s) in defs {
            let resolved = cv.resolve(s, 0);
            if is_object_schema(&resolved) {
                schema.entities.push(Entity::new(name, DEFAULT_COUNT, cv.fields(&resolved, 0)));
            }
        }
    } else {
        schema.entities.push(Entity::new(entity_name, DEFAULT_COUNT, vec![cv.field("value", &root, 0)]));
    }
    if schema.entities.is_empty() {
        return Err(DataError::Parse("no object schema found".into()));
    }
    Ok(schema)
}

fn entity_name(title: &str) -> String {
    let n: String = title.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    if n.is_empty() { "items".into() } else { n }
}

trait PointerGet {
    fn pointer_get(&self, a: &str, b: &str) -> Option<&Value>;
}
impl PointerGet for Map<String, Value> {
    fn pointer_get(&self, a: &str, b: &str) -> Option<&Value> {
        self.get(a).and_then(|x| x.get(b))
    }
}

fn is_object_schema(s: &Value) -> bool {
    s.get("properties").is_some_and(Value::is_object)
}

fn type_names(s: &Value) -> Vec<&str> {
    match s.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

struct Converter<'a> {
    root: &'a Value,
}

impl Converter<'_> {
    /// Follows local `$ref`s and merges `allOf`, so callers see a plain schema.
    fn resolve(&self, s: &Value, depth: usize) -> Value {
        if depth > MAX_REF_DEPTH {
            return Value::Object(Map::new());
        }
        let Some(obj) = s.as_object() else { return s.clone() };
        if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
            let target = r.strip_prefix('#').and_then(|p| self.root.pointer(&p.replace("~1", "/").replace("~0", "~")));
            let mut base = target.map(|t| self.resolve(t, depth + 1)).unwrap_or_else(|| Value::Object(Map::new()));
            // Sibling keywords next to $ref (OpenAPI 3.1 allows e.g. `description`, `nullable`).
            if let Value::Object(b) = &mut base {
                for (k, v) in obj.iter().filter(|(k, _)| *k != "$ref") {
                    b.insert(k.clone(), v.clone());
                }
            }
            return base;
        }
        if let Some(all) = obj.get("allOf").and_then(Value::as_array) {
            let mut merged = obj.clone();
            merged.remove("allOf");
            let mut props = merged.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
            let mut required: Vec<Value> = merged.get("required").and_then(Value::as_array).cloned().unwrap_or_default();
            for part in all {
                let part = self.resolve(part, depth + 1);
                if let Some(p) = part.get("properties").and_then(Value::as_object) {
                    for (k, v) in p {
                        props.insert(k.clone(), v.clone());
                    }
                }
                if let Some(r) = part.get("required").and_then(Value::as_array) {
                    required.extend(r.iter().cloned());
                }
                if let Some(o) = part.as_object() {
                    for (k, v) in o.iter().filter(|(k, _)| !matches!(k.as_str(), "properties" | "required")) {
                        merged.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
            if !props.is_empty() {
                merged.insert("properties".into(), Value::Object(props));
                merged.entry("type").or_insert_with(|| Value::String("object".into()));
            }
            if !required.is_empty() {
                merged.insert("required".into(), Value::Array(required));
            }
            return Value::Object(merged);
        }
        s.clone()
    }

    fn fields(&self, s: &Value, depth: usize) -> Vec<Field> {
        let Some(props) = s.get("properties").and_then(Value::as_object) else { return Vec::new() };
        props.iter().map(|(name, p)| self.field(name, p, depth + 1)).collect()
    }

    fn field(&self, name: &str, s: &Value, depth: usize) -> Field {
        let (kind, nullable) = self.kind(name, s, depth);
        let mut f = Field::new(name, kind);
        if nullable {
            f.null_percent = 10.0;
        }
        f
    }

    /// The generator for a (sub)schema and whether it allows `null`.
    fn kind(&self, key: &str, s: &Value, depth: usize) -> (FieldKind, bool) {
        if depth > MAX_REF_DEPTH {
            return (FieldKind::Json { value: Value::Null }, true);
        }
        let s = self.resolve(s, depth);
        let Some(obj) = s.as_object() else {
            // `true` / `{}` schemas: anything goes.
            return (FieldKind::Word, false);
        };
        let mut nullable = obj.get("nullable").and_then(Value::as_bool).unwrap_or(false) || type_names(&s).contains(&"null");

        if let Some(branches) = obj.get("oneOf").or_else(|| obj.get("anyOf")).and_then(Value::as_array) {
            let non_null: Vec<&Value> = branches.iter().filter(|b| !type_names(b).eq(&["null"])).collect();
            nullable |= non_null.len() < branches.len();
            if let Some(first) = non_null.first() {
                let (k, n) = self.kind(key, first, depth + 1);
                return (k, nullable || n);
            }
        }
        if let Some(c) = obj.get("const") {
            return (FieldKind::Json { value: c.clone() }, false);
        }
        if let Some(values) = obj.get("enum").and_then(Value::as_array) {
            let vals: Vec<Value> = values.iter().filter(|v| !v.is_null()).cloned().collect();
            nullable |= vals.len() < values.len();
            if !vals.is_empty() {
                return (FieldKind::Enum { values: vals, weights: None }, nullable);
            }
        }

        let types = type_names(&s);
        let ty = types.iter().copied().find(|t| *t != "null").unwrap_or_else(|| {
            if obj.contains_key("properties") {
                "object"
            } else if obj.contains_key("items") {
                "array"
            } else if obj.contains_key("format") || obj.contains_key("pattern") || obj.contains_key("minLength") {
                "string"
            } else if obj.contains_key("minimum") || obj.contains_key("maximum") {
                "number"
            } else {
                ""
            }
        });
        let kind = match ty {
            "string" => self.string_kind(key, obj),
            "integer" => int_kind(key, obj),
            "number" => number_kind(key, obj),
            "boolean" => FieldKind::Bool { true_percent: None },
            "array" => {
                let items = obj.get("items").cloned().unwrap_or(Value::Bool(true));
                let (of, _) = self.kind(&crate::infer::singular(key), &items, depth + 1);
                let min = obj.get("minItems").and_then(Value::as_u64).unwrap_or(1).min(1000) as u32;
                let max = obj.get("maxItems").and_then(Value::as_u64).map(|m| m.min(1000) as u32).unwrap_or(min.max(3)).max(min);
                FieldKind::Array { of: Box::new(of), min: Some(min), max: Some(max) }
            }
            "object" => {
                if obj.get("properties").and_then(Value::as_object).is_some_and(|p| !p.is_empty()) {
                    FieldKind::Object { fields: self.fields(&s, depth) }
                } else {
                    FieldKind::Json { value: Value::Object(Map::new()) }
                }
            }
            "null" => FieldKind::Json { value: Value::Null },
            _ => match obj.get("example").or_else(|| obj.get("examples").and_then(|e| e.get(0))).or_else(|| obj.get("default")) {
                Some(example) => infer_kind(key, &[example]),
                None => FieldKind::Word,
            },
        };
        (kind, nullable)
    }

    fn string_kind(&self, key: &str, obj: &Map<String, Value>) -> FieldKind {
        let min_len = obj.get("minLength").and_then(Value::as_u64);
        let max_len = obj.get("maxLength").and_then(Value::as_u64);
        if let Some(format) = obj.get("format").and_then(Value::as_str) {
            let kind = match format {
                "email" | "idn-email" => Some(FieldKind::email()),
                "uuid" | "guid" => Some(FieldKind::uuid()),
                "ulid" => Some(FieldKind::Ulid),
                "date" => Some(FieldKind::Date { from: None, to: None, format: None }),
                "date-time" => Some(FieldKind::Datetime { from: None, to: None, format: None }),
                "time" => Some(FieldKind::Regex { pattern: "([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]".into() }),
                "uri" | "url" | "iri" | "uri-reference" | "iri-reference" => Some(FieldKind::Url),
                "hostname" | "idn-hostname" => Some(FieldKind::Domain),
                "ipv4" => Some(FieldKind::Ipv4),
                "ipv6" => Some(FieldKind::Ipv6),
                "password" => Some(FieldKind::Password { min_length: min_len.map(|v| v.max(1) as u32), max_length: max_len.map(|v| v.max(1) as u32) }),
                "phone" | "tel" => Some(FieldKind::Phone),
                "byte" => Some(FieldKind::Regex { pattern: "[A-Za-z0-9+/]{16}".into() }),
                "binary" => Some(FieldKind::Regex { pattern: "[0-9a-f]{32}".into() }),
                _ => None,
            };
            if let Some(k) = kind {
                return k;
            }
        }
        if let Some(p) = obj.get("pattern").and_then(Value::as_str) {
            return FieldKind::Regex { pattern: p.to_string() };
        }
        if let Some(k) = string_kind_for_key(key) {
            // Fakers can't promise a length: only use them when the bounds leave room.
            let room = match &k {
                FieldKind::Paragraph { .. } | FieldKind::Sentence { .. } => 200,
                _ => 40,
            };
            if max_len.is_none_or(|m| m >= room) && min_len.is_none_or(|m| m <= 2) {
                return k;
            }
        }
        let nk = norm_key(key);
        if nk == "name" && min_len.is_none() && max_len.is_none() {
            return FieldKind::FullName;
        }
        match (min_len, max_len) {
            (None, None) => FieldKind::Word,
            (min, max) => {
                let lo = min.unwrap_or(1).min(256);
                let hi = max.unwrap_or(lo.max(12)).clamp(lo, 256.max(lo));
                FieldKind::Regex { pattern: if lo == hi { format!("[a-z]{{{lo}}}") } else { format!("[a-z]{{{lo},{hi}}}") } }
            }
        }
    }
}

fn bound(obj: &Map<String, Value>, inclusive: &str, exclusive: &str) -> Option<(f64, bool)> {
    if let Some(v) = obj.get(inclusive).and_then(Value::as_f64) {
        // Draft 4: `exclusiveMinimum: true` modifies `minimum`.
        let excl = obj.get(exclusive).and_then(Value::as_bool).unwrap_or(false);
        return Some((v, excl));
    }
    obj.get(exclusive).and_then(Value::as_f64).map(|v| (v, true))
}

fn int_kind(key: &str, obj: &Map<String, Value>) -> FieldKind {
    let min = bound(obj, "minimum", "exclusiveMinimum").map(|(v, excl)| if excl { v.floor() as i64 + 1 } else { v.ceil() as i64 });
    let max = bound(obj, "maximum", "exclusiveMaximum").map(|(v, excl)| if excl { v.ceil() as i64 - 1 } else { v.floor() as i64 });
    let k = norm_key(key);
    if min.is_none() && max.is_none() {
        if k == "id" {
            return FieldKind::sequence();
        }
        if obj.get("format").and_then(Value::as_str) == Some("int64") && (k.ends_with("at") || k.contains("timestamp")) {
            return FieldKind::Timestamp { from: None, to: None, unit: None };
        }
    }
    let lo = min.unwrap_or_else(|| max.map(|m| m.saturating_sub(1000).min(0)).unwrap_or(0));
    let hi = max.unwrap_or_else(|| lo.saturating_add(1000));
    FieldKind::Int { min: Some(lo), max: Some(hi.max(lo)) }
}

fn number_kind(key: &str, obj: &Map<String, Value>) -> FieldKind {
    let k = norm_key(key);
    let min = bound(obj, "minimum", "exclusiveMinimum").map(|(v, _)| v);
    let max = bound(obj, "maximum", "exclusiveMaximum").map(|(v, _)| v);
    if min.is_none() && max.is_none() {
        if k == "lat" || k == "latitude" {
            return FieldKind::Latitude;
        }
        if k == "lng" || k == "lon" || k == "longitude" {
            return FieldKind::Longitude;
        }
    }
    let lo = min.unwrap_or_else(|| max.map(|m| (m - 1000.0).min(0.0)).unwrap_or(0.0));
    let hi = max.unwrap_or(lo + 1000.0).max(lo);
    if ["price", "amount", "cost", "total", "balance", "fee", "salary"].iter().any(|m| k.ends_with(m)) {
        return FieldKind::Price { min: Some(lo), max: Some(hi), decimals: Some(2) };
    }
    FieldKind::Float { min: Some(lo), max: Some(hi), decimals: Some(2) }
}
