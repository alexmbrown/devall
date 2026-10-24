//! Fake-data schemas from a live connection: introspects SQL tables (columns, keys, unique
//! indexes, enum types and `CHECK (… IN …)` constraints) or samples MongoDB documents, and hands
//! them to [`devall_data::fromdb`]. Only the schema is read — never row data, except the sampled
//! MongoDB documents, which are reduced to field names and types before anything is saved.

use std::collections::HashMap;

use devall_data::fromdb::{self, ColumnSpec, ForeignKeySpec, FromDbOptions, FromDbResult, TableSpec};
use devall_data::SqlDialect;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{DbError, Result, invalid};
use crate::mongo::FindRequest;
use crate::session::{Engine, Session};
use crate::sql::{Dialect, SqlConn, TableDetails, TableKind};

/// A table to include (`schema` = the connection's default when omitted).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FakeSchemaRequest {
    /// Name of the generator schema.
    pub name: String,
    /// SQL tables; empty = every table of the default schema.
    pub tables: Vec<TableRef>,
    /// MongoDB database (default: the connection's).
    pub database: Option<String>,
    /// MongoDB collections; empty = every collection of the database (at most 50).
    pub collections: Vec<String>,
    /// Documents sampled per MongoDB collection (default 50, at most 1000).
    pub sample: Option<i64>,
    pub options: FromDbOptions,
}

fn dialect_of(d: Dialect) -> SqlDialect {
    match d {
        Dialect::Postgres => SqlDialect::Postgres,
        Dialect::Mysql => SqlDialect::Mysql,
        Dialect::Sqlite => SqlDialect::Sqlite,
        Dialect::Sqlserver => SqlDialect::Sqlserver,
    }
}

/// `"MySchema"."order_status"` / `order_status` → lookup keys for the enum map.
fn enum_values_for(data_type: &str, enums: &HashMap<String, Vec<String>>) -> Vec<String> {
    let t = data_type.trim().trim_end_matches("[]").replace('"', "");
    enums.get(&t).or_else(|| t.rsplit('.').next().and_then(|bare| enums.get(bare))).cloned().unwrap_or_default()
}

/// Turns introspected details into the neutral spec `devall-data` works with.
pub fn table_spec(d: &TableDetails, enums: &HashMap<String, Vec<String>>) -> TableSpec {
    let checks = d.ddl.as_deref().map(fromdb::check_enum_values).unwrap_or_default();
    let unique_cols: Vec<&str> = d.indexes.iter().filter(|i| i.unique && !i.primary && i.columns.len() == 1).map(|i| i.columns[0].as_str()).collect();
    let columns = d
        .columns
        .iter()
        .map(|c| {
            let mut enum_values = fromdb::mysql_enum_values(&c.data_type);
            if enum_values.is_empty() {
                enum_values = enum_values_for(&c.data_type, enums);
            }
            if enum_values.is_empty() {
                enum_values = checks.get(&c.name.to_ascii_lowercase()).cloned().unwrap_or_default();
            }
            ColumnSpec {
                name: c.name.clone(),
                data_type: c.data_type.clone(),
                nullable: c.nullable,
                default: c.default.clone(),
                primary_key: c.primary_key || d.primary_key.iter().any(|p| p == &c.name),
                unique: unique_cols.contains(&c.name.as_str()),
                enum_values,
            }
        })
        .collect();
    let foreign_keys = d
        .foreign_keys
        .iter()
        .map(|f| ForeignKeySpec {
            columns: f.columns.clone(),
            ref_schema: f.ref_schema.clone(),
            ref_table: f.ref_table.clone(),
            ref_columns: f.ref_columns.iter().filter(|c| !c.is_empty()).cloned().collect(),
        })
        .collect();
    TableSpec { schema: d.schema.clone(), name: d.name.clone(), columns, foreign_keys }
}

