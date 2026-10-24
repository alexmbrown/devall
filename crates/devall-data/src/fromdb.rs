//! Generator schemas from a database schema: tables become entities, columns become fields whose
//! generators come from the column type plus name heuristics (`email`, `first_name`, `created_at`,
//! `price`…), enum values from `ENUM` types / `CHECK (col IN (…))`, primary keys become
//! sequences / UUIDs, single-column foreign keys become `ref`s (so seeding is relationally
//! consistent) and nullable columns get a null rate.
//!
//! The engine-specific introspection lives in `devall-db`; this module only sees neutral
//! [`TableSpec`]s (and, for MongoDB, sampled documents), so it is pure and easy to test.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::infer::{merge_objects, norm_key, singular, string_kind_for_key};
use crate::model::{Entity, Field, FieldKind, Schema, SqlDialect};

/// One column as introspected.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ColumnSpec {
    pub name: String,
    /// The engine's type text: `varchar(255)`, `timestamp with time zone`, `enum('a','b')`,
    /// `INTEGER`, `numeric(10,2)`, a Postgres enum type name…
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    /// A single-column unique index or constraint.
    pub unique: bool,
    /// Allowed values (enum types, `CHECK (col IN (…))`).
    pub enum_values: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ForeignKeySpec {
    pub columns: Vec<String>,
    pub ref_schema: Option<String>,
    pub ref_table: String,
    /// Empty = the referenced table's primary key.
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TableSpec {
    pub schema: Option<String>,
    pub name: String,
    pub columns: Vec<ColumnSpec>,
    pub foreign_keys: Vec<ForeignKeySpec>,
}

impl TableSpec {
    fn pk(&self) -> Vec<&ColumnSpec> {
        self.columns.iter().filter(|c| c.primary_key).collect()
    }
}

fn default_count() -> u64 {
    25
}
fn default_null() -> f64 {
    10.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FromDbOptions {
    /// Rows per table without foreign keys; tables with foreign keys get twice as many.
    #[serde(default = "default_count")]
    pub count: u64,
    /// Null rate (0–100) for nullable columns that are not keys.
    #[serde(default = "default_null")]
    pub null_percent: f64,
    /// Pinned seed (default 42), so entities seeded one by one stay consistent.
    pub seed: Option<u64>,
    /// Shapes date/time formats so the target database accepts them.
    pub dialect: Option<SqlDialect>,
}

impl Default for FromDbOptions {
    fn default() -> Self {
        Self { count: default_count(), null_percent: default_null(), seed: None, dialect: None }
    }
}

/// A schema plus what could not be mapped exactly.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FromDbResult {
    pub schema: Schema,
    pub warnings: Vec<String>,
}

// ---------- type text helpers ----------

/// Lower-case base type without parameters or array brackets: `character varying(20)` →
/// `character varying`.
fn base_type(t: &str) -> String {
    let t = t.trim().to_ascii_lowercase();
    let t = t.split('(').next().unwrap_or("").trim().trim_end_matches("[]").trim();
    t.trim_start_matches("unsigned ").trim_end_matches(" unsigned").trim().to_string()
}

/// `(a, b)` parameters of a type: `numeric(10,2)` → `[10, 2]`.
fn type_params(t: &str) -> Vec<u32> {
    let Some(start) = t.find('(') else { return Vec::new() };
    let Some(end) = t[start..].find(')') else { return Vec::new() };
    t[start + 1..start + end].split(',').filter_map(|p| p.trim().parse().ok()).collect()
}

/// Quoted string literals in `s` (`'a'`, with `''` escapes).
fn quoted_strings(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            continue;
        }
        let mut lit = String::new();
        loop {
            match chars.next() {
                Some('\'') if chars.peek() == Some(&'\'') => {
                    chars.next();
                    lit.push('\'');
                }
                Some('\'') | None => break,
                Some(x) => lit.push(x),
            }
        }
        out.push(lit);
    }
    out
}

/// MySQL `enum('a','b')` / `set('a','b')` column types → their values.
pub fn mysql_enum_values(data_type: &str) -> Vec<String> {
    let lower = data_type.trim_start().to_ascii_lowercase();
    if lower.starts_with("enum(") || lower.starts_with("set(") { quoted_strings(data_type) } else { Vec::new() }
}

