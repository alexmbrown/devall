//! GraphQL schema explorer: runs the standard introspection query over HTTP (through the normal
//! request pipeline, so the request's URL, headers, auth and variables apply), turns the result
//! into a compact schema model, and generates a ready-to-run operation for every root field.

use std::collections::HashMap;

use devall_core::{Body, HttpRequestFile};
use devall_http::{ClientPool, PreparedRequest};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{ProtoError, Result};

/// The standard introspection query (no `{{` sequences, so template resolution leaves it alone).
pub const INTROSPECTION_QUERY: &str = r#"query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types { ...FullType }
  }
}

fragment FullType on __Type {
  kind
  name
  description
  fields(includeDeprecated: true) {
    name
    description
    args { ...InputValue }
    type { ...TypeRef }
    isDeprecated
    deprecationReason
  }
  inputFields { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) {
    name
    description
    isDeprecated
    deprecationReason
  }
  possibleTypes { ...TypeRef }
}

fragment InputValue on __InputValue {
  name
  description
  type { ...TypeRef }
  defaultValue
}

fragment TypeRef on __Type {
  kind
  name
  ofType {
    kind
    name
    ofType {
      kind
      name
      ofType {
        kind
        name
        ofType {
          kind
          name
          ofType {
            kind
            name
            ofType {
              kind
              name
              ofType {
                kind
                name
              }
            }
          }
        }
      }
    }
  }
}
"#;

/// Default nesting depth of generated selection sets (below the root field).
pub const DEFAULT_DEPTH: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    Query,
    Mutation,
    Subscription,
}

