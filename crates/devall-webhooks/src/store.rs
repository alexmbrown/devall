//! Event history in `<project data>/webhooks.sqlite` (on the device, outside the repository).
//!
//! History is unlimited unless a [`Retention`] is set. Every change is broadcast as a
//! [`StoreNotice`] so the UI can update live. [`EventStore::ingest`] stores an event that arrived
//! through another channel (the hosted relay) exactly like a locally received one.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::broadcast;

use crate::event::{EventPage, EventQuery, EventSummary, ForwardResult, IncomingEvent, Retention, StoreNotice, WebhookEvent, now_ms};
use crate::model::ProviderKind;
use crate::verify::VerifyResult;

pub const DB_FILE: &str = "webhooks.sqlite";
const RETENTION_KEY: &str = "retention";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("webhook database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("webhook database: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

pub struct EventStore {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
    tx: broadcast::Sender<StoreNotice>,
}

const COLUMNS: &str = "id, inbox, received_at, source, method, url, path, query, headers, body, body_size, body_truncated, content_type,
    remote_addr, public_url, label, provider, response_status, verification, forward, replay_of, external_id";
const SUMMARY_COLUMNS: &str = "id, inbox, received_at, source, method, path, query, label, body_size, content_type, remote_addr,
    response_status, verification, forward, replay_of";

impl EventStore {
    /// Opens (creating if needed) `<project data>/webhooks.sqlite` (see
    /// `devall_core::local::project_dir`).
    pub fn open_project(project_root: &Path) -> Result<Self> {
        let dir = devall_core::local::project_dir(project_root);
        std::fs::create_dir_all(&dir)?;
        Self::open(&dir.join(DB_FILE))
    }

    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn, Some(path.to_path_buf()))
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                inbox TEXT NOT NULL DEFAULT '',
                received_at INTEGER NOT NULL,
                source TEXT NOT NULL,
                method TEXT NOT NULL,
                url TEXT NOT NULL,
                path TEXT NOT NULL,
                query TEXT NOT NULL DEFAULT '',
                headers TEXT NOT NULL,
                body BLOB NOT NULL,
                body_size INTEGER NOT NULL,
                body_truncated INTEGER NOT NULL DEFAULT 0,
                content_type TEXT,
                remote_addr TEXT,
                public_url TEXT,
                label TEXT,
                provider TEXT,
                response_status INTEGER,
                verification TEXT,
                forward TEXT,
                replay_of TEXT,
                external_id TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS events_by_external ON events(external_id) WHERE external_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_by_inbox ON events(inbox, received_at DESC);
            CREATE INDEX IF NOT EXISTS events_by_time ON events(received_at DESC);
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            PRAGMA user_version = 1;",
        )?;
        let (tx, _) = broadcast::channel(512);
        Ok(Self { conn: Mutex::new(conn), path, tx })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Live notifications (added / updated / deleted / cleared / error).
    pub fn subscribe(&self) -> broadcast::Receiver<StoreNotice> {
        self.tx.subscribe()
    }

    pub(crate) fn notify(&self, n: StoreNotice) {
        let _ = self.tx.send(n);
    }

    /// Stores an event that arrived from outside the local receiver (e.g. the hosted relay) and
    /// broadcasts it. `inbox` is the routed inbox id, if any. An already stored `external_id`
    /// returns the existing event. Prefer [`crate::WebhookHub::ingest`], which also routes,
    /// verifies and forwards.
    pub fn ingest(&self, incoming: IncomingEvent, inbox: Option<&str>, provider: Option<ProviderKind>) -> Result<WebhookEvent> {
        if let Some(existing) = incoming.external_id.as_deref().map(|x| self.find_external(x)).transpose()?.flatten() {
            return Ok(existing);
        }
        let event = event_from_incoming(incoming, inbox, provider);
        self.insert(&event)?;
        Ok(event)
    }

    /// Inserts (or replaces) an event and broadcasts `added`.
    pub fn insert(&self, e: &WebhookEvent) -> Result<()> {
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                &format!("INSERT OR REPLACE INTO events ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)"),
                params![
                    e.id,
                    e.inbox.clone().unwrap_or_default(),
                    e.received_at,
                    e.source.as_str(),
                    e.method,
                    e.url,
                    e.path,
                    e.query,
                    serde_json::to_string(&e.headers).unwrap_or_else(|_| "[]".into()),
                    e.body,
                    e.body_size as i64,
                    e.body_truncated as i64,
                    e.content_type,
                    e.remote_addr,
                    e.public_url,
                    e.label,
                    e.provider.map(|p| p.id()),
                    e.response_status,
                    e.verification.as_ref().and_then(|v| serde_json::to_string(v).ok()),
                    e.forward.as_ref().and_then(|f| serde_json::to_string(f).ok()),
                    e.replay_of,
                    e.external_id,
                ],
            )?;
        }
        self.notify(StoreNotice::Added { event: e.summary() });
        self.apply_retention()?;
        Ok(())
    }

    pub fn set_verification(&self, id: &str, v: &VerifyResult) -> Result<bool> {
        let json = serde_json::to_string(v).unwrap_or_default();
        self.update(id, "verification", &json)
    }

    pub fn set_forward(&self, id: &str, f: &ForwardResult) -> Result<bool> {
        let json = serde_json::to_string(f).unwrap_or_default();
        self.update(id, "forward", &json)
    }

    pub fn set_response_status(&self, id: &str, status: u16) -> Result<bool> {
        let n = self.conn.lock().unwrap().execute("UPDATE events SET response_status = ?1 WHERE id = ?2", params![status, id])?;
        Ok(n > 0)
    }

    fn update(&self, id: &str, column: &'static str, json: &str) -> Result<bool> {
        let n = self.conn.lock().unwrap().execute(&format!("UPDATE events SET {column} = ?1 WHERE id = ?2"), params![json, id])?;
        if n > 0
            && let Some(s) = self.summary(id)?
        {
            self.notify(StoreNotice::Updated { event: s });
        }
        Ok(n > 0)
    }

    pub fn get(&self, id: &str) -> Result<Option<WebhookEvent>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(&format!("SELECT {COLUMNS} FROM events WHERE id = ?1"), params![id], row_to_event).optional().map_err(Into::into)
    }

    /// The event stored for a source's id (see [`IncomingEvent::external_id`]).
    pub fn find_external(&self, external_id: &str) -> Result<Option<WebhookEvent>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(&format!("SELECT {COLUMNS} FROM events WHERE external_id = ?1"), params![external_id], row_to_event).optional().map_err(Into::into)
    }

    pub fn summary(&self, id: &str) -> Result<Option<EventSummary>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(&format!("SELECT {SUMMARY_COLUMNS} FROM events WHERE id = ?1"), params![id], row_to_summary).optional().map_err(Into::into)
    }

    /// Newest first.
    pub fn list(&self, q: &EventQuery) -> Result<EventPage> {
        let limit = if q.limit == 0 { 100 } else { q.limit.min(1000) };
        let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")));
        let filter = "(?1 IS NULL OR inbox = ?1) AND (?2 IS NULL OR path LIKE ?2 ESCAPE '\\' OR method LIKE ?2 ESCAPE '\\' OR label LIKE ?2 ESCAPE '\\'
            OR CAST(body AS TEXT) LIKE ?2 ESCAPE '\\')";
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM events WHERE {filter}"), params![q.inbox, search], |r| r.get(0))?;
        let mut stmt = conn.prepare(&format!("SELECT {SUMMARY_COLUMNS} FROM events WHERE {filter} ORDER BY received_at DESC, id DESC LIMIT ?3 OFFSET ?4"))?;
        let events = stmt.query_map(params![q.inbox, search, limit as i64, q.offset as i64], row_to_summary)?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(EventPage { events, total: total as usize })
    }

    /// Event counts per inbox id (`""` = unrouted).
    pub fn counts(&self) -> Result<Vec<(String, usize)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT inbox, COUNT(*) FROM events GROUP BY inbox")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let n = self.conn.lock().unwrap().execute("DELETE FROM events WHERE id = ?1", params![id])?;
        if n > 0 {
            self.notify(StoreNotice::Deleted { id: id.to_string() });
        }
        Ok(n > 0)
    }

    /// Deletes one inbox's events (`Some("")` = unrouted ones) or all of them.
    pub fn clear(&self, inbox: Option<&str>) -> Result<usize> {
        let n = {
            let conn = self.conn.lock().unwrap();
            let n = conn.execute("DELETE FROM events WHERE (?1 IS NULL OR inbox = ?1)", params![inbox])?;
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            n
        };
        self.notify(StoreNotice::Cleared { inbox: inbox.map(str::to_string) });
        Ok(n)
    }

    pub fn retention(&self) -> Result<Retention> {
        let conn = self.conn.lock().unwrap();
        let v: Option<String> = conn.query_row("SELECT value FROM meta WHERE key = ?1", params![RETENTION_KEY], |r| r.get(0)).optional()?;
        Ok(v.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
    }

    /// Saves the retention policy and prunes immediately. Returns how many events were removed.
    pub fn set_retention(&self, r: Retention) -> Result<usize> {
        {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![RETENTION_KEY, serde_json::to_string(&r).unwrap_or_default()],
            )?;
        }
        self.apply_retention()
    }

    /// Prunes by the saved retention policy. Unlimited by default.
    pub fn apply_retention(&self) -> Result<usize> {
        let r = self.retention()?;
        if r.is_unlimited() {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let mut n = 0;
        if let Some(days) = r.max_age_days {
            let cutoff = now_ms() - (days as i64).saturating_mul(86_400_000);
            n += conn.execute("DELETE FROM events WHERE received_at < ?1", params![cutoff])?;
        }
        if let Some(max) = r.max_events {
            n += conn.execute(
                "DELETE FROM events WHERE id IN (SELECT id FROM events ORDER BY received_at DESC, id DESC LIMIT -1 OFFSET ?1)",
                params![max as i64],
            )?;
        }
        drop(conn);
        if n > 0 {
            self.notify(StoreNotice::Cleared { inbox: None });
        }
        Ok(n)
    }
}

