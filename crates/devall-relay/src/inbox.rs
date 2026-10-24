//! Hosted webhook inbox: receives the relay's buffered backlog and live events in order, hands
//! each one to a handler (the app passes them to `devall-webhooks` `ingest`) and acknowledges
//! it only after the handler succeeded. Delivery is at-least-once; ids already handled are
//! skipped (and re-acknowledged) when the relay redelivers them after a reconnect.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::error::{RelayError, Result, chain};
use crate::protocol::{ErrorPayload, Frame, GoAway, HEARTBEAT_PING, HEARTBEAT_PONG, InboxAck, InboxConfig, InboxEvent, InboxResponse, InboxStatus, QuotaPayload, ft};
use crate::session::{ConnState, Connector, Exit, ReconnectPolicy, Source, binary, exit_for_close, now_ms, read_hello, spawn_writer, last_error_hint};

/// Called for every event, in order. Return `Err` to have it retried (with backoff) — the relay
/// keeps the event until the handler succeeds.
pub type InboxHandler = Arc<dyn Fn(InboxEvent) -> BoxFuture<'static, std::result::Result<(), String>> + Send + Sync>;

/// Wraps an async closure as an [`InboxHandler`].
pub fn handler<F, Fut>(f: F) -> InboxHandler
where
    F: Fn(InboxEvent) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = std::result::Result<(), String>> + Send + 'static,
{
    Arc::new(move |ev| Box::pin(f(ev)))
}

#[derive(Debug, Clone)]
pub struct InboxOptions {
    /// Stable inbox per project (Supporter); ignored on the free tier (one inbox per account).
    pub project_key: Option<String>,
    /// What the inbox answers to webhook senders (default `200 {"ok":true}`).
    pub response: Option<InboxResponse>,
    pub reconnect: ReconnectPolicy,
    /// Backoff between handler retries.
    pub retry: ReconnectPolicy,
}

impl Default for InboxOptions {
    fn default() -> Self {
        Self { project_key: None, response: None, reconnect: ReconnectPolicy::default(), retry: ReconnectPolicy { initial: Duration::from_millis(500), max: Duration::from_secs(30) } }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxState {
    pub state: ConnState,
    pub url: Option<String>,
    pub inbox_id: Option<String>,
    pub plan: Option<String>,
    /// Events stored on the relay (not yet acknowledged).
    pub buffered: u64,
    pub delivered: u64,
    pub duplicates: u64,
    pub failures: u64,
    pub today_count: u64,
    pub daily_limit: Option<u64>,
    pub retention_seconds: Option<u64>,
    pub last_event_at: Option<String>,
    pub last_error: Option<String>,
    pub quota: Option<QuotaPayload>,
    pub response: Option<InboxResponse>,
    pub reconnects: u32,
}

impl Default for InboxState {
    fn default() -> Self {
        Self {
            state: ConnState::Connecting,
            url: None,
            inbox_id: None,
            plan: None,
            buffered: 0,
            delivered: 0,
            duplicates: 0,
            failures: 0,
            today_count: 0,
            daily_limit: None,
            retention_seconds: None,
            last_event_at: None,
            last_error: None,
            quota: None,
            response: None,
            reconnects: 0,
        }
    }
}

/// A delivered event, for the UI's log (the full event went to the handler).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxDelivery {
    pub id: String,
    pub method: String,
    pub path: String,
    pub received_at: String,
    pub body_size: u64,
    pub duplicate: bool,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum InboxUpdate {
    Status(Box<InboxState>),
    Delivered(InboxDelivery),
}

struct Shared {
    status: watch::Sender<InboxState>,
    events: broadcast::Sender<InboxUpdate>,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut InboxState)) {
        self.status.send_modify(f);
        let s = self.status.borrow().clone();
        let _ = self.events.send(InboxUpdate::Status(Box::new(s)));
    }
}

/// Ids already handled, bounded (oldest forgotten first).
struct Seen {
    set: HashSet<String>,
    order: VecDeque<String>,
    cap: usize,
}

