//! "Improve names with AI" for OpenAPI drafts inferred from traffic.
//!
//! The deterministic generator (devall-proxy) names operations after their method and path
//! (`getUsersOrdersByUserId`), leaves summaries as `GET /users/{userId}`, adds no tags and keeps
//! body schemas inline. This pass sends a compact *outline* — operations, parameters, top-level
//! body property names (never example values) — and asks for operationIds, summaries, tags and
//! names for the body schemas. The answer is schema-validated (one repair retry) and turned into
//! a patch over the deterministic document: renamed operations, top-level `tags`, and inline body
//! schemas moved to `components/schemas/<Name>` behind `$ref`s. Nothing is applied until the
//! user saves the reviewed result.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::prompts::fence;
use crate::{AiError, Result};

const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace"];

/// A body schema the model may name.
#[derive(Debug, Clone, PartialEq)]
pub struct BodySlot {
    /// `B1`, `B2`…
    pub id: String,
    /// `GET /users/{id}`.
    pub operation: String,
    /// JSON pointer of the schema (`/paths/~1users/get/responses/200/content/application~1json/schema`).
    pub pointer: String,
    /// Name the items of an array instead of the array.
    pub array_items: bool,
    /// `request body`, `200 response`.
    pub place: String,
    /// `object {id, email}`, `array of {id}`.
    pub shape: String,
}

impl BodySlot {
    pub fn label(&self) -> String {
        format!("{} {}", self.place, self.shape)
    }
}

/// Parses YAML or JSON spec text.
pub fn parse_spec(text: &str) -> Result<Value> {
    let t = text.trim();
    let v: Value = if t.starts_with('{') {
        serde_json::from_str(t).map_err(|e| AiError::Invalid(format!("the spec is not valid JSON: {e}")))?
    } else {
        serde_yaml_ng::from_str(t).map_err(|e| AiError::Invalid(format!("the spec is not valid YAML: {e}")))?
    };
    if v.get("paths").and_then(Value::as_object).is_none() {
        return Err(AiError::Invalid("not an OpenAPI document (no `paths`)".into()));
    }
    Ok(v)
}

fn esc(seg: &str) -> String {
    seg.replace('~', "~0").replace('/', "~1")
}

fn props_of(schema: &Value) -> String {
    let names: Vec<&str> = schema["properties"].as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
    let more = if names.len() > 12 { format!(", …{} more", names.len() - 12) } else { String::new() };
    format!("{{{}{more}}}", names.iter().take(12).copied().collect::<Vec<_>>().join(", "))
}

fn is_object(s: &Value) -> bool {
    s.get("$ref").is_none() && (s["type"] == "object" || s["type"].as_array().is_some_and(|a| a.contains(&json!("object")))) && s["properties"].is_object()
}

/// Operations in document order: (key, method, path, operation object).
pub fn operations(spec: &Value) -> Vec<(String, String, String, Value)> {
    let mut out = Vec::new();
    if let Some(paths) = spec["paths"].as_object() {
        for (path, item) in paths {
            for m in METHODS {
                if let Some(op) = item.get(*m).filter(|o| o.is_object()) {
                    out.push((format!("{} {path}", m.to_ascii_uppercase()), m.to_string(), path.clone(), op.clone()));
                }
            }
        }
    }
    out
}