/// The parenthesized text after `open` (balanced, quotes respected).
fn balanced(s: &str, open: usize) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_str = false;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'\'' => in_str = !in_str,
            b'(' if !in_str => depth += 1,
            b')' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[open + 1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

const CHECK_WORDS: &[&str] = &["in", "any", "array", "or", "text", "character", "varying", "varchar", "bpchar", "char", "string", "collate", "nocase"];

/// `CHECK (status IN ('a', 'b'))`, `CHECK ((status)::text = ANY ((ARRAY['a'::character varying,
/// …])::text[]))` and `CHECK (s = 'a' OR s = 'b')` constraints in DDL → column (lower case) →
/// allowed values.
pub fn check_enum_values(ddl: &str) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    let lower = ddl.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find("check") {
        let at = from + pos;
        from = at + 5;
        // Must be the keyword, not part of an identifier.
        if at > 0 && lower.as_bytes()[at - 1].is_ascii_alphanumeric() {
            continue;
        }
        let rest = &ddl[at + 5..];
        let skip = rest.len() - rest.trim_start().len();
        let Some(expr) = balanced(ddl, at + 5 + skip) else { continue };
        let el = expr.to_ascii_lowercase();
        if ["not ", "<", ">", " like", "between", "length(", "glob", "~", "<>", "!="].iter().any(|w| el.contains(w)) {
            continue;
        }
        if !(el.contains(" in") || el.contains("any") || el.contains('=')) {
            continue;
        }
        let values = quoted_strings(expr);
        if values.is_empty() {
            continue;
        }
        // Identifiers left once literals, casts and keywords are removed.
        let mut stripped = String::new();
        let mut in_str = false;
        for c in expr.chars() {
            if c == '\'' {
                // SQL Server's N'…' prefix is not an identifier.
                if !in_str && (stripped.ends_with('N') || stripped.ends_with('n')) {
                    let before = stripped[..stripped.len() - 1].chars().next_back();
                    if !before.is_some_and(|b| b.is_alphanumeric() || b == '_') {
                        stripped.pop();
                    }
                }
                in_str = !in_str;
                stripped.push(' ');
            } else if !in_str {
                stripped.push(c);
            }
        }
        let idents: BTreeSet<String> = stripped
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '"' || c == '`' || c == '[' || c == ']'))
            .map(|w| w.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']').to_ascii_lowercase())
            .filter(|w| !w.is_empty() && !w.chars().next().is_some_and(|c| c.is_ascii_digit()) && !CHECK_WORDS.contains(&w.as_str()))
            .collect();
        if idents.len() == 1 {
            let col = idents.into_iter().next().unwrap_or_default();
            let mut uniq: Vec<String> = Vec::new();
            for v in values {
                if !uniq.contains(&v) {
                    uniq.push(v);
                }
            }
            out.insert(col, uniq);
        }
    }
    out
}

// ---------- column → generator ----------

const MONEY: &[&str] = &["price", "amount", "cost", "total", "subtotal", "balance", "fee", "salary", "revenue", "tax", "discount", "unitprice", "charge", "payment"];

fn is_money_key(k: &str) -> bool {
    MONEY.iter().any(|m| k == *m || k.ends_with(m))
}

fn looks_time_key(k: &str) -> bool {
    k.ends_with("at") || k.ends_with("time") || k.ends_with("timestamp") || k.ends_with("date") || k.starts_with("created") || k.starts_with("updated") || k == "ts"
}

fn datetime_kind(dialect: Option<SqlDialect>) -> FieldKind {
    // MySQL DATETIME/TIMESTAMP literals don't take a `T`/`Z`; Postgres and SQLite take RFC 3339.
    // SQL Server's `datetime` misreads `YYYY-MM-DD hh:mm:ss` under some DATEFORMAT settings, so it
    // gets the unambiguous ISO 8601 form with a `T` (and no `Z`).
    let format = match dialect {
        Some(SqlDialect::Mysql) => Some("%Y-%m-%d %H:%M:%S".to_string()),
        Some(SqlDialect::Sqlserver) => Some("%Y-%m-%dT%H:%M:%S".to_string()),
        _ => None,
    };
    FieldKind::Datetime { from: None, to: None, format }
}

fn int_kind(k: &str, base: &str) -> FieldKind {
    if k == "age" {
        return FieldKind::int(18, 90);
    }
    if k == "year" || k.ends_with("year") {
        return FieldKind::int(1990, 2030);
    }
    if k == "quantity" || k == "qty" || k.ends_with("quantity") {
        return FieldKind::int(1, 10);
    }
    if k.ends_with("count") || k.starts_with("num") || k.ends_with("total") {
        return FieldKind::int(0, 100);
    }
    if k == "rating" || k == "stars" || k == "score" {
        return FieldKind::int(1, 5);
    }
    if k.ends_with("percent") || k.ends_with("pct") {
        return FieldKind::int(0, 100);
    }
    if looks_time_key(k) && matches!(base, "bigint" | "int8") {
        return FieldKind::Timestamp { from: None, to: None, unit: None };
    }
    match base {
        "tinyint" | "int1" => FieldKind::int(0, 100),
        "smallint" | "int2" | "smallserial" | "serial2" => FieldKind::int(0, 1000),
        _ => FieldKind::int(0, 10_000),
    }
}

/// A generator that never exceeds `max` characters.
fn short_text(k: &str, max: u32) -> FieldKind {
    if let Some(kind) = string_kind_for_key(k) {
        let fits = match &kind {
            FieldKind::CountryCode | FieldKind::Currency => max >= 3,
            FieldKind::Zip => max >= 10,
            FieldKind::FirstName | FieldKind::LastName | FieldKind::City | FieldKind::State | FieldKind::Country | FieldKind::Username | FieldKind::Phone => max >= 20,
            _ => max >= 40,
        };
        if fits {
            return kind;
        }
    }
    let n = max.clamp(1, 12);
    FieldKind::Regex { pattern: format!("[a-z]{{1,{n}}}") }
}

