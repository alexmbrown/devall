//! A tiny keep-alive HTTP/1.1 server for engine tests.
//!
//! Routes: `/delay/<ms>` sleeps, `/status/<code>` answers that status, `/login` sets a session
//! cookie, `/me` answers 401 without it, anything else answers `200 ok`. `stall_for` makes every
//! request (in flight or arriving) wait until the stall ends.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
pub struct ServerState {
    pub requests: AtomicU64,
    pub connections: AtomicU32,
    pub in_flight: AtomicU32,
    pub max_in_flight: AtomicU32,
    stall_until: Mutex<Option<Instant>>,
    /// `<path> | <cookie header>` per request.
    pub log: Mutex<Vec<String>>,
}

impl ServerState {
    pub fn stall_for(&self, d: Duration) {
        *self.stall_until.lock().unwrap() = Some(Instant::now() + d);
    }

    pub fn paths(&self) -> Vec<String> {
        self.log.lock().unwrap().iter().map(|l| l.split(" | ").next().unwrap_or_default().to_string()).collect()
    }
}

pub struct TestServer {
    pub base: String,
    pub state: Arc<ServerState>,
}

pub async fn start() -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(ServerState::default());
    let st = state.clone();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { return };
            let _ = sock.set_nodelay(true);
            tokio::spawn(connection(sock, st.clone()));
        }
    });
    TestServer { base: format!("http://{addr}"), state }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

async fn connection(mut sock: TcpStream, st: Arc<ServerState>) {
    st.connections.fetch_add(1, Ordering::SeqCst);
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        let head_end = loop {
            if let Some(p) = find(&buf, b"\r\n\r\n") {
                break p;
            }
            match sock.read(&mut tmp).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let header = |name: &str| {
            head.lines().skip(1).find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
            })
        };
        let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        while buf.len() < head_end + 4 + len {
            match sock.read(&mut tmp).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
        buf.drain(..head_end + 4 + len);
        let path = head.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("/").to_string();
        let cookie = header("cookie").unwrap_or_default();
        st.requests.fetch_add(1, Ordering::SeqCst);
        st.log.lock().unwrap().push(format!("{path} | {cookie}"));
        let now = st.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        st.max_in_flight.fetch_max(now, Ordering::SeqCst);

        let stall = *st.stall_until.lock().unwrap();
        if let Some(until) = stall
            && Instant::now() < until
        {
            tokio::time::sleep_until(tokio::time::Instant::from_std(until)).await;
        }
        let route = path.split('?').next().unwrap_or("/");
        let mut status = 200u16;
        let mut extra = String::new();
        if let Some(ms) = route.strip_prefix("/delay/").and_then(|v| v.parse::<u64>().ok()) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        } else if let Some(code) = route.strip_prefix("/status/").and_then(|v| v.parse::<u16>().ok()) {
            status = code;
        } else if route == "/login" {
            extra.push_str("Set-Cookie: sid=abc123; Path=/\r\n");
        } else if route == "/me" && !cookie.contains("sid=abc123") {
            status = 401;
        }
        st.in_flight.fetch_sub(1, Ordering::SeqCst);
        let body = if status < 400 { "ok" } else { "no" };
        let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n{extra}\r\n{body}", body.len());
        if sock.write_all(resp.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// A port with nothing listening (connections are refused).
pub fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}