pub(crate) fn event_from_incoming(mut i: IncomingEvent, inbox: Option<&str>, provider: Option<ProviderKind>) -> WebhookEvent {
    split_path_query(&mut i);
    let content_type = crate::event::header(&i.headers, "content-type").map(str::to_string);
    let label = crate::event::event_label(provider, &i.headers, &i.body);
    WebhookEvent {
        id: ulid::Ulid::generate().to_string(),
        inbox: inbox.filter(|s| !s.is_empty()).map(str::to_string),
        received_at: if i.received_at > 0 { i.received_at } else { now_ms() },
        source: i.source,
        method: if i.method.is_empty() { "POST".into() } else { i.method.to_ascii_uppercase() },
        url: i.url,
        path: if i.path.is_empty() { "/".into() } else { i.path },
        query: i.query,
        body_size: i.body.len(),
        body_truncated: i.body_truncated,
        headers: i.headers,
        body: i.body,
        content_type,
        remote_addr: i.remote_addr,
        public_url: i.public_url.filter(|u| !u.is_empty()),
        label,
        provider: provider.filter(|p| *p != ProviderKind::None),
        response_status: None,
        verification: None,
        forward: None,
        replay_of: None,
        external_id: i.external_id.filter(|x| !x.is_empty()),
    }
}

