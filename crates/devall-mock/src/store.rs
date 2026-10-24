//! In-memory stub store with WireMock ordering (priority ascending, most recently added first)
//! and scenario state.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::request::MockRequest;
use crate::stub::Stub;

pub const STARTED: &str = "Started";

#[derive(Debug, Default)]
pub struct StubStore {
    stubs: Vec<Arc<Stub>>,
    next_index: u64,
    scenarios: BTreeMap<String, String>,
}

/// Outcome of matching a request against the store.
#[derive(Debug, Clone)]
pub struct Matched {
    pub stub: Arc<Stub>,
    pub path_params: Vec<(String, String)>,
}

impl StubStore {
    pub fn all(&self) -> &[Arc<Stub>] {
        &self.stubs
    }

    pub fn len(&self) -> usize {
        self.stubs.len()
    }

    pub fn get(&self, id: &Uuid) -> Option<Arc<Stub>> {
        self.stubs.iter().find(|s| s.id == *id).cloned()
    }

    fn sort(&mut self) {
        self.stubs.sort_by(|a, b| a.effective_priority().cmp(&b.effective_priority()).then(b.insertion_index.cmp(&a.insertion_index)));
    }

    /// Adds a stub (replacing one with the same id). New stubs get the next insertion index;
    /// replacements keep their position unless `fresh_index` is set.
    pub fn insert(&mut self, mut stub: Stub, fresh_index: bool) -> Arc<Stub> {
        let existing = self.stubs.iter().position(|s| s.id == stub.id);
        stub.insertion_index = match existing {
            Some(i) if !fresh_index => self.stubs[i].insertion_index,
            _ => {
                self.next_index += 1;
                self.next_index
            }
        };
        if let Some(i) = existing {
            self.stubs.remove(i);
        }
        if let Some(name) = &stub.scenario_name {
            self.scenarios.entry(name.clone()).or_insert_with(|| STARTED.to_string());
        }
        let arc = Arc::new(stub);
        self.stubs.push(arc.clone());
        self.sort();
        self.prune_scenarios();
        arc
    }

    pub fn remove(&mut self, id: &Uuid) -> Option<Arc<Stub>> {
        let i = self.stubs.iter().position(|s| s.id == *id)?;
        let removed = self.stubs.remove(i);
        self.prune_scenarios();
        Some(removed)
    }

    pub fn clear(&mut self) {
        self.stubs.clear();
        self.scenarios.clear();
    }

    fn prune_scenarios(&mut self) {
        let used: std::collections::HashSet<&str> = self.stubs.iter().filter_map(|s| s.scenario_name.as_deref()).collect();
        self.scenarios.retain(|k, _| used.contains(k.as_str()));
    }

    /// First stub (in priority order) whose pattern and scenario state accept the request.
    pub fn find(&self, req: &MockRequest) -> Option<Matched> {
        self.stubs.iter().find_map(|s| {
            if let (Some(name), Some(required)) = (&s.scenario_name, &s.required_state)
                && self.scenarios.get(name).map_or(STARTED, String::as_str) != required
            {
                return None;
            }
            s.request.is_match(req).then(|| Matched { stub: s.clone(), path_params: s.request.path_params(req) })
        })
    }

    /// Applies a matched stub's scenario transition if the scenario is still in the state the
    /// match was made in. Returns `false` if another request moved it first.
    pub fn transition(&mut self, stub: &Stub) -> bool {
        let Some(name) = &stub.scenario_name else { return true };
        let current = self.scenarios.entry(name.clone()).or_insert_with(|| STARTED.to_string());
        if let Some(required) = &stub.required_state
            && current != required
        {
            return false;
        }
        if let Some(next) = &stub.new_state {
            *current = next.clone();
        }
        true
    }

    pub fn scenario_state(&self, name: &str) -> Option<&str> {
        self.scenarios.get(name).map(String::as_str)
    }

    pub fn reset_scenarios(&mut self) {
        for v in self.scenarios.values_mut() {
            *v = STARTED.to_string();
        }
    }