fn text_kind(k: &str, base: &str, len: Option<u32>) -> FieldKind {
    if let Some(max) = len.filter(|m| *m < 40) {
        if max <= 2 && (k.contains("country") || k == "cc" || k == "iso2") {
            return FieldKind::CountryCode;
        }
        return short_text(k, max);
    }
    if let Some(kind) = string_kind_for_key(k) {
        return kind;
    }
    if k == "name" || k.ends_with("name") {
        return if k == "name" || k.contains("product") || k.contains("item") { FieldKind::ProductName } else { FieldKind::FullName };
    }
    if k.ends_with("code") || k.ends_with("ref") || k.ends_with("sku") || k.ends_with("token") || k.ends_with("key") {
        return FieldKind::Regex { pattern: "[A-Z0-9]{8}".into() };
    }
    if k.ends_with("slug") {
        return FieldKind::Regex { pattern: "[a-z]{4,10}-[a-z]{4,10}".into() };
    }
    if k.ends_with("color") || k.ends_with("colour") {
        return FieldKind::Regex { pattern: "#[0-9a-f]{6}".into() };
    }
    match (base, len) {
        ("text" | "clob" | "longtext" | "mediumtext" | "ntext", _) => FieldKind::Paragraph { sentences: Some(2) },
        (_, Some(n)) if n >= 200 => FieldKind::Sentence { min_words: Some(4), max_words: Some(12) },
        _ => FieldKind::Sentence { min_words: Some(2), max_words: Some(5) },
    }
}

/// The generator for a column (without null rate / uniqueness).
pub fn kind_for_column(col: &ColumnSpec, dialect: Option<SqlDialect>) -> Option<FieldKind> {
    let k = norm_key(&col.name);
    let raw = col.data_type.trim();
    let base = base_type(raw);
    let params = type_params(raw);
    if !col.enum_values.is_empty() {
        return Some(FieldKind::Enum { values: col.enum_values.iter().map(|v| Value::String(v.clone())).collect(), weights: None });
    }
    let is = |list: &[&str]| list.contains(&base.as_str());
    // Binary data can't be generated meaningfully (and JSON text won't bind to bytea).
    if is(&["bytea", "blob", "binary", "varbinary", "longblob", "mediumblob", "tinyblob", "image"]) {
        return None;
    }
    if is(&["uuid", "uniqueidentifier"]) || (k.ends_with("uuid") && (base.contains("char") || base == "text")) || (is(&["char", "character", "bpchar"]) && params.first() == Some(&36)) {
        return Some(FieldKind::uuid());
    }
    if is(&["bool", "boolean"]) || (base == "tinyint" && params.first() == Some(&1)) || (base == "bit" && params.first().is_none_or(|n| *n == 1)) {
        return Some(FieldKind::Bool { true_percent: None });
    }
    if is(&["date"]) {
        return Some(FieldKind::Date { from: None, to: None, format: None });
    }
    if base.starts_with("timestamp") || is(&["datetime", "datetime2", "smalldatetime", "datetimeoffset"]) {
        return Some(datetime_kind(dialect));
    }
    if base.starts_with("time") || base == "timetz" {
        return Some(FieldKind::Regex { pattern: "(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]".into() });
    }
    if base == "interval" {
        return Some(FieldKind::Template { value: "1 day".into() });
    }
    if is(&["json", "jsonb"]) {
        return Some(FieldKind::Json { value: json!({}) });
    }
    if is(&["inet", "cidr"]) {
        return Some(FieldKind::Ipv4);
    }
    if is(&["macaddr", "macaddr8"]) {
        return Some(FieldKind::Regex { pattern: "([0-9a-f]{2}:){5}[0-9a-f]{2}".into() });
    }
    if is(&["year"]) {
        return Some(FieldKind::int(1990, 2030));
    }
    let int_like = is(&["int", "integer", "bigint", "smallint", "tinyint", "mediumint", "int2", "int4", "int8", "serial", "bigserial", "smallserial", "serial4", "serial8", "serial2"]);
    if int_like {
        if k == "lat" || k == "latitude" {
            return Some(FieldKind::Latitude);
        }
        if k == "lng" || k == "lon" || k == "longitude" {
            return Some(FieldKind::Longitude);
        }
        if is_money_key(&k) && !k.ends_with("count") {
            // Money in minor units (cents).
            return Some(FieldKind::int(100, 100_000));
        }
        return Some(int_kind(&k, &base));
    }
    let float_like = is(&["numeric", "decimal", "real", "double", "double precision", "float", "float4", "float8", "money", "smallmoney", "number"]);
    if float_like {
        if k == "lat" || k == "latitude" {
            return Some(FieldKind::Latitude);
        }
        if k == "lng" || k == "lon" || k == "longitude" {
            return Some(FieldKind::Longitude);
        }
        let scale = params.get(1).copied();
        // numeric(p, s) caps the magnitude at 10^(p-s).
        let cap = params.first().map(|p| 10f64.powi(p.saturating_sub(scale.unwrap_or(0)).min(12) as i32) - 1.0);
        if base == "money" || is_money_key(&k) {
            let max = cap.map(|c| c.min(1000.0)).unwrap_or(1000.0).max(1.0);
            return Some(FieldKind::Price { min: Some(1.0_f64.min(max)), max: Some(max), decimals: Some(scale.unwrap_or(2).min(2)) });
        }
        if k == "rating" || k == "score" {
            return Some(FieldKind::Float { min: Some(0.0), max: Some(5.0), decimals: Some(1) });
        }
        let max = cap.map(|c| c.min(1000.0)).unwrap_or(1000.0).max(1.0);
        return Some(FieldKind::Float { min: Some(0.0), max: Some(max), decimals: Some(scale.unwrap_or(2).min(4)) });
    }
    // Strings (and anything unknown, e.g. SQLite's free-form types).
    let len = params.first().copied().filter(|_| base.contains("char") || base == "string" || base == "nvarchar" || base == "varchar2");
    if base.is_empty() || base.contains("char") || base.contains("text") || base.contains("clob") || is(&["string", "citext", "name", "tsvector"]) {
        if base == "tsvector" {
            return Some(FieldKind::Lorem { words: Some(5) });
        }
        if looks_time_key(&k) && (k.starts_with("created") || k.starts_with("updated") || k.ends_with("at")) && len.is_none_or(|n| n >= 20) {
            return Some(datetime_kind(dialect));
        }
        return Some(text_kind(&k, &base, len));
    }
    // Unknown type: a Postgres enum without known values, a domain, a geometry…
    Some(text_kind(&k, &base, None))
}

