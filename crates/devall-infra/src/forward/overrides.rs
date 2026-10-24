//! Runtime environment-variable overrides set by running forwards.
//!
//! The HTTP send path adds [`RuntimeOverrides::variables`] as a resolver layer right below the
//! captured runtime values and above the environment file, so `{{baseUrl}}` follows the forward
//! while it runs and falls back to the file as soon as it stops.

use std::sync::{Arc, RwLock};

use indexmap::IndexMap;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverrideEntry {
    /// Project root the forward was started for.
    pub root: String,
    /// Environment id; `None` = applies to whichever environment is active.
    pub environment: Option<String>,
    pub variable: String,
    pub value: String,
    pub forward_id: String,
    pub forward_name: String,
    /// Unix ms when set; the newest override wins on conflicts.
    pub since: i64,
}

/// Cheap to clone; all clones share the same table.
#[derive(Debug, Clone, Default)]
pub struct RuntimeOverrides {
    inner: Arc<RwLock<Vec<OverrideEntry>>>,
}

/// The process-wide table: what the app's forward manager writes and the HTTP send path reads,
/// without threading state through every resolver call site.
pub fn global_overrides() -> &'static RuntimeOverrides {
    static GLOBAL: std::sync::OnceLock<RuntimeOverrides> = std::sync::OnceLock::new();
    GLOBAL.get_or_init(RuntimeOverrides::new)
}

/// Project roots compare without trailing separators, with `/` ≡ `\`, and case-insensitively on
/// Windows, so the UI and the send path can't disagree on spelling.
pub fn same_root(a: &str, b: &str) -> bool {
    norm_root(a) == norm_root(b)
}

pub(crate) fn norm_root(s: &str) -> String {
    let s = s.replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

impl RuntimeOverrides {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets (or replaces) the override of `entry.forward_id` in `entry.root`.
    pub fn set(&self, entry: OverrideEntry) {
        let mut v = self.inner.write().unwrap();
        v.retain(|e| !(same_root(&e.root, &entry.root) && e.forward_id == entry.forward_id));
        v.push(entry);
    }

    /// Removes the override of a forward; returns whether there was one.
    pub fn clear(&self, root: &str, forward_id: &str) -> bool {
        let mut v = self.inner.write().unwrap();
        let before = v.len();
        v.retain(|e| !(same_root(&e.root, root) && e.forward_id == forward_id));
        v.len() != before
    }

    pub fn get(&self, root: &str, forward_id: &str) -> Option<OverrideEntry> {
        self.inner.read().unwrap().iter().find(|e| same_root(&e.root, root) && e.forward_id == forward_id).cloned()
    }

    /// Every override (of `root`, when given), newest last.
    pub fn list(&self, root: Option<&str>) -> Vec<OverrideEntry> {
        let mut v: Vec<OverrideEntry> = self.inner.read().unwrap().iter().filter(|e| root.is_none_or(|r| same_root(&e.root, r))).cloned().collect();
        v.sort_by_key(|e| e.since);
        v
    }

    /// The variable layer for a send in `root` with environment `env` active. Overrides bound to
    /// that environment beat unbound ones; among equals the newest wins.
    pub fn variables(&self, root: &str, env: Option<&str>) -> IndexMap<String, String> {
        let env = env.filter(|e| !e.is_empty());
        let mut matching: Vec<OverrideEntry> = self
            .list(Some(root))
            .into_iter()
            .filter(|e| match (&e.environment, env) {
                (None, _) => true,
                (Some(want), Some(active)) => want == active,
                (Some(_), None) => false,
            })
            .collect();
        // Apply in ascending priority so later inserts overwrite earlier ones.
        matching.sort_by_key(|e| (e.environment.is_some(), e.since));
        let mut out = IndexMap::new();
        for e in matching {
            out.insert(e.variable, e.value);
        }
        out
    }

    /// Which forward currently provides `variable` (for the variable catalog's "source" column).
    pub fn source_of(&self, root: &str, env: Option<&str>, variable: &str) -> Option<String> {
        let env = env.filter(|e| !e.is_empty());
        self.list(Some(root))
            .into_iter()
            .filter(|e| e.variable == variable && (e.environment.is_none() || e.environment.as_deref() == env))
            .max_by_key(|e| (e.environment.is_some(), e.since))
            .map(|e| e.forward_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(root: &str, env: Option<&str>, var: &str, value: &str, id: &str, since: i64) -> OverrideEntry {
        OverrideEntry {
            root: root.into(),
            environment: env.map(str::to_string),
            variable: var.into(),
            value: value.into(),
            forward_id: id.into(),
            forward_name: format!("fwd {id}"),
            since,
        }
    }

    #[test]
    fn layering_rules() {
        let o = RuntimeOverrides::new();
        o.set(entry("/p", Some("local"), "baseUrl", "http://127.0.0.1:18080", "api", 1));
        o.set(entry("/p", None, "authUrl", "http://127.0.0.1:19000", "auth", 2));
        o.set(entry("/p", Some("staging"), "baseUrl", "http://127.0.0.1:28080", "api-stg", 3));
        o.set(entry("/other", Some("local"), "baseUrl", "http://127.0.0.1:1", "x", 4));

        let local = o.variables("/p", Some("local"));
        assert_eq!(local.get("baseUrl").unwrap(), "http://127.0.0.1:18080");
        assert_eq!(local.get("authUrl").unwrap(), "http://127.0.0.1:19000");
        let stg = o.variables("/p/", Some("staging"));
        assert_eq!(stg.get("baseUrl").unwrap(), "http://127.0.0.1:28080");
        // No environment active: only unbound overrides apply.
        let none = o.variables("/p", None);
        assert_eq!(none.len(), 1);
        assert!(none.contains_key("authUrl"));
        assert_eq!(o.source_of("/p", Some("local"), "baseUrl").as_deref(), Some("fwd api"));
    }

    #[test]
    fn env_specific_beats_unbound_and_newest_wins() {
        let o = RuntimeOverrides::new();
        o.set(entry("/p", Some("local"), "baseUrl", "specific", "a", 1));
        o.set(entry("/p", None, "baseUrl", "unbound-newer", "b", 5));
        assert_eq!(o.variables("/p", Some("local")).get("baseUrl").unwrap(), "specific");
        o.set(entry("/p", Some("local"), "baseUrl", "specific-newer", "c", 6));
        assert_eq!(o.variables("/p", Some("local")).get("baseUrl").unwrap(), "specific-newer");
        // Stopping the newest falls back to the previous one.
        assert!(o.clear("/p", "c"));
        assert_eq!(o.variables("/p", Some("local")).get("baseUrl").unwrap(), "specific");
        assert!(!o.clear("/p", "c"));
    }

    #[test]
    fn replacing_and_root_spelling() {
        let o = RuntimeOverrides::new();
        o.set(entry("C:\\work\\shop", Some("local"), "baseUrl", "v1", "api", 1));
        o.set(entry("C:/work/shop/", Some("local"), "baseUrl", "v2", "api", 2));
        assert_eq!(o.list(None).len(), 1, "same forward replaces its override");
        assert_eq!(o.variables("C:\\work\\shop\\", Some("local")).get("baseUrl").unwrap(), "v2");
        assert!(o.get("C:/work/shop", "api").is_some());
        // Clones share state.
        let c = o.clone();
        c.clear("C:/work/shop", "api");
        assert!(o.list(None).is_empty());
    }
}
