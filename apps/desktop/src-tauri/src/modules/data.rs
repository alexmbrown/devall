//! Fake-data generator commands — a thin layer over `crates/devall-data`.
//!
//! Schema files live in `<project>/data/**/*.data.yaml`; every `path` argument is relative to the
//! project's `data/` folder (e.g. `users.data.yaml`).
//!
//! # Integration (for whoever wires the module in)
//!
//! 1. `apps/desktop/src-tauri/Cargo.toml`, `[dependencies]`:
//!    `devall-data = { path = "../../../crates/devall-data" }`
//!    (or add `devall-data = { path = "crates/devall-data" }` to the root `[workspace.dependencies]`
//!    and use `devall-data.workspace = true`). `tauri`, `serde` and `serde_json` are already deps.
//! 2. `src/modules/mod.rs`: `pub mod data;` — and `src/lib.rs`: `mod modules;` if not declared yet.
//! 3. No managed state: nothing to `.manage(...)`.
//! 4. `src/lib.rs`, inside `tauri::generate_handler![…]`:
//!    ```ignore
//!    modules::data::data_list,
//!    modules::data::data_read,
//!    modules::data::data_save,
//!    modules::data::data_create,
//!    modules::data::data_delete,
//!    modules::data::data_generate,
//!    modules::data::data_export,
//!    modules::data::data_infer,
//!    modules::data::data_from_json_schema,
//!    modules::data::data_to_template,
//!    modules::data::data_template_sample,
//!    ```
//! 5. Frontend: route the `data` section to `features/data/DataPage` in `App.tsx` (typed wrappers
//!    are in `src/lib/api/data.ts`).

use std::path::{Path, PathBuf};

use devall_data::store::{self, DataFileEntry};
use devall_data::{ExportResult, GenerateOptions, OutputFormat, Schema, TemplateOptions};

type CmdResult<T> = Result<T, String>;

/// Rows per entity in a preview (the UI usually asks for 20).
const PREVIEW_MAX_ROWS: u64 = 1000;
/// Previews are cut here; exports are never cut.
const PREVIEW_MAX_BYTES: usize = 2 * 1024 * 1024;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn project_root(root: &str) -> CmdResult<PathBuf> {
    let p = PathBuf::from(root);
    if !p.is_dir() {
        return Err(format!("project folder not found: {root}"));
    }
    Ok(p)
}

/// Schemas under `data/`, sorted by path. Unparseable files are listed with `error` set.
#[tauri::command]
pub fn data_list(root: String) -> CmdResult<Vec<DataFileEntry>> {
    store::list(&project_root(&root)?).map_err(err)
}

#[tauri::command]
pub fn data_read(root: String, path: String) -> CmdResult<Schema> {
    store::read(&project_root(&root)?, &path).map_err(err)
}

#[tauri::command]
pub fn data_save(root: String, path: String, schema: Schema) -> CmdResult<()> {
    store::save(&project_root(&root)?, &path, &schema).map_err(err)
}

/// Creates `data/<slug>.data.yaml` from `schema` (default: a starter schema); returns its path.
#[tauri::command]
pub fn data_create(root: String, name: String, schema: Option<Schema>) -> CmdResult<String> {
    let root = project_root(&root)?;
    match schema {
        Some(s) => store::create_from(&root, &name, &s),
        None => store::create(&root, &name),
    }
    .map_err(err)
}

/// Moves the schema file to the OS trash.
#[tauri::command]
pub fn data_delete(root: String, path: String) -> CmdResult<()> {
    store::delete(&project_root(&root)?, &path).map_err(err)
}

/// Preview: generates (at most `limit`, capped at 1000, rows per entity) and returns the text in
/// `format`. Output over 2 MB is cut and ends with a note. `root` is accepted for future
/// project-relative datasets and is currently unused.
#[tauri::command]
pub async fn data_generate(
    root: Option<String>,
    schema: Schema,
    entity: Option<String>,
    count_override: Option<u64>,
    limit: Option<u64>,
    format: OutputFormat,
) -> CmdResult<String> {
    let _ = root;
    let opts = GenerateOptions {
        entity: entity.filter(|e| !e.is_empty()),
        count_override,
        limit: Some(limit.unwrap_or(PREVIEW_MAX_ROWS).min(PREVIEW_MAX_ROWS)),
        seed: None,
    };
    let rendered = tauri::async_runtime::spawn_blocking(move || devall_data::generate_string(&schema, &opts, &format, Some(PREVIEW_MAX_BYTES)))
        .await
        .map_err(err)?
        .map_err(err)?;
    let mut text = rendered.text;
    if rendered.truncated {
        text.push_str("\n\n… preview truncated at 2 MB — export to get everything");
    }
    Ok(text)
}

/// Generates the saved schema at `path` in full and writes it to `dest_path` (atomically). CSV
/// and NDJSON exports of several entities write one `<stem>.<entity>.<ext>` file each.
#[tauri::command]
pub async fn data_export(
    root: String,
    path: String,
    format: OutputFormat,
    dest_path: String,
    entity: Option<String>,
    count_override: Option<u64>,
) -> CmdResult<ExportResult> {
    let schema = store::read(&project_root(&root)?, &path).map_err(err)?;
    let opts = GenerateOptions { entity: entity.filter(|e| !e.is_empty()), count_override, limit: None, seed: None };
    tauri::async_runtime::spawn_blocking(move || devall_data::export_to_path(&schema, &opts, &format, Path::new(&dest_path)))
        .await
        .map_err(err)?
        .map_err(err)
}

/// A one-entity schema guessed from sample JSON (an array of objects, an object, or a value).
#[tauri::command]
pub fn data_infer(json_text: String, entity: Option<String>) -> CmdResult<Schema> {
    let sample: serde_json::Value = serde_json::from_str(&json_text).map_err(|e| format!("not valid JSON: {e}"))?;
    let name = entity.map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).unwrap_or_else(|| "items".into());
    let mut schema = Schema::new("Inferred");
    schema.entities.push(devall_data::infer_entity(&name, &sample));
    Ok(schema)
}

/// A schema from JSON Schema or an OpenAPI document (JSON or YAML text).
#[tauri::command]
pub fn data_from_json_schema(text: String) -> CmdResult<Schema> {
    devall_data::from_json_schema_text(&text).map_err(err)
}

/// WireMock `response-template` body for a real JSON response body.
#[tauri::command]
pub fn data_to_template(json_text: String, keep_ids: Option<bool>) -> CmdResult<String> {
    devall_data::to_response_template_str(&json_text, &TemplateOptions { keep_ids: keep_ids.unwrap_or(false) }).map_err(err)
}

/// Renders a response template locally (same helpers as the mock engine) to show a sample.
#[tauri::command]
pub fn data_template_sample(template: String, seed: Option<u64>) -> String {
    devall_data::render_template_sample(&template, seed.unwrap_or(1))
}
