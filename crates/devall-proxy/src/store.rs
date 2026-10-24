//! In-memory capture: a ring of exchanges with a total body-memory budget and a live event
//! channel. Every change to an exchange is broadcast as its new summary.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast;

use crate::filter::ExchangeFilter;
use crate::model::{Exchange, ExchangeId, ExchangeSummary, ProxyEvent};

struct Inner {
    order: VecDeque<ExchangeId>,
    map: HashMap<ExchangeId, Exchange>,
    body_bytes: usize,
    max_exchanges: usize,
    max_total_body: usize,
}

pub struct CaptureStore {
    inner: Mutex<Inner>,
    next_id: AtomicU64,
    events: broadcast::Sender<ProxyEvent>,
}

impl std::fmt::Debug for CaptureStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureStore").field("len", &self.len()).finish()
    }
}

impl CaptureStore {
    pub fn new(max_exchanges: usize, max_total_body: usize, events: broadcast::Sender<ProxyEvent>) -> Self {
        Self {
            inner: Mutex::new(Inner { order: VecDeque::new(), map: HashMap::new(), body_bytes: 0, max_exchanges: max_exchanges.max(1), max_total_body }),
            next_id: AtomicU64::new(1),
            events,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, ev: ProxyEvent) {
        let _ = self.events.send(ev);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProxyEvent> {
        self.events.subscribe()
    }

    pub fn set_limits(&self, max_exchanges: usize, max_total_body: usize) {
        let mut g = self.lock();
        g.max_exchanges = max_exchanges.max(1);
        g.max_total_body = max_total_body;
        self.enforce(&mut g, None);
    }

    pub fn len(&self) -> usize {
        self.lock().order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds an exchange (assigning its id) and announces it.
    pub fn insert(&self, mut ex: Exchange) -> ExchangeId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        ex.id = id;
        let mut g = self.lock();
        g.body_bytes += ex.body_bytes();
        let summary = ex.summary();
        g.order.push_back(id);
        g.map.insert(id, ex);
        self.emit(ProxyEvent::Exchange { exchange: summary });
        self.enforce(&mut g, Some(id));
        id
    }

    /// Changes an exchange and announces its new summary. `None` when it is gone.
    pub fn update<R>(&self, id: ExchangeId, f: impl FnOnce(&mut Exchange) -> R) -> Option<R> {
        self.update_inner(id, true, f)
    }

    /// Like [`update`](Self::update) without an event (for high-frequency changes such as
    /// WebSocket frames; call [`touch`](Self::touch) to announce).
    pub(crate) fn update_quiet<R>(&self, id: ExchangeId, f: impl FnOnce(&mut Exchange) -> R) -> Option<R> {
        self.update_inner(id, false, f)
    }

    pub(crate) fn touch(&self, id: ExchangeId) {
        let g = self.lock();
        if let Some(ex) = g.map.get(&id) {
            self.emit(ProxyEvent::Exchange { exchange: ex.summary() });
        }
    }

    fn update_inner<R>(&self, id: ExchangeId, announce: bool, f: impl FnOnce(&mut Exchange) -> R) -> Option<R> {
        let mut g = self.lock();
        let ex = g.map.get_mut(&id)?;
        let before = ex.body_bytes();
        let r = f(ex);
        let after = ex.body_bytes();
        let summary = announce.then(|| ex.summary());
        g.body_bytes = (g.body_bytes + after).saturating_sub(before);
        if let Some(s) = summary {
            self.emit(ProxyEvent::Exchange { exchange: s });
        }
        if after > before {
            self.enforce(&mut g, Some(id));
        }
        Some(r)
    }

    /// Drops the oldest exchanges beyond capacity, then the oldest bodies beyond the memory budget.
    fn enforce(&self, g: &mut Inner, keep: Option<ExchangeId>) {
        let mut removed = Vec::new();
        while g.order.len() > g.max_exchanges {
            let Some(old) = g.order.pop_front() else {
                break;
            };
            if let Some(ex) = g.map.remove(&old) {
                g.body_bytes = g.body_bytes.saturating_sub(ex.body_bytes());
            }
            removed.push(old);
        }
        if !removed.is_empty() {
            self.emit(ProxyEvent::Removed { ids: removed });
        }
        if g.body_bytes <= g.max_total_body {
            return;
        }
        let ids: Vec<ExchangeId> = g.order.iter().copied().collect();
        for id in ids {
            if g.body_bytes <= g.max_total_body {
                break;
            }
            if Some(id) == keep {
                continue;
            }
            if let Some(ex) = g.map.get_mut(&id) {
                let freed = ex.body_bytes();
                if freed == 0 {
                    continue;
                }
                for b in std::iter::once(&mut ex.request.body).chain(ex.response.as_mut().map(|r| &mut r.body)) {
                    if !b.raw.is_empty() {
                        b.raw = bytes::Bytes::new();
                        b.evicted = true;
                    }
                }
                g.body_bytes = g.body_bytes.saturating_sub(freed);
            }
        }
    }

    pub fn get(&self, id: ExchangeId) -> Option<Exchange> {
        self.lock().map.get(&id).cloned()
    }

    /// The requested exchanges that still exist, oldest first.
    pub fn get_many(&self, ids: &[ExchangeId]) -> Vec<Exchange> {
        let g = self.lock();
        let mut out: Vec<Exchange> = ids.iter().filter_map(|id| g.map.get(id).cloned()).collect();
        out.sort_by_key(|e| e.id);
        out.dedup_by_key(|e| e.id);
        out
    }

    /// Every exchange, oldest first.
    pub fn all(&self) -> Vec<Exchange> {
        let g = self.lock();
        g.order.iter().filter_map(|id| g.map.get(id).cloned()).collect()
    }

    /// Summaries matching `filter`, oldest first (the newest `limit` when set).
    pub fn list(&self, filter: &ExchangeFilter) -> Vec<ExchangeSummary> {
        let g = self.lock();
        let mut out: Vec<ExchangeSummary> = g.order.iter().filter_map(|id| g.map.get(id)).map(Exchange::summary).filter(|s| filter.matches(s)).collect();
        if let Some(limit) = filter.limit
            && out.len() > limit
        {
            out.drain(..out.len() - limit);
        }
        out
    }

    pub fn clear(&self) {
        let mut g = self.lock();
        g.order.clear();
        g.map.clear();
        g.body_bytes = 0;
        self.emit(ProxyEvent::Cleared);
    }

    pub fn remove(&self, ids: &[ExchangeId]) -> usize {
        let mut g = self.lock();
        let mut removed = Vec::new();
        for id in ids {
            if let Some(ex) = g.map.remove(id) {
                g.body_bytes = g.body_bytes.saturating_sub(ex.body_bytes());
                removed.push(*id);
            }
        }
        g.order.retain(|id| !removed.contains(id));
        let n = removed.len();
        if n > 0 {
            self.emit(ProxyEvent::Removed { ids: removed });
        }
        n
    }

    /// Body bytes currently held.
    pub fn body_bytes(&self) -> usize {
        self.lock().body_bytes
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::model::CapturedBody;

    fn ex(body: usize) -> Exchange {
        let mut e = Exchange::new("GET", "http://a/", "HTTP/1.1", vec![]);
        e.request.body = CapturedBody::new(Bytes::from(vec![1u8; body]), usize::MAX);
        e
    }

    #[test]
    fn ring_and_budget() {
        let (tx, mut rx) = broadcast::channel(64);
        let store = CaptureStore::new(3, 250, tx);
        let ids: Vec<_> = (0..4).map(|_| store.insert(ex(100))).collect();
        assert_eq!(store.len(), 3);
        assert!(store.get(ids[0]).is_none());
        // 3 × 100 bytes > 250: the oldest remaining body was evicted.
        assert!(store.get(ids[1]).unwrap().request.body.evicted);
        assert!(!store.get(ids[3]).unwrap().request.body.evicted);
        assert!(store.body_bytes() <= 250);
        let mut saw_removed = false;
        while let Ok(ev) = rx.try_recv() {
            if let ProxyEvent::Removed { ids: r } = ev {
                saw_removed |= r == vec![ids[0]];
            }
        }
        assert!(saw_removed);
        store.update(ids[3], |e| e.tag("x")).unwrap();
        assert_eq!(store.get(ids[3]).unwrap().tags, ["x"]);
        assert_eq!(store.remove(&[ids[3], 999]), 1);
        assert_eq!(store.list(&ExchangeFilter::default()).len(), 2);
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.body_bytes(), 0);
    }
}
