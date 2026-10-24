//! GraphQL mocking on the HTTP port.
//!
//! - **Schemas** live in `mocks/graphql/`: SDL files (`*.graphql`, `*.graphqls`, `*.gql`) or an
//!   introspection result (`*.json` containing `__schema`); a sub-folder `mocks/graphql/<name>/`
//!   merges all its SDL files into one schema. Each schema is served at `/graphql` (a schema named
//!   `schema`, or the only one) or `/graphql/<name>`; `mocks/graphql/config.json` can set
//!   `{"<name>": {"endpoint": "/api/graphql", "seed": 7, "listLength": 3}}`.
//! - **Auto-mocking**: a POST (JSON, or a JSON array for batches) or GET (`?query=`) to an
//!   endpoint that no stub matched is validated against the schema and answered with generated
//!   data shaped by the selection set ([`execute`]); introspection works, so GraphiQL / Apollo
//!   tooling / /dev/all's own schema explorer can discover the mock.
//! - **Stubs** are ordinary WireMock mappings with a `graphql` request matcher
//!   (`operationName`, `operationType`, `field`, `query`, `variables` — JSON matchers) or the
//!   WireMock GraphQL extension's `customMatcher` (`gql-matcher`: semantically equal query,
//!   `variables`, `operationName`). A stub response with `"transformers": ["graphql-automock"]`
//!   generates data from the schema and overlays the stub body's `data` (per-field overrides,
//!   keyed like the response) and `errors`.

pub mod ast;
pub mod execute;
mod fake;
pub mod parser;
pub mod schema;
pub mod validate;

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value as Json, json};

use crate::files::{LoadError, MOCKS_DIR};
use crate::matchers::{Eval, ValuePattern};
use crate::request::MockRequest;
use ast::{Document, OpKind, Operation, Pos, Selection};
use schema::Schema;

pub const GRAPHQL_DIR: &str = "graphql";
/// Response transformer that generates data from the schema under the stub's body.
pub const AUTOMOCK_TRANSFORMER: &str = "graphql-automock";

/// A GraphQL error (`{"message", "locations", "extensions"}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GqlError {
    pub message: String,
    pub locations: Vec<Pos>,
}

impl GqlError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), locations: Vec::new() }
    }

    pub fn at(message: impl Into<String>, pos: Pos) -> Self {
        Self { message: message.into(), locations: vec![pos] }
    }

    pub fn to_json(&self, code: &str) -> Json {
        let mut m = Map::new();
        m.insert("message".into(), json!(self.message));
        if !self.locations.is_empty() {
            m.insert("locations".into(), Json::Array(self.locations.iter().map(|p| json!({ "line": p.line, "column": p.column })).collect()));
        }
        m.insert("extensions".into(), json!({ "code": code }));
        Json::Object(m)
    }
}

impl std::fmt::Display for GqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.locations.first() {
            Some(p) => write!(f, "{} ({}:{})", self.message, p.line, p.column),
            None => f.write_str(&self.message),
        }
    }
}

// ---------------------------------------------------------------- requests

/// `{query, variables, operationName}` from a POST body or GET query string.
#[derive(Debug, Clone, PartialEq)]
pub struct Payload {
    pub query: String,
    pub variables: Map<String, Json>,
    pub operation_name: Option<String>,
}

impl Payload {
    fn from_json(v: &Json) -> Option<Self> {
        let query = v.get("query")?.as_str()?.to_string();
        let variables = match v.get("variables") {
            Some(Json::Object(o)) => o.clone(),
            Some(Json::String(s)) => serde_json::from_str::<Map<String, Json>>(s).unwrap_or_default(),
            _ => Map::new(),
        };
        let operation_name = v.get("operationName").and_then(Json::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        Some(Self { query, variables, operation_name })
    }

    /// One or more payloads (`Err` = not a GraphQL request at all).
    pub fn from_request(req: &MockRequest) -> Result<(Vec<Self>, bool), String> {
        if req.method.eq_ignore_ascii_case("GET") {
            let q = req.query_values("query");
            let Some(query) = q.first() else { return Err("GET requests need a `query` parameter".into()) };
            let variables = req.query_values("variables").first().and_then(|v| serde_json::from_str::<Map<String, Json>>(v).ok()).unwrap_or_default();
            let operation_name = req.query_values("operationName").first().map(|s| s.to_string()).filter(|s| !s.is_empty());
            return Ok((vec![Self { query: query.to_string(), variables, operation_name }], false));
        }
        let ct = req.content_type().unwrap_or("").to_ascii_lowercase();
        if ct.starts_with("application/graphql") && !ct.contains("json") {
            return Ok((vec![Self { query: req.body_text().into_owned(), variables: Map::new(), operation_name: None }], false));
        }
        let v: Json = serde_json::from_slice(&req.body).map_err(|e| format!("the body is not valid JSON: {e}"))?;
        match &v {
            Json::Array(items) => Ok((items.iter().map(|i| Self::from_json(i).ok_or("every batched item needs a `query` string")).collect::<Result<_, _>>()?, true)),
            _ => Ok((vec![Self::from_json(&v).ok_or("the body needs a `query` string")?], false)),
        }
    }
}

/// The selected operation of a parsed payload.
fn select<'d>(doc: &'d Document, name: Option<&str>) -> Result<&'d Operation, GqlError> {
    let ops: Vec<&Operation> = doc.operations().collect();
    match name {
        Some(n) => ops.into_iter().find(|o| o.name.as_deref() == Some(n)).ok_or_else(|| GqlError::new(format!("Unknown operation named \"{n}\"."))),
        None if ops.len() == 1 => Ok(ops[0]),
        None if ops.is_empty() => Err(GqlError::new("Must provide an operation.")),
        None => Err(GqlError::new("Must provide operation name if query contains multiple operations.")),
    }
}