impl Seen {
    fn new(cap: usize) -> Self {
        Self { set: HashSet::new(), order: VecDeque::new(), cap }
    }
    fn contains(&self, id: &str) -> bool {
        self.set.contains(id)
    }
    fn insert(&mut self, id: String) {
        if self.set.insert(id.clone()) {
            self.order.push_back(id);
            while self.order.len() > self.cap {
                if let Some(old) = self.order.pop_front() {
                    self.set.remove(&old);
                }
            }
        }
    }
}

pub struct InboxHandle {
    status: watch::Receiver<InboxState>,
    events: broadcast::Sender<InboxUpdate>,
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl InboxHandle {
    pub fn status(&self) -> InboxState {
        self.status.borrow().clone()
    }

    pub fn watch(&self) -> watch::Receiver<InboxState> {
        self.status.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<InboxUpdate> {
        self.events.subscribe()
    }

    pub fn is_running(&self) -> bool {
        self.task.lock().expect("task lock").as_ref().is_some_and(|t| !t.is_finished())
    }

    pub async fn wait_connected(&self, timeout: Duration) -> Result<InboxState> {
        let mut rx = self.status.clone();
        let fut = rx.wait_for(|s| matches!(s.state, ConnState::Connected | ConnState::Failed | ConnState::Stopped));
        let s = tokio::time::timeout(timeout, fut).await.map_err(|_| RelayError::Timeout(format!("waiting for the inbox{}", last_error_hint(self.status.borrow().last_error.as_deref()))))?.map_err(|_| RelayError::Connect("inbox task ended".into()))?.clone();
        match s.state {
            ConnState::Connected => Ok(s),
            _ => Err(RelayError::Connect(s.last_error.unwrap_or_else(|| "inbox stopped".into()))),
        }
    }

    pub fn signal_stop(&self) {
        let _ = self.stop.send(true);
    }

    pub async fn stop(&self) {
        self.signal_stop();
        let task = self.task.lock().expect("task lock").take();
        if let Some(t) = task {
            let abort = t.abort_handle();
            if tokio::time::timeout(Duration::from_secs(5), t).await.is_err() {
                abort.abort();
            }
        }
    }
}

pub(crate) fn spawn(connector: Arc<Connector>, opts: InboxOptions, handler: InboxHandler) -> InboxHandle {
    let (status_tx, status_rx) = watch::channel(InboxState::default());
    let (events, _) = broadcast::channel(1024);
    let (stop_tx, stop_rx) = watch::channel(false);
    let shared = Arc::new(Shared { status: status_tx, events: events.clone() });
    let task = tokio::spawn(run(connector, opts, handler, shared, stop_rx));
    InboxHandle { status: status_rx, events, stop: stop_tx, task: Mutex::new(Some(task)) }
}

async fn run(connector: Arc<Connector>, opts: InboxOptions, handler: InboxHandler, shared: Arc<Shared>, mut stop: watch::Receiver<bool>) {
    let seen = Arc::new(Mutex::new(Seen::new(4096)));
    let mut attempt = 0u32;
    loop {
        if *stop.borrow() {
            break;
        }
        let exit = session(&connector, &opts, &handler, &shared, &seen, &mut stop).await;
        let delay = match exit {
            Exit::Stopped => break,
            Exit::Failed(e) if e.is_fatal() => {
                shared.update(|s| {
                    s.state = ConnState::Failed;
                    s.last_error = Some(e.to_string());
                });
                return;
            }
            Exit::Failed(e) => {
                let retry = match &e {
                    RelayError::Rejected { retry_after, .. } => *retry_after,
                    _ => None,
                };
                shared.update(|s| s.last_error = Some(e.to_string()));
                retry
            }
            Exit::Closed { reconnect: false, reason, .. } => {
                shared.update(|s| {
                    s.state = ConnState::Stopped;
                    s.last_error = Some(reason);
                });
                return;
            }
            Exit::Closed { reconnect: true, delay, reason } => {
                attempt = 0;
                shared.update(|s| s.last_error = Some(reason));
                delay
            }
        };
        let wait = delay.unwrap_or_else(|| opts.reconnect.delay(attempt));
        attempt = attempt.saturating_add(1);
        shared.update(|s| {
            s.state = ConnState::Reconnecting;
            s.reconnects += 1;
        });
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stop.changed() => {}
        }
    }
    shared.update(|s| s.state = ConnState::Stopped);
}

async fn session(connector: &Connector, opts: &InboxOptions, handler: &InboxHandler, shared: &Arc<Shared>, seen: &Arc<Mutex<Seen>>, stop: &mut watch::Receiver<bool>) -> Exit {
    let query: Vec<(&str, String)> = opts.project_key.iter().map(|k| ("project", k.clone())).collect();
    let ws = tokio::select! {
        r = connector.open("/inbox", &query) => match r {
            Ok(ws) => ws,
            Err(e) => return Exit::Failed(e),
        },
        _ = stop.changed() => return Exit::Stopped,
    };
    let (sink, mut src) = ws.split();
    let hello = match read_hello(&mut src).await {
        Ok(h) => h,
        Err(e) => return Exit::Failed(e),
    };
    let (out, out_rx) = mpsc::channel::<Message>(1024);
    let writer = spawn_writer(sink, out_rx);
    if let Some(r) = &opts.response {
        let _ = out.send(binary(Frame::json(ft::INBOX_CONFIG, 0, &InboxConfig { response: Some(r.clone()) }))).await;
    }
    shared.update(|s| {
        s.state = ConnState::Connected;
        s.url = Some(hello.public_url.clone());
        s.inbox_id.clone_from(&hello.inbox_id);
        s.plan = Some(hello.plan.clone());
        s.retention_seconds = Some(hello.limits.inbox_retention_seconds);
        s.daily_limit = hello.limits.inbox_events_per_day;
        s.last_error = None;
        s.quota = None;
    });

    // Events are handled one at a time, in order, by this worker; the socket loop keeps reading.
    let (ev_tx, ev_rx) = mpsc::unbounded_channel::<InboxEvent>();
    let (worker_stop_tx, worker_stop_rx) = watch::channel(false);
    let worker = tokio::spawn(process(ev_rx, handler.clone(), out.clone(), shared.clone(), seen.clone(), opts.retry, worker_stop_rx));

    let heartbeat = Duration::from_secs(hello.heartbeat_seconds.clamp(5, 120));
    let exit = pump(&mut src, &out, &ev_tx, shared, stop, heartbeat).await;

    // Let the current handler call finish (it may be writing to disk), then stop the worker.
    drop(ev_tx);
    let _ = worker_stop_tx.send(true);
    if tokio::time::timeout(Duration::from_secs(10), worker).await.is_err() {
        // stuck handler: give up on it; the event will be redelivered
    }
    drop(out);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    exit
}

async fn pump(src: &mut Source, out: &mpsc::Sender<Message>, ev_tx: &mpsc::UnboundedSender<InboxEvent>, shared: &Arc<Shared>, stop: &mut watch::Receiver<bool>, heartbeat: Duration) -> Exit {
    let mut hb = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    let mut last_seen = Instant::now();
    let mut goaway: Option<GoAway> = None;
    loop {
        tokio::select! {
            _ = stop.changed() => {
                if *stop.borrow() {
                    let _ = out.send(Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "stopped".into() }))).await;
                    return Exit::Stopped;
                }
            }
            _ = hb.tick() => {
                if last_seen.elapsed() > heartbeat * 3 {
                    return Exit::Closed { reconnect: true, delay: None, reason: "the relay stopped answering".into() };
                }
                let _ = out.try_send(Message::Text(HEARTBEAT_PING.into()));
            }
            msg = src.next() => match msg {
                None => return exit_for_close(None, "", goaway.as_ref()),
                Some(Err(e)) => return Exit::Closed { reconnect: true, delay: None, reason: chain(&e) },
                Some(Ok(Message::Text(t))) => {
                    if t.as_str() == HEARTBEAT_PONG {
                        last_seen = Instant::now();
                    }
                }
                Some(Ok(Message::Binary(b))) => {
                    last_seen = Instant::now();
                    let res = match Frame::decode_bytes(b) {
                        Ok(f) => handle(f, out, ev_tx, shared, &mut goaway).await,
                        Err(e) => Err(e),
                    };
                    if let Err(e) = res {
                        let _ = out.send(binary(Frame::json(ft::ERROR, 0, &ErrorPayload { code: "protocol_error".into(), message: e.to_string(), fatal: true }))).await;
                        return Exit::Closed { reconnect: true, delay: None, reason: e.to_string() };
                    }
                }
                Some(Ok(Message::Close(cf))) => {
                    let (code, reason) = cf.map(|c| (Some(u16::from(c.code)), c.reason.to_string())).unwrap_or((None, String::new()));
                    return exit_for_close(code, &reason, goaway.as_ref());
                }
                Some(Ok(_)) => {}
            }
        }
    }
}