impl OperationKind {
    fn keyword(self) -> &'static str {
        match self {
            OperationKind::Query => "query",
            OperationKind::Mutation => "mutation",
            OperationKind::Subscription => "subscription",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GqlInputValue {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// e.g. `[ID!]!`
    pub type_ref: String,
    /// Innermost named type, e.g. `ID`.
    pub named_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
}

impl GqlInputValue {
    pub fn required(&self) -> bool {
        self.type_ref.ends_with('!') && self.default_value.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GqlField {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<GqlInputValue>,
    pub type_ref: String,
    pub named_type: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub deprecated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GqlEnumValue {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GqlType {
    pub name: String,
    /// `OBJECT`, `INTERFACE`, `UNION`, `ENUM`, `INPUT_OBJECT` or `SCALAR`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<GqlField>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub input_fields: Vec<GqlInputValue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<GqlEnumValue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub possible_types: Vec<String>,
}

/// A root field with a generated operation.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GqlOperation {
    pub kind: OperationKind,
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub type_ref: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub deprecated: bool,
    /// Operation text, e.g. `query User($id: ID!) { user(id: $id) { id name } }` (formatted).
    pub query: String,
    /// Variables JSON with placeholders for the required arguments.
    pub variables: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlSchema {
    pub query_type: Option<String>,
    pub mutation_type: Option<String>,
    pub subscription_type: Option<String>,
    /// User types (introspection `__*` types are left out), sorted by name.
    pub types: Vec<GqlType>,
    /// Every root field of Query, Mutation and Subscription, in schema order.
    pub operations: Vec<GqlOperation>,
}

// ---------------------------------------------------------------------------------------------
// Raw introspection JSON
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSchema {
    query_type: Option<Named>,
    mutation_type: Option<Named>,
    subscription_type: Option<Named>,
    #[serde(default)]
    types: Vec<RawType>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawType {
    kind: String,
    name: Option<String>,
    description: Option<String>,
    fields: Option<Vec<RawField>>,
    input_fields: Option<Vec<RawInput>>,
    interfaces: Option<Vec<RawTypeRef>>,
    enum_values: Option<Vec<RawEnum>>,
    possible_types: Option<Vec<RawTypeRef>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawField {
    name: String,
    description: Option<String>,
    #[serde(default)]
    args: Vec<RawInput>,
    #[serde(rename = "type")]
    ty: RawTypeRef,
    #[serde(default)]
    is_deprecated: bool,
    deprecation_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInput {
    name: String,
    description: Option<String>,
    #[serde(rename = "type")]
    ty: RawTypeRef,
    default_value: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEnum {
    name: String,
    description: Option<String>,
    #[serde(default)]
    is_deprecated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTypeRef {
    kind: String,
    name: Option<String>,
    of_type: Option<Box<RawTypeRef>>,
}

impl RawTypeRef {
    fn render(&self) -> String {
        match self.kind.as_str() {
            "NON_NULL" => format!("{}!", self.of_type.as_ref().map(|t| t.render()).unwrap_or_default()),
            "LIST" => format!("[{}]", self.of_type.as_ref().map(|t| t.render()).unwrap_or_default()),
            _ => self.name.clone().unwrap_or_default(),
        }
    }

    fn named(&self) -> String {
        match &self.of_type {
            Some(t) if self.name.is_none() => t.named(),
            _ => self.name.clone().unwrap_or_default(),
        }
    }
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|d| d.trim().to_string()).filter(|d| !d.is_empty())
}

fn input(i: RawInput) -> GqlInputValue {
    GqlInputValue { type_ref: i.ty.render(), named_type: i.ty.named(), name: i.name, description: clean(i.description), default_value: i.default_value }
}

/// Parses an introspection response (`{"data":{"__schema":…}}`, or just `{"__schema":…}`).
pub fn parse_introspection(v: &Value) -> Result<GraphqlSchema> {
    let schema = v.get("data").and_then(|d| d.get("__schema")).or_else(|| v.get("__schema"));
    let Some(schema) = schema.filter(|s| !s.is_null()) else {
        let errors: Vec<String> = v
            .get("errors")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|e| e.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| e.to_string())).collect())
            .unwrap_or_default();
        return Err(ProtoError::Protocol(if errors.is_empty() {
            "the response has no data.__schema — is this a GraphQL endpoint?".into()
        } else {
            format!("introspection failed: {}", errors.join("; "))
        }));
    };
    let raw: RawSchema = serde_json::from_value(schema.clone()).map_err(|e| ProtoError::Protocol(format!("unexpected introspection result: {e}")))?;
    let mut types: Vec<GqlType> = raw
        .types
        .into_iter()
        .filter_map(|t| {
            let name = t.name?;
            if name.starts_with("__") {
                return None;
            }
            Some(GqlType {
                name,
                kind: t.kind,
                description: clean(t.description),
                fields: t
                    .fields
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| GqlField {
                        type_ref: f.ty.render(),
                        named_type: f.ty.named(),
                        name: f.name,
                        description: clean(f.description),
                        args: f.args.into_iter().map(input).collect(),
                        deprecated: f.is_deprecated,
                        deprecation_reason: clean(f.deprecation_reason),
                    })
                    .collect(),
                input_fields: t.input_fields.unwrap_or_default().into_iter().map(input).collect(),
                enum_values: t
                    .enum_values
                    .unwrap_or_default()
                    .into_iter()
                    .map(|e| GqlEnumValue { name: e.name, description: clean(e.description), deprecated: e.is_deprecated })
                    .collect(),
                interfaces: t.interfaces.unwrap_or_default().iter().map(RawTypeRef::named).collect(),
                possible_types: t.possible_types.unwrap_or_default().iter().map(RawTypeRef::named).collect(),
            })
        })
        .collect();
    types.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = GraphqlSchema {
        query_type: raw.query_type.and_then(|n| n.name),
        mutation_type: raw.mutation_type.and_then(|n| n.name),
        subscription_type: raw.subscription_type.and_then(|n| n.name),
        types,
        operations: Vec::new(),
    };
    out.operations = root_operations(&out, DEFAULT_DEPTH);
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Operation generation
// ---------------------------------------------------------------------------------------------

struct Index<'a> {
    types: HashMap<&'a str, &'a GqlType>,
}

impl<'a> Index<'a> {
    fn new(s: &'a GraphqlSchema) -> Self {
        Self { types: s.types.iter().map(|t| (t.name.as_str(), t)).collect() }
    }

    fn get(&self, name: &str) -> Option<&'a GqlType> {
        self.types.get(name).copied()
    }

    fn is_leaf(&self, name: &str) -> bool {
        self.get(name).is_none_or(|t| t.kind == "SCALAR" || t.kind == "ENUM")
    }

    /// Lines of a selection set for `type_name` (without braces), or None for leaf types.
    fn selection(&self, type_name: &str, depth: usize, max_depth: usize, path: &mut Vec<String>) -> Option<Vec<String>> {
        let t = self.get(type_name)?;
        match t.kind.as_str() {
            "OBJECT" | "INTERFACE" => {
                path.push(t.name.clone());
                let mut lines = Vec::new();
                for f in t.fields.iter().filter(|f| !f.deprecated && !f.args.iter().any(GqlInputValue::required)) {
                    if self.is_leaf(&f.named_type) {
                        lines.push(f.name.clone());
                    } else if depth < max_depth && !path.contains(&f.named_type)
                        && let Some(sub) = self.selection(&f.named_type, depth + 1, max_depth, path)
                    {
                        lines.push(format!("{} {{", f.name));
                        lines.extend(sub.into_iter().map(|l| format!("  {l}")));
                        lines.push("}".into());
                    }
                }
                path.pop();
                if lines.is_empty() {
                    lines.push("__typename".into());
                }
                Some(lines)
            }
            "UNION" => {
                let mut lines = vec!["__typename".to_string()];
                if depth < max_depth {
                    for pt in &t.possible_types {
                        if path.contains(pt) {
                            continue;
                        }
                        if let Some(sub) = self.selection(pt, depth + 1, max_depth, path) {
                            lines.push(format!("... on {pt} {{"));
                            lines.extend(sub.into_iter().map(|l| format!("  {l}")));
                            lines.push("}".into());
                        }
                    }
                }
                Some(lines)
            }
            _ => None,
        }
    }

    /// Example JSON value for an input type reference.
    fn placeholder(&self, type_ref: &str, named: &str, depth: usize) -> Value {
        let inner = type_ref.trim_end_matches('!');
        if inner.starts_with('[') {
            return json!([]);
        }
        match named {
            "Int" => json!(0),
            "Float" => json!(0.0),
            "Boolean" => json!(false),
            "String" | "ID" => json!(""),
            _ => match self.get(named) {
                Some(t) if t.kind == "ENUM" => t.enum_values.iter().find(|e| !e.deprecated).map(|e| json!(e.name)).unwrap_or(Value::Null),
                Some(t) if t.kind == "INPUT_OBJECT" && depth < 4 => {
                    let mut o = serde_json::Map::new();
                    for f in t.input_fields.iter().filter(|f| f.required()) {
                        o.insert(f.name.clone(), self.placeholder(&f.type_ref, &f.named_type, depth + 1));
                    }
                    Value::Object(o)
                }
                Some(t) if t.kind == "INPUT_OBJECT" => json!({}),
                _ => json!(""),
            },
        }
    }
}

fn pascal(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => "Operation".into(),
    }
}

/// A runnable operation for one root field (`max_depth` levels of nested selections).
pub fn generate_operation(schema: &GraphqlSchema, kind: OperationKind, field: &str, max_depth: usize) -> Option<(String, String)> {
    let root = match kind {
        OperationKind::Query => schema.query_type.as_deref(),
        OperationKind::Mutation => schema.mutation_type.as_deref(),
        OperationKind::Subscription => schema.subscription_type.as_deref(),
    }?;
    let idx = Index::new(schema);
    let f = idx.get(root)?.fields.iter().find(|f| f.name == field)?;
    Some(operation_for(&idx, kind, f, max_depth))
}

fn operation_for(idx: &Index, kind: OperationKind, f: &GqlField, max_depth: usize) -> (String, String) {
    let decls: Vec<String> = f.args.iter().map(|a| format!("${}: {}", a.name, a.type_ref)).collect();
    let call_args: Vec<String> = f.args.iter().map(|a| format!("{}: ${}", a.name, a.name)).collect();
    let mut q = format!("{} {}", kind.keyword(), pascal(&f.name));
    if !decls.is_empty() {
        q.push_str(&format!("({})", decls.join(", ")));
    }
    q.push_str(" {\n  ");
    q.push_str(&f.name);
    if !call_args.is_empty() {
        q.push_str(&format!("({})", call_args.join(", ")));
    }
    let mut path = Vec::new();
    match idx.selection(&f.named_type, 1, max_depth, &mut path) {
        Some(lines) => {
            q.push_str(" {\n");
            for l in lines {
                q.push_str(&format!("    {l}\n"));
            }
            q.push_str("  }\n");
        }
        None => q.push('\n'),
    }
    q.push_str("}\n");
    let mut vars = serde_json::Map::new();
    for a in f.args.iter().filter(|a| a.required()) {
        vars.insert(a.name.clone(), idx.placeholder(&a.type_ref, &a.named_type, 0));
    }
    let variables = if vars.is_empty() { String::new() } else { serde_json::to_string_pretty(&Value::Object(vars)).unwrap_or_default() };
    (q, variables)
}

fn root_operations(schema: &GraphqlSchema, max_depth: usize) -> Vec<GqlOperation> {
    let idx = Index::new(schema);
    let mut out = Vec::new();
    for (kind, root) in [
        (OperationKind::Query, &schema.query_type),
        (OperationKind::Mutation, &schema.mutation_type),
        (OperationKind::Subscription, &schema.subscription_type),
    ] {
        let Some(t) = root.as_deref().and_then(|r| idx.get(r)) else { continue };
        for f in &t.fields {
            let (query, variables) = operation_for(&idx, kind, f, max_depth);
            out.push(GqlOperation {
                kind,
                field: f.name.clone(),
                description: f.description.clone(),
                type_ref: f.type_ref.clone(),
                deprecated: f.deprecated,
                query,
                variables,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Running it
// ---------------------------------------------------------------------------------------------

/// The request to send: `req` (URL, params, headers, auth, settings) as a POST carrying the
/// introspection query.
pub fn introspection_request(req: &HttpRequestFile) -> HttpRequestFile {
    let mut r = req.clone();
    r.method = "POST".into();
    r.body = Body::Graphql { query: INTROSPECTION_QUERY.into(), variables: String::new(), operation_name: Some("IntrospectionQuery".into()) };
    r.captures.clear();
    r.tests.clear();
    r.scripts = None;
    r
}

/// Sends a prepared [`introspection_request`] and parses the schema.
pub async fn execute_introspection(pool: &ClientPool, prepared: &PreparedRequest) -> Result<GraphqlSchema> {
    let resp = pool.execute(prepared).await.map_err(|e| ProtoError::Connect(e.to_string()))?;
    let text = String::from_utf8_lossy(&resp.body);
    let parsed: Option<Value> = serde_json::from_slice(&resp.body).ok();
    if !(200..300).contains(&resp.status) {
        // GraphQL servers often answer 400 with an `errors` array; show that when present.
        if let Some(v) = &parsed
            && v.get("errors").is_some()
        {
            return parse_introspection(v).map_err(|e| ProtoError::Protocol(format!("HTTP {}: {e}", resp.status)));
        }
        return Err(ProtoError::Protocol(format!("HTTP {} {}: {}", resp.status, resp.status_text, text.chars().take(300).collect::<String>())));
    }
    let v = parsed.ok_or_else(|| ProtoError::Protocol(format!("the response is not JSON: {}", text.chars().take(200).collect::<String>())))?;
    parse_introspection(&v)
}

#[cfg(test)]
mod tests;
