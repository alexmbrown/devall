//! Shared helpers for the integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use devall_core::{EnvironmentEntry, HttpRequestFile, Project};
use devall_import::{ApplyOptions, ApplyReport, ParseOptions, apply, parse_path};

pub fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(rel)
}

pub struct Imported {
    pub dir: tempfile::TempDir,
    pub project: Project,
    pub report: ApplyReport,
}

pub fn new_project() -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::init(dir.path(), "Test project").unwrap();
    (dir, project)
}

/// Parses `path` and applies it into a fresh project with default options.
pub fn import(path: &Path, opts: &ParseOptions) -> Imported {
    let plan = parse_path(path, opts).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (dir, mut project) = new_project();
    let report = apply(plan, &mut project, "", &ApplyOptions::default()).unwrap();
    Imported { dir, project, report }
}

/// Every request in tree order, with its path.
pub fn requests_of(project: &Project) -> Vec<(String, HttpRequestFile)> {
    project.tree().unwrap().request_paths().into_iter().map(|p| (p.clone(), project.read_request(&p).unwrap())).collect()
}

impl Imported {
    pub fn requests(&self) -> Vec<(String, HttpRequestFile)> {
        requests_of(&self.project)
    }

    /// The single request with this name.
    pub fn request(&self, name: &str) -> (String, HttpRequestFile) {
        let mut found: Vec<(String, HttpRequestFile)> = self.requests().into_iter().filter(|(_, r)| r.name == name).collect();
        assert_eq!(found.len(), 1, "expected one request named {name:?}, have {:?}", self.requests().iter().map(|(_, r)| r.name.clone()).collect::<Vec<_>>());
        found.remove(0)
    }

    pub fn env(&self, name: &str) -> EnvironmentEntry {
        let envs = self.project.environments().unwrap();
        let names: Vec<String> = envs.iter().map(|e| e.file.name.clone()).collect();
        envs.into_iter().find(|e| e.file.name == name).unwrap_or_else(|| panic!("no environment {name:?} in {names:?}"))
    }

    /// No project file contains any of `values`.
    pub fn assert_not_on_disk(&self, values: &[&str]) {
        for file in walk(self.dir.path()) {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            for v in values {
                assert!(!text.contains(v), "{v:?} leaked into {}:\n{text}", file.display());
            }
        }
    }
}

pub fn walk(dir: &Path) -> Vec<PathBuf> {
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
