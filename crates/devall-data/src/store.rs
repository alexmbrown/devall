//! `<project>/data/**/*.data.yaml` files. Paths in and out are `/`-separated and relative to the
//! project's `data/` folder (e.g. `users.data.yaml`, `shop/orders.data.yaml`).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use devall_core::fsutil::{normalize_rel, read_yaml, safe_join, slugify, unique_name, write_yaml};

use crate::error::{DataError, Result};
use crate::model::{Entity, FORMAT_VERSION, Field, FieldKind, Schema};

pub const DATA_DIR: &str = "data";
pub const DATA_EXT: &str = ".data.yaml";

/// A schema file in the project, as listed in the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataFileEntry {
    /// Relative to `data/`, e.g. `users.data.yaml`.
    pub path: String,
    pub name: String,
    pub entities: Vec<String>,
    /// Set when the file can't be parsed (it is still listed so it can be fixed or deleted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn data_dir(root: &Path) -> PathBuf {
    root.join(DATA_DIR)
}

fn file_path(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = normalize_rel(rel);
    if !rel.ends_with(DATA_EXT) {
        return Err(DataError::Invalid(format!("'{rel}' is not a {DATA_EXT} file")));
    }
    Ok(safe_join(&data_dir(root), &rel)?)
}

/// Every schema under `data/` (recursively), sorted by path.
pub fn list(root: &Path) -> Result<Vec<DataFileEntry>> {
    let dir = data_dir(root);
    let mut out = Vec::new();
    if dir.is_dir() {
        walk(&dir, "", &mut out)?;
    }
    out.sort_by_key(|e| e.path.to_lowercase());
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<DataFileEntry>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let ty = entry.file_type()?;
        if ty.is_dir() {
            walk(&entry.path(), &rel, out)?;
        } else if name.ends_with(DATA_EXT) {
            let fallback = name.trim_end_matches(DATA_EXT).to_string();
            match read_yaml::<Schema>(&entry.path()) {
                Ok(s) => out.push(DataFileEntry {
                    path: rel,
                    name: if s.name.trim().is_empty() { fallback } else { s.name },
                    entities: s.entities.into_iter().map(|e| e.name).collect(),
                    error: None,
                }),
                Err(e) => out.push(DataFileEntry { path: rel, name: fallback, entities: Vec::new(), error: Some(e.to_string()) }),
            }
        }
    }
    Ok(())
}

pub fn read(root: &Path, rel: &str) -> Result<Schema> {
    let path = file_path(root, rel)?;
    if !path.is_file() {
        return Err(devall_core::Error::NotFound(format!("{DATA_DIR}/{}", normalize_rel(rel))).into());
    }
    Ok(read_yaml(&path)?)
}

/// Writes the schema (always as format version 1). Saving does not validate, so half-finished
/// schemas can be kept; generation reports problems.
pub fn save(root: &Path, rel: &str, schema: &Schema) -> Result<()> {
    let path = file_path(root, rel)?;
    let mut s = schema.clone();
    s.version = FORMAT_VERSION;
    Ok(write_yaml(&path, &s)?)
}

/// Creates `data/<slug>.data.yaml` from [`starter_schema`] and returns its relative path.
pub fn create(root: &Path, name: &str) -> Result<String> {
    create_from(root, name, &starter_schema(name.trim()))
}

/// Creates `data/<slug>.data.yaml` holding `schema` (its name set to `name`) and returns its
/// relative path; an existing file is never overwritten (`users-2.data.yaml`).
pub fn create_from(root: &Path, name: &str, schema: &Schema) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DataError::Invalid("a name is required".into()));
    }
    let dir = data_dir(root);
    fs::create_dir_all(&dir)?;
    let file = unique_name(&dir, &slugify(name), DATA_EXT);
    let mut s = schema.clone();
    s.version = FORMAT_VERSION;
    s.name = name.to_string();
    write_yaml(&dir.join(&file), &s)?;
    Ok(file)
}

/// Moves a schema file to the OS trash.
pub fn delete(root: &Path, rel: &str) -> Result<()> {
    let path = file_path(root, rel)?;
    if !path.is_file() {
        return Err(devall_core::Error::NotFound(format!("{DATA_DIR}/{}", normalize_rel(rel))).into());
    }
    trash::delete(&path).map_err(|e| DataError::Io(std::io::Error::other(format!("{}: {e}", path.display()))))
}

/// A small users → orders example showing derived e-mails, a reference and nulls.
pub fn starter_schema(name: &str) -> Schema {
    let mut s = Schema::new(name);
    s.seed = Some(42);
    s.entities.push(Entity::new(
        "users",
        25,
        vec![
            Field::new("id", FieldKind::sequence()),
            Field::new("firstName", FieldKind::FirstName),
            Field::new("lastName", FieldKind::LastName),
            Field::new("email", FieldKind::email()).unique(),
            Field::new("city", FieldKind::City),
            Field::new("createdAt", FieldKind::Datetime { from: None, to: None, format: None }),
        ],
    ));
    s.entities.push(Entity::new(
        "orders",
        100,
        vec![
            Field::new("id", FieldKind::uuid()),
            Field::new("userId", FieldKind::reference("users", "id")),
            Field::new("total", FieldKind::Price { min: Some(5.0), max: Some(500.0), decimals: None }),
            Field::new(
                "status",
                FieldKind::Enum {
                    values: vec!["pending".into(), "paid".into(), "shipped".into(), "cancelled".into()],
                    weights: Some(vec![2.0, 5.0, 4.0, 1.0]),
                },
            ),
            Field::new("note", FieldKind::Sentence { min_words: None, max_words: None }).nullable(70.0),
        ],
    ));
    s
}
