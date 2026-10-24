//! Secret storage. Project files only ever contain secret *names*; values are looked up here.
//!
//! Lookup order for `{{secret.NAME}}` with active environment `env`:
//! 1. keychain entry for (project, env, NAME)
//! 2. keychain entry for (project, `*`, NAME) — project-wide secrets
//! 3. `.env.local` then `.env` in the project root (both git-ignored)

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use indexmap::IndexMap;

use crate::error::Result;
use crate::vars::SecretLookup;

/// Environment id used for secrets shared by every environment of a project.
pub const ALL_ENVIRONMENTS: &str = "*";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretScope {
    pub project_id: String,
    /// Environment id, or [`ALL_ENVIRONMENTS`].
    pub environment: String,
}

impl SecretScope {
    pub fn new(project_id: impl Into<String>, environment: impl Into<String>) -> Self {
        Self { project_id: project_id.into(), environment: environment.into() }
    }

    /// Account name used in the OS credential store.
    pub fn account(&self, name: &str) -> String {
        format!("{}/{}/{}", self.project_id, self.environment, name)
    }
}

pub trait SecretStore: Send + Sync {
    fn get(&self, scope: &SecretScope, name: &str) -> Result<Option<String>>;
    fn set(&self, scope: &SecretScope, name: &str, value: &str) -> Result<()>;
    fn delete(&self, scope: &SecretScope, name: &str) -> Result<()>;
}

/// In-memory store for tests and for running without a keychain (e.g. headless CI).
#[derive(Default)]
pub struct MemorySecretStore {
    values: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemorySecretStore {
    fn get(&self, scope: &SecretScope, name: &str) -> Result<Option<String>> {
        Ok(self.values.lock().unwrap().get(&scope.account(name)).cloned())
    }
    fn set(&self, scope: &SecretScope, name: &str, value: &str) -> Result<()> {
        self.values.lock().unwrap().insert(scope.account(name), value.to_string());
        Ok(())
    }
    fn delete(&self, scope: &SecretScope, name: &str) -> Result<()> {
        self.values.lock().unwrap().remove(&scope.account(name));
        Ok(())
    }
}

/// macOS Keychain / Windows Credential Manager / Secret Service via the `keyring` crate.
#[cfg(feature = "keychain")]
pub struct KeychainSecretStore {
    service: String,
}

#[cfg(feature = "keychain")]
impl KeychainSecretStore {
    pub const DEFAULT_SERVICE: &'static str = "dev.devall.secrets";

    pub fn new() -> Self {
        Self { service: Self::DEFAULT_SERVICE.into() }
    }

    pub fn with_service(service: impl Into<String>) -> Self {
        Self { service: service.into() }
    }

    /// Whether the platform credential store could be initialized.
    pub fn available() -> bool {
        keyring::Entry::store_status().is_ok()
    }

    fn entry(&self, scope: &SecretScope, name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &scope.account(name)).map_err(|e| crate::Error::Secret(e.to_string()))
    }
}

#[cfg(feature = "keychain")]
impl Default for KeychainSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "keychain")]
impl SecretStore for KeychainSecretStore {
    fn get(&self, scope: &SecretScope, name: &str) -> Result<Option<String>> {
        match self.entry(scope, name)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(crate::Error::Secret(e.to_string())),
        }
    }
    fn set(&self, scope: &SecretScope, name: &str, value: &str) -> Result<()> {
        self.entry(scope, name)?.set_password(value).map_err(|e| crate::Error::Secret(e.to_string()))
    }
    fn delete(&self, scope: &SecretScope, name: &str) -> Result<()> {
        match self.entry(scope, name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(crate::Error::Secret(e.to_string())),
        }
    }
}

/// Reads `.env.local` and `.env` from `root` (`.env.local` wins). Missing files are fine.
pub fn load_dotenv(root: &Path) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for file in [".env.local", ".env"] {
        let Ok(iter) = dotenvy::from_path_iter(root.join(file)) else { continue };
        for (k, v) in iter.flatten() {
            out.entry(k).or_insert(v);
        }
    }
    out
}

/// [`SecretLookup`] over: keychain (environment scope, then all-environments scope) → the
/// environment's secret-manager reference (`secretSources`) → `.env.local` / `.env`.
pub struct SecretResolver<'a> {
    store: &'a dyn SecretStore,
    scopes: Vec<SecretScope>,
    dotenv: IndexMap<String, String>,
    sources: IndexMap<String, String>,
    cache: Option<&'a crate::providers::ProviderCache>,
    /// Store errors encountered during lookup (e.g. a locked keychain), for diagnostics.
    pub errors: Mutex<Vec<String>>,
}