/// What request matchers look at: the selected operation's type, name and root fields, and a
/// normalised query text.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub operation_type: OpKind,
    pub operation_name: Option<String>,
    pub root_fields: Vec<String>,
    pub normalized: String,
}

pub fn analyze(p: &Payload) -> Option<Analysis> {
    let doc = parser::parse(&p.query).ok()?;
    let op = select(&doc, p.operation_name.as_deref()).ok()?;
    let mut root_fields = Vec::new();
    fn roots(doc: &Document, sel: &[Selection], out: &mut Vec<String>, depth: usize) {
        for s in sel {
            match s {
                Selection::Field(f) => out.push(f.name.clone()),
                Selection::Inline { selection, .. } => roots(doc, selection, out, depth),
                Selection::Spread { name, .. } if depth < 8 => {
                    if let Some(fr) = doc.fragments().find(|f| &f.name == name) {
                        roots(doc, &fr.selection, out, depth + 1);
                    }
                }
                Selection::Spread { .. } => {}
            }
        }
    }
    roots(&doc, &op.selection, &mut root_fields, 0);
    Some(Analysis { operation_type: op.kind, operation_name: op.name.clone(), root_fields, normalized: normalize(&doc) })
}

/// Canonical text of a document: fields sorted, fragments inlined into name order, whitespace
/// and comments dropped — two queries that ask for the same thing normalise equally (the
/// WireMock GraphQL extension compares queries this way).
pub fn normalize(doc: &Document) -> String {
    fn sel(s: &[Selection]) -> String {
        if s.is_empty() {
            return String::new();
        }
        let mut items: Vec<String> = s
            .iter()
            .map(|x| match x {
                Selection::Field(f) => {
                    let mut args: Vec<String> = f.args.iter().map(|(k, v)| format!("{k}:{}", v.print())).collect();
                    args.sort();
                    format!(
                        "{}{}{}{}",
                        f.alias.as_ref().map(|a| format!("{a}:")).unwrap_or_default(),
                        f.name,
                        if args.is_empty() { String::new() } else { format!("({})", args.join(",")) },
                        sel(&f.selection)
                    )
                }
                Selection::Spread { name, .. } => format!("...{name}"),
                Selection::Inline { type_cond, selection, .. } => format!("...{}{}", type_cond.as_ref().map(|t| format!("on {t}")).unwrap_or_default(), sel(selection)),
            })
            .collect();
        items.sort();
        format!("{{{}}}", items.join(" "))
    }
    let mut parts: Vec<String> = Vec::new();
    for op in doc.operations() {
        let mut vars: Vec<String> = op.variables.iter().map(|v| format!("${}:{}", v.name, v.ty)).collect();
        vars.sort();
        parts.push(format!("{} {}({}){}", op.kind.keyword(), op.name.clone().unwrap_or_default(), vars.join(","), sel(&op.selection)));
    }
    let mut frags: Vec<String> = doc.fragments().map(|f| format!("fragment {} on {}{}", f.name, f.type_cond, sel(&f.selection))).collect();
    frags.sort();
    parts.extend(frags);
    parts.join("\n")
}

// ---------------------------------------------------------------- request matching

/// The `graphql` request matcher (and the extension's `gql-matcher`).
#[derive(Debug, Clone)]
pub struct GqlPattern {
    operation_name: Option<ValuePattern>,
    operation_type: Option<OpKind>,
    field: Option<ValuePattern>,
    query: Option<ValuePattern>,
    /// Normalised query that must be equal (gql-matcher).
    equal_query: Option<String>,
    variables: Option<ValuePattern>,
}

