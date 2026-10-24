//! What a cell sees of the project: variables as environment variables, and opt-in templating.
//!
//! - Always: the project's and the active environment's **non-secret** variables (resolved, the
//!   environment overriding the project) as `DEVALL_<name>` (characters outside `[A-Za-z0-9_]`
//!   become `_`; case is kept, so `{{baseUrl}}` is `$DEVALL_baseUrl`), plus `DEVALL_ENV` (the
//!   environment id), `DEVALL_PROJECT_ROOT` and `DEVALL_NOTEBOOK`.
//! - Only when the cell opts in (`metadata.devall.templating: true`): `{{variables}}` in the code
//!   are substituted before it runs.
//! - Secrets only when the user enabled them for that cell (a user-level grant, see
//!   [`crate::trust`]): `{{secret.NAME}}` resolves while templating, and each secret declared by
//!   the environment is injected as `DEVALL_SECRET_<NAME>` — the same variable the `devall` CLI
//!   reads, so `devall run` inside a cell just works. Every secret value used is returned for
//!   redaction from the cell's output.

use std::collections::BTreeSet;
use std::path::Path;

use devall_core::vars::{NoSecrets, SecretLookup};
use devall_core::{Project, Resolver};
use indexmap::IndexMap;

use crate::error::Result;

/// Code and environment ready to run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreparedCell {
    pub code: String,
    pub env: Vec<(String, String)>,
    /// Secret values that went into the code or environment (redact them from outputs).
    pub secret_values: BTreeSet<String>,
    /// `{{references}}` that could not be resolved while templating.
    pub unresolved: BTreeSet<String>,
}

/// Options for [`prepare`].
#[derive(Default, Clone, Copy)]
pub struct PrepareOptions<'a> {
    /// Active environment id (file stem in `environments/`); unknown ids are ignored.
    pub environment: Option<&'a str>,
    /// Substitute `{{variables}}` in the code.
    pub templating: bool,
    /// Secret source; `None` keeps secrets out entirely.
    pub secrets: Option<&'a dyn SecretLookup>,
    /// Notebook / note path (relative to `notes/`), exported as `DEVALL_NOTEBOOK`.
    pub notebook: Option<&'a str>,
}

/// `DEVALL_` + the name with every character outside `[A-Za-z0-9_]` replaced by `_`.
pub fn env_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    format!("DEVALL_{clean}")
}

pub fn prepare(project: &Project, code: &str, opts: PrepareOptions<'_>) -> Result<PreparedCell> {
    let env_id = opts.environment.filter(|e| !e.is_empty());
    let env_file = env_id.and_then(|id| project.read_environment(id).ok());
    let no_secrets = NoSecrets;
    let lookup: &dyn SecretLookup = opts.secrets.unwrap_or(&no_secrets);
    let mut r = Resolver::new(lookup);
    if let Some(e) = &env_file {
        r.push_layer(e.variables.clone());
    }
    r.push_layer(project.manifest.variables.clone());

    // Environment overrides project (insertion keeps the project's order first).
    let mut merged: IndexMap<String, String> = project.manifest.variables.clone();
    if let Some(e) = &env_file {
        for (k, v) in &e.variables {
            merged.insert(k.clone(), v.clone());
        }
    }
    let mut env: Vec<(String, String)> = Vec::new();
    for (name, raw) in &merged {
        env.push((env_name(name), r.resolve(raw)));
    }
    if opts.secrets.is_some()
        && let Some(e) = &env_file
    {
        for name in &e.secrets {
            if let Some(v) = lookup.lookup(name) {
                if !v.is_empty() {
                    r.secret_values.insert(v.clone());
                }
                env.push((format!("DEVALL_SECRET_{}", env_name(name).trim_start_matches("DEVALL_")), v));
            }
        }
    }
    if let Some(id) = env_id.filter(|_| env_file.is_some()) {
        env.push(("DEVALL_ENV".into(), id.to_string()));
    }
    env.push(("DEVALL_PROJECT_ROOT".into(), path_string(project.root())));
    if let Some(nb) = opts.notebook {
        env.push(("DEVALL_NOTEBOOK".into(), nb.to_string()));
    }

    // Values that themselves referenced secrets must not leak into a non-secret run.
    let code = if opts.templating {
        r.unresolved.clear();
        r.resolve(code)
    } else {
        code.to_string()
    };
    Ok(PreparedCell { code, env, secret_values: r.secret_values.clone(), unresolved: r.unresolved.clone() })
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use devall_core::model::EnvironmentFile;

    struct S;
    impl SecretLookup for S {
        fn lookup(&self, name: &str) -> Option<String> {
            (name == "API_TOKEN").then(|| "s3cr3t-token".to_string())
        }
    }

    #[test]
    fn variables_templating_and_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::init(dir.path(), "Env").unwrap();
        p.manifest.variables.insert("baseUrl".into(), "http://project".into());
        p.manifest.variables.insert("api-version".into(), "v1".into());
        p.save_manifest().unwrap();
        let mut e = EnvironmentFile::new("Staging");
        e.variables.insert("baseUrl".into(), "https://staging.example".into());
        e.variables.insert("auth".into(), "Bearer {{secret.API_TOKEN}}".into());
        e.secrets.push("API_TOKEN".into());
        p.save_environment("staging", &e).unwrap();

        let code = "curl {{baseUrl}}/{{api-version}} -H '{{secret.API_TOKEN}}'";
        let plain = prepare(&p, code, PrepareOptions { environment: Some("staging"), ..Default::default() }).unwrap();
        assert_eq!(plain.code, code, "no templating unless the cell opts in");
        let get = |c: &PreparedCell, k: &str| c.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get(&plain, "DEVALL_baseUrl").as_deref(), Some("https://staging.example"));
        assert_eq!(get(&plain, "DEVALL_api_version").as_deref(), Some("v1"));
        assert_eq!(get(&plain, "DEVALL_ENV").as_deref(), Some("staging"));
        assert_eq!(get(&plain, "DEVALL_auth").as_deref(), Some("Bearer {{secret.API_TOKEN}}"), "secrets stay out");
        assert!(get(&plain, "DEVALL_SECRET_API_TOKEN").is_none());
        assert!(plain.secret_values.is_empty());

        let tpl = prepare(&p, code, PrepareOptions { environment: Some("staging"), templating: true, ..Default::default() }).unwrap();
        assert_eq!(tpl.code, "curl https://staging.example/v1 -H '{{secret.API_TOKEN}}'");
        assert!(tpl.unresolved.contains("secret.API_TOKEN"));

        let sec = prepare(&p, code, PrepareOptions { environment: Some("staging"), templating: true, secrets: Some(&S), notebook: Some("a.ipynb") }).unwrap();
        assert_eq!(sec.code, "curl https://staging.example/v1 -H 's3cr3t-token'");
        assert_eq!(get(&sec, "DEVALL_SECRET_API_TOKEN").as_deref(), Some("s3cr3t-token"));
        assert_eq!(get(&sec, "DEVALL_NOTEBOOK").as_deref(), Some("a.ipynb"));
        assert!(sec.secret_values.contains("s3cr3t-token"));

        let none = prepare(&p, "x", PrepareOptions { environment: Some("missing"), ..Default::default() }).unwrap();
        assert_eq!(get(&none, "DEVALL_baseUrl").as_deref(), Some("http://project"));
        assert!(get(&none, "DEVALL_ENV").is_none());
    }
}