impl<'a> SecretResolver<'a> {
    pub fn new(store: &'a dyn SecretStore, project_id: &str, environment: Option<&str>, project_root: &Path) -> Self {
        let mut scopes = Vec::new();
        if let Some(env) = environment.filter(|e| !e.is_empty()) {
            scopes.push(SecretScope::new(project_id, env));
        }
        scopes.push(SecretScope::new(project_id, ALL_ENVIRONMENTS));
        Self { store, scopes, dotenv: load_dotenv(project_root), sources: IndexMap::new(), cache: None, errors: Mutex::new(Vec::new()) }
    }

    /// Resolver for an environment, including its secret-manager references when `env` is given.
    pub fn for_environment(
        store: &'a dyn SecretStore,
        project_id: &str,
        env_id: Option<&str>,
        project_root: &Path,
        env: Option<&crate::model::EnvironmentFile>,
        cache: &'a crate::providers::ProviderCache,
    ) -> Self {
        let r = Self::new(store, project_id, env_id, project_root);
        match env {
            Some(e) if !e.secret_sources.is_empty() => r.with_sources(e.secret_sources.clone(), cache),
            _ => r,
        }
    }

    /// Enables secret-manager references (`op://…`, `aws-sm://…`) from an environment file.
    pub fn with_sources(mut self, sources: IndexMap<String, String>, cache: &'a crate::providers::ProviderCache) -> Self {
        self.sources = sources;
        self.cache = Some(cache);
        self
    }

    /// Where a secret would currently come from, without revealing (or fetching) it.
    pub fn source_of(&self, name: &str) -> Option<&'static str> {
        for s in &self.scopes {
            if let Ok(Some(_)) = self.store.get(s, name) {
                return Some(if s.environment == ALL_ENVIRONMENTS { "keychain (all environments)" } else { "keychain" });
            }
        }
        if let Some(r) = self.sources.get(name) {
            let scheme = r.split("://").next().unwrap_or("");
            return Some(crate::providers::PROVIDERS.iter().find(|p| p.scheme == scheme).map(|p| p.name).unwrap_or("secret manager"));
        }
        self.dotenv.contains_key(name).then_some(".env")
    }
}

impl SecretLookup for SecretResolver<'_> {
    fn lookup(&self, name: &str) -> Option<String> {
        for s in &self.scopes {
            match self.store.get(s, name) {
                Ok(Some(v)) => return Some(v),
                Ok(None) => {}
                Err(e) => self.errors.lock().unwrap().push(e.to_string()),
            }
        }
        if let (Some(r), Some(cache)) = (self.sources.get(name), self.cache) {
            match cache.get(r) {
                Ok(v) => return Some(v),
                Err(e) => self.errors.lock().unwrap().push(format!("{name}: {e}")),
            }
        }
        self.dotenv.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "A=from-dotenv\nB=from-dotenv\nC=dotenv\n").unwrap();
        std::fs::write(dir.path().join(".env.local"), "C=dotenv-local\n").unwrap();
        let store = MemorySecretStore::default();
        store.set(&SecretScope::new("p", "staging"), "A", "env-scoped").unwrap();
        store.set(&SecretScope::new("p", ALL_ENVIRONMENTS), "B", "project-wide").unwrap();
        let r = SecretResolver::new(&store, "p", Some("staging"), dir.path());
        assert_eq!(r.lookup("A").as_deref(), Some("env-scoped"));
        assert_eq!(r.lookup("B").as_deref(), Some("project-wide"));
        assert_eq!(r.lookup("C").as_deref(), Some("dotenv-local"));
        assert_eq!(r.lookup("D"), None);
        assert_eq!(r.source_of("A"), Some("keychain"));
        assert_eq!(r.source_of("C"), Some(".env"));

        // Another environment doesn't see staging's value.
        let r = SecretResolver::new(&store, "p", Some("prod"), dir.path());
        assert_eq!(r.lookup("A").as_deref(), Some("from-dotenv"));
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;

    #[test]
    fn provider_sources_resolve_after_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemorySecretStore::default();
        // SAFETY: test-only variable.
        unsafe { std::env::set_var("DEVALL_SRC_TEST", "from-env-provider") };
        let cache = crate::providers::ProviderCache::default();
        let sources: IndexMap<String, String> = [("TOKEN".to_string(), "env://DEVALL_SRC_TEST".to_string())].into_iter().collect();
        let r = SecretResolver::new(&store, "p", Some("dev"), dir.path()).with_sources(sources.clone(), &cache);
        assert_eq!(r.lookup("TOKEN").as_deref(), Some("from-env-provider"));
        assert_eq!(r.source_of("TOKEN"), Some("Environment variable"));
        store.set(&SecretScope::new("p", "dev"), "TOKEN", "keychain-wins").unwrap();
        let r = SecretResolver::new(&store, "p", Some("dev"), dir.path()).with_sources(sources, &cache);
        assert_eq!(r.lookup("TOKEN").as_deref(), Some("keychain-wins"));
    }
}
