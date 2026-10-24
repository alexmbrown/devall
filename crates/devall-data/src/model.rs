//! Serde model of `<project>/data/<slug>.data.yaml` (format version 1) and of the output formats.
//!
//! Field order here is the key order on disk. A field is written flat:
//!
//! ```yaml
//! - name: age
//!   type: int
//!   min: 18
//!   max: 90
//!   nullPercent: 5
//! ```

use devall_core::Extra;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FORMAT_VERSION: u32 = 1;

fn format_version() -> u32 {
    FORMAT_VERSION
}
fn default_count() -> u64 {
    10
}
fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(f: &f64) -> bool {
    *f == 0.0
}
fn yes() -> bool {
    true
}

/// A generator schema: one or more entities (tables) generated in dependency order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    #[serde(default = "format_version")]
    pub version: u32,
    #[serde(default)]
    pub name: String,
    /// Same seed + same schema = same data. Omitted = a fresh random seed per run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// `en` (default), `fr_fr`, `de_de`, `it_it`, `pt_br`, `pt_pt`, `nl_nl`, `ja_jp`, `zh_cn`,
    /// `zh_tw`, `ar_sa`, `tr_tr`, `fa_ir`, `cy_gb` (short forms like `de` or `pt-BR` also work).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default)]
    pub entities: Vec<Entity>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Schema {
    pub fn new(name: impl Into<String>) -> Self {
        Self { version: FORMAT_VERSION, name: name.into(), seed: None, locale: None, entities: Vec::new(), extra: Extra::new() }
    }

    pub fn entity(&self, name: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.name == name)
    }
}

/// One table / collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    pub name: String,
    #[serde(default = "default_count")]
    pub count: u64,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity {
    pub fn new(name: impl Into<String>, count: u64, fields: Vec<Field>) -> Self {
        Self { name: name.into(), count, fields, extra: Extra::new() }
    }
}

/// A column / property: a name, a generator (`type` + its parameters) and optional null rate and
/// uniqueness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    #[serde(flatten)]
    pub kind: FieldKind,
    /// 0–100: share of rows where the value is `null`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub null_percent: f64,
    /// Every non-null value of this field is distinct across the entity.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unique: bool,
}

