//! Project files: `mocks/mappings/**/*.json` (single mappings or `{"mappings": [...]}`) and
//! `mocks/__files/` body files. Writes are atomic and pretty-printed with a stable key order;
//! deletes go to the OS trash unless disabled.

use std::fs;
use std::path::{Path, PathBuf};

use devall_core::fsutil::{atomic_write, normalize_rel, safe_join, slugify, unique_name};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::MockError;
use crate::stub::{Stub, StubSource};
use crate::util::{short_id, stable_uuid};

pub const MOCKS_DIR: &str = "mocks";
pub const MAPPINGS_DIR: &str = "mappings";
pub const FILES_DIR: &str = "__files";

/// A mapping file that could not be loaded (the rest of the project still loads).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadError {
    /// Path relative to the project root, `/`-separated.
    pub file: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct ProjectFiles {
    pub root: PathBuf,
    pub mappings: PathBuf,
    pub files: PathBuf,
    pub trash_deletes: bool,
}

/// A `{"mappings": [...]}` file: its path, the other top-level keys and the entries.
type MultiFile = (PathBuf, Map<String, Value>, Vec<Value>);

pub fn pretty_json(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into());
    s.push('\n');
    s
}

fn rel_of(base: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path);
    normalize_rel(&rel.to_string_lossy())
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_json(&path, out);
        } else if name.to_ascii_lowercase().ends_with(".json") {
            out.push(path);
        }
    }
}

/// One definition from a protocol folder (`mocks/websocket`, `mocks/grpc`, `mocks/mqtt`).
#[derive(Debug, Clone)]
pub(crate) struct JsonDef {
    /// File relative to the project root, `/`-separated.
    pub file: String,
    /// Position inside a `{"<list_key>": [...]}` / array file.
    pub index: Option<usize>,
    pub value: Value,
}

impl JsonDef {
    /// `file` or `file#n` (stable seed for ids, and where errors point).
    pub fn location(&self) -> String {
        match self.index {
            Some(i) => format!("{}#{}", self.file, i + 1),
            None => self.file.clone(),
        }
    }
}

/// Loads every `*.json` under `<root>/mocks/<sub>` (recursively, sorted): single objects,
/// arrays, or `{"<list_key>": [...]}`. Files named in `skip` are ignored.
pub(crate) fn load_json_defs(root: &Path, sub: &str, list_key: &str, skip: &[&str]) -> (Vec<JsonDef>, Vec<LoadError>) {
    let dir = root.join(MOCKS_DIR).join(sub);
    let mut paths = Vec::new();
    collect_json(&dir, &mut paths);
    paths.sort();
    let mut defs = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        if path.file_name().is_some_and(|n| skip.iter().any(|s| n.eq_ignore_ascii_case(s))) {
            continue;
        }
        let file = rel_of(root, &path);
        let parsed = fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).map_err(|e| format!("invalid JSON: {e}")));
        match parsed {
            Ok(Value::Array(items)) => defs.extend(items.into_iter().enumerate().map(|(i, value)| JsonDef { file: file.clone(), index: Some(i), value })),
            Ok(Value::Object(mut o)) if o.get(list_key).is_some_and(Value::is_array) => {
                if let Some(Value::Array(items)) = o.remove(list_key) {
                    defs.extend(items.into_iter().enumerate().map(|(i, value)| JsonDef { file: file.clone(), index: Some(i), value }));
                }
            }
            Ok(v @ Value::Object(_)) => defs.push(JsonDef { file, index: None, value: v }),
            Ok(_) => errors.push(LoadError { file, error: format!("expected an object, an array or {{\"{list_key}\": [...]}}") }),
            Err(error) => errors.push(LoadError { file, error }),
        }
    }
    (defs, errors)
}

impl ProjectFiles {
    pub fn new(root: &Path, trash_deletes: bool) -> Self {
        let mocks = root.join(MOCKS_DIR);
        Self { root: root.to_path_buf(), mappings: mocks.join(MAPPINGS_DIR), files: mocks.join(FILES_DIR), trash_deletes }
    }

