//! The format-neutral result of parsing an export: what [`crate::apply`] will write.

use indexmap::IndexMap;
use serde::Serialize;

use devall_core::{EnvironmentFile, FolderFile, HttpRequestFile};

use crate::detect::Format;
use crate::util::folder_has_settings;

/// Everything an importer found. Folders and requests form one ordered tree (`items`) so
/// interleaved ordering survives; [`ImportPlan::requests`] and [`ImportPlan::folders`] give flat
/// views with folder paths.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    /// Collection / API / file name; used for the wrapping folder.
    pub name: String,
    pub format: Format,
    /// Collection-level headers, auth and variables (go to the wrapping folder's `_folder.yaml`).
    pub settings: Option<FolderFile>,
    pub items: Vec<PlanItem>,
    pub environments: Vec<EnvironmentFile>,
    pub project_variables: IndexMap<String, String>,
    pub warnings: Vec<String>,
}

// Plans are small trees built once; boxing folders would only add noise to every importer.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PlanItem {
    Folder(PlanFolder),
    Request(Box<HttpRequestFile>),
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFolder {
    pub name: String,
    /// Folder headers / auth / variables. `name` and `order` are ignored (set on apply).
    pub settings: Option<FolderFile>,
    pub items: Vec<PlanItem>,
}

impl PlanFolder {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), settings: None, items: Vec::new() }
    }
}

impl ImportPlan {
    pub fn new(name: impl Into<String>, format: Format) -> Self {
        Self {
            name: name.into(),
            format,
            settings: None,
            items: Vec::new(),
            environments: Vec::new(),
            project_variables: IndexMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Every request with the names of the folders containing it (outermost first).
    pub fn requests(&self) -> Vec<(Vec<String>, &HttpRequestFile)> {
        fn walk<'a>(items: &'a [PlanItem], path: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'a HttpRequestFile)>) {
            for it in items {
                match it {
                    PlanItem::Request(r) => out.push((path.clone(), r)),
                    PlanItem::Folder(f) => {
                        path.push(f.name.clone());
                        walk(&f.items, path, out);
                        path.pop();
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut Vec::new(), &mut out);
        out
    }

    /// Every folder with its path (including its own name).
    pub fn folders(&self) -> Vec<(Vec<String>, &PlanFolder)> {
        fn walk<'a>(items: &'a [PlanItem], path: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'a PlanFolder)>) {
            for it in items {
                if let PlanItem::Folder(f) = it {
                    path.push(f.name.clone());
                    out.push((path.clone(), f));
                    walk(&f.items, path, out);
                    path.pop();
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut Vec::new(), &mut out);
        out
    }

    pub fn request_count(&self) -> usize {
        self.requests().len()
    }

    pub fn folder_count(&self) -> usize {
        self.folders().len()
    }

    pub(crate) fn visit_requests_mut(&mut self, f: &mut dyn FnMut(&mut HttpRequestFile)) {
        fn walk(items: &mut [PlanItem], f: &mut dyn FnMut(&mut HttpRequestFile)) {
            for it in items {
                match it {
                    PlanItem::Request(r) => f(r),
                    PlanItem::Folder(x) => walk(&mut x.items, f),
                }
            }
        }
        walk(&mut self.items, f);
    }

    /// Visits the collection settings and every folder's settings, with a display label.
    pub(crate) fn visit_settings_mut(&mut self, f: &mut dyn FnMut(&str, &mut FolderFile)) {
        fn walk(items: &mut [PlanItem], f: &mut dyn FnMut(&str, &mut FolderFile)) {
            for it in items {
                if let PlanItem::Folder(x) = it {
                    if let Some(s) = &mut x.settings {
                        f(&x.name, s);
                    }
                    walk(&mut x.items, f);
                }
            }
        }
        if let Some(s) = &mut self.settings {
            f("collection", s);
        }
        walk(&mut self.items, f);
    }

    /// A UI-friendly preview: tree, environments, variable names, secret candidates (names
    /// only, never values) and warnings.
    pub fn summary(&self) -> PlanSummary {
        let mut probe = self.clone();
        let secrets = crate::secrets::extract(&mut probe);
        let env_names: Vec<String> = self.environments.iter().map(|e| e.name.clone()).collect();
        let mut secret_names: Vec<SecretCandidate> = secrets
            .iter()
            .map(|s| SecretCandidate {
                name: s.name.clone(),
                scope: match s.scope {
                    crate::secrets::Scope::Env(i) => env_names.get(i).cloned().unwrap_or_default(),
                    crate::secrets::Scope::All => String::new(),
                },
                source: s.source.clone(),
                has_value: !s.value.is_empty(),
            })
            .collect();
        secret_names.dedup_by(|a, b| a.name == b.name && a.scope == b.scope);
        PlanSummary {
            name: self.name.clone(),
            format: self.format,
            format_label: self.format.label().to_string(),
            request_count: self.request_count(),
            folder_count: self.folder_count(),
            has_settings: self.settings.as_ref().is_some_and(folder_has_settings),
            tree: summarize(&probe.items),
            environments: probe
                .environments
                .iter()
                .map(|e| EnvSummary { name: e.name.clone(), variables: e.variables.keys().cloned().collect(), secrets: e.secrets.clone() })
                .collect(),
            project_variables: probe.project_variables.keys().cloned().collect(),
            secrets: secret_names,
            warnings: self.warnings.clone(),
        }
    }
}

fn summarize(items: &[PlanItem]) -> Vec<SummaryNode> {
    items
        .iter()
        .map(|it| match it {
            PlanItem::Request(r) => SummaryNode::Request { name: r.name.clone(), method: r.method.clone(), url: r.url.clone() },
            PlanItem::Folder(f) => SummaryNode::Folder {
                name: f.name.clone(),
                has_settings: f.settings.as_ref().is_some_and(folder_has_settings),
                children: summarize(&f.items),
            },
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    pub name: String,
    pub format: Format,
    pub format_label: String,
    pub request_count: usize,
    pub folder_count: usize,
    /// Whether collection-level auth/headers/variables exist (they go to the wrapping folder).
    pub has_settings: bool,
    pub tree: Vec<SummaryNode>,
    pub environments: Vec<EnvSummary>,
    pub project_variables: Vec<String>,
    pub secrets: Vec<SecretCandidate>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SummaryNode {
    Folder { name: String, has_settings: bool, children: Vec<SummaryNode> },
    Request { name: String, method: String, url: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvSummary {
    pub name: String,
    /// Variable names (after secret extraction).
    pub variables: Vec<String>,
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretCandidate {
    pub name: String,
    /// Environment name, or empty for every environment.
    pub scope: String,
    pub source: String,
    /// Whether the export contained a value (otherwise the user has to set it).
    pub has_value: bool,
}
