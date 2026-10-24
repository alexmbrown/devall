//! WebSocket pass-through with frame capture. The upgrade request goes upstream on its own
//! HTTP/1.1 connection (without `Sec-WebSocket-Extensions`, so frames stay uncompressed and
//! readable); after both sides switch protocols, messages are relayed and recorded.

use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use http::request::Parts;
use http::{HeaderValue, Request, Response, StatusCode, Version, header};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::Inner;
use crate::body::{BoxError, ProxyBody, boxed, empty, full};
use crate::model::{CapturedResponse, ExchangeId, ExchangeState, WsDirection, WsFrame};
use crate::rules::{Plan, Target};
use crate::util::{error_chain, header_list};

/// Text/binary payload kept per frame.
const MAX_FRAME_CAPTURE: usize = 256 * 1024;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn bad_gateway(msg: &str) -> Response<ProxyBody> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full(format!("/dev/all proxy: {msg}\n")))
        .unwrap_or_else(|_| Response::new(empty()))
}

pub(crate) async fn handle(
    eng: &Arc<Inner>,
    id: ExchangeId,
    mut parts: Parts,
    dest: Target,
    preserve_host: bool,
    started: Instant,
) -> Result<Response<ProxyBody>, BoxError> {
    let store = eng.store.clone();
    let fail = |msg: String| {
        store.update(id, |e| {
            e.state = ExchangeState::Failed;
            e.error = Some(msg);
            e.timing.total_ms = Some(ms(started));
        });
    };
    let Some(client_upgrade) = parts.extensions.remove::<hyper::upgrade::OnUpgrade>() else {
        fail("this client connection cannot be upgraded to a WebSocket".into());
        return Ok(bad_gateway("WebSocket upgrade is not possible on this connection"));
    };
    let https = matches!(dest.scheme.as_str(), "https" | "wss");
    let upstream = eng.runtime().upstream.clone();
    let io = match upstream.shared.connect(https, &dest.host, dest.port, false).await {
        Ok(io) => io,
        Err(e) => {
            let msg = e.to_string();
            fail(msg.clone());
            return Ok(bad_gateway(&msg));
        }
    };
    let meta = io.meta.clone();
    let (mut sender, conn) = match hyper::client::conn::http1::Builder::new().preserve_header_case(true).handshake::<_, ProxyBody>(TokioIo::new(io)).await {
        Ok(x) => x,
        Err(e) => {
            let msg = error_chain(&e);
            fail(msg.clone());
            return Ok(bad_gateway(&msg));
        }
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });

    let mut headers = parts.headers.clone();
    for h in ["sec-websocket-extensions", "proxy-connection", "proxy-authorization", "keep-alive"] {
        headers.remove(h);
    }
    let original_host = headers.remove(header::HOST);
    let host = match (preserve_host, original_host) {
        (true, Some(h)) => h,
        _ => HeaderValue::from_str(&dest.authority()).unwrap_or_else(|_| HeaderValue::from_static("localhost")),
    };
    headers.insert(header::HOST, host);
    let mut req = Request::new(empty());
    *req.method_mut() = parts.method.clone();
    *req.uri_mut() = dest.path_and_query.parse().unwrap_or_else(|_| http::Uri::from_static("/"));
    *req.version_mut() = Version::HTTP_11;
    *req.headers_mut() = headers;
    let sent = Instant::now();
    let mut resp = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => {
            let msg = error_chain(&e);
            fail(msg.clone());
            return Ok(bad_gateway(&msg));
        }
    };
    let ttfb = ms(sent);
    store.update(id, |e| {
        e.server_addr = meta.remote_addr.map(|a| a.to_string());
        e.timing.dns_ms = meta.dns_ms;
        e.timing.connect_ms = Some(meta.connect_ms);
        e.timing.tls_ms = meta.tls_ms;
        e.timing.wait_ms = Some(ttfb);
        e.timing.connection_reused = Some(false);
    });

    if resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        // The server declined the upgrade: an ordinary response.
        let resp = resp.map(boxed);
        return crate::pipeline::respond(eng, id, &Plan::default(), resp, started, ExchangeState::Complete, false).await;
    }

    let resp_headers = resp.headers().clone();
    store.update(id, |e| {
        e.response = Some(CapturedResponse {
            status: 101,
            reason: "Switching Protocols".into(),
            http_version: "HTTP/1.1".into(),
            headers: header_list(&resp_headers),
            body: Default::default(),
        });
        e.state = ExchangeState::WebSocket;
    });
    let server_upgrade = hyper::upgrade::on(&mut resp);
    let eng2 = eng.clone();
    tokio::spawn(async move {
        match tokio::join!(client_upgrade, server_upgrade) {
            (Ok(c), Ok(s)) => relay(eng2, id, TokioIo::new(c), TokioIo::new(s), started).await,
            (c, s) => {
                let msg = c.err().or(s.err()).map(|e| error_chain(&e)).unwrap_or_default();
                eng2.store.update(id, |e| {
                    e.state = ExchangeState::Failed;
                    e.error = Some(format!("WebSocket upgrade failed: {msg}"));
                });
            }
        }
    });

    let mut out = Response::new(empty());
    *out.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    *out.headers_mut() = resp_headers;
    Ok(out)
}