    fn project_rel(&self, path: &Path) -> String {
        rel_of(&self.root, path)
    }

    /// Loads every mapping file. Stubs come back marked persistent with their source recorded.
    pub fn load(&self) -> (Vec<Stub>, Vec<LoadError>) {
        let mut paths = Vec::new();
        collect_json(&self.mappings, &mut paths);
        paths.sort();
        let mut stubs: Vec<Stub> = Vec::new();
        let mut errors = Vec::new();
        for path in paths {
            let file = self.project_rel(&path);
            let rel = rel_of(&self.mappings, &path);
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    errors.push(LoadError { file, error: e.to_string() });
                    continue;
                }
            };
            let value: Value = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(LoadError { file, error: format!("invalid JSON: {e}") });
                    continue;
                }
            };
            let (entries, multi): (Vec<Value>, bool) = match value {
                Value::Object(mut o) if o.get("mappings").is_some_and(Value::is_array) => match o.remove("mappings") {
                    Some(Value::Array(a)) => (a, true),
                    _ => (Vec::new(), true),
                },
                v @ Value::Object(_) => (vec![v], false),
                _ => {
                    errors.push(LoadError { file, error: "expected a stub mapping object or {\"mappings\": [...]}".into() });
                    continue;
                }
            };
            for (i, entry) in entries.iter().enumerate() {
                let seed = if multi { format!("{rel}#{i}") } else { rel.clone() };
                match Stub::from_json(entry, || stable_uuid(&seed)) {
                    Ok(mut stub) => {
                        if stubs.iter().any(|s| s.id == stub.id) {
                            errors.push(LoadError { file: file.clone(), error: format!("duplicate stub id {} (skipped)", stub.id) });
                            continue;
                        }
                        stub.set_persistent(true);
                        stub.source = Some(StubSource { rel_path: rel.clone(), multi });
                        stubs.push(stub);
                    }
                    Err(e) => {
                        let error = if multi { format!("mapping #{}: {e}", i + 1) } else { e };
                        errors.push(LoadError { file: file.clone(), error });
                    }
                }
            }
        }
        (stubs, errors)
    }

    fn file_name_for(&self, stub: &Stub) -> String {
        let base = match &stub.name {
            Some(n) if !n.trim().is_empty() => slugify(n),
            _ => slugify(&stub.summary()),
        };
        let stem = format!("{base}-{}", short_id(&stub.id));
        unique_name(&self.mappings, &stem, ".json")
    }

    /// Writes a stub to its file (creating one if it has none yet) and records the source.
    pub fn save_stub(&self, stub: &mut Stub) -> Result<(), MockError> {
        stub.set_persistent(true);
        match stub.source.clone() {
            Some(src) if src.multi => self.rewrite_multi(&src.rel_path, Some(stub)),
            Some(src) => {
                let path = safe_join(&self.mappings, &src.rel_path)?;
                atomic_write(&path, pretty_json(&stub.to_file_json()).as_bytes())?;
                Ok(())
            }
            None => {
                fs::create_dir_all(&self.mappings).map_err(|e| MockError::io(&self.mappings, e))?;
                let name = self.file_name_for(stub);
                atomic_write(&self.mappings.join(&name), pretty_json(&stub.to_file_json()).as_bytes())?;
                stub.source = Some(StubSource { rel_path: name, multi: false });
                Ok(())
            }
        }
    }

    /// Removes a stub from disk: its own file goes (to the trash), or its entry leaves a multi file.
    pub fn remove_stub(&self, stub: &Stub) -> Result<(), MockError> {
        let Some(src) = &stub.source else { return Ok(()) };
        if src.multi {
            return self.rewrite_multi_remove(&src.rel_path, stub);
        }
        let path = safe_join(&self.mappings, &src.rel_path)?;
        self.delete_path(&path)
    }

    fn delete_path(&self, path: &Path) -> Result<(), MockError> {
        if !path.exists() {
            return Ok(());
        }
        if self.trash_deletes {
            trash::delete(path).map_err(|e| MockError::Invalid(format!("cannot move {} to the trash: {e}", path.display())))
        } else {
            fs::remove_file(path).map_err(|e| MockError::io(path, e))
        }
    }

    fn read_multi(&self, rel: &str) -> Result<MultiFile, MockError> {
        let path = safe_join(&self.mappings, rel)?;
        let text = fs::read_to_string(&path).map_err(|e| MockError::io(&path, e))?;
        let mut doc: Map<String, Value> = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| MockError::Invalid(format!("{}: {e}", path.display())))?;
        let mut entries = match doc.remove("mappings") {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        // Make every entry's id explicit so positions never matter again.
        for (i, entry) in entries.iter_mut().enumerate() {
            if let Value::Object(o) = entry {
                let id = o
                    .get("id")
                    .or_else(|| o.get("uuid"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| stable_uuid(&format!("{rel}#{i}")).to_string());
                if !o.contains_key("id") {
                    let mut with_id = Map::new();
                    with_id.insert("id".into(), Value::String(id));
                    with_id.extend(std::mem::take(o));
                    *o = with_id;
                }
            }
        }
        Ok((path, doc, entries))
    }

    fn write_multi(&self, path: &Path, mut doc: Map<String, Value>, entries: Vec<Value>) -> Result<(), MockError> {
        if entries.is_empty() && doc.is_empty() {
            return self.delete_path(path);
        }
        let mut out = Map::new();
        out.insert("mappings".into(), Value::Array(entries));
        out.append(&mut doc);
        atomic_write(path, pretty_json(&Value::Object(out)).as_bytes())?;
        Ok(())
    }

    fn entry_id(v: &Value) -> Option<&str> {
        v.get("id").or_else(|| v.get("uuid")).and_then(Value::as_str)
    }

    fn rewrite_multi(&self, rel: &str, stub: Option<&Stub>) -> Result<(), MockError> {
        let (path, doc, mut entries) = self.read_multi(rel)?;
        if let Some(stub) = stub {
            let id = stub.id.to_string();
            let json = stub.to_file_json();
            match entries.iter().position(|e| Self::entry_id(e) == Some(id.as_str())) {
                Some(i) => entries[i] = json,
                None => entries.push(json),
            }
        }
        self.write_multi(&path, doc, entries)
    }

    fn rewrite_multi_remove(&self, rel: &str, stub: &Stub) -> Result<(), MockError> {
        let (path, doc, mut entries) = self.read_multi(rel)?;
        let id = stub.id.to_string();
        entries.retain(|e| Self::entry_id(e) != Some(id.as_str()));
        self.write_multi(&path, doc, entries)
    }

    // ---- __files

    pub fn body_file(&self, rel: &str) -> Result<PathBuf, MockError> {
        Ok(safe_join(&self.files, rel)?)
    }

    pub fn read_body_file(&self, rel: &str) -> Result<Vec<u8>, MockError> {
        let path = self.body_file(rel)?;
        fs::read(&path).map_err(|e| MockError::io(&path, e))
    }

    pub fn list_files(&self) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if e.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    out.push(rel_of(base, &p));
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.files, &self.files, &mut out);
        out.sort();
        out
    }

    pub fn write_file(&self, rel: &str, data: &[u8]) -> Result<(), MockError> {
        if normalize_rel(rel).is_empty() {
            return Err(MockError::Invalid("file path is required".into()));
        }
        let path = self.body_file(rel)?;
        atomic_write(&path, data)?;
        Ok(())
    }

    pub fn delete_file(&self, rel: &str) -> Result<bool, MockError> {
        let path = self.body_file(rel)?;
        if !path.is_file() {
            return Ok(false);
        }
        self.delete_path(&path)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn project() -> (tempfile::TempDir, ProjectFiles) {
        let dir = tempfile::tempdir().unwrap();
        let pf = ProjectFiles::new(dir.path(), false);
        fs::create_dir_all(&pf.mappings).unwrap();
        (dir, pf)
    }

    #[test]
    fn load_single_multi_and_errors() {
        let (_d, pf) = project();
        fs::write(pf.mappings.join("a.json"), r#"{"request":{"urlPath":"/a"},"response":{"body":"A"}}"#).unwrap();
        fs::create_dir_all(pf.mappings.join("sub")).unwrap();
        fs::write(pf.mappings.join("sub/many.json"), r#"{"mappings":[{"request":{"urlPath":"/b"}},{"request":{"urlPattern":"("}},{"request":{"urlPath":"/c"}}]}"#).unwrap();
        fs::write(pf.mappings.join("bad.json"), "{nope").unwrap();
        fs::write(pf.mappings.join("notes.txt"), "ignored").unwrap();
        let (stubs, errors) = pf.load();
        assert_eq!(stubs.len(), 3);
        assert!(stubs.iter().all(|s| s.persistent));
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].file, "mocks/mappings/bad.json");
        assert!(errors[1].error.starts_with("mapping #2"));
        // ids are stable across loads
        let (again, _) = pf.load();
        assert_eq!(stubs.iter().map(|s| s.id).collect::<Vec<_>>(), again.iter().map(|s| s.id).collect::<Vec<_>>());
    }

    #[test]
    fn save_and_remove() {
        let (_d, pf) = project();
        let mut s = Stub::from_json(&json!({"name": "List users", "request": {"method": "GET", "urlPath": "/users"}}), Uuid::new_v4).unwrap();
        pf.save_stub(&mut s).unwrap();
        let src = s.source.clone().unwrap();
        assert!(src.rel_path.starts_with("list-users-") && src.rel_path.ends_with(".json"));
        let text = fs::read_to_string(pf.mappings.join(&src.rel_path)).unwrap();
        assert!(text.starts_with("{\n  \"id\""));
        assert!(text.contains("\"persistent\": true"));
        pf.remove_stub(&s).unwrap();
        assert!(!pf.mappings.join(&src.rel_path).exists());
    }

    #[test]
    fn multi_file_edits() {
        let (_d, pf) = project();
        fs::write(pf.mappings.join("m.json"), r#"{"mappings":[{"request":{"urlPath":"/b"}},{"request":{"urlPath":"/c"}}],"meta":{"x":1}}"#).unwrap();
        let (mut stubs, _) = pf.load();
        let mut edited = Stub::from_json(&json!({"id": stubs[0].id.to_string(), "request": {"urlPath": "/b2"}}), Uuid::new_v4).unwrap();
        edited.source = stubs[0].source.clone();
        pf.save_stub(&mut edited).unwrap();
        let (reloaded, errors) = pf.load();
        assert!(errors.is_empty());
        assert_eq!(reloaded.len(), 2);
        assert_eq!(reloaded[0].id, stubs[0].id);
        assert_eq!(reloaded[1].id, stubs[1].id);
        assert_eq!(reloaded[0].summary(), "ANY /b2");
        let gone = stubs.remove(1);
        pf.remove_stub(&gone).unwrap();
        let (left, _) = pf.load();
        assert_eq!(left.len(), 1);
        let doc: Value = serde_json::from_str(&fs::read_to_string(pf.mappings.join("m.json")).unwrap()).unwrap();
        assert_eq!(doc["meta"]["x"], 1);
    }

    #[test]
    fn body_files_are_path_safe() {
        let (_d, pf) = project();
        pf.write_file("users/1.json", b"{\"id\":1}").unwrap();
        assert_eq!(pf.read_body_file("users/1.json").unwrap(), b"{\"id\":1}");
        assert_eq!(pf.list_files(), vec!["users/1.json".to_string()]);
        assert!(pf.read_body_file("../secret").is_err());
        assert!(pf.write_file("../../evil", b"x").is_err());
        assert!(pf.delete_file("users/1.json").unwrap());
        assert!(!pf.delete_file("users/1.json").unwrap());
    }
}