/// Inline JSON body schemas worth naming (objects, or arrays of objects).
pub fn body_slots(spec: &Value) -> Vec<BodySlot> {
    let mut slots = Vec::new();
    for (key, method, path, op) in operations(spec) {
        let base = format!("/paths/{}/{method}", esc(&path));
        let mut add = |ptr: String, schema: &Value, where_: String| {
            if is_object(schema) {
                slots.push(BodySlot { id: String::new(), operation: key.clone(), pointer: ptr, array_items: false, place: where_, shape: format!("object {}", props_of(schema)) });
            } else if schema["type"] == "array" && is_object(&schema["items"]) {
                slots.push(BodySlot { id: String::new(), operation: key.clone(), pointer: ptr, array_items: true, place: where_, shape: format!("array of {}", props_of(&schema["items"])) });
            }
        };
        if let Some(content) = op["requestBody"]["content"].as_object() {
            for (media, m) in content {
                if media.contains("json") {
                    add(format!("{base}/requestBody/content/{}/schema", esc(media)), &m["schema"], "request body".into());
                    break;
                }
            }
        }
        if let Some(responses) = op["responses"].as_object() {
            for (status, r) in responses {
                if let Some(content) = r["content"].as_object() {
                    for (media, m) in content {
                        if media.contains("json") {
                            add(format!("{base}/responses/{}/content/{}/schema", esc(status), esc(media)), &m["schema"], format!("{status} response"));
                            break;
                        }
                    }
                }
            }
        }
    }
    for (i, s) in slots.iter_mut().enumerate() {
        s.id = format!("B{}", i + 1);
    }
    slots
}

/// The compact outline sent to the model (names and shapes only, no values).
pub fn outline(spec: &Value) -> String {
    let mut s = String::new();
    if let Some(t) = spec.pointer("/info/title").and_then(Value::as_str) {
        s.push_str(&format!("API: {t}\n"));
    }
    s.push_str("Operations:\n");
    let slots = body_slots(spec);
    for (key, _, _, op) in operations(spec) {
        let params: Vec<String> = op["parameters"]
            .as_array()
            .map(|a| a.iter().filter_map(|p| Some(format!("{}({})", p["name"].as_str()?, p["in"].as_str().unwrap_or("?")))).collect())
            .unwrap_or_default();
        let statuses: Vec<&str> = op["responses"].as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
        s.push_str(&format!(
            "- {key} | operationId: {} | params: {} | responses: {}\n",
            op["operationId"].as_str().unwrap_or("-"),
            if params.is_empty() { "none".into() } else { params.join(", ") },
            statuses.join(", ")
        ));
        for b in slots.iter().filter(|b| b.operation == key) {
            s.push_str(&format!("    {}: {}\n", b.id, b.label()));
        }
    }
    s
}

pub fn naming_schema() -> Value {
    let obj = |props: Value| {
        let required: Vec<Value> = props.as_object().map(|o| o.keys().cloned().map(Value::String).collect()).unwrap_or_default();
        json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
    };
    obj(json!({
        "operations": { "type": "array", "items": obj(json!({ "key": { "type": "string" }, "operationId": { "type": "string" }, "summary": { "type": "string" }, "tags": { "type": "array", "items": { "type": "string" } } })) },
        "schemas": { "type": "array", "items": obj(json!({ "body": { "type": "string" }, "name": { "type": "string" } })) },
        "tags": { "type": "array", "items": obj(json!({ "name": { "type": "string" }, "description": { "type": "string" } })) }
    }))
}

pub const NAMING_SYSTEM: &str = "You name the operations of an OpenAPI document that was inferred from captured HTTP traffic. \
For every operation give: a camelCase operationId that says what it does (listUsers, getUser, createOrder, cancelOrder — verbs list/get/create/update/replace/delete/search plus the resource), \
a short summary in sentence case (\"List users\"), and one or two tags naming the resource group (\"Users\"). \
For the body schemas (B1, B2…) give PascalCase singular model names (User, Order, CreateOrderRequest, ErrorResponse); reuse the same name when two bodies are clearly the same model. \
Reply with JSON only: {\"operations\": [{\"key\": \"GET /path\", \"operationId\": string, \"summary\": string, \"tags\": [string]}], \"schemas\": [{\"body\": \"B1\", \"name\": string}], \"tags\": [{\"name\": string, \"description\": string}]}. \
Use the operation keys exactly as given.";

fn ident_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_]{0,79}$").unwrap())
}

fn model_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[A-Z][A-Za-z0-9]{0,79}$").unwrap())
}

