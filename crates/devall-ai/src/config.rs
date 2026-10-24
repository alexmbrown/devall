//! User-level AI settings: `<app data>/ai.json` (never inside a project, so never committed).
//! API keys are stored in the OS keychain (`devall_core::SecretStore`) under the account
//! `_devall-ai/providers/<provider id>`; the file only records *that* a key exists.

use std::path::{Path, PathBuf};

use devall_core::secrets::SecretScope;
use devall_core::{SecretStore, fsutil::atomic_write};
use serde::{Deserialize, Serialize};

use crate::provider::{Provider, ProviderConfig};
use crate::{AiError, Result};

pub const CONFIG_FILE: &str = "ai.json";
const KEY_PROJECT: &str = "_devall-ai";
const KEY_ENV: &str = "providers";

fn v1() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    #[serde(default = "v1")]
    pub version: u32,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Provider used when a feature doesn't name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    /// Allow notes and personal to-dos to be sent to non-local (cloud) providers. Off by default:
    /// project knowledge stays on the machine unless the user opts in.
    #[serde(default)]
    pub allow_remote_context: bool,
    /// Model used for local semantic search (unset: the managed embedding model when running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<crate::embed::EmbeddingSettings>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self { version: 1, providers: Vec::new(), default_provider: None, allow_remote_context: false, embeddings: None }
    }
}

impl AiConfig {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(CONFIG_FILE)
    }

    /// Reads `<dir>/ai.json`; a missing file is an empty configuration.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| AiError::Invalid(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(AiError::Io { path, source: e }),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| AiError::Invalid(e.to_string()))?;
        atomic_write(&Self::path(dir), &json)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Inserts or replaces (by id). The first provider becomes the default.
    pub fn upsert(&mut self, cfg: ProviderConfig) {
        match self.providers.iter_mut().find(|p| p.id == cfg.id) {
            Some(existing) => *existing = cfg,
            None => {
                if self.default_provider.is_none() {
                    self.default_provider = Some(cfg.id.clone());
                }
                self.providers.push(cfg);
            }
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.providers.len();
        self.providers.retain(|p| p.id != id);
        if self.default_provider.as_deref() == Some(id) {
            self.default_provider = self.providers.first().map(|p| p.id.clone());
        }
        self.providers.len() != before
    }

    /// The named provider, else the default, else the first one.
    pub fn resolve(&self, id: Option<&str>) -> Result<&ProviderConfig> {
        match id.filter(|s| !s.is_empty()) {
            Some(id) => self.get(id).ok_or_else(|| AiError::UnknownProvider(id.to_string())),
            None => self
                .default_provider
                .as_deref()
                .and_then(|d| self.get(d))
                .or_else(|| self.providers.first())
                .ok_or(AiError::NoProvider),
        }
    }
}

fn key_scope() -> SecretScope {
    SecretScope::new(KEY_PROJECT, KEY_ENV)
}

pub fn store_api_key(store: &dyn SecretStore, provider_id: &str, key: &str) -> Result<()> {
    store.set(&key_scope(), provider_id, key.trim())?;
    Ok(())
}

pub fn delete_api_key(store: &dyn SecretStore, provider_id: &str) -> Result<()> {
    store.delete(&key_scope(), provider_id)?;
    Ok(())
}

pub fn load_api_key(store: &dyn SecretStore, provider_id: &str) -> Result<Option<String>> {
    Ok(store.get(&key_scope(), provider_id)?)
}

/// Builds a ready-to-call [`Provider`] for a configured entry (loading its key when it has one).
pub fn provider(cfg: &ProviderConfig, store: &dyn SecretStore) -> Result<Provider> {
    let key = if cfg.has_api_key { load_api_key(store, &cfg.id)? } else { None };
    Provider::new(cfg.clone(), key)
}

/// Saves a provider: validates it, stores a new API key in the keychain (`Some("")` deletes the
/// stored key, `None` keeps whatever is there) and writes `ai.json`.
pub fn save_provider(dir: &Path, store: &dyn SecretStore, mut cfg: ProviderConfig, api_key: Option<String>) -> Result<AiConfig> {
    cfg.validate()?;
    let mut config = AiConfig::load(dir)?;
    let had_key = config.get(&cfg.id).is_some_and(|p| p.has_api_key);
    match api_key.as_deref().map(str::trim) {
        Some("") => {
            delete_api_key(store, &cfg.id)?;
            cfg.has_api_key = false;
        }
        Some(k) => {
            store_api_key(store, &cfg.id, k)?;
            cfg.has_api_key = true;
        }
        None => cfg.has_api_key = had_key,
    }
    config.upsert(cfg);
    config.save(dir)?;
    Ok(config)
}

/// Removes a provider and its keychain entry.
pub fn remove_provider(dir: &Path, store: &dyn SecretStore, id: &str) -> Result<AiConfig> {
    let mut config = AiConfig::load(dir)?;
    config.remove(id);
    let _ = delete_api_key(store, id);
    config.save(dir)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderKind;
    use devall_core::MemorySecretStore;

    #[test]
    fn keys_live_in_the_store_not_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemorySecretStore::default();
        let cfg = ProviderConfig::new("openrouter", "OpenRouter", ProviderKind::OpenAiCompatible, "https://openrouter.ai/api/v1", "qwen/qwen3.8-27b");
        let saved = save_provider(dir.path(), &store, cfg.clone(), Some("sk-or-v1-supersecretkey".into())).unwrap();
        assert!(saved.providers[0].has_api_key);
        assert_eq!(saved.default_provider.as_deref(), Some("openrouter"));
        let on_disk = std::fs::read_to_string(AiConfig::path(dir.path())).unwrap();
        assert!(!on_disk.contains("supersecretkey"), "{on_disk}");
        assert_eq!(load_api_key(&store, "openrouter").unwrap().as_deref(), Some("sk-or-v1-supersecretkey"));

        // Saving without a key keeps the stored one; an empty key deletes it.
        let again = save_provider(dir.path(), &store, cfg.clone(), None).unwrap();
        assert!(again.providers[0].has_api_key);
        let cleared = save_provider(dir.path(), &store, cfg.clone(), Some(String::new())).unwrap();
        assert!(!cleared.providers[0].has_api_key);
        assert!(load_api_key(&store, "openrouter").unwrap().is_none());

        let local = ProviderConfig::new("ollama", "Ollama", ProviderKind::Ollama, "http://127.0.0.1:11434", "qwen3.5:9b");
        save_provider(dir.path(), &store, local, None).unwrap();
        let c = AiConfig::load(dir.path()).unwrap();
        assert_eq!(c.resolve(None).unwrap().id, "openrouter");
        assert_eq!(c.resolve(Some("ollama")).unwrap().id, "ollama");
        assert!(matches!(c.resolve(Some("nope")), Err(AiError::UnknownProvider(_))));
        let c = remove_provider(dir.path(), &store, "openrouter").unwrap();
        assert_eq!(c.default_provider.as_deref(), Some("ollama"));
        assert!(!c.allow_remote_context);
        assert!(matches!(AiConfig::default().resolve(None), Err(AiError::NoProvider)));
    }
}