    /// Sets (or with `None`, resets) a scenario's state. `false` when the scenario is unknown.
    pub fn set_scenario_state(&mut self, name: &str, state: Option<String>) -> bool {
        match self.scenarios.get_mut(name) {
            Some(v) => {
                *v = state.unwrap_or_else(|| STARTED.to_string());
                true
            }
            None => false,
        }
    }

    /// `GET /__admin/scenarios` body.
    pub fn scenarios_json(&self) -> Value {
        let list: Vec<Value> = self
            .scenarios
            .iter()
            .map(|(name, state)| {
                let mappings: Vec<&Arc<Stub>> = self.stubs.iter().filter(|s| s.scenario_name.as_deref() == Some(name)).collect();
                let mut possible = vec![STARTED.to_string()];
                for s in &mappings {
                    for st in [&s.required_state, &s.new_state].into_iter().flatten() {
                        if !possible.contains(st) {
                            possible.push(st.clone());
                        }
                    }
                }
                json!({
                    "id": name,
                    "name": name,
                    "state": state,
                    "possibleStates": possible,
                    "mappings": mappings.iter().map(|s| s.to_json()).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({ "scenarios": list })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub(v: Value) -> Stub {
        Stub::from_json(&v, Uuid::new_v4).unwrap()
    }

    fn get(url: &str) -> MockRequest {
        MockRequest::new("GET", url, vec![], Vec::new())
    }

    #[test]
    fn priority_and_recency() {
        let mut st = StubStore::default();
        st.insert(stub(json!({"name": "old", "request": {"urlPath": "/a"}})), true);
        st.insert(stub(json!({"name": "new", "request": {"urlPath": "/a"}})), true);
        assert_eq!(st.find(&get("/a")).unwrap().stub.name.as_deref(), Some("new"));
        st.insert(stub(json!({"name": "important", "priority": 1, "request": {"urlPattern": "/.*"}})), true);
        assert_eq!(st.find(&get("/a")).unwrap().stub.name.as_deref(), Some("important"));
        st.insert(stub(json!({"name": "fallback", "priority": 10, "request": {"method": "ANY"}})), true);
        assert_eq!(st.find(&get("/a")).unwrap().stub.name.as_deref(), Some("important"));
        assert_eq!(st.all().last().unwrap().name.as_deref(), Some("fallback"));
        assert_eq!(st.len(), 4);
    }

    #[test]
    fn replace_keeps_insertion_index() {
        let mut st = StubStore::default();
        let a = st.insert(stub(json!({"request": {"urlPath": "/a"}})), true);
        st.insert(stub(json!({"request": {"urlPath": "/b"}})), true);
        let mut edited = stub(json!({"request": {"urlPath": "/a2"}}));
        edited.id = a.id;
        let b = st.insert(edited, false);
        assert_eq!(b.insertion_index, a.insertion_index);
        assert_eq!(st.len(), 2);
        assert!(st.remove(&a.id).is_some());
        assert!(st.remove(&a.id).is_none());
    }

    #[test]
    fn scenarios() {
        let mut st = StubStore::default();
        st.insert(stub(json!({"scenarioName": "cart", "requiredScenarioState": "Started", "newScenarioState": "full", "request": {"urlPath": "/cart"}, "response": {"body": "empty"}})), true);
        st.insert(stub(json!({"scenarioName": "cart", "requiredScenarioState": "full", "request": {"urlPath": "/cart"}, "response": {"body": "1 item"}})), true);
        let m = st.find(&get("/cart")).unwrap();
        assert_eq!(m.stub.new_state.as_deref(), Some("full"));
        assert!(st.transition(&m.stub));
        assert_eq!(st.scenario_state("cart"), Some("full"));
        assert!(!st.transition(&m.stub), "stale transition must be rejected");
        let m2 = st.find(&get("/cart")).unwrap();
        assert!(m2.stub.new_state.is_none());
        let json = st.scenarios_json();
        assert_eq!(json["scenarios"][0]["possibleStates"], json!(["Started", "full"]));
        st.reset_scenarios();
        assert_eq!(st.scenario_state("cart"), Some("Started"));
        assert!(st.set_scenario_state("cart", Some("full".into())));
        assert!(!st.set_scenario_state("nope", None));
        st.clear();
        assert!(st.scenario_state("cart").is_none());
    }
}