fn op_kind(s: &str) -> Result<OpKind, String> {
    match s.to_ascii_lowercase().as_str() {
        "query" => Ok(OpKind::Query),
        "mutation" => Ok(OpKind::Mutation),
        "subscription" => Ok(OpKind::Subscription),
        other => Err(format!("graphql.operationType must be query, mutation or subscription, not {other}")),
    }
}

/// A string is shorthand for `{"equalTo": …}`.
fn pattern(v: &Json, what: &str) -> Result<ValuePattern, String> {
    match v {
        Json::String(s) => ValuePattern::from_json(&json!({ "equalTo": s })),
        other => ValuePattern::from_json(other),
    }
    .map_err(|e| format!("graphql.{what}: {e}"))
}

impl GqlPattern {
    /// `{"operationName", "operationType", "field", "query", "variables"}`.
    pub fn from_json(v: &Json) -> Result<Self, String> {
        let o = v.as_object().ok_or("graphql must be an object")?;
        for k in o.keys() {
            if !matches!(k.as_str(), "operationName" | "operationType" | "field" | "query" | "variables") {
                return Err(format!("graphql: unknown key {k} (operationName, operationType, field, query, variables)"));
            }
        }
        Ok(Self {
            operation_name: o.get("operationName").map(|x| pattern(x, "operationName")).transpose()?,
            operation_type: o.get("operationType").and_then(Json::as_str).map(op_kind).transpose()?,
            field: o.get("field").map(|x| pattern(x, "field")).transpose()?,
            query: o.get("query").map(|x| pattern(x, "query")).transpose()?,
            equal_query: None,
            variables: o
                .get("variables")
                .map(|x| match x {
                    // A plain object means "these variables (others allowed)".
                    Json::Object(m) if !m.keys().any(|k| ValuePattern::is_operator(k)) => ValuePattern::from_json(&json!({ "equalToJson": x, "ignoreExtraElements": true })),
                    other => ValuePattern::from_json(other),
                })
                .transpose()
                .map_err(|e| format!("graphql.variables: {e}"))?,
        })
    }

    /// WireMock GraphQL extension: `customMatcher: {name: "gql-matcher", parameters: {query, variables, operationName}}`.
    pub fn from_custom_matcher(params: &Json) -> Result<Self, String> {
        let params = params.get("expectedJson").unwrap_or(params);
        let equal_query = match params.get("query").and_then(Json::as_str) {
            Some(q) => Some(normalize(&parser::parse(q).map_err(|e| format!("gql-matcher query: {e}"))?)),
            None => None,
        };
        Ok(Self {
            operation_name: params.get("operationName").map(|x| pattern(x, "operationName")).transpose()?,
            operation_type: None,
            field: None,
            query: None,
            equal_query,
            variables: params.get("variables").map(|x| ValuePattern::from_json(&json!({ "equalToJson": x }))).transpose().map_err(|e| format!("gql-matcher variables: {e}"))?,
        })
    }

    /// Per-field results `(label, expected, actual, eval)`.
    pub fn eval(&self, req: &MockRequest) -> Vec<(String, String, Option<String>, Eval)> {
        let payload = Payload::from_request(req).ok().and_then(|(mut p, batch)| if batch { None } else { p.pop() });
        let analysis = payload.as_ref().and_then(analyze);
        let mut out = Vec::new();
        let none = |label: &str, expected: String| (label.to_string(), expected, None, Eval::exact(false));
        if let Some(p) = &self.operation_name {
            let actual = analysis.as_ref().and_then(|a| a.operation_name.clone()).or_else(|| payload.as_ref().and_then(|p| p.operation_name.clone()));
            out.push(("GraphQL operation name".into(), p.describe(), actual.clone(), p.eval(actual.as_deref())));
        }
        if let Some(k) = self.operation_type {
            match &analysis {
                Some(a) => out.push(("GraphQL operation type".into(), k.keyword().into(), Some(a.operation_type.keyword().into()), Eval::exact(a.operation_type == k))),
                None => out.push(none("GraphQL operation type", k.keyword().into())),
            }
        }
        if let Some(p) = &self.field {
            match &analysis {
                Some(a) => {
                    let best = a.root_fields.iter().map(|f| p.eval(Some(f))).fold(Eval { matched: false, distance: 1.0 }, |acc, e| if e.matched || e.distance < acc.distance { e } else { acc });
                    out.push(("GraphQL root field".into(), p.describe(), Some(a.root_fields.join(", ")), best));
                }
                None => out.push(none("GraphQL root field", p.describe())),
            }
        }
        if let Some(p) = &self.query {
            let actual = payload.as_ref().map(|p| p.query.clone());
            out.push(("GraphQL query".into(), p.describe(), actual.clone(), p.eval(actual.as_deref())));
        }
        if let Some(q) = &self.equal_query {
            let actual = analysis.as_ref().map(|a| a.normalized.clone());
            let ok = actual.as_deref() == Some(q.as_str());
            out.push(("GraphQL query (normalised)".into(), q.clone(), actual, Eval::exact(ok)));
        }
        if let Some(p) = &self.variables {
            let actual = payload.as_ref().map(|p| Json::Object(p.variables.clone()).to_string());
            out.push(("GraphQL variables".into(), p.describe(), actual.clone(), p.eval(actual.as_deref())));
        }
        out
    }
}