async fn handle(f: Frame, out: &mpsc::Sender<Message>, ev_tx: &mpsc::UnboundedSender<InboxEvent>, shared: &Arc<Shared>, goaway: &mut Option<GoAway>) -> Result<()> {
    match f.kind {
        ft::INBOX_EVENT => {
            let ev: InboxEvent = f.parse()?;
            let _ = ev_tx.send(ev);
        }
        ft::INBOX_STATUS => {
            let st: InboxStatus = f.parse()?;
            shared.update(|s| {
                s.buffered = st.buffered;
                s.today_count = st.today_count;
                s.daily_limit = st.daily_limit;
                s.retention_seconds = Some(st.retention_seconds);
                s.response = st.response.clone();
            });
        }
        ft::QUOTA => {
            let q: QuotaPayload = f.parse()?;
            shared.update(|s| s.quota = Some(q));
        }
        ft::GOAWAY => *goaway = Some(f.parse()?),
        ft::ERROR => {
            let e: ErrorPayload = f.parse()?;
            shared.update(|s| s.last_error = Some(format!("{} ({})", e.message, e.code)));
        }
        ft::PING => {
            let _ = out.send(binary(Frame::new(ft::PONG, 0, f.payload))).await;
        }
        _ => {}
    }
    Ok(())
}