/// Sources may pass `path` with its query (`/stripe?x=1`); split it when `query` is empty.
pub(crate) fn split_path_query(i: &mut IncomingEvent) {
    if i.query.is_empty()
        && let Some((p, q)) = i.path.split_once('?')
    {
        let (p, q) = (p.to_string(), q.to_string());
        i.path = p;
        i.query = q;
    }
}

fn parse_provider(s: Option<String>) -> Option<ProviderKind> {
    let s = s?;
    ProviderKind::ALL.iter().copied().find(|p| p.id() == s)
}

fn json_col<T: serde::de::DeserializeOwned>(s: Option<String>) -> Option<T> {
    s.and_then(|s| serde_json::from_str(&s).ok())
}

fn inbox_col(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

fn row_to_event(r: &rusqlite::Row) -> rusqlite::Result<WebhookEvent> {
    let headers: String = r.get(8)?;
    Ok(WebhookEvent {
        id: r.get(0)?,
        inbox: inbox_col(r.get(1)?),
        received_at: r.get(2)?,
        source: crate::event::EventSource::parse(&r.get::<_, String>(3)?),
        method: r.get(4)?,
        url: r.get(5)?,
        path: r.get(6)?,
        query: r.get(7)?,
        headers: serde_json::from_str(&headers).unwrap_or_default(),
        body: r.get(9)?,
        body_size: r.get::<_, i64>(10)? as usize,
        body_truncated: r.get::<_, i64>(11)? != 0,
        content_type: r.get(12)?,
        remote_addr: r.get(13)?,
        public_url: r.get(14)?,
        label: r.get(15)?,
        provider: parse_provider(r.get(16)?),
        response_status: r.get(17)?,
        verification: json_col(r.get(18)?),
        forward: json_col(r.get(19)?),
        replay_of: r.get(20)?,
        external_id: r.get(21)?,
    })
}

fn row_to_summary(r: &rusqlite::Row) -> rusqlite::Result<EventSummary> {
    let verification: Option<VerifyResult> = json_col(r.get(12)?);
    let forward: Option<ForwardResult> = json_col(r.get(13)?);
    Ok(EventSummary {
        id: r.get(0)?,
        inbox: inbox_col(r.get(1)?),
        received_at: r.get(2)?,
        source: crate::event::EventSource::parse(&r.get::<_, String>(3)?),
        method: r.get(4)?,
        path: r.get(5)?,
        query: r.get(6)?,
        label: r.get(7)?,
        body_size: r.get::<_, i64>(8)? as usize,
        content_type: r.get(9)?,
        remote_addr: r.get(10)?,
        response_status: r.get(11)?,
        signature: verification.map(|v| crate::event::SignatureBadge { valid: v.valid, status: v.status, reason: v.reason }),
        forward_status: forward.as_ref().and_then(|f| f.status),
        forward_error: forward.as_ref().and_then(|f| f.error.clone()),
        forward_ms: forward.as_ref().map(|f| f.duration_ms),
        replay_of: r.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventSource;

    fn incoming(path: &str, body: &str, at: i64) -> IncomingEvent {
        IncomingEvent {
            method: "post".into(),
            url: format!("http://127.0.0.1:4590{path}"),
            path: path.into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.as_bytes().to_vec(),
            received_at: at,
            source: EventSource::Relay,
            ..Default::default()
        }
    }

    #[test]
    fn insert_list_get_update_delete() {
        let data = devall_core::local::isolate_for_tests();
        let dir = tempfile::tempdir().unwrap();
        let s = EventStore::open_project(dir.path()).unwrap();
        assert!(devall_core::local::project_dir(dir.path()).join(DB_FILE).is_file());
        assert!(devall_core::local::project_dir(dir.path()).starts_with(&data) && !dir.path().join(".devall").exists());
        let mut rx = s.subscribe();
        let a = s.ingest(incoming("/stripe", r#"{"type":"invoice.paid"}"#, 1000), Some("stripe"), Some(ProviderKind::Stripe)).unwrap();
        let b = s.ingest(incoming("/nope", "hello", 2000), None, None).unwrap();
        assert!(matches!(rx.try_recv().unwrap(), StoreNotice::Added { .. }));
        assert_eq!(a.method, "POST");
        assert_eq!(a.label.as_deref(), Some("invoice.paid"));

        let all = s.list(&EventQuery::default()).unwrap();
        assert_eq!(all.total, 2);
        assert_eq!(all.events[0].id, b.id, "newest first");
        assert_eq!(s.list(&EventQuery { inbox: Some("stripe".into()), ..Default::default() }).unwrap().events.len(), 1);
        assert_eq!(s.list(&EventQuery { inbox: Some(String::new()), ..Default::default() }).unwrap().events[0].id, b.id);
        assert_eq!(s.list(&EventQuery { search: Some("INVOICE".into()), ..Default::default() }).unwrap().total, 1);
        assert_eq!(s.list(&EventQuery { search: Some("hello".into()), ..Default::default() }).unwrap().total, 1, "body search");
        assert_eq!(s.list(&EventQuery { search: Some("100%".into()), ..Default::default() }).unwrap().total, 0);
        let page = s.list(&EventQuery { limit: 1, offset: 1, ..Default::default() }).unwrap();
        assert_eq!((page.total, page.events.len(), page.events[0].id.clone()), (2, 1, a.id.clone()));

        let got = s.get(&a.id).unwrap().unwrap();
        assert_eq!(got.body, br#"{"type":"invoice.paid"}"#);
        assert_eq!(got.source, EventSource::Relay);
        assert_eq!(got.provider, Some(ProviderKind::Stripe));

        let f = ForwardResult { url: "http://localhost:3000".into(), status: Some(204), duration_ms: 3.5, ..Default::default() };
        assert!(s.set_forward(&a.id, &f).unwrap());
        let row = s.summary(&a.id).unwrap().unwrap();
        assert_eq!((row.forward_status, row.forward_ms), (Some(204), Some(3.5)));
        assert_eq!(s.get(&a.id).unwrap().unwrap().forward.unwrap(), f);

        assert_eq!(s.counts().unwrap().len(), 2);
        assert!(s.delete(&a.id).unwrap());
        assert!(!s.delete(&a.id).unwrap());
        assert_eq!(s.clear(None).unwrap(), 1);
        assert_eq!(s.list(&EventQuery::default()).unwrap().total, 0);
    }

    #[test]
    fn external_ids_deduplicate_and_paths_split() {
        let s = EventStore::open_in_memory().unwrap();
        let mut i = incoming("/stripe?x=1", "{}", 1);
        i.external_id = Some("evt_relay_1".into());
        let a = s.ingest(i.clone(), Some("stripe"), None).unwrap();
        assert_eq!((a.path.as_str(), a.query.as_str()), ("/stripe", "x=1"));
        let b = s.ingest(i, Some("stripe"), None).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(s.list(&EventQuery::default()).unwrap().total, 1);
        assert_eq!(s.find_external("evt_relay_1").unwrap().unwrap().external_id.as_deref(), Some("evt_relay_1"));
    }

    #[test]
    fn binary_bodies_round_trip() {
        let s = EventStore::open_in_memory().unwrap();
        let mut i = incoming("/bin", "", 1);
        i.body = vec![0, 255, 1, 2];
        let e = s.ingest(i, None, None).unwrap();
        assert_eq!(s.get(&e.id).unwrap().unwrap().body, vec![0, 255, 1, 2]);
        let v = s.get(&e.id).unwrap().unwrap().into_view();
        assert_eq!(v.body_base64.as_deref(), Some("AP8BAg=="));
    }

    #[test]
    fn retention() {
        let s = EventStore::open_in_memory().unwrap();
        let now = now_ms();
        // Ages just under 0, 1, 2, 3 and 4 days.
        for i in 0..5 {
            s.ingest(incoming("/a", "{}", now - i * 86_400_000 + 60_000), Some("a"), None).unwrap();
        }
        assert_eq!(s.retention().unwrap(), Retention::default());
        assert_eq!(s.set_retention(Retention { max_events: None, max_age_days: Some(2) }).unwrap(), 2);
        assert_eq!(s.list(&EventQuery::default()).unwrap().total, 3);
        assert_eq!(s.set_retention(Retention { max_events: Some(1), max_age_days: None }).unwrap(), 2);
        let left = s.list(&EventQuery::default()).unwrap();
        assert_eq!(left.total, 1);
        assert!(left.events[0].received_at >= now, "keeps the newest");
        // New inserts are pruned too.
        s.ingest(incoming("/a", "{}", now + 1), Some("a"), None).unwrap();
        assert_eq!(s.list(&EventQuery::default()).unwrap().total, 1);
    }
}