impl Field {
    pub fn new(name: impl Into<String>, kind: FieldKind) -> Self {
        Self { name: name.into(), kind, null_percent: 0.0, unique: false }
    }
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }
    pub fn nullable(mut self, percent: f64) -> Self {
        self.null_percent = percent;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TimestampUnit {
    S,
    Ms,
}

/// The generator of a field. Optional parameters fall back to the defaults documented on each
/// variant; they are omitted from the file when unset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FieldKind {
    // ---- ids ----
    /// Random UUID; `version` 4 (default) or 7 (time-ordered).
    Uuid {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u8>,
    },
    /// Time-ordered ULID.
    Ulid,
    /// `start` (1), `start + step` (step 1), …
    Sequence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<i64>,
    },

    // ---- person ----
    FirstName,
    LastName,
    /// Built from the row's `firstName`/`lastName` fields when present.
    FullName,
    /// Built from the row's name fields when present.
    Username,
    /// Built from the row's name fields when present; `domain` defaults to example.com/.net/.org.
    Email {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        domain: Option<String>,
    },
    Phone,

    // ---- internet ----
    Url,
    Domain,
    Ipv4,
    Ipv6,
    UserAgent,
    /// `minLength` (10) – `maxLength` (16) characters.
    Password {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_length: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<u32>,
    },

    // ---- address ----
    Street,
    City,
    State,
    Zip,
    Country,
    CountryCode,
    Latitude,
    Longitude,

    // ---- company ----
    Company,
    JobTitle,
    CatchPhrase,

    // ---- commerce ----
    ProductName,
    /// `min` (1) – `max` (1000), `decimals` (2).
    Price {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decimals: Option<u32>,
    },
    /// ISO 4217 code.
    Currency,

    // ---- text ----
    Word,
    /// `minWords` (4) – `maxWords` (10) words.
    Sentence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_words: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_words: Option<u32>,
    },
    /// `sentences` (3).
    Paragraph {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sentences: Option<u32>,
    },
    /// `words` (5) lorem-ipsum words.
    Lorem {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        words: Option<u32>,
    },

    // ---- numbers ----
    /// `min` (0) – `max` (1000), inclusive.
    Int {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    /// `min` (0) – `max` (1), rounded to `decimals` (2).
    Float {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decimals: Option<u32>,
    },
    /// `truePercent` (50).
    Bool {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        true_percent: Option<f64>,
    },

    // ---- dates (UTC; bounds are `YYYY-MM-DD` or RFC 3339; default 2020-01-01 … 2025-12-31) ----
    /// `format` is strftime (default `%Y-%m-%d`).
    Date {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// `format` is strftime (default RFC 3339, `%Y-%m-%dT%H:%M:%SZ`).
    Datetime {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// Unix time as a number; `unit` `s` (default) or `ms`.
    Timestamp {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<TimestampUnit>,
    },

    // ---- logic & structure ----
    /// One of `values` (any JSON), optionally weighted.
    Enum {
        #[serde(default)]
        values: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weights: Option<Vec<f64>>,
    },
    /// A string matching `pattern` (`^`/`$` anchors are ignored; classes are ASCII).
    Regex { pattern: String },
    /// Text with `{{field}}` references to other fields of the same row (or an enclosing object),
    /// e.g. `{{firstName|lower}}.{{lastName|lower}}@acme.test`. Filters: `lower`, `upper`,
    /// `slug`, `ascii`, `trim`.
    Template { value: String },
    /// Foreign key: a value of `field` from a row of `entity` (generated first). A reference to
    /// the same entity picks from earlier rows (`null` for the first). `unique` makes it 1:1.
    Ref { entity: String, field: String },
    /// Nested JSON object.
    Object {
        #[serde(default)]
        fields: Vec<Field>,
    },
    /// JSON array of `min` (1) – `max` (3) items generated by `of`.
    Array {
        of: Box<FieldKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
    },
    /// A constant.
    Json {
        #[serde(default)]
        value: Value,
    },
}

impl FieldKind {
    /// The `type` tag as written in the file.
    pub fn type_name(&self) -> &'static str {
        match self {
            FieldKind::Uuid { .. } => "uuid",
            FieldKind::Ulid => "ulid",
            FieldKind::Sequence { .. } => "sequence",
            FieldKind::FirstName => "firstName",
            FieldKind::LastName => "lastName",
            FieldKind::FullName => "fullName",
            FieldKind::Username => "username",
            FieldKind::Email { .. } => "email",
            FieldKind::Phone => "phone",
            FieldKind::Url => "url",
            FieldKind::Domain => "domain",
            FieldKind::Ipv4 => "ipv4",
            FieldKind::Ipv6 => "ipv6",
            FieldKind::UserAgent => "userAgent",
            FieldKind::Password { .. } => "password",
            FieldKind::Street => "street",
            FieldKind::City => "city",
            FieldKind::State => "state",
            FieldKind::Zip => "zip",
            FieldKind::Country => "country",
            FieldKind::CountryCode => "countryCode",
            FieldKind::Latitude => "latitude",
            FieldKind::Longitude => "longitude",
            FieldKind::Company => "company",
            FieldKind::JobTitle => "jobTitle",
            FieldKind::CatchPhrase => "catchPhrase",
            FieldKind::ProductName => "productName",
            FieldKind::Price { .. } => "price",
            FieldKind::Currency => "currency",
            FieldKind::Word => "word",
            FieldKind::Sentence { .. } => "sentence",
            FieldKind::Paragraph { .. } => "paragraph",
            FieldKind::Lorem { .. } => "lorem",
            FieldKind::Int { .. } => "int",
            FieldKind::Float { .. } => "float",
            FieldKind::Bool { .. } => "bool",
            FieldKind::Date { .. } => "date",
            FieldKind::Datetime { .. } => "datetime",
            FieldKind::Timestamp { .. } => "timestamp",
            FieldKind::Enum { .. } => "enum",
            FieldKind::Regex { .. } => "regex",
            FieldKind::Template { .. } => "template",
            FieldKind::Ref { .. } => "ref",
            FieldKind::Object { .. } => "object",
            FieldKind::Array { .. } => "array",
            FieldKind::Json { .. } => "json",
        }
    }

    pub fn int(min: i64, max: i64) -> Self {
        FieldKind::Int { min: Some(min), max: Some(max) }
    }
    pub fn sequence() -> Self {
        FieldKind::Sequence { start: None, step: None }
    }
    pub fn uuid() -> Self {
        FieldKind::Uuid { version: None }
    }
    pub fn email() -> Self {
        FieldKind::Email { domain: None }
    }
    pub fn reference(entity: &str, field: &str) -> Self {
        FieldKind::Ref { entity: entity.into(), field: field.into() }
    }
    pub fn template(value: &str) -> Self {
        FieldKind::Template { value: value.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SqlDialect {
    Postgres,
    Mysql,
    Sqlite,
    /// Microsoft SQL Server (T-SQL): `[identifiers]`, `N'unicode'` strings, `1`/`0` booleans.
    Sqlserver,
}

/// How generated rows are serialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OutputFormat {
    /// One entity → a JSON array; several → an object keyed by entity name.
    Json {
        #[serde(default = "yes")]
        pretty: bool,
    },
    /// One JSON object per line.
    Ndjson,
    /// RFC 4180; nested objects flatten to `a.b` columns, arrays become JSON text.
    Csv {
        /// One character (default `,`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delimiter: Option<String>,
        #[serde(default = "yes")]
        header: bool,
    },
    /// `INSERT` statements (parents first); nested values are JSON text.
    Sql {
        dialect: SqlDialect,
        #[serde(default)]
        create_table: bool,
        /// Rows per `INSERT` statement (default 100; 1 = one statement per row).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch_size: Option<usize>,
    },
    Yaml,
}

impl OutputFormat {
    pub fn json() -> Self {
        OutputFormat::Json { pretty: true }
    }
    pub fn csv() -> Self {
        OutputFormat::Csv { delimiter: None, header: true }
    }
    pub fn sql(dialect: SqlDialect) -> Self {
        OutputFormat::Sql { dialect, create_table: false, batch_size: None }
    }

    /// File extension (without the dot).
    pub fn extension(&self) -> &'static str {
        match self {
            OutputFormat::Json { .. } => "json",
            OutputFormat::Ndjson => "ndjson",
            OutputFormat::Csv { .. } => "csv",
            OutputFormat::Sql { .. } => "sql",
            OutputFormat::Yaml => "yaml",
        }
    }

    /// Formats that hold exactly one table per file (exports split per entity).
    pub fn single_table(&self) -> bool {
        matches!(self, OutputFormat::Csv { .. } | OutputFormat::Ndjson)
    }
}