// ---------- tables → schema ----------

fn entity_names(tables: &[TableSpec]) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in tables {
        *counts.entry(t.name.as_str()).or_default() += 1;
    }
    tables
        .iter()
        .map(|t| match (&t.schema, counts.get(t.name.as_str()).copied().unwrap_or(0) > 1) {
            (Some(s), true) => format!("{s}.{}", t.name),
            _ => t.name.clone(),
        })
        .collect()
}

fn qualified(t: &TableSpec) -> String {
    match &t.schema {
        Some(s) => format!("{s}.{}", t.name),
        None => t.name.clone(),
    }
}

/// Builds a generator schema from introspected tables. Tables outside `tables` that are
/// referenced by a foreign key can't be generated: those columns get a plain generator and a
/// warning.
pub fn schema_from_tables(name: &str, tables: &[TableSpec], opts: &FromDbOptions) -> FromDbResult {
    let mut warnings = Vec::new();
    let names = entity_names(tables);
    let find_table = |schema: Option<&str>, table: &str| -> Option<usize> {
        tables
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(table) && (schema.is_none() || t.schema.is_none() || t.schema.as_deref().is_some_and(|s| Some(s) == schema)))
    };

    // Single-column foreign keys: (table, column) → (target table, target column).
    let mut fk: HashMap<(usize, String), (usize, String)> = HashMap::new();
    for (ti, t) in tables.iter().enumerate() {
        for f in &t.foreign_keys {
            let Some(target) = find_table(f.ref_schema.as_deref(), &f.ref_table) else {
                warnings.push(format!("{}: {} references {} which isn't included, so it gets plain values", qualified(t), f.columns.join(", "), f.ref_table));
                continue;
            };
            let ref_cols: Vec<String> = if f.ref_columns.iter().all(|c| c.trim().is_empty()) {
                tables[target].pk().iter().map(|c| c.name.clone()).collect()
            } else {
                f.ref_columns.clone()
            };
            if f.columns.len() > 1 {
                warnings.push(format!(
                    "{}: composite foreign key ({}) → {} — only `{}` follows the reference; check the generated pairs exist",
                    qualified(t),
                    f.columns.join(", "),
                    f.ref_table,
                    f.columns[0]
                ));
            }
            if let (Some(c), Some(rc)) = (f.columns.first(), ref_cols.first()) {
                fk.insert((ti, c.clone()), (target, rc.clone()));
            }
        }
    }

    // Break reference cycles between tables (A → B → A) at a nullable column.
    let mut nulled: BTreeSet<(usize, String)> = BTreeSet::new();
    loop {
        let deps: Vec<BTreeSet<usize>> =
            (0..tables.len()).map(|ti| fk.iter().filter(|((s, _), (t, _))| *s == ti && *t != ti).map(|(_, (t, _))| *t).collect()).collect();
        let Some(cycle) = find_cycle(&deps) else { break };
        let mut edges: Vec<(usize, String)> = fk.iter().filter(|((s, _), (t, _))| cycle.contains(s) && cycle.contains(t) && s != t).map(|(k, _)| k.clone()).collect();
        edges.sort();
        let pick = edges.iter().find(|(s, c)| tables[*s].columns.iter().any(|col| &col.name == c && col.nullable)).or_else(|| edges.first()).cloned();
        let Some(edge) = pick else { break };
        let nullable = tables[edge.0].columns.iter().any(|c| c.name == edge.1 && c.nullable);
        warnings.push(format!(
            "{}.{}: tables reference each other in a loop; this column {} so generation can order the tables",
            qualified(&tables[edge.0]),
            edge.1,
            if nullable { "is left null" } else { "gets plain values (not a reference)" }
        ));
        fk.remove(&edge);
        if nullable {
            nulled.insert(edge);
        }
    }

    let mut schema = Schema::new(name);
    schema.seed = Some(opts.seed.unwrap_or(42));
    let null = opts.null_percent.clamp(0.0, 100.0);
    for (ti, t) in tables.iter().enumerate() {
        let has_fk = fk.keys().any(|(s, _)| *s == ti);
        let count = if has_fk { opts.count.saturating_mul(2) } else { opts.count }.max(1);
        let single_pk = t.pk().len() == 1;
        let mut fields = Vec::new();
        for col in &t.columns {
            if nulled.contains(&(ti, col.name.clone())) {
                // Cycle broken at a nullable column: always null.
                fields.push(Field::new(&col.name, FieldKind::Json { value: Value::Null }));
                continue;
            }
            if let Some((target, tcol)) = fk.get(&(ti, col.name.clone())) {
                let mut f = Field::new(&col.name, FieldKind::reference(&names[*target], tcol));
                if col.nullable && !col.primary_key {
                    f.null_percent = null;
                }
                // A unique FK is a 1:1 relation.
                f.unique = col.unique || (col.primary_key && single_pk);
                fields.push(f);
                continue;
            }
            let kind = if col.primary_key && single_pk { pk_kind(col, opts.dialect) } else { kind_for_column(col, opts.dialect) };
            let Some(kind) = kind else {
                if col.nullable || col.default.is_some() {
                    warnings.push(format!("{}.{}: {} values aren't generated (the column is left to its default / NULL)", qualified(t), col.name, col.data_type));
                } else {
                    warnings.push(format!("{}.{}: {} is NOT NULL but binary values can't be generated — edit this field", qualified(t), col.name, col.data_type));
                    fields.push(Field::new(&col.name, FieldKind::Regex { pattern: "[0-9a-f]{16}".into() }));
                }
                continue;
            };
            let mut f = Field::new(&col.name, kind);
            if col.nullable && !col.primary_key {
                f.null_percent = null;
            }
            if (col.unique || (col.primary_key && single_pk)) && can_be_unique(&f.kind) {
                f.unique = true;
                if let FieldKind::Int { min, max } = &mut f.kind {
                    let (lo, hi) = (min.unwrap_or(0), max.unwrap_or(1000));
                    if ((hi - lo) as u64) < count.saturating_mul(10) {
                        *max = Some(lo + count.saturating_mul(10) as i64);
                    }
                }
            } else if col.unique && !can_be_unique(&f.kind) {
                warnings.push(format!("{}.{} is unique but its generator has few distinct values — edit it before seeding many rows", qualified(t), col.name));
            }
            fields.push(f);
        }
        if t.pk().len() > 1 {
            warnings.push(format!("{}: composite primary key ({}) — duplicate key pairs are possible with many rows", qualified(t), t.pk().iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
        }
        let mut e = Entity::new(&names[ti], count, fields);
        e.extra.insert("table".into(), Value::String(qualified(t)));
        schema.entities.push(e);
    }
    FromDbResult { schema, warnings }
}

/// Generator for a single-column primary key: integers count up, UUIDs stay UUIDs, text keys
/// get short random tokens.
fn pk_kind(col: &ColumnSpec, dialect: Option<SqlDialect>) -> Option<FieldKind> {
    if base_type(&col.data_type).is_empty() {
        return Some(FieldKind::sequence());
    }
    let token = || {
        let n = type_params(&col.data_type).first().copied().unwrap_or(10).clamp(1, 10);
        FieldKind::Regex { pattern: format!("[A-Z0-9]{{{n}}}") }
    };
    Some(match kind_for_column(col, dialect) {
        Some(FieldKind::Int { .. } | FieldKind::Timestamp { .. } | FieldKind::Float { .. } | FieldKind::Price { .. }) => FieldKind::sequence(),
        Some(FieldKind::Uuid { .. }) => FieldKind::uuid(),
        Some(k @ (FieldKind::Email { .. } | FieldKind::Username | FieldKind::Date { .. } | FieldKind::Datetime { .. } | FieldKind::Enum { .. } | FieldKind::CountryCode | FieldKind::Currency)) => k,
        _ => token(),
    })
}

fn can_be_unique(k: &FieldKind) -> bool {
    !matches!(k, FieldKind::Bool { .. } | FieldKind::Enum { .. } | FieldKind::Json { .. } | FieldKind::Country | FieldKind::CountryCode | FieldKind::Currency | FieldKind::State)
}

/// Some cycle in a dependency graph (`deps[i]` = nodes `i` points to), as its nodes.
fn find_cycle(deps: &[BTreeSet<usize>]) -> Option<BTreeSet<usize>> {
    fn visit(n: usize, deps: &[BTreeSet<usize>], state: &mut [u8], stack: &mut Vec<usize>) -> Option<BTreeSet<usize>> {
        state[n] = 1;
        stack.push(n);
        for &m in &deps[n] {
            if state[m] == 1 {
                let start = stack.iter().position(|x| *x == m).unwrap_or(0);
                return Some(stack[start..].iter().copied().collect());
            }
            if state[m] == 0
                && let Some(c) = visit(m, deps, state, stack)
            {
                return Some(c);
            }
        }
        stack.pop();
        state[n] = 2;
        None
    }
    let mut state = vec![0u8; deps.len()];
    for n in 0..deps.len() {
        if state[n] == 0
            && let Some(c) = visit(n, deps, &mut state, &mut Vec::new())
        {
            return Some(c);
        }
    }
    None
}

// ---------- MongoDB samples → schema ----------

/// What an Extended JSON value was, for collections sampled from MongoDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ext {
    ObjectId,
    Date,
}

