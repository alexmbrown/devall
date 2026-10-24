//! JSON Schemas for `*.load.yaml` and run summaries, generated from the serde model. Committed
//! copies live in `crates/devall-load/schemas/` (copy them to the repo's `schemas/` when
//! integrating); regenerate with `UPDATE_SCHEMAS=1 cargo test -p devall-load schemas_are_up_to_date`.

use schemars::schema_for;
use serde_json::Value;

use crate::model::LoadTestFile;
use crate::summary::LoadSummary;

pub const SCHEMA_BASE_URL: &str = devall_core::schema::SCHEMA_BASE_URL;

fn with_id(name: &str, mut v: Value, description: &str) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.insert("$id".into(), Value::String(format!("{SCHEMA_BASE_URL}/{name}")));
        o.insert("description".into(), Value::String(description.into()));
    }
    v
}

/// `(file name, schema)` pairs.
pub fn all() -> Vec<(&'static str, Value)> {
    vec![
        ("load.schema.json", with_id("load.schema.json", schema_for!(LoadTestFile).to_value(), "load/**/*.load.yaml — a load test")),
        ("load-run.schema.json", with_id("load-run.schema.json", schema_for!(LoadSummary).to_value(), "load/runs/<test>/<timestamp>.json — a load test run summary")),
    ]
}

pub fn load_test_schema() -> Value {
    all().remove(0).1
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn schemas_are_up_to_date() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas");
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
    fn schema_knows_executors_and_scenario() {
        let text = super::load_test_schema().to_string();
        for t in ["constantVus", "rampingVus", "constantArrivalRate", "rampingArrivalRate", "preAllocatedVus", "thinkTime", "flow", "bodyContains"] {
            assert!(text.contains(&format!("\"{t}\"")), "missing {t}");
        }
    }
}
