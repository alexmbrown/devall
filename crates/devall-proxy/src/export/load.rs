//! Captured exchanges → a load-test scenario for `crates/devall-load`: the requests are saved
//! under `http/captured/<slug>/` (identical requests once) and `load/<slug>.load.yaml` replays
//! them as an ordered flow with one virtual user for 30 s — a starting point to edit.

use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};

use super::requests::to_request_file;
use crate::error::Result;
use crate::model::Exchange;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadExport {
    /// Relative to `load/`.
    pub load_file: String,
    /// Saved requests, relative to `http/`.
    pub requests: Vec<String>,
}

pub fn save_load_test(root: &Path, exchanges: &[Exchange], name: &str) -> Result<LoadExport> {
    let project = devall_core::Project::open(root)?;
    let name = if name.trim().is_empty() { "Captured traffic" } else { name.trim() };
    let slug = devall_core::fsutil::slugify(name);
    let http_dir = project.http_root().join("captured");
    let folder_name = devall_core::fsutil::unique_name(&http_dir, &slug, "");
    let folder = format!("captured/{folder_name}");
    let mut saved: Vec<(String, String)> = Vec::new();
    let mut flow: Vec<String> = Vec::new();
    for ex in exchanges.iter().filter(|e| e.request.method != "CONNECT" && e.state != crate::model::ExchangeState::Tunnel) {
        let key = format!("{} {} {}", ex.request.method, ex.request.url, ex.request.body.raw.len());
        let rel = match saved.iter().find(|(k, _)| *k == key) {
            Some((_, rel)) => rel.clone(),
            None => {
                let rel = project.create_request(&folder, to_request_file(ex))?;
                saved.push((key, rel.clone()));
                rel
            }
        };
        flow.push(rel);
    }
    let load_dir = root.join("load");
    let file = devall_core::fsutil::unique_name(&load_dir, &slug, ".load.yaml");
    let doc: Value = json!({
        "version": 1,
        "name": name,
        "description": format!("Replays {} captured request(s) in order. Captured by the traffic inspector.", flow.len()),
        "scenario": { "flow": flow },
        "executor": { "type": "constantVus", "vus": 1, "duration": "30s" },
        "thresholds": ["errorRate < 1%"],
    });
    devall_core::fsutil::write_yaml(&load_dir.join(&file), &doc)?;
    Ok(LoadExport { load_file: file, requests: saved.into_iter().map(|(_, r)| r).collect() })
}