// ---------------------------------------------------------------- schemas in the project

#[derive(Debug, Clone)]
pub struct SchemaEntry {
    pub name: String,
    pub endpoint: String,
    /// Files relative to the project root.
    pub files: Vec<String>,
    pub schema: Arc<Schema>,
    pub options: execute::ExecOptions,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
    pub endpoint: String,
    pub files: Vec<String>,
    pub seed: u64,
    pub list_length: usize,
    /// Type count and root fields.
    pub summary: Json,
}

#[derive(Debug, Clone, Default)]
pub struct GraphqlSet {
    pub schemas: Vec<SchemaEntry>,
    pub errors: Vec<LoadError>,
}

fn is_sdl(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".graphql") || n.ends_with(".graphqls") || n.ends_with(".gql")
}

impl GraphqlSet {
    /// Loads `<root>/mocks/graphql`.
    pub fn load(root: &Path) -> Self {
        let dir = root.join(MOCKS_DIR).join(GRAPHQL_DIR);
        let mut set = Self::default();
        let Ok(rd) = std::fs::read_dir(&dir) else { return set };
        let rel = |p: &Path| format!("{MOCKS_DIR}/{GRAPHQL_DIR}/{}", p.strip_prefix(&dir).unwrap_or(p).to_string_lossy().replace('\\', "/"));
        let config: Map<String, Json> = std::fs::read_to_string(dir.join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut found: Vec<(String, Vec<std::path::PathBuf>, bool)> = Vec::new();
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let fname = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if fname.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                let mut files: Vec<_> = std::fs::read_dir(&p).into_iter().flatten().flatten().map(|e| e.path()).filter(|f| f.is_file() && is_sdl(&f.to_string_lossy())).collect();
                files.sort();
                if !files.is_empty() {
                    found.push((fname, files, false));
                }
            } else if is_sdl(&fname) {
                let stem = fname.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(fname.clone());
                found.push((stem, vec![p], false));
            } else if fname.to_ascii_lowercase().ends_with(".json") && fname != "config.json" {
                let stem = fname.trim_end_matches(".json").trim_end_matches(".introspection").to_string();
                found.push((stem, vec![p], true));
            }
        }
        let single = found.len() == 1;
        for (name, files, introspection) in found {
            let file_list: Vec<String> = files.iter().map(|f| rel(f)).collect();
            let built = if introspection {
                std::fs::read_to_string(&files[0])
                    .map_err(|e| GqlError::new(e.to_string()))
                    .and_then(|t| serde_json::from_str::<Json>(t.trim_start_matches('\u{feff}')).map_err(|e| GqlError::new(format!("invalid JSON: {e}"))))
                    .and_then(|v| Schema::from_introspection(&v))
            } else {
                let texts: Result<Vec<String>, _> = files.iter().map(std::fs::read_to_string).collect();
                match texts {
                    Ok(t) => Schema::from_sdl(&t.iter().map(String::as_str).collect::<Vec<_>>()),
                    Err(e) => Err(GqlError::new(e.to_string())),
                }
            };
            let cfg = config.get(&name);
            let endpoint = cfg
                .and_then(|c| c.get("endpoint"))
                .and_then(Json::as_str)
                .map(|e| if e.starts_with('/') { e.to_string() } else { format!("/{e}") })
                .unwrap_or_else(|| if name == "schema" || single { "/graphql".into() } else { format!("/graphql/{name}") });
            let options = execute::ExecOptions {
                seed: cfg.and_then(|c| c.get("seed")).and_then(Json::as_u64).unwrap_or(42),
                list_length: cfg.and_then(|c| c.get("listLength")).and_then(Json::as_u64).map(|n| n.min(100) as usize).unwrap_or(2),
            };
            match built {
                Ok(schema) => {
                    if set.schemas.iter().any(|s| s.endpoint == endpoint) {
                        set.errors.push(LoadError { file: file_list[0].clone(), error: format!("another schema already serves {endpoint} (set an endpoint in mocks/graphql/config.json)") });
                        continue;
                    }
                    set.schemas.push(SchemaEntry { name, endpoint, files: file_list, schema: Arc::new(schema), options });
                }
                Err(e) => set.errors.push(LoadError { file: file_list.first().cloned().unwrap_or_default(), error: format!("GraphQL schema: {e}") }),
            }
        }
        set
    }

    pub fn for_path(&self, path: &str) -> Option<&SchemaEntry> {
        let p = path.trim_end_matches('/');
        self.schemas.iter().find(|s| s.endpoint.trim_end_matches('/') == p)
    }

    pub fn by_name(&self, name: &str) -> Option<&SchemaEntry> {
        self.schemas.iter().find(|s| s.name == name)
    }

    pub fn info(&self) -> Vec<SchemaInfo> {
        self.schemas
            .iter()
            .map(|s| SchemaInfo {
                name: s.name.clone(),
                endpoint: s.endpoint.clone(),
                files: s.files.clone(),
                seed: s.options.seed,
                list_length: s.options.list_length,
                summary: s.schema.summary(),
            })
            .collect()
    }
}

