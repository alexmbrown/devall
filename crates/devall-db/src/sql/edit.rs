//! Parameterized SQL for grid edits (update/insert/delete by primary key) and batched inserts
//! (seeding from the fake-data generator). Every statement is returned twice: with placeholders
//! (what runs) and with inlined literals (what the user reviews before applying).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::lex::Dialect;
use crate::error::{Result, invalid};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RowEdit {
    /// `UPDATE … SET values WHERE key` — must change exactly one row.
    Update { key: Map<String, Value>, values: Map<String, Value> },
    Insert { values: Map<String, Value> },
    /// `DELETE … WHERE key` — must delete exactly one row.
    Delete { key: Map<String, Value> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub table: String,
    pub edits: Vec<RowEdit>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamStatement {
    /// SQL with placeholders (`$1` for Postgres, `@P1` for SQL Server, `?` otherwise).
    pub sql: String,
    pub params: Vec<Value>,
    /// The same statement with literal values, for display.
    pub preview: String,
    /// Updates/deletes by key must affect exactly one row (otherwise everything is rolled back).
    pub expect_one: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditPreview {
    pub statements: Vec<ParamStatement>,
    /// All statements with literal values, `;`-terminated, inside a transaction.
    pub script: String,
}

pub fn quote_ident(name: &str, d: Dialect) -> String {
    devall_data::output::sql_ident(name, d.data_dialect())
}

/// `"schema"."table"` (or just `"table"`).
pub fn qualified(schema: Option<&str>, table: &str, d: Dialect) -> String {
    match schema.filter(|s| !s.is_empty()) {
        Some(s) => format!("{}.{}", quote_ident(s, d), quote_ident(table, d)),
        None => quote_ident(table, d),
    }
}

pub fn literal(v: &Value, d: Dialect) -> String {
    let mut out = String::new();
    devall_data::output::sql_literal(v, d.data_dialect(), &mut out);
    out
}

struct Builder {
    d: Dialect,
    sql: String,
    lit: String,
    params: Vec<Value>,
}

impl Builder {
    fn new(d: Dialect) -> Self {
        Self { d, sql: String::new(), lit: String::new(), params: Vec::new() }
    }
    fn push(&mut self, s: &str) {
        self.sql.push_str(s);
        self.lit.push_str(s);
    }
    fn param(&mut self, v: &Value) {
        self.params.push(v.clone());
        match self.d {
            Dialect::Postgres => self.sql.push_str(&format!("${}", self.params.len())),
            Dialect::Sqlserver => self.sql.push_str(&format!("@P{}", self.params.len())),
            _ => self.sql.push('?'),
        }
        self.lit.push_str(&literal(v, self.d));
    }
    fn where_key(&mut self, key: &Map<String, Value>) {
        self.push(" WHERE ");
        for (i, (k, v)) in key.iter().enumerate() {
            if i > 0 {
                self.push(" AND ");
            }
            self.push(&quote_ident(k, self.d));
            if v.is_null() {
                self.push(" IS NULL");
            } else {
                self.push(" = ");
                self.param(v);
            }
        }
    }
    fn finish(self, expect_one: bool) -> ParamStatement {
        ParamStatement { sql: self.sql, params: self.params, preview: self.lit, expect_one }
    }
}

/// Builds the statements for a set of grid edits.
pub fn build_edits(d: Dialect, req: &EditRequest) -> Result<EditPreview> {
    if req.table.trim().is_empty() {
        return Err(invalid("no table to edit"));
    }
    let table = qualified(req.schema.as_deref(), &req.table, d);
    let mut statements = Vec::new();
    for e in &req.edits {
        let mut b = Builder::new(d);
        let expect_one = match e {
            RowEdit::Update { key, values } => {
                if key.is_empty() {
                    return Err(invalid("rows can only be edited by primary key (this result has none)"));
                }
                if values.is_empty() {
                    continue;
                }
                b.push(&format!("UPDATE {table} SET "));
                for (i, (k, v)) in values.iter().enumerate() {
                    if i > 0 {
                        b.push(", ");
                    }
                    b.push(&quote_ident(k, d));
                    b.push(" = ");
                    b.param(v);
                }
                b.where_key(key);
                true
            }
            RowEdit::Insert { values } => {
                if values.is_empty() {
                    b.push(&format!("INSERT INTO {table} {}", if d == Dialect::Mysql { "() VALUES ()" } else { "DEFAULT VALUES" }));
                } else {
                    let cols: Vec<String> = values.keys().map(|k| quote_ident(k, d)).collect();
                    b.push(&format!("INSERT INTO {table} ({}) VALUES (", cols.join(", ")));
                    for (i, v) in values.values().enumerate() {
                        if i > 0 {
                            b.push(", ");
                        }
                        b.param(v);
                    }
                    b.push(")");
                }
                false
            }
            RowEdit::Delete { key } => {
                if key.is_empty() {
                    return Err(invalid("rows can only be deleted by primary key (this result has none)"));
                }
                b.push(&format!("DELETE FROM {table}"));
                b.where_key(key);
                true
            }
        };
        statements.push(b.finish(expect_one));
    }
    if statements.is_empty() {
        return Err(invalid("nothing to apply"));
    }
    let begin = match d {
        Dialect::Mysql => "START TRANSACTION;",
        Dialect::Sqlserver => "BEGIN TRANSACTION;",
        _ => "BEGIN;",
    };
    let mut script = format!("{begin}\n");
    for s in &statements {
        script.push_str(&s.preview);
        script.push_str(";\n");
    }
    script.push_str("COMMIT;\n");
    Ok(EditPreview { statements, script })
}

/// Batched multi-row `INSERT`s for `rows` (column order: first appearance across rows; missing
/// values are NULL; nested objects/arrays become JSON text).
pub fn insert_batches(d: Dialect, schema: Option<&str>, table: &str, rows: &[Map<String, Value>]) -> Result<Vec<ParamStatement>> {
    if table.trim().is_empty() {
        return Err(invalid("no table to insert into"));
    }
    let mut cols: Vec<&str> = Vec::new();
    for r in rows {
        for k in r.keys() {
            if !cols.contains(&k.as_str()) {
                cols.push(k);
            }
        }
    }
    if cols.is_empty() {
        return Ok(Vec::new());
    }
    let qt = qualified(schema, table, d);
    let col_list = cols.iter().map(|c| quote_ident(c, d)).collect::<Vec<_>>().join(", ");
    // Stay well below every engine's bound-parameter limit (SQLite 32766, MySQL/Postgres 65535,
    // SQL Server 2100 per call and 1000 rows per VALUES list).
    let per_batch = if d == Dialect::Sqlserver { (2_000 / cols.len()).clamp(1, 1000) } else { (30_000 / cols.len()).clamp(1, 500) };
    let mut out = Vec::new();
    for chunk in rows.chunks(per_batch) {
        let mut b = Builder::new(d);
        b.push(&format!("INSERT INTO {qt} ({col_list}) VALUES "));
        for (ri, r) in chunk.iter().enumerate() {
            if ri > 0 {
                b.push(", ");
            }
            b.push("(");
            for (ci, c) in cols.iter().enumerate() {
                if ci > 0 {
                    b.push(", ");
                }
                let v = match r.get(*c) {
                    Some(v @ (Value::Object(_) | Value::Array(_))) => Value::String(v.to_string()),
                    Some(v) => v.clone(),
                    None => Value::Null,
                };
                b.param(&v);
            }
            b.push(")");
        }
        out.push(b.finish(false));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn edits_are_parameterized() {
        let req = EditRequest {
            schema: Some("public".into()),
            table: "users".into(),
            edits: vec![
                RowEdit::Update { key: map(json!({"id": 7})), values: map(json!({"name": "O'Brien", "active": true})) },
                RowEdit::Insert { values: map(json!({"name": "Ada", "meta": {"a": 1}})) },
                RowEdit::Delete { key: map(json!({"id": 9, "tenant": null})) },
            ],
        };
        let p = build_edits(Dialect::Postgres, &req).unwrap();
        assert_eq!(p.statements[0].sql, r#"UPDATE "public"."users" SET "name" = $1, "active" = $2 WHERE "id" = $3"#);
        assert_eq!(p.statements[0].params, vec![json!("O'Brien"), json!(true), json!(7)]);
        assert_eq!(p.statements[0].preview, r#"UPDATE "public"."users" SET "name" = 'O''Brien', "active" = TRUE WHERE "id" = 7"#);
        assert!(p.statements[0].expect_one);
        assert_eq!(p.statements[1].sql, r#"INSERT INTO "public"."users" ("name", "meta") VALUES ($1, $2)"#);
        assert_eq!(p.statements[2].sql, r#"DELETE FROM "public"."users" WHERE "id" = $1 AND "tenant" IS NULL"#);
        assert!(p.script.starts_with("BEGIN;\nUPDATE") && p.script.ends_with("COMMIT;\n"));

        let m = build_edits(Dialect::Mysql, &EditRequest { schema: None, table: "t`x".into(), edits: vec![RowEdit::Delete { key: map(json!({"id": 1})) }] }).unwrap();
        assert_eq!(m.statements[0].sql, "DELETE FROM `t``x` WHERE `id` = ?");
        assert!(m.script.starts_with("START TRANSACTION;"));

        let ms = build_edits(Dialect::Sqlserver, &EditRequest { schema: Some("dbo".into()), table: "users".into(), edits: vec![RowEdit::Update { key: map(json!({"id": 7})), values: map(json!({"name": "Zoë", "active": false})) }] }).unwrap();
        assert_eq!(ms.statements[0].sql, "UPDATE [dbo].[users] SET [name] = @P1, [active] = @P2 WHERE [id] = @P3");
        assert_eq!(ms.statements[0].preview, "UPDATE [dbo].[users] SET [name] = N'Zoë', [active] = 0 WHERE [id] = 7");
        assert!(ms.script.starts_with("BEGIN TRANSACTION;"));

        let no_key = EditRequest { schema: None, table: "t".into(), edits: vec![RowEdit::Update { key: Map::new(), values: map(json!({"a": 1})) }] };
        assert!(build_edits(Dialect::Sqlite, &no_key).is_err());
        assert!(build_edits(Dialect::Sqlite, &EditRequest { schema: None, table: "t".into(), edits: vec![] }).is_err());
    }

    #[test]
    fn inserts_are_batched() {
        let rows: Vec<Map<String, Value>> = (0..1203).map(|i| map(json!({"id": i, "tags": ["x"]}))).collect();
        let mut rows = rows;
        rows[5].insert("extra".into(), json!("late column"));
        let b = insert_batches(Dialect::Sqlite, None, "items", &rows).unwrap();
        assert_eq!(b.len(), 3);
        assert!(b[0].sql.starts_with(r#"INSERT INTO "items" ("id", "tags", "extra") VALUES (?, ?, ?), (?"#));
        assert_eq!(b[0].params.len(), 500 * 3);
        assert_eq!(b[0].params[1], json!("[\"x\"]"));
        assert_eq!(b[0].params[2], Value::Null);
        assert_eq!(b[2].params.len(), 203 * 3);
        let ms = insert_batches(Dialect::Sqlserver, Some("dbo"), "t", &rows[..1001]).unwrap();
        assert_eq!(ms.len(), 2, "2000 parameters per call at most");
        assert!(ms[0].sql.starts_with("INSERT INTO [dbo].[t] ([id], [tags], [extra]) VALUES (@P1, @P2, @P3), (@P4"), "{}", ms[0].sql);
        assert_eq!(ms[0].params.len(), 666 * 3);
        let pg = insert_batches(Dialect::Postgres, Some("s"), "t", &rows[4..6]).unwrap();
        assert_eq!(pg[0].sql, r#"INSERT INTO "s"."t" ("id", "tags", "extra") VALUES ($1, $2, $3), ($4, $5, $6)"#);
        assert!(insert_batches(Dialect::Sqlite, None, "t", &[]).unwrap().is_empty());
    }
}
