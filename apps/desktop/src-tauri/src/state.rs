use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use devall_core::secrets::SecretScope;
use devall_core::{KeychainSecretStore, MemorySecretStore, SecretStore, Variables};
use devall_history::History;
use devall_http::ClientPool;
use devall_http::oauth2::TokenStore;
use serde::{Deserialize, Serialize};

pub struct AppState {
    pub secrets: Box<dyn SecretStore>,
    pub keychain_available: bool,
    /// Secret-manager CLI results, cached for 10 minutes.
    pub providers: devall_core::providers::ProviderCache,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    /// HTTP clients + cookie jar per project root (cookies don't leak between projects).
    pools: Mutex<HashMap<String, ClientPool>>,
    histories: Mutex<HashMap<String, Arc<History>>>,
    /// Runtime variables (captured values) per project root, for this app session.
    pub runtime: Mutex<HashMap<String, Variables>>,
    /// In-flight sends by client-chosen id, so the UI can cancel them.
    pub in_flight: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    /// Cancel flags for collection runs by run id.
    pub runs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    recent_lock: Mutex<()>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    pub opened_at: i64,
}

const MAX_RECENT: usize = 20;

impl AppState {
    pub fn new(config_dir: PathBuf, data_dir: PathBuf) -> Self {
        let keychain_available = KeychainSecretStore::available();
        let secrets: Box<dyn SecretStore> =
            if keychain_available { Box::new(KeychainSecretStore::new()) } else { Box::new(MemorySecretStore::default()) };
        Self {
            secrets,
            keychain_available,
            providers: Default::default(),
            config_dir,
            data_dir,
            pools: Mutex::new(HashMap::new()),
            histories: Mutex::new(HashMap::new()),
            runtime: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            recent_lock: Mutex::new(()),
        }
    }

    pub fn pool(&self, root: &str) -> ClientPool {
        self.pools.lock().unwrap().entry(root.to_string()).or_default().clone()
    }

    pub fn history(&self, root: &str) -> Result<Arc<History>, String> {
        let mut map = self.histories.lock().unwrap();
        if let Some(h) = map.get(root) {
            return Ok(h.clone());
        }
        let h = Arc::new(
            History::open_project(std::path::Path::new(root))
                .map_err(|e| e.to_string())?
                .with_retention(devall_core::local::load_settings().responses_per_request),
        );
        map.insert(root.to_string(), h.clone());
        Ok(h)
    }

    /// Applies a new "responses per request" setting to every open project's history.
    pub fn set_history_retention(&self, keep_per_request: usize) {
        for h in self.histories.lock().unwrap().values() {
            h.set_retention(keep_per_request);
        }
    }

    pub fn runtime_vars(&self, root: &str) -> Variables {
        self.runtime.lock().unwrap().get(root).cloned().unwrap_or_default()
    }

    pub fn scratch_dir(&self) -> PathBuf {
        self.data_dir.join("scratch")
    }

    fn recent_path(&self) -> PathBuf {
        self.config_dir.join("recent.json")
    }

    pub fn recent(&self) -> Vec<RecentProject> {
        std::fs::read(self.recent_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn touch_recent(&self, path: &str, name: &str) {
        let _g = self.recent_lock.lock().unwrap();
        let mut list = self.recent();
        list.retain(|r| r.path != path);
        list.insert(0, RecentProject { path: path.to_string(), name: name.to_string(), opened_at: now_ms() });
        list.truncate(MAX_RECENT);
        self.write_recent(&list);
    }

    pub fn forget_recent(&self, path: &str) {
        let _g = self.recent_lock.lock().unwrap();
        let mut list = self.recent();
        list.retain(|r| r.path != path);
        self.write_recent(&list);
    }

    fn write_recent(&self, list: &[RecentProject]) {
        if let Ok(json) = serde_json::to_vec_pretty(list) {
            let _ = devall_core::fsutil::atomic_write(&self.recent_path(), &json);
        }
    }
}

/// OAuth2 tokens cached in the keychain under a per-project scope (they are secrets).
pub struct KeychainTokens<'a> {
    pub store: &'a dyn SecretStore,
    pub project_id: String,
}

impl KeychainTokens<'_> {
    fn scope(&self) -> SecretScope {
        SecretScope::new(self.project_id.clone(), "__oauth2")
    }
}

impl TokenStore for KeychainTokens<'_> {
    fn get(&self, key: &str) -> Option<String> {
        self.store.get(&self.scope(), key).ok().flatten()
    }
    fn set(&self, key: &str, value: &str) {
        let _ = self.store.set(&self.scope(), key, value);
    }
    fn delete(&self, key: &str) {
        let _ = self.store.delete(&self.scope(), key);
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
