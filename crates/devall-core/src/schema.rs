//! JSON Schemas for every project file, generated from the serde model so they can't drift.
//! The committed copies live in `schemas/` at the repo root; regenerate with
//! `UPDATE_SCHEMAS=1 cargo test -p devall-core schemas_are_up_to_date`.

use schemars::schema_for;
use serde_json::Value;

use crate::model::{EnvironmentFile, FolderFile, HttpRequestFile, ProjectManifest};
use crate::regexes::RegexFile;

pub const SCHEMA_BASE_URL: &str = "https://devall.dev/schemas/v1";

/// `(file name, schema)` for each project file type.
pub fn all() -> Vec<(&'static str, Value)> {
    let with_id = |name: &str, mut v: Value, description: &str| {
        if let Some(o) = v.as_object_mut() {
            o.insert("$id".into(), Value::String(format!("{SCHEMA_BASE_URL}/{name}")));
            o.insert("description".into(), Value::String(description.into()));
        }
        v
    };
    vec![
        ("project.schema.json", with_id("project.schema.json", schema_for!(ProjectManifest).to_value(), "devall.yaml — the project manifest")),
        ("request.schema.json", with_id("request.schema.json", schema_for!(HttpRequestFile).to_value(), "http/**/*.req.yaml — one HTTP request")),
        ("folder.schema.json", with_id("folder.schema.json", schema_for!(FolderFile).to_value(), "http/**/_folder.yaml — folder order and inherited settings")),
        ("environment.schema.json", with_id("environment.schema.json", schema_for!(EnvironmentFile).to_value(), "environments/*.env.yaml — variables and secret names")),
        ("regex.schema.json", with_id("regex.schema.json", schema_for!(RegexFile).to_value(), "utilities/regex/*.regex.yaml — a saved regex with test cases")),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn schemas_are_up_to_date() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        let update = std::env::var_os("UPDATE_SCHEMAS").is_some();
        for (name, schema) in super::all() {
            let text = format!("{}\n", serde_json::to_string_pretty(&schema).unwrap());
            let path = dir.join(name);
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &text).unwrap();
            } else {
                let committed = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
                assert_eq!(committed, text, "{name} is stale — run with UPDATE_SCHEMAS=1");
            }
        }
    }

    #[test]
    fn request_schema_knows_body_types() {
        let (_, s) = super::all().into_iter().find(|(n, _)| *n == "request.schema.json").unwrap();
        let text = s.to_string();
        for t in ["json", "form", "multipart", "graphql", "binary", "bearer", "apiKey"] {
            assert!(text.contains(&format!("\"{t}\"")), "missing {t}");
        }
    }
}
