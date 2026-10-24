//! OpenAPI 3.0 / 3.1 tooling for /dev/all — pure Rust, no network.
//!
//! * [`spec`]: load a spec (YAML or JSON), list operations, resolve local `$ref`s, match a
//!   concrete request path to its operation (server base paths stripped, literal segments
//!   preferred over templates).
//! * [`schema`]: validate JSON values against OpenAPI schemas (3.0 `nullable`, 3.1 type lists,
//!   read/write-only, discriminators, formats).
//! * [`lint`]: a built-in rule set with severities (`devall openapi lint`).
//! * [`conform`]: per-exchange checks (path, method, status, request/response bodies) — used
//!   by the drift report and the inspector's live validation rule.
//! * [`drift`]: spec ↔ mock ↔ traffic checks — WireMock stubs from `mocks/mappings/`, captured
//!   traffic and HAR files ([`har`]) against the spec (`devall openapi drift`).
//!
//! ```
//! let report = devall_openapi::lint::lint_text("openapi: 3.1.0\ninfo: { title: Demo, version: '1' }\nservers: [{ url: / }]\npaths: { /ping: { get: { operationId: ping, summary: Ping, tags: [x], responses: { '204': { description: ok } } } } }");
//! assert!(report.passed());
//! ```

pub mod conform;
pub mod drift;
mod error;
pub mod har;
pub mod lint;
pub mod schema;
pub mod spec;

use std::path::Path;

pub use conform::{Conformance, Violation};
pub use drift::{Drift, DriftFinding, DriftKind, DriftReport, MockInput, TrafficOptions, TrafficSample};
pub use error::{Error, Result};
pub use lint::{Finding, LintReport, Severity, lint, lint_text};
pub use spec::{Spec, looks_like_openapi};

/// Reads a stub body file (relative to `mocks/__files/`) for drift checks; `None` if missing,
/// outside that folder or not UTF-8.
pub fn mock_body_reader(project_root: &Path) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| {
        let dir = project_root.join("mocks").join("__files");
        let path = devall_core::fsutil::safe_join(&dir, name).ok()?;
        std::fs::read_to_string(path).ok()
    }
}

/// Spec files in a project: `*.yaml` / `*.yml` / `*.json` that look like OpenAPI 3 documents,
/// outside the module folders (`http/`, `mocks/`, `load/`, …), `node_modules/`, build output and
/// dot-folders. Project-relative, `/`-separated, sorted.
pub fn find_specs(project_root: &Path) -> Vec<String> {
    const SKIP: &[&str] = &["http", "mocks", "load", "environments", "node_modules", "target", "dist", "build", "notes", "data", "connections", "webhooks", "queries", "utilities"];
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
        if depth > 6 || out.len() > 200 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                if SKIP.contains(&name.as_str()) && (depth == 0 || name == "node_modules") {
                    continue;
                }
                walk(root, &p, depth + 1, out);
                continue;
            }
            let lower = name.to_ascii_lowercase();
            if !(lower.ends_with(".yaml") || lower.ends_with(".yml") || lower.ends_with(".json")) || lower.ends_with(".req.yaml") || lower.ends_with(".env.yaml") {
                continue;
            }
            if e.metadata().map(|m| m.len() > 20 * 1024 * 1024).unwrap_or(true) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            if looks_like_openapi(&text) {
                out.push(p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(project_root, project_root, 0, &mut out);
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_specs_outside_module_folders() {
        let d = tempfile::tempdir().unwrap();
        let w = |rel: &str, text: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w("openapi/api.yaml", "openapi: 3.0.0\ninfo: {}\n");
        w("docs/spec.json", r#"{"openapi":"3.1.0"}"#);
        w("docs/other.yaml", "name: x\n");
        w("http/x.req.yaml", "openapi: 3.0.0\n");
        w("mocks/mappings/a.json", r#"{"openapi":"3.1.0"}"#);
        assert_eq!(find_specs(d.path()), vec!["docs/spec.json".to_string(), "openapi/api.yaml".into()]);
        w("mocks/__files/user.json", r#"{"id":1}"#);
        assert_eq!(mock_body_reader(d.path())("user.json").as_deref(), Some(r#"{"id":1}"#));
        assert!(mock_body_reader(d.path())("../../openapi/api.yaml").is_none());
    }
}