/// Introspects `tables` (default: every base table of the default schema).
pub async fn sql_table_specs(conn: &SqlConn, tables: &[TableRef]) -> Result<Vec<TableSpec>> {
    let tree = conn.schema().await?;
    let default_schema = tree.default_schema.clone();
    let wanted: Vec<TableRef> = if tables.is_empty() {
        let schema = tree.schemas.iter().find(|s| Some(&s.name) == default_schema.as_ref()).or_else(|| tree.schemas.first());
        schema
            .map(|s| s.tables.iter().filter(|t| t.kind == TableKind::Table).map(|t| TableRef { schema: Some(s.name.clone()), name: t.name.clone() }).collect())
            .unwrap_or_default()
    } else {
        tables.to_vec()
    };
    if wanted.is_empty() {
        return Err(invalid("no tables to read — create some tables first"));
    }
    if wanted.len() > 200 {
        return Err(invalid("pick at most 200 tables"));
    }
    let enums = conn.enum_types().await.unwrap_or_default();
    let mut out = Vec::with_capacity(wanted.len());
    for t in &wanted {
        let schema = t.schema.clone().or_else(|| default_schema.clone());
        let details = conn.table_details(schema.as_deref(), &t.name).await?;
        out.push(table_spec(&details, &enums));
    }
    Ok(out)
}

/// A compact text description of tables for prompts (names, types, keys, enum values — never
/// rows): `users(id integer PK, email text NOT NULL UNIQUE, team_id integer → teams.id)`.
pub fn summarize_specs(dialect: &str, specs: &[TableSpec], more: &[String]) -> String {
    let mut s = format!("Dialect: {dialect}\n");
    for t in specs {
        let fks: std::collections::HashMap<&str, String> = t
            .foreign_keys
            .iter()
            .filter(|f| f.columns.len() == 1)
            .map(|f| (f.columns[0].as_str(), format!("{}.{}", f.ref_table, f.ref_columns.first().cloned().unwrap_or_else(|| "<pk>".into()))))
            .collect();
        let cols: Vec<String> = t
            .columns
            .iter()
            .map(|c| {
                let mut x = format!("{} {}", c.name, c.data_type);
                if c.primary_key {
                    x.push_str(" PK");
                }
                if !c.nullable && !c.primary_key {
                    x.push_str(" NOT NULL");
                }
                if c.unique {
                    x.push_str(" UNIQUE");
                }
                if !c.enum_values.is_empty() {
                    x.push_str(&format!(" IN ({})", c.enum_values.iter().take(12).map(|v| format!("'{v}'")).collect::<Vec<_>>().join(", ")));
                }
                if let Some(target) = fks.get(c.name.as_str()) {
                    x.push_str(&format!(" → {target}"));
                }
                x
            })
            .collect();
        let name = match &t.schema {
            Some(sc) => format!("{sc}.{}", t.name),
            None => t.name.clone(),
        };
        s.push_str(&format!("{name}({})\n", cols.join(", ")));
        for f in t.foreign_keys.iter().filter(|f| f.columns.len() > 1) {
            s.push_str(&format!("  FOREIGN KEY ({}) → {}({})\n", f.columns.join(", "), f.ref_table, f.ref_columns.join(", ")));
        }
    }
    if !more.is_empty() {
        s.push_str(&format!("Other tables (columns not listed): {}\n", more.join(", ")));
    }
    s
}

