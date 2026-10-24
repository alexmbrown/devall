//! The notebooks shipped in the repo stay valid and runnable.

use std::path::PathBuf;

use devall_notebook::Language;
use serde_json::Value;

fn repo_file(rel: &str) -> Option<String> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
    std::fs::read_to_string(&p).ok()
}

#[test]
fn shipped_notebooks_are_valid_and_every_code_cell_can_run() {
    for rel in ["examples/jsonplaceholder/notes/run-the-app.ipynb", "docs/run-dev-all.ipynb"] {
        let Some(text) = repo_file(rel) else {
            eprintln!("skipping {rel}: not found");
            continue;
        };
        devall_core::notebook::validate(&text).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(devall_core::notebook::notebook_text(&text).0.is_some(), "{rel} has a title");
        devall_notebook::trust::notebook_hash(&text).unwrap();
        let nb: Value = serde_json::from_str(&text).unwrap();
        let mut ids = std::collections::HashSet::new();
        for cell in nb["cells"].as_array().unwrap() {
            assert!(ids.insert(cell["id"].as_str().expect("cell ids").to_string()), "{rel}: duplicate cell id");
            if cell["cell_type"] == "code" {
                let (lang, code) = devall_core::notebook::cell_language(cell, &nb);
                assert!(lang.and_then(Language::parse).is_some(), "{rel}: cell without a runnable language: {code}");
                assert_eq!(cell["outputs"], serde_json::json!([]), "{rel}: outputs aren't committed");
            }
        }
    }
}