fn frame(direction: WsDirection, msg: &Message, at_ms: f64) -> Option<WsFrame> {
    let (opcode, data, is_text): (&str, &[u8], bool) = match msg {
        Message::Text(t) => ("text", t.as_bytes(), true),
        Message::Binary(b) => ("binary", b.as_ref(), false),
        Message::Ping(b) => ("ping", b.as_ref(), false),
        Message::Pong(b) => ("pong", b.as_ref(), false),
        Message::Close(c) => {
            let text = c.as_ref().map(|c| format!("{} {}", u16::from(c.code), c.reason.as_str())).unwrap_or_default();
            return Some(WsFrame { direction, at_ms, opcode: "close".into(), size: text.len(), text: Some(text), base64: None, truncated: false });
        }
        Message::Frame(_) => return None,
    };
    let truncated = data.len() > MAX_FRAME_CAPTURE;
    let kept = &data[..data.len().min(MAX_FRAME_CAPTURE)];
    let (text, base64) = if is_text {
        (Some(String::from_utf8_lossy(kept).into_owned()), None)
    } else {
        match std::str::from_utf8(kept) {
            Ok(t) if !t.is_empty() && opcode != "binary" => (Some(t.to_string()), None),
            _ => (None, Some(base64::engine::general_purpose::STANDARD.encode(kept))),
        }
    };
    Some(WsFrame { direction, at_ms, opcode: opcode.into(), size: data.len(), text, base64, truncated })
}

async fn relay<C, S>(eng: Arc<Inner>, id: ExchangeId, client: C, server: S, started: Instant)
where
    C: AsyncRead + AsyncWrite + Unpin + Send,
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let max_frames = eng.runtime().cfg.max_ws_frames;
    let client_ws = WebSocketStream::from_raw_socket(client, Role::Server, None).await;
    let server_ws = WebSocketStream::from_raw_socket(server, Role::Client, None).await;
    let (mut c_tx, mut c_rx) = client_ws.split();
    let (mut s_tx, mut s_rx) = server_ws.split();
    let mut last_emit = Instant::now();
    let record = |dir: WsDirection, msg: &Message, last_emit: &mut Instant| {
        if let Some(f) = frame(dir, msg, ms(started)) {
            eng.store.update_quiet(id, |e| {
                e.ws_frame_count += 1;
                if e.ws_frames.len() < max_frames {
                    e.ws_frames.push(f);
                }
            });
            if last_emit.elapsed().as_millis() >= 200 {
                *last_emit = Instant::now();
                eng.store.touch(id);
            }
        }
    };
    let mut error: Option<String> = None;
    loop {
        tokio::select! {
            m = c_rx.next() => match m {
                Some(Ok(msg)) => {
                    record(WsDirection::Send, &msg, &mut last_emit);
                    if let Err(e) = s_tx.send(msg).await {
                        if !matches!(e, tokio_tungstenite::tungstenite::Error::ConnectionClosed | tokio_tungstenite::tungstenite::Error::AlreadyClosed) {
                            error = Some(format!("sending to the server failed: {e}"));
                        }
                        break;
                    }
                }
                Some(Err(e)) => {
                    if !matches!(e, tokio_tungstenite::tungstenite::Error::ConnectionClosed | tokio_tungstenite::tungstenite::Error::AlreadyClosed) {
                        error = Some(format!("client connection: {e}"));
                    }
                    let _ = s_tx.close().await;
                    break;
                }
                None => {
                    let _ = s_tx.close().await;
                    break;
                }
            },
            m = s_rx.next() => match m {
                Some(Ok(msg)) => {
                    record(WsDirection::Receive, &msg, &mut last_emit);
                    if let Err(e) = c_tx.send(msg).await {
                        if !matches!(e, tokio_tungstenite::tungstenite::Error::ConnectionClosed | tokio_tungstenite::tungstenite::Error::AlreadyClosed) {
                            error = Some(format!("sending to the client failed: {e}"));
                        }
                        break;
                    }
                }
                Some(Err(e)) => {
                    if !matches!(e, tokio_tungstenite::tungstenite::Error::ConnectionClosed | tokio_tungstenite::tungstenite::Error::AlreadyClosed) {
                        error = Some(format!("server connection: {e}"));
                    }
                    let _ = c_tx.close().await;
                    break;
                }
                None => {
                    let _ = c_tx.close().await;
                    break;
                }
            },
        }
    }
    eng.store.update(id, |e| {
        e.timing.total_ms = Some(ms(started));
        e.tag("closed");
        if let Some(err) = error {
            e.error = Some(err);
        }
    });
}