/// Validates and executes one payload: `{"data": …}` or `{"errors": […]}`.
pub fn execute_payload(entry: &SchemaEntry, p: &Payload, overrides: Option<&Json>, extra_errors: &[Json], read_only: bool) -> Json {
    let fail = |errors: Vec<GqlError>, code: &str| json!({ "errors": errors.iter().map(|e| e.to_json(code)).collect::<Vec<_>>() });
    let doc = match parser::parse(&p.query) {
        Ok(d) => d,
        Err(e) => return fail(vec![e], "GRAPHQL_PARSE_FAILED"),
    };
    let errors = validate::validate(&entry.schema, &doc);
    if !errors.is_empty() {
        return fail(errors, "GRAPHQL_VALIDATION_FAILED");
    }
    let op = match select(&doc, p.operation_name.as_deref()) {
        Ok(o) => o,
        Err(e) => return fail(vec![e], "OPERATION_RESOLUTION_FAILURE"),
    };
    if read_only && op.kind != OpKind::Query {
        return fail(vec![GqlError::new(format!("Can only perform a {} operation from a POST request.", op.kind.keyword()))], "BAD_REQUEST");
    }
    if op.kind == OpKind::Subscription {
        return fail(vec![GqlError::new("Subscriptions are not served over HTTP by the mock (use a WebSocket stub in mocks/websocket/).")], "BAD_REQUEST");
    }
    let vars = match validate::coerce_variables(&entry.schema, op, &p.variables) {
        Ok(v) => v,
        Err(errors) => return fail(errors, "BAD_USER_INPUT"),
    };
    let data = execute::Executor::new(&entry.schema, &doc, vars, entry.options).run(op, overrides);
    let mut out = Map::new();
    if !extra_errors.is_empty() {
        out.insert("errors".into(), Json::Array(extra_errors.to_vec()));
    }
    out.insert("data".into(), data);
    Json::Object(out)
}

/// The HTTP answer for a request to a schema endpoint (`status`, JSON body).
pub fn respond(entry: &SchemaEntry, req: &MockRequest, stub_body: Option<&Json>) -> (u16, Json) {
    if !matches!(req.method.as_str(), "GET" | "POST") {
        return (405, json!({ "errors": [{ "message": "GraphQL only supports GET and POST requests.", "extensions": { "code": "METHOD_NOT_ALLOWED" } }] }));
    }
    let (payloads, batch) = match Payload::from_request(req) {
        Ok(p) => p,
        Err(e) => return (400, json!({ "errors": [{ "message": e, "extensions": { "code": "BAD_REQUEST" } }] })),
    };
    let overrides = stub_body.and_then(|b| b.get("data")).filter(|d| d.is_object());
    let extra: Vec<Json> = stub_body.and_then(|b| b.get("errors")).and_then(Json::as_array).cloned().unwrap_or_default();
    let read_only = req.method.eq_ignore_ascii_case("GET");
    let results: Vec<Json> = payloads.iter().map(|p| execute_payload(entry, p, overrides, &extra, read_only)).collect();
    if batch { (200, Json::Array(results)) } else { (200, results.into_iter().next().unwrap_or(Json::Null)) }
}

#[cfg(test)]
mod tests;
