//! Writes an [`ImportPlan`] into a project through the `devall_core::Project` APIs.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use devall_core::fsutil::{normalize_rel, rel_join, safe_join};
use devall_core::project::REQUEST_EXT;
use devall_core::secrets::ALL_ENVIRONMENTS;
use devall_core::{FolderFile, HttpRequestFile, Project};

use crate::error::{Error, Result};
use crate::plan::{ImportPlan, PlanItem};
use crate::secrets::{self, DetectedSecret, Scope};
use crate::util::folder_has_settings;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApplyOptions {
    /// Put everything in a new folder named after the collection (default) instead of directly
    /// into the target folder.
    pub wrap_in_folder: bool,
    /// Overrides the wrapping folder's name.
    pub folder_name: Option<String>,
    /// Update same-named folders/requests/environments in place (keeps ids) instead of creating
    /// uniquely named copies. Useful to re-sync an OpenAPI spec.
    pub overwrite: bool,
    pub import_environments: bool,
    pub import_variables: bool,
    /// Turn credentials into `{{secret.NAME}}` references (values returned, never written).
    pub extract_secrets: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self { wrap_in_folder: true, folder_name: None, overwrite: false, import_environments: true, import_variables: true, extract_secrets: true }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    /// Folder (relative to `http/`) that received the import.
    pub folder: String,
    pub folders_created: usize,
    pub folders_merged: usize,
    pub requests_created: usize,
    pub requests_updated: usize,
    /// Environment ids.
    pub environments_created: Vec<String>,
    pub environments_updated: Vec<String>,
    pub variables_added: Vec<String>,
    /// Credentials lifted out of the import, for the UI to offer storing in the keychain.
    pub secrets: Vec<DetectedSecret>,
    pub warnings: Vec<String>,
    /// Path of the first request written (to open it afterwards).
    pub first_request: Option<String>,
}

