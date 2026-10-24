//! NL → SQL / MongoDB / kubectl, checked against the real system without changing it.
//!
//! The model only sees the *shape* of the data: a schema summary (tables, columns, types,
//! foreign keys) or sampled field names and types, or the Kubernetes context/namespace and the
//! known resource kinds — never row data. Answers are schema-validated, then checked by the host
//! through a [`Checker`] (`EXPLAIN` for SQL, `explain` for MongoDB, a parse + server-side dry run
//! for kubectl) with one repair round. Nothing is executed: results go into the editor or are
//! shown with a copy button.

use serde::Serialize;
use serde_json::{Value, json};

use crate::prompts::fence;
use crate::structured::CheckNote;

/// Statement keywords that read only (a first-pass check; the host classifies with the real
/// dialect-aware lexer).
const READ_KEYWORDS: &[&str] = &["select", "with", "explain", "show", "describe", "desc", "values", "table", "pragma"];

fn obj(props: Value) -> Value {
    let required: Vec<Value> = props.as_object().map(|o| o.keys().cloned().map(Value::String).collect()).unwrap_or_default();
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

// ---------- SQL ----------

pub fn sql_system(dialect: &str, allow_writes: bool) -> String {
    let tsql = matches!(dialect.to_ascii_lowercase().as_str(), "sqlserver" | "mssql" | "tsql" | "t-sql");
    let (dialect, limit) = if tsql {
        ("SQL Server (T-SQL)", "add TOP (100) to listings (unless the user asks otherwise; T-SQL has no LIMIT) and quote identifiers with [brackets] when needed")
    } else {
        (dialect, "add a LIMIT (100 unless the user asks otherwise) to listings")
    };
    format!(
        "You write ONE {dialect} SQL statement that answers the user's request, using ONLY the tables and columns in the schema (quote identifiers the way {dialect} needs). \
Join on the foreign keys shown, list columns explicitly, and {limit}. {} \
Reply with JSON only: {{\"sql\": string, \"explanation\": string}}. No comments, no trailing text, a single statement.",
        if allow_writes {
            "Write a read-only SELECT (or WITH … SELECT) unless the user explicitly asks to insert, update or delete data."
        } else {
            "The statement MUST be read-only: SELECT or WITH … SELECT only."
        }
    )
}

pub fn sql_schema() -> Value {
    obj(json!({ "sql": { "type": "string" }, "explanation": { "type": "string" } }))
}

pub fn sql_message(question: &str, schema_summary: &str) -> String {
    format!("Database schema:\n{}\nRequest:\n{}", fence("data", schema_summary), fence("data", question.trim()))
}

/// Trims fences/semicolons from the model's SQL.
pub fn clean_sql(sql: &str) -> String {
    let t = sql.trim();
    let t = t.strip_prefix("```sql").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    t.trim_end_matches(';').trim().to_string()
}

/// First keyword, lower-case (`select`, `with`, `update`…).
pub fn first_keyword(sql: &str) -> String {
    sql.trim_start().trim_start_matches('(').split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("").to_ascii_lowercase()
}

/// Cheap checks before the database sees it.
pub fn check_sql(v: &Value, allow_writes: bool) -> std::result::Result<(), String> {
    let sql = clean_sql(v["sql"].as_str().unwrap_or_default());
    if sql.is_empty() {
        return Err("`sql` is empty.".into());
    }
    let kw = first_keyword(&sql);
    if !allow_writes && !READ_KEYWORDS.contains(&kw.as_str()) {
        return Err(format!("the statement starts with {} — only a read-only SELECT is allowed here.", kw.to_uppercase()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlSuggestion {
    pub sql: String,
    pub explanation: String,
    /// The database accepted it (`EXPLAIN` ran) — `false` with `problem` set otherwise.
    pub validated: bool,
    /// What the check found: `{ readOnly, keyword, plan }` (plan = EXPLAIN text, abridged).
    pub check: CheckNote,
    pub problem: Option<String>,
    pub meta: crate::features::Meta,
}

// ---------- MongoDB ----------

pub const MONGO_SYSTEM: &str = "You write a MongoDB query for one collection. Use a `find` (filter, projection, sort, limit) for lookups and an `aggregate` pipeline for grouping, counting, joins ($lookup) and reshaping. \
Use only the fields in the schema. Values that are ObjectIds or dates use Extended JSON ({\"$oid\": \"…\"}, {\"$date\": \"2024-01-01T00:00:00Z\"}). Never use $out or $merge. \
Reply with JSON only: {\"type\": \"find\"|\"aggregate\", \"filter\": string, \"projection\": string, \"sort\": string, \"limit\": integer, \"pipeline\": string, \"explanation\": string}. \
filter/projection/sort are JSON objects written as strings (\"\" when unused); pipeline is a JSON array written as a string (\"\" for find); limit is 0 for none.";

pub fn mongo_schema() -> Value {
    obj(json!({
        "type": { "type": "string", "enum": ["find", "aggregate"] },
        "filter": { "type": "string" },
        "projection": { "type": "string" },
        "sort": { "type": "string" },
        "limit": { "type": "integer" },
        "pipeline": { "type": "string" },
        "explanation": { "type": "string" }
    }))
}

pub fn mongo_message(question: &str, database: &str, collection: &str, fields: &str) -> String {
    format!(
        "Collection: {database}.{collection}\nFields (sampled names and types, no values):\n{}\nRequest:\n{}",
        fence("data", fields),
        fence("data", question.trim())
    )
}

fn json_text_ok(s: &str, want_array: bool) -> std::result::Result<(), String> {
    if s.trim().is_empty() {
        return Ok(());
    }
    let v: Value = serde_json::from_str(s.trim()).map_err(|e| format!("not valid JSON ({e})"))?;
    match (want_array, &v) {
        (true, Value::Array(a)) if a.iter().all(Value::is_object) => Ok(()),
        (false, Value::Object(_)) => Ok(()),
        _ => Err(if want_array { "must be a JSON array of stage objects".into() } else { "must be a JSON object".into() }),
    }
}

/// Shape checks before the database sees it (`$out`/`$merge` are refused).
pub fn check_mongo(v: &Value) -> std::result::Result<(), String> {
    let kind = v["type"].as_str().unwrap_or_default();
    let field = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    match kind {
        "find" => {
            for k in ["filter", "projection", "sort"] {
                json_text_ok(&field(k), false).map_err(|e| format!("`{k}` {e}."))?;
            }
        }
        "aggregate" => {
            let p = field("pipeline");
            if p.trim().is_empty() {
                return Err("`pipeline` is empty for an aggregate.".into());
            }
            json_text_ok(&p, true).map_err(|e| format!("`pipeline` {e}."))?;
            if p.contains("\"$out\"") || p.contains("\"$merge\"") {
                return Err("the pipeline writes ($out/$merge); only read-only queries are allowed.".into());
            }
        }
        other => return Err(format!("`type` must be find or aggregate, not `{other}`.")),
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MongoSuggestion {
    /// `find` or `aggregate`.
    #[serde(rename = "type")]
    pub kind: String,
    pub filter: String,
    pub projection: String,
    pub sort: String,
    pub limit: i64,
    pub pipeline: String,
    pub explanation: String,
    pub validated: bool,
    /// `{ plan }` from the server's explain.
    pub check: CheckNote,
    pub problem: Option<String>,
    pub meta: crate::features::Meta,
}

fn pretty_or(s: &str) -> String {
    match serde_json::from_str::<Value>(s.trim()) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| s.to_string()),
        Err(_) => s.trim().to_string(),
    }
}

pub fn mongo_from(v: &Value) -> (String, String, String, String, i64, String) {
    let f = |k: &str| v[k].as_str().unwrap_or_default().trim().to_string();
    let kind = f("type");
    let pipeline = if kind == "aggregate" { pretty_or(&f("pipeline")) } else { String::new() };
    (kind, pretty_or(&f("filter")), f("projection"), f("sort"), v["limit"].as_i64().unwrap_or(0).max(0), pipeline)
}

// ---------- kubectl ----------

pub fn kubectl_system(context: &str, namespace: &str) -> String {
    format!(
        "You translate a request into ONE kubectl command for the Kubernetes context `{context}`, namespace `{namespace}` (pass -n only for another namespace, or -A for all). \
Prefer read-only commands: get, describe, logs, top, events, explain. Produce a changing command (delete, scale, rollout restart, label, annotate) ONLY when the user explicitly asks for that change. \
Never chain commands; no pipes, redirects, shell substitutions, files (-f) or interactive flags (-it, edit). \
Reply with JSON only: {{\"command\": string, \"explanation\": string}}. The command starts with `kubectl`."
    )
}

pub fn kubectl_schema() -> Value {
    obj(json!({ "command": { "type": "string" }, "explanation": { "type": "string" } }))
}

pub fn kubectl_message(question: &str, kinds: &str) -> String {
    format!("Resource kinds in this cluster:\n{}\nRequest:\n{}", fence("data", kinds), fence("data", question.trim()))
}

/// Shell-free checks: one `kubectl …` command, no chaining/redirection/substitution.
pub fn check_kubectl(v: &Value) -> std::result::Result<(), String> {
    let cmd = v["command"].as_str().unwrap_or_default().trim();
    if !cmd.starts_with("kubectl ") {
        return Err("the command must start with `kubectl `.".into());
    }
    for bad in ["|", ";", "&&", "||", "$(", "`", ">", "<", "\n"] {
        if cmd.contains(bad) {
            return Err(format!("the command contains `{bad}` — give exactly one kubectl command without shell syntax."));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubectlSuggestion {
    pub command: String,
    pub explanation: String,
    pub validated: bool,
    /// What the check found: `{ verb, kind, name, namespace, mutating, dryRun }`.
    pub check: CheckNote,
    pub problem: Option<String>,
    pub meta: crate::features::Meta,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_checks() {
        assert_eq!(clean_sql("```sql\nSELECT 1;\n```"), "SELECT 1");
        assert_eq!(first_keyword("  (SELECT 1)"), "select");
        assert!(check_sql(&json!({"sql": "WITH x AS (SELECT 1) SELECT * FROM x"}), false).is_ok());
        assert!(check_sql(&json!({"sql": "DELETE FROM users"}), false).unwrap_err().contains("DELETE"));
        assert!(check_sql(&json!({"sql": "DELETE FROM users"}), true).is_ok());
        assert!(check_sql(&json!({"sql": " "}), false).is_err());
    }

    #[test]
    fn mongo_checks() {
        assert!(check_mongo(&json!({"type": "find", "filter": "{\"age\": {\"$gt\": 30}}", "projection": "", "sort": "{\"age\": -1}", "pipeline": ""})).is_ok());
        assert!(check_mongo(&json!({"type": "find", "filter": "[1]"})).unwrap_err().contains("filter"));
        assert!(check_mongo(&json!({"type": "aggregate", "pipeline": "[{\"$group\": {\"_id\": \"$status\"}}]"})).is_ok());
        assert!(check_mongo(&json!({"type": "aggregate", "pipeline": "[{\"$out\": \"copy\"}]"})).unwrap_err().contains("$out"));
        assert!(check_mongo(&json!({"type": "update"})).is_err());
    }

    #[test]
    fn kubectl_checks() {
        assert!(check_kubectl(&json!({"command": "kubectl get pods -n shop"})).is_ok());
        assert!(check_kubectl(&json!({"command": "kubectl get pods | grep api"})).unwrap_err().contains('|'));
        assert!(check_kubectl(&json!({"command": "rm -rf /"})).is_err());
    }
}
