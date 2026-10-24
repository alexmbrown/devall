//! "Where used": every `{{name}}` / `{{secret.name}}` reference to a set of variables across the
//! project's files (requests, folders, realtime files, mocks, notes, load tests, fake-data
//! schemas, connections, webhook fixtures, environments and `devall.yaml`).

use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;

/// Files larger than this are skipped (fixtures, recordings).
const MAX_FILE: u64 = 2 * 1024 * 1024;
/// Usages reported per variable name.
const MAX_PER_NAME: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VarUsage {
    /// Referenced name as written in the template (`baseUrl`, `secret.API_TOKEN`).
    pub name: String,
    /// Path relative to the project root, `/`-separated.
    pub path: String,
    /// `request`, `folder`, `realtime`, `mock`, `note`, `load`, `data`, `connection`, `query`,
    /// `webhook`, `environment`, `project`, `other`.
    pub kind: &'static str,
    /// 1-based.
    pub line: usize,
    /// 1-based, in characters.
    pub column: usize,
    /// The line, trimmed and shortened.
    pub excerpt: String,
}

fn kind_of(rel: &str) -> &'static str {
    let top = rel.split('/').next().unwrap_or("");
    match top {
        "http" if rel.ends_with("/_folder.yaml") || rel == "http/_folder.yaml" => "folder",
        "http" if rel.ends_with(".req.yaml") => "request",
        "http" if [".ws.yaml", ".grpc.yaml", ".mqtt.yaml", ".sio.yaml"].iter().any(|e| rel.ends_with(e)) => "realtime",
        "mocks" => "mock",
        "notes" => "note",
        "load" => "load",
        "data" => "data",
        "connections" => "connection",
        "queries" => "query",
        "webhooks" => "webhook",
        "environments" => "environment",
        _ if rel == crate::project::MANIFEST => "project",
        _ => "other",
    }
}

fn scannable(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    if (lower.starts_with("load/") && lower.contains("/runs/")) || lower.contains("/__snapshots__/") {
        return false;
    }
    [".yaml", ".yml", ".json", ".md", ".http", ".hurl", ".ipynb", ".graphql", ".gql", ".sql", ".redis", ".txt", ".csv", ".xml", ".html", ".hbs"].iter().any(|e| lower.ends_with(e))
}

/// Finds `{{ name }}` references (optionally with a `| filter` suffix) on one line.
fn refs_in_line(line: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut rest = line;
    let mut offset = 0usize;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let inner = after[..end].trim();
        let name = inner.split('|').next().unwrap_or("").trim();
        let valid = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '$'));
        if valid {
            out.push((offset + start, name));
        }
        offset += start + 2 + end + 2;
        rest = &after[end + 2..];
    }
    out
}

/// Every reference to one of `names` (`baseUrl`, `secret.API_TOKEN`) under `root`.
pub fn find_usages(root: &Path, names: &[String]) -> Vec<VarUsage> {
    let wanted: HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    if wanted.is_empty() {
        return out;
    }
    let mut counts = std::collections::HashMap::<String, usize>::new();
    let mut files = Vec::new();
    collect(root, "", &mut files);
    files.sort();
    for rel in files {
        let path = root.join(&rel);
        if std::fs::metadata(&path).map(|m| m.len() > MAX_FILE).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if !text.contains("{{") {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            for (byte, name) in refs_in_line(line) {
                if !wanted.contains(name) {
                    continue;
                }
                let n = counts.entry(name.to_string()).or_default();
                *n += 1;
                if *n > MAX_PER_NAME {
                    continue;
                }
                let trimmed = line.trim();
                let excerpt: String = if trimmed.chars().count() > 160 { format!("{}…", trimmed.chars().take(160).collect::<String>()) } else { trimmed.to_string() };
                out.push(VarUsage {
                    name: name.to_string(),
                    path: rel.clone(),
                    kind: kind_of(&rel),
                    line: i + 1,
                    column: line[..byte].chars().count() + 1,
                    excerpt,
                });
            }
        }
    }
    out
}

fn collect(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "node_modules" || name == "target" || name == "__snapshots__" {
            continue;
        }
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            collect(&e.path(), &child, out);
        } else if ft.is_file() && scannable(&child) {
            out.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_references_with_kinds_and_positions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let w = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w("http/users/get.req.yaml", "name: Get user\nurl: '{{baseUrl}}/users/{{ userId }}'\nheaders:\n  Authorization: Bearer {{secret.API_TOKEN}}\n");
        w("http/users/_folder.yaml", "headers:\n  X-Tenant: '{{tenant}}'\n");
        w("mocks/mappings/a.json", r#"{"response":{"body":"{{request.path}} {{baseUrl}}"}}"#);
        w("notes/setup.md", "Point `{{baseUrl | upper}}` at staging.\n");
        w("load/smoke.load.yaml", "target: '{{baseUrl}}'\n");
        w("load/runs/smoke/1.json", "{{baseUrl}}");
        w(".devall/history.json", "{{baseUrl}}");
        w("http/__snapshots__/x.snap.json", "{{baseUrl}}");
        w("devall.yaml", "variables:\n  api: '{{baseUrl}}/v1'\n");
        let names = vec!["baseUrl".to_string(), "secret.API_TOKEN".to_string(), "tenant".to_string(), "userId".to_string()];
        let u = find_usages(root, &names);
        let base: Vec<(&str, &str)> = u.iter().filter(|x| x.name == "baseUrl").map(|x| (x.path.as_str(), x.kind)).collect();
        assert_eq!(
            base,
            [("devall.yaml", "project"), ("http/users/get.req.yaml", "request"), ("load/smoke.load.yaml", "load"), ("mocks/mappings/a.json", "mock"), ("notes/setup.md", "note")]
        );
        let tok = u.iter().find(|x| x.name == "secret.API_TOKEN").unwrap();
        assert_eq!((tok.line, tok.column), (4, 25));
        assert_eq!(tok.excerpt, "Authorization: Bearer {{secret.API_TOKEN}}");
        assert_eq!(u.iter().find(|x| x.name == "tenant").unwrap().kind, "folder");
        assert!(u.iter().any(|x| x.name == "userId"), "spaces inside braces");
        // WireMock helpers aren't project variables.
        assert!(!u.iter().any(|x| x.name.starts_with("request.")));
        assert!(find_usages(root, &[]).is_empty());
    }

    #[test]
    fn parses_template_references() {
        assert_eq!(refs_in_line("a {{x}} b {{ y.z | lower }} {{$uuid}} {{}} {{ not valid! }}"), [(2, "x"), (10, "y.z"), (28, "$uuid")]);
        assert!(refs_in_line("{{unclosed").is_empty());
    }
}