/// Writes `plan` below `target_folder` (relative to `http/`, empty = root). Never overwrites
/// anything unless `options.overwrite` is set; name clashes get ` (2)`, ` (3)`… suffixes.
pub fn apply(plan: ImportPlan, project: &mut Project, target_folder: &str, options: &ApplyOptions) -> Result<ApplyReport> {
    let mut plan = plan;
    let mut report = ApplyReport { warnings: std::mem::take(&mut plan.warnings), ..Default::default() };
    let pending = if options.extract_secrets { secrets::extract(&mut plan) } else { Vec::new() };

    let target = normalize_rel(target_folder);
    let target_dir = safe_join(&project.http_root(), &target)?;
    if !target_dir.is_dir() {
        if target.is_empty() {
            std::fs::create_dir_all(&target_dir).map_err(crate::error::io_err(&target_dir))?;
        } else {
            return Err(Error::Core(devall_core::Error::NotFound(target)));
        }
    }

    // Folders and requests.
    let has_tree = !plan.items.is_empty() || plan.settings.as_ref().is_some_and(folder_has_settings);
    let root = if options.wrap_in_folder && has_tree {
        let name = options.folder_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(plan.name.trim());
        let name = if name.is_empty() { "Imported" } else { name };
        let existing = children(project, &target)?;
        match existing.folders.get(&name.to_lowercase()) {
            Some(rel) if options.overwrite => {
                if let Some(s) = &plan.settings {
                    merge_folder_settings(project, rel, s, true)?;
                }
                report.folders_merged += 1;
                rel.clone()
            }
            _ => {
                let taken: HashSet<String> = existing.folders.keys().cloned().collect();
                let rel = project.create_folder(&target, &unique_display(name, &taken))?;
                if let Some(s) = &plan.settings {
                    merge_folder_settings(project, &rel, s, true)?;
                }
                report.folders_created += 1;
                rel
            }
        }
    } else {
        if let Some(s) = &plan.settings
            && folder_has_settings(s)
        {
            merge_folder_settings(project, &target, s, options.overwrite)?;
        }
        target.clone()
    };
    apply_items(project, options, &mut report, &root, std::mem::take(&mut plan.items))?;
    report.folder = root;

    // Environments.
    let mut env_ids: HashMap<usize, String> = HashMap::new();
    if options.import_environments && !plan.environments.is_empty() {
        let existing = project.environments()?;
        let mut taken: HashSet<String> = existing.iter().map(|e| e.file.name.to_lowercase()).collect();
        for (i, env) in std::mem::take(&mut plan.environments).into_iter().enumerate() {
            let same = existing.iter().find(|e| e.file.name.eq_ignore_ascii_case(env.name.trim()));
            match same {
                Some(e) if options.overwrite => {
                    let mut file = e.file.clone();
                    file.variables.extend(env.variables);
                    for s in env.secrets {
                        if !file.secrets.contains(&s) {
                            file.secrets.push(s);
                        }
                    }
                    if env.color.is_some() {
                        file.color = env.color;
                    }
                    project.save_environment(&e.id, &file)?;
                    env_ids.insert(i, e.id.clone());
                    if !report.environments_updated.contains(&e.id) {
                        report.environments_updated.push(e.id.clone());
                    }
                }
                _ => {
                    let base = if env.name.trim().is_empty() { "Imported" } else { env.name.trim() };
                    let name = unique_display(base, &taken);
                    taken.insert(name.to_lowercase());
                    let id = project.create_environment(&name)?;
                    let mut file = env;
                    file.name = name;
                    project.save_environment(&id, &file)?;
                    env_ids.insert(i, id.clone());
                    report.environments_created.push(id);
                }
            }
        }
    }

    // Project variables.
    if options.import_variables && !plan.project_variables.is_empty() {
        let mut changed = false;
        for (k, v) in std::mem::take(&mut plan.project_variables) {
            match project.manifest.variables.get(&k) {
                Some(cur) if *cur == v => {}
                Some(_) if !options.overwrite => {
                    report.warnings.push(format!("Project variable “{k}” already exists; kept the current value"));
                }
                _ => {
                    project.manifest.variables.insert(k.clone(), v);
                    report.variables_added.push(k);
                    changed = true;
                }
            }
        }
        if changed {
            project.save_manifest()?;
        }
    }

    // Secrets (values only travel back to the caller).
    for p in pending {
        let environment = match p.scope {
            Scope::All => ALL_ENVIRONMENTS.to_string(),
            Scope::Env(i) => match env_ids.get(&i) {
                Some(id) => id.clone(),
                None => continue,
            },
        };
        if !report.secrets.iter().any(|s| s.environment == environment && s.name == p.name) {
            report.secrets.push(DetectedSecret { environment, name: p.name, value: p.value, source: p.source });
        }
    }
    Ok(report)
}

#[derive(Default)]
struct Children {
    /// Lowercased display name → relative path.
    folders: HashMap<String, String>,
    requests: HashMap<String, String>,
}

fn children(project: &Project, folder_rel: &str) -> Result<Children> {
    let dir = safe_join(&project.http_root(), folder_rel)?;
    let mut c = Children::default();
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(c) };
    for e in rd.flatten() {
        let file = e.file_name().to_string_lossy().to_string();
        if file.starts_with('.') {
            continue;
        }
        let rel = rel_join(folder_rel, &file);
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let name = project.read_folder(&rel).ok().and_then(|f| f.name).unwrap_or(file);
            c.folders.insert(name.to_lowercase(), rel);
        } else if file.ends_with(REQUEST_EXT)
            && let Ok(r) = project.read_request(&rel)
        {
            c.requests.insert(r.name.to_lowercase(), rel);
        }
    }
    Ok(c)
}

/// `name`, or `name (2)`, `name (3)`… — the first not in `taken` (lowercased).
fn unique_display(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(&name.to_lowercase()) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name} ({n})")).find(|c| !taken.contains(&c.to_lowercase())).unwrap_or_else(|| name.to_string())
}