/// Relaxed Extended JSON → plain JSON for inference, remembering which paths held ObjectIds and
/// dates.
fn plain(v: &Value, path: &str, marks: &mut BTreeMap<String, Ext>) -> Value {
    match v {
        Value::Object(o) if o.len() == 1 && o.contains_key("$oid") => {
            marks.insert(path.to_string(), Ext::ObjectId);
            Value::String(o["$oid"].as_str().unwrap_or("").to_string())
        }
        Value::Object(o) if o.len() == 1 && o.contains_key("$date") => {
            marks.insert(path.to_string(), Ext::Date);
            match &o["$date"] {
                Value::String(s) => Value::String(s.clone()),
                other => other.get("$numberLong").and_then(Value::as_str).map(|s| Value::String(s.to_string())).unwrap_or(Value::Null),
            }
        }
        Value::Object(o) if o.len() == 1 && (o.contains_key("$numberDecimal") || o.contains_key("$numberLong") || o.contains_key("$numberDouble") || o.contains_key("$numberInt")) => {
            let s = o.values().next().and_then(Value::as_str).unwrap_or("0");
            s.parse::<i64>().map(Value::from).or_else(|_| s.parse::<f64>().map(Value::from)).unwrap_or(Value::Null)
        }
        Value::Object(o) if o.keys().any(|k| k.starts_with('$')) => Value::Null,
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), plain(x, &format!("{path}.{k}"), marks))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(|x| plain(x, &format!("{path}[]"), marks)).collect()),
        other => other.clone(),
    }
}

