//! Seeded, relational fake-data generator for /dev/all — the free, local replacement for
//! Mockaroo, and the data engine behind "request → mock → randomize", database seeding and
//! load-test bodies.
//!
//! * [`model`] — the `<project>/data/<slug>.data.yaml` format (version 1).
//! * [`engine`] — compile (validate, order) and generate rows deterministically from a seed:
//!   entities parents-first (`ref` foreign keys), fields in dependency order (templates,
//!   e-mails derived from the row's names), unique values with retries or sampling without
//!   replacement.
//! * [`output`] — streaming JSON / NDJSON / CSV / SQL (postgres, mysql, sqlite) / YAML writers
//!   and file export.
//! * [`infer`] / [`jsonschema`] — schemas from sample JSON, JSON Schema or OpenAPI.
//! * [`template`] — the WireMock response-template bridge.
//! * [`fromdb`] — schemas from an introspected database (tables, keys, enums) or sampled
//!   MongoDB documents.
//! * [`store`] — project file operations.
//!
//! ```
//! use devall_data::{Entity, Field, FieldKind, GenerateOptions, OutputFormat, Schema, generate_string};
//!
//! let mut schema = Schema::new("demo");
//! schema.seed = Some(7);
//! schema.entities.push(Entity::new("users", 3, vec![
//!     Field::new("id", FieldKind::sequence()),
//!     Field::new("firstName", FieldKind::FirstName),
//!     Field::new("email", FieldKind::email()).unique(),
//! ]));
//! let out = generate_string(&schema, &GenerateOptions::default(), &OutputFormat::json(), None).unwrap();
//! assert!(out.text.starts_with('['));
//! ```

pub mod engine;
pub mod error;
mod fakers;
pub mod fromdb;
pub mod infer;
pub mod jsonschema;
pub mod model;
pub mod output;
mod regex_gen;
pub mod store;
pub mod template;

#[cfg(test)]
mod tests;

pub use engine::{CollectSink, GenerateOptions, GenerateStats, RowSink, generate, generate_value, generate_values, validate};
pub use error::{DataError, Result};
pub use fakers::{LOCALES, Locale};
pub use fromdb::{ColumnSpec, ForeignKeySpec, FromDbOptions, FromDbResult, TableSpec, schema_from_documents, schema_from_tables};
pub use infer::{infer_entity, infer_from_json};
pub use jsonschema::{from_json_schema, from_json_schema_text};
pub use model::*;
pub use output::{ExportResult, Rendered, export_to_path, generate_string, write_output};
pub use template::{FAKER_KEYS, TemplateOptions, render_template_sample, substitute_helpers, to_response_template, to_response_template_str};

/// JSON Schema of the `.data.yaml` format (for editor validation; can be committed to
/// `schemas/data.schema.json`).
pub fn json_schema() -> serde_json::Value {
    let mut v = schemars::schema_for!(Schema).to_value();
    if let Some(o) = v.as_object_mut() {
        o.insert("$id".into(), serde_json::Value::String("https://devall.dev/schemas/v1/data.schema.json".into()));
        o.insert("description".into(), serde_json::Value::String("data/**/*.data.yaml — a fake-data generator schema".into()));
    }
    v
}