/// Problems with a proposal (unknown keys, invalid or duplicate ids/names).
pub fn check_proposal(v: &Value, spec: &Value) -> std::result::Result<(), String> {
    let keys: BTreeSet<String> = operations(spec).into_iter().map(|o| o.0).collect();
    let slots: BTreeSet<String> = body_slots(spec).into_iter().map(|b| b.id).collect();
    let mut problems = Vec::new();
    let mut ids = BTreeSet::new();
    for o in v["operations"].as_array().into_iter().flatten() {
        let key = o["key"].as_str().unwrap_or_default();
        if !keys.contains(key) {
            problems.push(format!("unknown operation key `{key}`"));
        }
        let id = o["operationId"].as_str().unwrap_or_default();
        if !ident_re().is_match(id) {
            problems.push(format!("operationId `{id}` is not a valid identifier"));
        } else if !ids.insert(id.to_string()) {
            problems.push(format!("operationId `{id}` is used twice"));
        }
    }
    for sch in v["schemas"].as_array().into_iter().flatten() {
        let b = sch["body"].as_str().unwrap_or_default();
        if !slots.contains(b) {
            problems.push(format!("unknown body `{b}`"));
        }
        let n = sch["name"].as_str().unwrap_or_default();
        if !model_re().is_match(n) {
            problems.push(format!("schema name `{n}` is not PascalCase"));
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(format!("{}.", problems.join("; "))) }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationName {
    pub key: String,
    pub operation_id: String,
    pub summary: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaName {
    pub body: String,
    pub name: String,
    /// `GET /users → 200 response` (filled in by the app).
    #[serde(default)]
    pub location: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamingProposal {
    pub operations: Vec<OperationName>,
    pub schemas: Vec<SchemaName>,
    pub tags: Vec<TagInfo>,
}

/// The valid parts of a model answer (invalid or unknown entries are dropped).
pub fn clean_proposal(v: &Value, spec: &Value) -> NamingProposal {
    let keys: BTreeSet<String> = operations(spec).into_iter().map(|o| o.0).collect();
    let slots = body_slots(spec);
    let mut used = BTreeSet::new();
    let mut p = NamingProposal::default();
    for o in v["operations"].as_array().into_iter().flatten() {
        let key = o["key"].as_str().unwrap_or_default().trim().to_string();
        let id = o["operationId"].as_str().unwrap_or_default().trim().to_string();
        if !keys.contains(&key) || !ident_re().is_match(&id) || !used.insert(id.clone()) || p.operations.iter().any(|x| x.key == key) {
            continue;
        }
        let tags: Vec<String> = o["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).take(3).collect();
        p.operations.push(OperationName { key, operation_id: id, summary: o["summary"].as_str().unwrap_or_default().trim().chars().take(120).collect(), tags });
    }
    for sch in v["schemas"].as_array().into_iter().flatten() {
        let body = sch["body"].as_str().unwrap_or_default().trim().to_string();
        let name = sch["name"].as_str().unwrap_or_default().trim().to_string();
        let Some(slot) = slots.iter().find(|s| s.id == body) else { continue };
        if !model_re().is_match(&name) || p.schemas.iter().any(|x| x.body == body) {
            continue;
        }
        p.schemas.push(SchemaName { location: format!("{} → {}", slot.operation, slot.place), body, name });
    }
    let used_tags: BTreeSet<&str> = p.operations.iter().flat_map(|o| o.tags.iter().map(String::as_str)).collect();
    for t in v["tags"].as_array().into_iter().flatten() {
        let name = t["name"].as_str().unwrap_or_default().trim().to_string();
        if used_tags.contains(name.as_str()) && !p.tags.iter().any(|x| x.name == name) {
            p.tags.push(TagInfo { name, description: t["description"].as_str().unwrap_or_default().trim().to_string() });
        }
    }
    for t in used_tags {
        if !p.tags.iter().any(|x| x.name == t) {
            p.tags.push(TagInfo { name: t.to_string(), description: String::new() });
        }
    }
    p
}

/// Applies a proposal to the deterministic document. Returns the new document and a list of the
/// changes made (for the review).
pub fn apply_proposal(spec: &Value, p: &NamingProposal) -> (Value, Vec<String>) {
    let mut doc = spec.clone();
    let mut changes = Vec::new();
    // Operations.
    for o in &p.operations {
        let Some((method, path)) = o.key.split_once(' ') else { continue };
        let ptr = format!("/paths/{}/{}", esc(path), method.to_ascii_lowercase());
        let Some(op) = doc.pointer_mut(&ptr).and_then(Value::as_object_mut) else { continue };
        let old = op.get("operationId").and_then(Value::as_str).unwrap_or("").to_string();
        if old != o.operation_id {
            changes.push(format!("{}: operationId {old} → {}", o.key, o.operation_id));
        }
        op.insert("operationId".into(), json!(o.operation_id));
        if !o.summary.is_empty() {
            op.insert("summary".into(), json!(o.summary));
        }
        if !o.tags.is_empty() {
            op.insert("tags".into(), json!(o.tags));
        }
    }
    if !p.tags.is_empty() {
        let tags: Vec<Value> = p.tags.iter().map(|t| if t.description.is_empty() { json!({ "name": t.name }) } else { json!({ "name": t.name, "description": t.description }) }).collect();
        changes.push(format!("tags: {}", p.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")));
        if let Some(o) = doc.as_object_mut() {
            // Keep `tags` next to `info` like hand-written specs.
            let mut rebuilt = Map::new();
            for (k, v) in std::mem::take(o) {
                let after_servers = k == "servers";
                rebuilt.insert(k, v);
                if after_servers {
                    rebuilt.insert("tags".into(), Value::Array(tags.clone()));
                }
            }
            if !rebuilt.contains_key("tags") {
                rebuilt.insert("tags".into(), Value::Array(tags));
            }
            *o = rebuilt;
        }
    }
    // Body schemas → components.
    let slots = body_slots(spec);
    let mut components: BTreeMap<String, Value> = doc.pointer("/components/schemas").and_then(Value::as_object).map(|o| o.clone().into_iter().collect()).unwrap_or_default();
    let mut moved = 0;
    for s in &p.schemas {
        let Some(slot) = slots.iter().find(|x| x.id == s.body) else { continue };
        let target_ptr = if slot.array_items { format!("{}/items", slot.pointer) } else { slot.pointer.clone() };
        let Some(schema) = doc.pointer(&target_ptr).cloned() else { continue };
        // Same name + identical schema → share it; different schema → suffix.
        let mut name = s.name.clone();
        let mut n = 2;
        while let Some(existing) = components.get(&name) {
            if *existing == schema {
                break;
            }
            name = format!("{}{n}", s.name);
            n += 1;
        }
        components.insert(name.clone(), schema);
        if let Some(slot_v) = doc.pointer_mut(&target_ptr) {
            *slot_v = json!({ "$ref": format!("#/components/schemas/{name}") });
        }
        changes.push(format!("{} → components/schemas/{name}", s.location));
        moved += 1;
    }
    if moved > 0 && let Some(o) = doc.as_object_mut() {
        let comps = o.entry("components").or_insert_with(|| json!({}));
        if let Some(c) = comps.as_object_mut() {
            c.insert("schemas".into(), Value::Object(components.into_iter().collect()));
        }
    }
    (doc, changes)
}

/// The user message for the naming call.
pub fn user_message(spec: &Value) -> String {
    format!("Name the operations and body schemas of this API.\n{}", fence("data", &outline(spec)))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn draft() -> Value {
        json!({
            "openapi": "3.1.0",
            "info": { "title": "shop API", "version": "0.1.0" },
            "servers": [{ "url": "https://shop.example.com" }],
            "paths": {
                "/users": {
                    "get": { "operationId": "getUsers", "summary": "GET /users", "responses": { "200": { "description": "OK", "content": { "application/json": { "schema": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "integer" }, "email": { "type": "string" } } } } } } } } },
                    "post": { "operationId": "postUsers", "summary": "POST /users", "requestBody": { "content": { "application/json": { "schema": { "type": "object", "properties": { "email": { "type": "string", "example": "ada@example.com" } } } } } }, "responses": { "201": { "description": "Created", "content": { "application/json": { "schema": { "type": "object", "properties": { "id": { "type": "integer" }, "email": { "type": "string" } } } } } } } }
                },
                "/users/{userId}": {
                    "get": { "operationId": "getUsersByUserId", "summary": "GET /users/{userId}", "parameters": [{ "name": "userId", "in": "path", "required": true, "schema": { "type": "integer" } }], "responses": { "200": { "description": "OK", "content": { "application/json": { "schema": { "type": "object", "properties": { "id": { "type": "integer" }, "email": { "type": "string" } } } } } }, "404": { "description": "Not Found" } } }
                }
            }
        })
    }

    #[test]
    fn outline_lists_operations_and_bodies_without_values() {
        let o = outline(&draft());
        assert!(o.contains("- GET /users | operationId: getUsers"), "{o}");
        assert!(o.contains("B1: 200 response array of {id, email}"), "{o}");
        assert!(o.contains("B2: request body object {email}"), "{o}");
        assert!(o.contains("params: userId(path)"));
        assert!(!o.contains("ada@example.com"), "example values are never sent");
        assert_eq!(body_slots(&draft()).len(), 4);
    }

    #[test]
    fn proposals_are_checked_and_applied_as_a_patch() {
        let spec = draft();
        let answer = json!({
            "operations": [
                { "key": "GET /users", "operationId": "listUsers", "summary": "List users", "tags": ["Users"] },
                { "key": "POST /users", "operationId": "createUser", "summary": "Create a user", "tags": ["Users"] },
                { "key": "GET /users/{userId}", "operationId": "getUser", "summary": "Get a user", "tags": ["Users"] },
                { "key": "DELETE /nope", "operationId": "bad id!", "summary": "", "tags": [] }
            ],
            "schemas": [{ "body": "B1", "name": "User" }, { "body": "B3", "name": "User" }, { "body": "B4", "name": "User" }, { "body": "B2", "name": "createUserRequest" }],
            "tags": [{ "name": "Users", "description": "People with accounts" }]
        });
        let err = check_proposal(&answer, &spec).unwrap_err();
        assert!(err.contains("unknown operation key `DELETE /nope`") && err.contains("not PascalCase"), "{err}");
        let p = clean_proposal(&answer, &spec);
        assert_eq!(p.operations.len(), 3);
        assert_eq!(p.schemas.len(), 3, "the invalid name is dropped");
        let (doc, changes) = apply_proposal(&spec, &p);
        assert_eq!(doc["paths"]["/users"]["get"]["operationId"], "listUsers");
        assert_eq!(doc["paths"]["/users"]["get"]["tags"], json!(["Users"]));
        assert_eq!(doc["tags"][0]["description"], "People with accounts");
        // B1 is an array: its items become the model; B3/B4 are the same shape → shared.
        assert_eq!(doc["paths"]["/users"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["items"]["$ref"], "#/components/schemas/User");
        assert_eq!(doc["paths"]["/users/{userId}"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]["$ref"], "#/components/schemas/User");
        assert!(doc["components"]["schemas"]["User"]["properties"]["email"].is_object());
        assert!(doc["components"]["schemas"].get("User2").is_none(), "{}", doc["components"]);
        assert!(changes.iter().any(|c| c.contains("getUsers → listUsers")), "{changes:?}");
        // The keys before `tags` keep their order.
        let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
        assert_eq!(keys[..4], ["openapi", "info", "servers", "tags"]);
        let yaml = serde_yaml_ng::to_string(&doc).unwrap();
        assert_eq!(parse_spec(&yaml).unwrap(), doc);
        assert!(parse_spec("name: x").is_err());
    }
}