fn merge_folder_settings(project: &Project, rel: &str, incoming: &FolderFile, overwrite: bool) -> Result<()> {
    let mut f = project.read_folder(rel)?;
    for h in &incoming.headers {
        match f.headers.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&h.name)) {
            Some(x) if overwrite => *x = h.clone(),
            Some(_) => {}
            None => f.headers.push(h.clone()),
        }
    }
    if !incoming.auth.is_inherit() && (overwrite || f.auth.is_inherit()) {
        f.auth = incoming.auth.clone();
    }
    for (k, v) in &incoming.variables {
        if overwrite || !f.variables.contains_key(k) {
            f.variables.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in &incoming.extra {
        f.extra.entry(k.clone()).or_insert_with(|| v.clone());
    }
    if folder_has_settings(&f) || !f.extra.is_empty() || f.name.is_some() || !f.order.is_empty() {
        project.save_folder(rel, &f)?;
    }
    Ok(())
}

fn apply_items(project: &Project, opts: &ApplyOptions, report: &mut ApplyReport, folder_rel: &str, items: Vec<PlanItem>) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let existing = children(project, folder_rel)?;
    let mut taken_folders: HashSet<String> = existing.folders.keys().cloned().collect();
    let mut taken_requests: HashSet<String> = existing.requests.keys().cloned().collect();
    // Names written by this import: never overwrite those, even in overwrite mode.
    let mut ours_folders: HashSet<String> = HashSet::new();
    let mut ours_requests: HashSet<String> = HashSet::new();
    for item in items {
        match item {
            PlanItem::Folder(f) => {
                let base = if f.name.trim().is_empty() { "Untitled folder".to_string() } else { f.name.trim().to_string() };
                let key = base.to_lowercase();
                let rel = match existing.folders.get(&key) {
                    Some(rel) if opts.overwrite && !ours_folders.contains(&key) => {
                        if let Some(s) = &f.settings {
                            merge_folder_settings(project, rel, s, true)?;
                        }
                        report.folders_merged += 1;
                        ours_folders.insert(key);
                        rel.clone()
                    }
                    _ => {
                        let name = unique_display(&base, &taken_folders);
                        let rel = project.create_folder(folder_rel, &name)?;
                        if let Some(s) = &f.settings {
                            merge_folder_settings(project, &rel, s, true)?;
                        }
                        report.folders_created += 1;
                        let k = name.to_lowercase();
                        taken_folders.insert(k.clone());
                        ours_folders.insert(k);
                        rel
                    }
                };
                apply_items(project, opts, report, &rel, f.items)?;
            }
            PlanItem::Request(r) => {
                let mut req: HttpRequestFile = *r;
                if req.name.trim().is_empty() {
                    req.name = "Untitled request".into();
                }
                req.name = req.name.trim().to_string();
                let key = req.name.to_lowercase();
                let rel = match existing.requests.get(&key) {
                    Some(rel) if opts.overwrite && !ours_requests.contains(&key) => {
                        if let Ok(old) = project.read_request(rel) {
                            req.id = old.id;
                        }
                        project.save_request(rel, &req)?;
                        report.requests_updated += 1;
                        ours_requests.insert(key);
                        rel.clone()
                    }
                    _ => {
                        req.name = unique_display(&req.name, &taken_requests);
                        let k = req.name.to_lowercase();
                        let rel = project.create_request(folder_rel, req)?;
                        report.requests_created += 1;
                        taken_requests.insert(k.clone());
                        ours_requests.insert(k);
                        rel
                    }
                };
                if report.first_request.is_none() {
                    report.first_request = Some(rel);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Format;
    use crate::plan::PlanFolder;
    use devall_core::{Auth, EnvironmentFile, KeyValue};

    fn plan() -> ImportPlan {
        let mut p = ImportPlan::new("Demo API", Format::Postman);
        p.settings = Some(FolderFile { auth: Auth::Bearer { token: "{{token}}".into(), prefix: None }, ..Default::default() });
        let mut users = PlanFolder::new("Users");
        let mut list = HttpRequestFile::new("List users");
        list.url = "{{baseUrl}}/users".into();
        users.items.push(PlanItem::Request(Box::new(list)));
        p.items.push(PlanItem::Folder(users));
        let mut health = HttpRequestFile::new("Health");
        health.url = "{{baseUrl}}/health".into();
        p.items.push(PlanItem::Request(Box::new(health)));
        let mut env = EnvironmentFile::new("Local");
        env.variables.insert("baseUrl".into(), "http://localhost:3000".into());
        env.variables.insert("token".into(), "dev-token-123".into());
        p.environments.push(env);
        p.project_variables.insert("apiVersion".into(), "v2".into());
        p
    }

    #[test]
    fn applies_with_unique_names_and_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path(), "P").unwrap();
        let r = apply(plan(), &mut project, "", &ApplyOptions::default()).unwrap();
        assert_eq!(r.folder, "demo-api");
        assert_eq!(r.requests_created, 2);
        assert_eq!(r.folders_created, 2);
        assert_eq!(r.first_request.as_deref(), Some("demo-api/users/list-users.req.yaml"));
        // "Local" already exists in a new project → a second environment, not an overwrite.
        let envs = project.environments().unwrap();
        assert!(envs.iter().any(|e| e.file.name == "Local (2)"), "{envs:?}");
        let imported = envs.iter().find(|e| e.file.name == "Local (2)").unwrap();
        assert!(!imported.file.variables.contains_key("token"));
        assert_eq!(imported.file.secrets, vec!["token".to_string()]);
        assert_eq!(r.secrets.len(), 1);
        assert_eq!(r.secrets[0].value, "dev-token-123");
        assert_eq!(r.secrets[0].environment, imported.id);
        let folder = project.read_folder("demo-api").unwrap();
        assert_eq!(folder.auth, Auth::Bearer { token: "{{secret.token}}".into(), prefix: None });
        assert_eq!(project.manifest.variables.get("apiVersion").map(String::as_str), Some("v2"));
        // No secret value anywhere on disk.
        for entry in walk(dir.path()) {
            let text = std::fs::read_to_string(&entry).unwrap_or_default();
            assert!(!text.contains("dev-token-123"), "{}", entry.display());
        }

        // Importing again creates "Demo API (2)" instead of touching the first copy.
        let r2 = apply(plan(), &mut project, "", &ApplyOptions::default()).unwrap();
        assert_eq!(r2.folder, "demo-api-2");
        assert_eq!(project.read_folder("demo-api-2").unwrap().name.as_deref(), Some("Demo API (2)"));
    }

    #[test]
    fn overwrite_updates_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path(), "P").unwrap();
        apply(plan(), &mut project, "", &ApplyOptions::default()).unwrap();
        let before = project.read_request("demo-api/health.req.yaml").unwrap();
        let mut p = plan();
        if let PlanItem::Request(r) = &mut p.items[1] {
            r.headers.push(KeyValue::new("X-New", "1"));
        }
        let opts = ApplyOptions { overwrite: true, ..Default::default() };
        let r = apply(p, &mut project, "", &opts).unwrap();
        assert_eq!(r.folder, "demo-api");
        assert_eq!(r.requests_updated, 2);
        assert_eq!(r.requests_created, 0);
        let after = project.read_request("demo-api/health.req.yaml").unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.headers[0].name, "X-New");
        assert!(r.environments_updated.contains(&"local".to_string()));
    }

    #[test]
    fn unknown_target_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path(), "P").unwrap();
        assert!(apply(plan(), &mut project, "nope", &ApplyOptions::default()).is_err());
        assert!(apply(plan(), &mut project, "../x", &ApplyOptions::default()).is_err());
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