fn oid_kind() -> FieldKind {
    FieldKind::Object { fields: vec![Field::new("$oid", FieldKind::Regex { pattern: "[0-9a-f]{24}".into() })] }
}

fn date_kind() -> FieldKind {
    FieldKind::Object { fields: vec![Field::new("$date", FieldKind::Datetime { from: None, to: None, format: Some("%Y-%m-%dT%H:%M:%S%.3fZ".into()) })] }
}

/// Collection whose name matches a reference field (`userId` / `user_id` → `users`, `user`).
fn collection_for(field: &str, collections: &[String]) -> Option<usize> {
    let k = norm_key(field);
    let stem = k.strip_suffix("ids").or_else(|| k.strip_suffix("id")).filter(|s| !s.is_empty())?;
    collections.iter().position(|c| {
        let n = norm_key(c);
        n == stem || singular(&n) == stem || n == format!("{stem}s") || n == format!("{stem}es")
    })
}

/// Rewrites the inferred fields of one collection: ObjectId paths become `{"$oid": …}` objects
/// (or references to another sampled collection), date paths `{"$date": …}`.
fn apply_marks(fields: &mut [Field], prefix: &str, marks: &BTreeMap<String, Ext>, collections: &[String], me: usize, warnings: &mut Vec<String>) {
    for f in fields.iter_mut() {
        let path = format!("{prefix}.{}", f.name);
        match marks.get(&path) {
            Some(Ext::ObjectId) => {
                if f.name == "_id" {
                    f.kind = oid_kind();
                    f.unique = false;
                } else if let Some(target) = collection_for(&f.name, collections).filter(|t| *t != me || prefix.contains('.')) {
                    f.kind = FieldKind::reference(&collections[target], "_id");
                } else {
                    if collection_for(&f.name, collections).is_none() && f.name.to_ascii_lowercase().ends_with("id") {
                        warnings.push(format!("{}: `{}` holds ObjectIds but no sampled collection matches — it gets random ids", collections[me], f.name));
                    }
                    f.kind = oid_kind();
                }
            }
            Some(Ext::Date) => f.kind = date_kind(),
            None => {}
        }
        match &mut f.kind {
            FieldKind::Object { fields: inner } if !inner.iter().any(|x| x.name.starts_with('$')) => apply_marks(inner, &path, marks, collections, me, warnings),
            FieldKind::Array { of, .. } => {
                let item_path = format!("{path}[]");
                match marks.get(&item_path) {
                    Some(Ext::ObjectId) => {
                        **of = match collection_for(&f.name, collections) {
                            Some(t) => FieldKind::reference(&collections[t], "_id"),
                            None => oid_kind(),
                        }
                    }
                    Some(Ext::Date) => **of = date_kind(),
                    None => {
                        if let FieldKind::Object { fields: inner } = of.as_mut() {
                            apply_marks(inner, &item_path, marks, collections, me, warnings);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// A schema from sampled MongoDB documents (relaxed Extended JSON, as `devall-db` returns
/// them), one entity per collection. ObjectIds and dates are generated as Extended JSON
/// (`{"$oid": …}`, `{"$date": …}`) so seeding inserts real BSON types; fields such as `userId`
/// holding ObjectIds become references to a sampled `users` collection.
pub fn schema_from_documents(name: &str, collections: &[(String, Vec<Value>)], opts: &FromDbOptions) -> FromDbResult {
    let mut warnings = Vec::new();
    let names: Vec<String> = collections.iter().map(|(n, _)| n.clone()).collect();
    let mut schema = Schema::new(name);
    schema.seed = Some(opts.seed.unwrap_or(42));
    for (i, (coll, docs)) in collections.iter().enumerate() {
        let mut marks = BTreeMap::new();
        let plain_docs: Vec<Value> = docs.iter().map(|d| plain(d, "", &mut marks)).collect();
        if plain_docs.is_empty() {
            warnings.push(format!("{coll}: no documents to sample — add fields by hand"));
        }
        let objects: Vec<&Value> = plain_docs.iter().filter(|d| d.is_object()).collect();
        let mut fields = merge_objects(&objects);
        apply_marks(&mut fields, "", &marks, &names, i, &mut warnings);
        let has_ref = fields.iter().any(|f| matches!(f.kind, FieldKind::Ref { .. }));
        let count = if has_ref { opts.count.saturating_mul(2) } else { opts.count }.max(1);
        let mut e = Entity::new(coll, count, fields);
        e.extra.insert("table".into(), Value::String(coll.clone()));
        schema.entities.push(e);
    }
    FromDbResult { schema, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str) -> ColumnSpec {
        ColumnSpec { name: name.into(), data_type: ty.into(), nullable: true, ..Default::default() }
    }

    #[test]
    fn check_constraints_become_enums() {
        let sqlite = "CREATE TABLE orders (id INTEGER PRIMARY KEY, status TEXT NOT NULL CHECK (status IN ('pending', 'paid', 'it''s shipped')), qty INT CHECK (qty > 0))";
        let m = check_enum_values(sqlite);
        assert_eq!(m.get("status").unwrap(), &vec!["pending".to_string(), "paid".into(), "it's shipped".into()]);
        assert!(!m.contains_key("qty"));
        let pg = "CONSTRAINT \"orders_status_check\" CHECK (status::text = ANY (ARRAY['new'::character varying, 'done'::character varying]::text[]))";
        assert_eq!(check_enum_values(pg).get("status").unwrap(), &vec!["new".to_string(), "done".into()]);
        let or = "CHECK ((\"Kind\" = 'a') OR (\"Kind\" = 'b'))";
        assert_eq!(check_enum_values(or).get("kind").unwrap(), &vec!["a".to_string(), "b".into()]);
        assert!(check_enum_values("CHECK (a = 'x' OR b = 'y')").is_empty(), "two columns: not an enum");
        assert_eq!(mysql_enum_values("enum('S','M','L')"), vec!["S", "M", "L"]);
        assert!(mysql_enum_values("varchar(10)").is_empty());
    }

    #[test]
    fn column_types_and_names_pick_generators() {
        let k = |name: &str, ty: &str| kind_for_column(&col(name, ty), Some(SqlDialect::Postgres)).map(|k| k.type_name());
        assert_eq!(k("email", "character varying(255)"), Some("email"));
        assert_eq!(k("first_name", "varchar(50)"), Some("firstName"));
        assert_eq!(k("created_at", "timestamp with time zone"), Some("datetime"));
        assert_eq!(k("birthday", "date"), Some("date"));
        assert_eq!(k("id", "uuid"), Some("uuid"));
        assert_eq!(k("price", "numeric(10,2)"), Some("price"));
        assert_eq!(k("is_active", "boolean"), Some("bool"));
        assert_eq!(k("meta", "jsonb"), Some("json"));
        assert_eq!(k("avatar", "bytea"), None);
        assert_eq!(k("age", "integer"), Some("int"));
        assert_eq!(k("code", "char(2)"), Some("regex"));
        assert_eq!(k("country", "char(2)"), Some("countryCode"));
        assert_eq!(k("bio", "text"), Some("paragraph"));
        assert_eq!(k("flag", "tinyint(1)"), Some("bool"));
        assert_eq!(k("lat", "double precision"), Some("latitude"));
        // Short columns never get long generators.
        match kind_for_column(&col("title", "varchar(10)"), None).unwrap() {
            FieldKind::Regex { pattern } => assert_eq!(pattern, "[a-z]{1,10}"),
            other => panic!("{other:?}"),
        }
        let mut status = col("status", "order_status");
        status.enum_values = vec!["a".into(), "b".into()];
        assert_eq!(kind_for_column(&status, None).unwrap().type_name(), "enum");
        match kind_for_column(&col("updated_at", "datetime"), Some(SqlDialect::Mysql)).unwrap() {
            FieldKind::Datetime { format, .. } => assert_eq!(format.as_deref(), Some("%Y-%m-%d %H:%M:%S")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn foreign_keys_become_refs_and_cycles_are_broken() {
        let pk = |name: &str, ty: &str| ColumnSpec { name: name.into(), data_type: ty.into(), primary_key: true, ..Default::default() };
        let users = TableSpec {
            schema: Some("public".into()),
            name: "users".into(),
            columns: vec![pk("id", "integer"), ColumnSpec { unique: true, ..col("email", "text") }, col("best_friend_id", "integer"), col("team_id", "integer")],
            foreign_keys: vec![
                ForeignKeySpec { columns: vec!["best_friend_id".into()], ref_table: "users".into(), ref_columns: vec!["id".into()], ..Default::default() },
                ForeignKeySpec { columns: vec!["team_id".into()], ref_table: "teams".into(), ref_columns: vec![], ..Default::default() },
            ],
        };
        let teams = TableSpec {
            schema: Some("public".into()),
            name: "teams".into(),
            columns: vec![pk("id", "uuid"), col("owner_id", "integer")],
            foreign_keys: vec![ForeignKeySpec { columns: vec!["owner_id".into()], ref_table: "users".into(), ref_columns: vec!["id".into()], ..Default::default() }],
        };
        let r = schema_from_tables("App", &[users, teams], &FromDbOptions::default());
        let u = &r.schema.entities[0];
        assert_eq!(u.fields[0].kind, FieldKind::sequence());
        assert!(u.fields[0].unique);
        assert!(u.fields[1].unique && u.fields[1].kind.type_name() == "email");
        assert_eq!(u.fields[2].kind, FieldKind::reference("users", "id"), "self-references are fine");
        assert_eq!(u.extra.get("table").and_then(Value::as_str), Some("public.users"));
        // users.team_id ↔ teams.owner_id form a loop: one side is cut (both nullable).
        assert!(r.warnings.iter().any(|w| w.contains("loop")), "{:?}", r.warnings);
        crate::validate(&r.schema).expect("the generated schema compiles");
        let out = crate::generate_values(&r.schema, &crate::GenerateOptions::default()).unwrap();
        assert_eq!(out["users"].len(), 50, "tables with foreign keys get twice the base count");
    }

    #[test]
    fn mongo_samples_keep_object_ids_and_references() {
        let users = vec![
            json!({"_id": {"$oid": "65f0c1aa0000000000000001"}, "name": "Ada", "email": "ada@example.com", "createdAt": {"$date": "2024-05-01T10:00:00Z"}}),
            json!({"_id": {"$oid": "65f0c1aa0000000000000002"}, "name": "Bob", "email": "bob@example.com", "createdAt": {"$date": "2024-05-02T10:00:00Z"}}),
        ];
        let orders = vec![json!({"_id": {"$oid": "65f0c1aa00000000000000f1"}, "userId": {"$oid": "65f0c1aa0000000000000001"}, "total": {"$numberDecimal": "12.50"}, "items": [{"sku": "A1", "qty": 2}]})];
        let r = schema_from_documents("Shop", &[("users".into(), users), ("orders".into(), orders)], &FromDbOptions::default());
        let u = &r.schema.entities[0];
        assert_eq!(u.fields[0].name, "_id");
        assert!(matches!(&u.fields[0].kind, FieldKind::Object { fields } if fields[0].name == "$oid"));
        assert!(matches!(&u.fields[3].kind, FieldKind::Object { fields } if fields[0].name == "$date"));
        let o = &r.schema.entities[1];
        assert_eq!(o.fields[1].kind, FieldKind::reference("users", "_id"));
        let values = crate::generate_values(&r.schema, &crate::GenerateOptions::default()).unwrap();
        let first = &values["orders"][0];
        assert!(first["userId"]["$oid"].as_str().unwrap().len() == 24, "{first}");
        let ids: Vec<&Value> = values["users"].iter().map(|u| &u["_id"]).collect();
        assert!(ids.contains(&&first["userId"]), "the reference points at a generated user");
    }
}
