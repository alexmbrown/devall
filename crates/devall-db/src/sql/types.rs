//! Query results as they cross IPC: columns `(name, type)` + rows of JSON values. Integers beyond
//! ±2^53 and decimals are strings (JavaScript numbers would lose precision); bytes are base64.

use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_SAFE_INT: i64 = 9_007_199_254_740_991;
/// Default row cap per result set.
pub const DEFAULT_LIMIT: usize = 1_000;
/// Hard cap on rows per result set (the grid virtualizes; memory is the limit).
pub const MAX_LIMIT: usize = 500_000;

/// How a column's values are represented in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ValueKind {
    Bool,
    /// Number, or a string when outside ±2^53.
    Int,
    Float,
    /// Exact numerics, always strings.
    Decimal,
    Text,
    /// Parsed JSON (objects/arrays/…).
    Json,
    /// Base64 strings.
    Bytes,
    Date,
    Time,
    Timestamp,
    Uuid,
    Other,
}

impl ValueKind {
    pub fn is_numeric(self) -> bool {
        matches!(self, ValueKind::Int | ValueKind::Float | ValueKind::Decimal)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMeta {
    pub name: String,
    /// The engine's type name (`int4`, `VARCHAR`, `TEXT`, …).
    pub type_name: String,
    pub kind: ValueKind,
}

/// One statement's outcome.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementResult {
    pub index: usize,
    pub sql: String,
    /// Leading keyword(s) (`SELECT`, `UPDATE`, `CREATE TABLE`, …).
    pub keyword: String,
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
    /// Rows changed by DML (or returned, when the engine reports it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_affected: Option<u64>,
    /// More rows exist beyond `offset + rows.len()` (re-run with a larger `offset` to page).
    pub more: bool,
    pub offset: usize,
    pub elapsed_ms: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A whole script's outcome. Execution stops at the first failing statement.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryOutput {
    pub results: Vec<StatementResult>,
    pub elapsed_ms: f64,
    pub cancelled: bool,
    /// Statements in the script (including ones not run after an error or cancel).
    pub total_statements: usize,
}

/// Streamed while a script runs.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum QueryProgress {
    Started { index: usize, total: usize, sql: String },
    Finished { result: StatementResult },
}

#[derive(Debug, Clone, Copy)]
pub struct ExecOptions {
    /// Rows to return per result set (capped at [`MAX_LIMIT`]).
    pub limit: usize,
    /// Rows to skip first (paging by re-running a read-only statement).
    pub offset: usize,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self { limit: DEFAULT_LIMIT, offset: 0 }
    }
}

impl ExecOptions {
    pub fn new(limit: Option<usize>, offset: Option<usize>) -> Self {
        Self { limit: limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT), offset: offset.unwrap_or(0) }
    }
}

pub fn int_value(i: i64) -> Value {
    if (-MAX_SAFE_INT..=MAX_SAFE_INT).contains(&i) { Value::from(i) } else { Value::String(i.to_string()) }
}

pub fn uint_value(u: u64) -> Value {
    if u <= MAX_SAFE_INT as u64 { Value::from(u) } else { Value::String(u.to_string()) }
}

pub fn float_value(f: f64) -> Value {
    serde_json::Number::from_f64(f).map(Value::Number).unwrap_or_else(|| Value::String(f.to_string()))
}

pub fn bytes_value(b: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(b))
}

/// Decodes `0a0b…` hex (Postgres `bytea` text output after the `\x`).
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Converts a value in the engine's text representation to JSON according to `kind`.
pub fn text_to_value(s: &str, kind: ValueKind) -> Value {
    match kind {
        ValueKind::Bool => match s {
            "t" | "true" | "TRUE" | "1" | "on" | "yes" => Value::Bool(true),
            "f" | "false" | "FALSE" | "0" | "off" | "no" => Value::Bool(false),
            _ => Value::String(s.to_string()),
        },
        ValueKind::Int => {
            if let Ok(i) = s.parse::<i64>() {
                int_value(i)
            } else if let Ok(u) = s.parse::<u64>() {
                uint_value(u)
            } else {
                Value::String(s.to_string())
            }
        }
        ValueKind::Float => match s.parse::<f64>() {
            Ok(f) if f.is_finite() => float_value(f),
            _ => Value::String(s.to_string()),
        },
        ValueKind::Json => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string())),
        ValueKind::Bytes => match s.strip_prefix("\\x").and_then(hex_decode) {
            Some(b) => bytes_value(&b),
            None => bytes_value(s.as_bytes()),
        },
        _ => Value::String(s.to_string()),
    }
}

/// Rows as objects (`[{column: value}]`) — for mocks and JSON export.
pub fn rows_to_objects(columns: &[ColumnMeta], rows: &[Vec<Value>]) -> Value {
    Value::Array(
        rows.iter()
            .map(|r| {
                let mut m = serde_json::Map::new();
                for (c, v) in columns.iter().zip(r) {
                    // Duplicate names (e.g. two `id` columns from a join) get a numeric suffix.
                    let mut name = c.name.clone();
                    let mut n = 2;
                    while m.contains_key(&name) {
                        name = format!("{}_{n}", c.name);
                        n += 1;
                    }
                    m.insert(name, v.clone());
                }
                Value::Object(m)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(text_to_value("42", ValueKind::Int), Value::from(42));
        assert_eq!(text_to_value("9223372036854775807", ValueKind::Int), Value::String("9223372036854775807".into()));
        assert_eq!(text_to_value("18446744073709551615", ValueKind::Int), Value::String("18446744073709551615".into()));
        assert_eq!(text_to_value("t", ValueKind::Bool), Value::Bool(true));
        assert_eq!(text_to_value("1.5", ValueKind::Float), serde_json::json!(1.5));
        assert_eq!(text_to_value("NaN", ValueKind::Float), Value::String("NaN".into()));
        assert_eq!(text_to_value("12.3400", ValueKind::Decimal), Value::String("12.3400".into()));
        assert_eq!(text_to_value("{\"a\":[1]}", ValueKind::Json), serde_json::json!({"a": [1]}));
        assert_eq!(text_to_value("\\x00ff", ValueKind::Bytes), Value::String("AP8=".into()));
        assert_eq!(int_value(-MAX_SAFE_INT - 1), Value::String("-9007199254740992".into()));
        let cols = vec![
            ColumnMeta { name: "id".into(), type_name: "int".into(), kind: ValueKind::Int },
            ColumnMeta { name: "id".into(), type_name: "int".into(), kind: ValueKind::Int },
        ];
        assert_eq!(rows_to_objects(&cols, &[vec![Value::from(1), Value::from(2)]]), serde_json::json!([{"id": 1, "id_2": 2}]));
    }
}
