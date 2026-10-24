//! Requests and responses held at breakpoints. The proxy task waits on a oneshot channel until
//! the UI resumes (optionally with edits) or aborts — or the timeout resumes it unchanged.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::{broadcast, oneshot};

use crate::model::{BreakpointEdit, ExchangeId, PausedExchange, ProxyEvent};

#[derive(Debug)]
pub(crate) enum Decision {
    Resume(Option<BreakpointEdit>),
    Abort,
    /// Nobody answered in time; continue unchanged.
    TimedOut,
}

struct Pending {
    info: PausedExchange,
    tx: oneshot::Sender<Decision>,
}

#[derive(Default)]
pub(crate) struct Breakpoints {
    pending: Mutex<HashMap<ExchangeId, Pending>>,
}

/// Removes the pending entry when the waiting task goes away (client disconnected).
struct Guard<'a> {
    bps: &'a Breakpoints,
    id: ExchangeId,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.bps.lock().remove(&self.id);
    }
}

impl Breakpoints {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ExchangeId, Pending>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub async fn wait(&self, info: PausedExchange, timeout: Duration, events: &broadcast::Sender<ProxyEvent>) -> Decision {
        let id = info.id;
        let (tx, rx) = oneshot::channel();
        self.lock().insert(id, Pending { info: info.clone(), tx });
        let _guard = Guard { bps: self, id };
        let _ = events.send(ProxyEvent::Paused { paused: info });
        let decision = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(d)) => d,
            Ok(Err(_)) => Decision::Resume(None),
            Err(_) => Decision::TimedOut,
        };
        let _ = events.send(ProxyEvent::Resumed { id });
        decision
    }

    fn send(&self, id: ExchangeId, d: Decision) -> bool {
        match self.lock().remove(&id) {
            Some(p) => p.tx.send(d).is_ok(),
            None => false,
        }
    }

    pub fn resume(&self, id: ExchangeId, edit: Option<BreakpointEdit>) -> bool {
        self.send(id, Decision::Resume(edit))
    }

    pub fn abort(&self, id: ExchangeId) -> bool {
        self.send(id, Decision::Abort)
    }

    /// Paused exchanges, oldest first.
    pub fn list(&self) -> Vec<PausedExchange> {
        let mut v: Vec<PausedExchange> = self.lock().values().map(|p| p.info.clone()).collect();
        v.sort_by_key(|p| p.id);
        v
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Resumes everything unchanged (used when the proxy stops).
    pub fn release_all(&self) {
        let all: Vec<Pending> = self.lock().drain().map(|(_, p)| p).collect();
        for p in all {
            let _ = p.tx.send(Decision::Resume(None));
        }
    }
}