async fn process(
    mut rx: mpsc::UnboundedReceiver<InboxEvent>,
    handler: InboxHandler,
    out: mpsc::Sender<Message>,
    shared: Arc<Shared>,
    seen: Arc<Mutex<Seen>>,
    retry: ReconnectPolicy,
    mut stop: watch::Receiver<bool>,
) {
    while let Some(ev) = rx.recv().await {
        let id = ev.id.clone();
        let delivery = InboxDelivery { id: id.clone(), method: ev.method.clone(), path: ev.path.clone(), received_at: ev.received_at.clone(), body_size: ev.body_size, duplicate: false, at_ms: now_ms() };
        let duplicate = seen.lock().expect("seen lock").contains(&id);
        if duplicate {
            shared.update(|s| s.duplicates += 1);
            let _ = shared.events.send(InboxUpdate::Delivered(InboxDelivery { duplicate: true, ..delivery }));
        } else {
            let mut attempt = 0u32;
            loop {
                match handler(ev.clone()).await {
                    Ok(()) => break,
                    Err(e) => {
                        shared.update(|s| {
                            s.failures += 1;
                            s.last_error = Some(format!("handling {id} failed: {e}"));
                        });
                        let wait = retry.delay(attempt);
                        attempt = attempt.saturating_add(1);
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = stop.changed() => {}
                        }
                        if *stop.borrow() {
                            return; // not acknowledged: the relay redelivers it next time
                        }
                    }
                }
            }
            seen.lock().expect("seen lock").insert(id.clone());
            shared.update(|s| {
                s.delivered += 1;
                s.last_event_at = Some(ev.received_at.clone());
                if attempt > 0 {
                    s.last_error = None;
                }
            });
            let _ = shared.events.send(InboxUpdate::Delivered(delivery));
        }
        if out.send(binary(Frame::json(ft::INBOX_ACK, 0, &InboxAck { ids: vec![id] }))).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seen_set_is_bounded() {
        let mut s = Seen::new(2);
        s.insert("a".into());
        s.insert("b".into());
        s.insert("a".into());
        assert!(s.contains("a") && s.contains("b"));
        s.insert("c".into());
        assert!(!s.contains("a"));
        assert!(s.contains("b") && s.contains("c"));
    }
}