/// Field paths and value types seen in sampled MongoDB documents (relaxed Extended JSON) —
/// `user.email: string (48/50)`. Values themselves are dropped.
pub fn field_summary(docs: &[Value]) -> String {
    fn kind(v: &Value) -> &'static str {
        match v {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(n) if n.is_i64() || n.is_u64() => "int",
            Value::Number(_) => "double",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(o) if o.contains_key("$oid") => "objectId",
            Value::Object(o) if o.contains_key("$date") => "date",
            Value::Object(o) if o.contains_key("$numberDecimal") => "decimal",
            Value::Object(o) if o.contains_key("$numberLong") => "long",
            Value::Object(o) if o.contains_key("$binary") => "binary",
            Value::Object(_) => "object",
        }
    }
    fn walk(v: &Value, path: &str, depth: usize, out: &mut std::collections::BTreeMap<String, (std::collections::BTreeSet<&'static str>, usize)>) {
        if depth > 6 {
            return;
        }
        if let Value::Object(o) = v {
            for (k, x) in o {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                let e = out.entry(p.clone()).or_default();
                e.0.insert(kind(x));
                e.1 += 1;
                match x {
                    Value::Object(m) if kind(x) == "object" && !m.is_empty() => walk(x, &p, depth + 1, out),
                    Value::Array(a) => {
                        for item in a.iter().take(20) {
                            let ip = format!("{p}[]");
                            let e = out.entry(ip.clone()).or_default();
                            e.0.insert(kind(item));
                            if kind(item) == "object" {
                                walk(item, &ip, depth + 1, out);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    for d in docs {
        walk(d, "", 0, &mut out);
    }
    if out.is_empty() {
        return "(no documents sampled)".into();
    }
    let n = docs.len();
    out.into_iter()
        .take(300)
        .map(|(p, (kinds, count))| {
            let k = kinds.into_iter().collect::<Vec<_>>().join("|");
            if p.ends_with("[]") || count == n { format!("{p}: {k}") } else { format!("{p}: {k} ({count}/{n})") }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Session {
    /// Schema summary for NL → SQL: every table of the default schema (or `tables`) with
    /// columns, keys and enums; at most `max_tables` described, the rest only named.
    pub async fn schema_summary(&self, tables: &[TableRef], max_tables: usize) -> Result<String> {
        let conn = self.sql()?;
        let tree = conn.schema().await?;
        let default_schema = tree.default_schema.clone();
        let mut wanted: Vec<TableRef> = if tables.is_empty() {
            let mut v: Vec<TableRef> = Vec::new();
            // Default schema first, then the others.
            let mut schemas: Vec<&crate::sql::SchemaNode> = tree.schemas.iter().collect();
            schemas.sort_by_key(|s| Some(&s.name) != default_schema.as_ref());
            for s in schemas {
                for t in &s.tables {
                    v.push(TableRef { schema: Some(s.name.clone()), name: t.name.clone() });
                }
            }
            v
        } else {
            tables.to_vec()
        };
        let more: Vec<String> = wanted.split_off(wanted.len().min(max_tables.max(1))).into_iter().map(|t| t.name).collect();
        let specs = sql_table_specs(conn, &wanted).await?;
        let dialect = format!("{:?}", conn.dialect()).to_ascii_lowercase();
        let mut s = summarize_specs(&dialect, &specs, &more);
        if let Some(d) = &default_schema {
            s = s.replacen("\n", &format!(" (default schema: {d})\n"), 1);
        }
        Ok(s)
    }

    /// Field names and types of `sample` documents of a MongoDB collection (no values).
    pub async fn mongo_field_summary(&self, database: &str, collection: &str, sample: i64) -> Result<String> {
        let m = self.mongo()?;
        let docs = m.find(&FindRequest { database: database.to_string(), collection: collection.to_string(), limit: Some(sample.clamp(1, 500)), ..Default::default() }).await?;
        Ok(field_summary(&docs.docs))
    }

    /// A fake-data schema for this connection's tables (SQL) or sampled collections (MongoDB).
    pub async fn fake_schema(&self, req: &FakeSchemaRequest) -> Result<FromDbResult> {
        let name = if req.name.trim().is_empty() { self.info().name } else { req.name.trim().to_string() };
        match self.engine() {
            Engine::Sql(conn) => {
                let specs = sql_table_specs(conn, &req.tables).await?;
                let mut opts = req.options.clone();
                opts.dialect = Some(dialect_of(conn.dialect()));
                Ok(fromdb::schema_from_tables(&name, &specs, &opts))
            }
            Engine::Mongo(m) => {
                let db = req.database.clone().filter(|d| !d.is_empty()).or_else(|| m.default_db.clone()).ok_or_else(|| invalid("choose a database"))?;
                let collections: Vec<String> = if req.collections.is_empty() {
                    m.collections(&db).await?.into_iter().filter(|c| c.kind == "collection" && !c.name.starts_with("system.")).map(|c| c.name).take(50).collect()
                } else {
                    req.collections.clone()
                };
                if collections.is_empty() {
                    return Err(invalid(format!("database {db} has no collections")));
                }
                let limit = req.sample.unwrap_or(50).clamp(1, 1000);
                let mut samples: Vec<(String, Vec<Value>)> = Vec::new();
                for c in collections {
                    let docs = m.find(&FindRequest { database: db.clone(), collection: c.clone(), limit: Some(limit), ..Default::default() }).await?;
                    samples.push((c, docs.docs));
                }
                Ok(fromdb::schema_from_documents(&name, &samples, &req.options))
            }
            Engine::Redis(_) => Err(DbError::Invalid("fake-data schemas can be built from SQL and MongoDB connections, not Redis".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::{ColumnInfo, ForeignKeyInfo, IndexInfo};

    #[test]
    fn summaries_have_shapes_not_values() {
        let docs = vec![
            serde_json::json!({ "_id": { "$oid": "65f0c1aa0000000000000001" }, "email": "ada@example.com", "tags": ["a"], "address": { "city": "Paris" }, "at": { "$date": "2024-01-01T00:00:00Z" } }),
            serde_json::json!({ "_id": { "$oid": "65f0c1aa0000000000000002" }, "email": "bob@example.com", "age": 3 }),
        ];
        let s = field_summary(&docs);
        assert!(s.contains("_id: objectId") && s.contains("address.city: string (1/2)") && s.contains("tags[]: string") && s.contains("at: date (1/2)"), "{s}");
        assert!(!s.contains("ada@example.com"));
        let spec = TableSpec {
            schema: Some("public".into()),
            name: "orders".into(),
            columns: vec![
                ColumnSpec { name: "id".into(), data_type: "integer".into(), primary_key: true, ..Default::default() },
                ColumnSpec { name: "status".into(), data_type: "text".into(), enum_values: vec!["new".into()], ..Default::default() },
                ColumnSpec { name: "user_id".into(), data_type: "integer".into(), nullable: true, ..Default::default() },
            ],
            foreign_keys: vec![ForeignKeySpec { columns: vec!["user_id".into()], ref_table: "users".into(), ref_columns: vec!["id".into()], ..Default::default() }],
        };
        let s = summarize_specs("postgres", &[spec], &["audit_log".into()]);
        assert!(s.contains("public.orders(id integer PK, status text NOT NULL IN ('new'), user_id integer → users.id)"), "{s}");
        assert!(s.contains("Other tables (columns not listed): audit_log"));
    }

    #[test]
    fn details_become_specs() {
        let col = |name: &str, ty: &str, pk: bool| ColumnInfo { name: name.into(), data_type: ty.into(), nullable: !pk, default: None, primary_key: pk };
        let d = TableDetails {
            schema: Some("public".into()),
            name: "orders".into(),
            kind: TableKind::Table,
            columns: vec![col("id", "integer", true), col("status", "order_status", false), col("size", "enum('S','M')", false), col("email", "text", false), col("kind", "text", false)],
            primary_key: vec!["id".into()],
            indexes: vec![IndexInfo { name: "orders_email_key".into(), columns: vec!["email".into()], unique: true, primary: false, method: None, definition: None }],
            foreign_keys: vec![ForeignKeyInfo { name: "fk".into(), columns: vec!["user_id".into()], ref_schema: None, ref_table: "users".into(), ref_columns: vec![String::new()], on_update: None, on_delete: None }],
            ddl: Some("CREATE TABLE orders (kind text CHECK (kind IN ('a','b')))".into()),
        };
        let enums: HashMap<String, Vec<String>> = [("order_status".to_string(), vec!["new".to_string(), "paid".into()])].into();
        let s = table_spec(&d, &enums);
        assert_eq!(s.columns[1].enum_values, vec!["new", "paid"]);
        assert_eq!(s.columns[2].enum_values, vec!["S", "M"]);
        assert!(s.columns[3].unique && !s.columns[1].unique);
        assert_eq!(s.columns[4].enum_values, vec!["a", "b"]);
        assert!(s.foreign_keys[0].ref_columns.is_empty(), "SQLite's implicit primary-key reference");
        assert_eq!(enum_values_for("\"public\".\"order_status\"[]", &enums), vec!["new", "paid"]);
    }
}
