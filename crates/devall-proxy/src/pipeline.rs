//! One proxied request, start to finish: capture → rules → (request breakpoint) → (OpenAPI
//! request check) → mock / map local / block, or forward upstream → response rules → (response
//! breakpoint) → (OpenAPI response check) → stream back to the client while the tee captures the
//! body. Validation in flag mode runs after the body has streamed, off the I/O path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::request::Parts;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, Version, header};
use http_body_util::BodyExt;
use hyper::body::Body;

use crate::Inner;
use crate::body::{BoxError, ProxyBody, TeeBody, ThrottleBody, boxed, full};
use crate::breakpoints::Decision;
use crate::model::{BodyView, BreakpointEdit, CapturedBody, CapturedResponse, Exchange, ExchangeId, ExchangeState, PausedExchange, Phase, SpecValidation};
use crate::rules::{BlockMode, NameValue, Plan, RuleInput, SpecCheck, Target, Terminal, ValidateMode, apply_header_ops};
use crate::upstream::ConnMeta;
use crate::util::{error_chain, header_list, header_map, now_ms, strip_hop_by_hop, version_str};

/// Bodies larger than this skip breakpoints (they are streamed instead of held in memory).
const MAX_BREAKPOINT_BODY: u64 = 64 * 1024 * 1024;
/// Bodies larger than this are not schema-validated (status/path checks still run).
const MAX_VALIDATE_BODY: usize = 8 * 1024 * 1024;

/// Decoded UTF-8 text of a body for validation (`None` when undecodable or too large).
fn body_text(raw: &[u8], encoding: Option<&str>) -> Option<String> {
    if raw.len() > MAX_VALIDATE_BODY {
        return None;
    }
    let bytes = match encoding.filter(|e| !e.trim().is_empty() && !e.eq_ignore_ascii_case("identity")) {
        Some(enc) => crate::body::decode_body(raw, enc).ok()?,
        None => raw.to_vec(),
    };
    String::from_utf8(bytes).ok()
}

/// RFC 9457 problem response for a spec violation (block mode).
fn problem_response(status: u16, title: &str, check: &SpecCheck, violations: &[devall_openapi::Violation]) -> Response<ProxyBody> {
    let body = serde_json::json!({
        "type": "https://devall.dev/problems/openapi-violation",
        "title": title,
        "status": status,
        "detail": violations.first().map(|v| v.message.clone()).unwrap_or_default(),
        "rule": check.rule,
        "spec": check.spec_name,
        "violations": violations,
    });
    let bytes = Bytes::from(serde_json::to_vec_pretty(&body).unwrap_or_default());
    let headers = [NameValue { name: "x-devall-proxy".into(), value: "openapi-validation".into() }];
    synthetic(status, &headers, Some("application/problem+json"), bytes)
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// Runs a (possibly slow) schema validation on the blocking pool when a runtime is available.
async fn off_io<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let job = move || {
        let _ = tx.send(f());
    };
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn_blocking(job);
        }
        Err(_) => job(),
    }
    rx.await.expect("validation task ended without a result")
}

pub(crate) struct Origin {
    pub client_addr: Option<String>,
    /// Replays skip breakpoints.
    pub replay: bool,
}

enum ReqBody {
    Buffered(Bytes),
    Streaming(ProxyBody),
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

pub(crate) fn is_ws_upgrade(headers: &HeaderMap) -> bool {
    let has = |name: header::HeaderName, token: &str| {
        headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    };
    has(header::UPGRADE, "websocket") && has(header::CONNECTION, "upgrade")
}

fn text_response(status: StatusCode, msg: &str) -> Response<ProxyBody> {
    let body = format!("/dev/all proxy: {msg}\n");
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CONTENT_LENGTH, body.len())
        .header("x-devall-proxy", "error")
        .body(full(body))
        .unwrap_or_else(|_| Response::new(full("")))
}

fn synthetic(status: u16, headers: &[NameValue], content_type: Option<&str>, body: Bytes) -> Response<ProxyBody> {
    let mut map = HeaderMap::new();
    if let Some(ct) = content_type.and_then(|c| HeaderValue::from_str(c).ok()) {
        map.insert(header::CONTENT_TYPE, ct);
    }
    for h in headers {
        if let (Ok(n), Ok(v)) = (header::HeaderName::from_bytes(h.name.trim().as_bytes()), HeaderValue::from_str(&h.value)) {
            map.append(n, v);
        }
    }
    map.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    let mut resp = Response::new(full(body));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    *resp.headers_mut() = map;
    resp
}

fn sniff_type(body: &[u8]) -> &'static str {
    let t = String::from_utf8_lossy(&body[..body.len().min(512)]);
    let t = t.trim_start();
    if (t.starts_with('{') || t.starts_with('[')) && serde_json::from_slice::<serde_json::Value>(body).is_ok() {
        "application/json"
    } else if t.starts_with('<') && t.to_ascii_lowercase().contains("html") {
        "text/html; charset=utf-8"
    } else if t.starts_with("<?xml") {
        "application/xml"
    } else {
        "text/plain; charset=utf-8"
    }
}

pub(crate) fn mime_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "txt" | "log" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "yaml" | "yml" => "application/yaml",
        "csv" => "text/csv; charset=utf-8",
        _ => return None,
    })
}

async fn map_local(
    file: Option<&str>,
    body: Option<&str>,
    status: u16,
    content_type: Option<&str>,
    headers: &[NameValue],
    target: &Target,
    prefix: &str,
) -> Response<ProxyBody> {
    let (bytes, guessed) = match file {
        Some(f) => {
            let base = PathBuf::from(f);
            let path = if base.is_dir() {
                let path = target.path();
                let rest = path.strip_prefix(prefix).unwrap_or(path);
                let rel = percent_encoding::percent_decode_str(rest).decode_utf8_lossy().trim_start_matches('/').to_string();
                match devall_core::fsutil::safe_join(&base, &rel) {
                    Ok(p) if p.is_dir() => p.join("index.html"),
                    Ok(p) => p,
                    Err(_) => {
                        return text_response(StatusCode::FORBIDDEN, "Map Local: path escapes the mapped folder");
                    }
                }
            } else {
                base
            };
            match tokio::fs::read(&path).await {
                Ok(b) => (Bytes::from(b), mime_for(&path)),
                Err(e) => {
                    return text_response(StatusCode::NOT_FOUND, &format!("Map Local: cannot read {}: {e}", path.display()));
                }
            }
        }
        None => (Bytes::from(body.unwrap_or_default().to_string()), None),
    };
    let ct = content_type.filter(|c| !c.trim().is_empty()).or(guessed).unwrap_or_else(|| sniff_type(&bytes));
    synthetic(status, headers, Some(ct), bytes)
}

#[allow(clippy::too_many_arguments)]
fn paused_view(
    id: ExchangeId,
    phase: Phase,
    method: &str,
    url: &str,
    status: Option<u16>,
    headers: &HeaderMap,
    body: &Bytes,
    timeout: Duration,
) -> PausedExchange {
    let list = header_list(headers);
    let enc = headers.get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()).map(str::to_string);
    let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    PausedExchange {
        id,
        phase,
        method: method.to_string(),
        url: url.to_string(),
        status,
        headers: list,
        body: BodyView::build(&CapturedBody::new(body.clone(), usize::MAX), enc.as_deref(), ct),
        deadline: now_ms() + timeout.as_millis() as i64,
    }
}

/// Applies a breakpoint edit to a buffered message. Returns true when anything changed.
fn apply_edit(headers: &mut HeaderMap, body: &mut Bytes, edit: &BreakpointEdit) -> bool {
    let mut changed = false;
    if let Some(h) = &edit.headers {
        *headers = header_map(h);
        changed = true;
    }
    if let Some(b) = edit.new_body() {
        *body = b;
        headers.remove(header::CONTENT_ENCODING);
        headers.remove(header::TRANSFER_ENCODING);
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
        changed = true;
    }
    changed
}

/// Runs a request through the proxy. Returns the exchange id with the response for the client
/// (`Err` closes the connection: Block → close, Abort).
pub(crate) async fn handle(
    eng: &Arc<Inner>,
    origin: Origin,
    mut parts: Parts,
    body: ProxyBody,
    target: Target,
) -> (ExchangeId, Result<Response<ProxyBody>, BoxError>) {
    let rt = eng.runtime();
    let cfg = &rt.cfg;
    let started = Instant::now();
    let method = parts.method.as_str().to_string();
    let ws = is_ws_upgrade(&parts.headers);
    let mut ex = Exchange::new(&method, target.url(), version_str(parts.version), header_list(&parts.headers));
    ex.client_addr = origin.client_addr.clone();
    if origin.replay {
        ex.tag("replay");
    }
    let plan = rt.rules.plan(&RuleInput { method: &method, target: &target, headers: &parts.headers });
    ex.rules = plan.matched.clone();
    let id = eng.store.insert(ex);
    let store = eng.store.clone();
    let mut blocked_ms = 0.0;

    apply_header_ops(&mut parts.headers, &plan.req_headers);
    let mut dest = target.clone();
    let preserve_host = plan.map_remote.as_ref().is_some_and(|m| m.preserve_host);
    if let Some(mr) = &plan.map_remote {
        dest = mr.apply(&target);
        if dest != target {
            let url = dest.url();
            store.update(id, |e| {
                e.mapped_url = Some(url);
                e.tag("mapRemote");
            });
        }
    }

    // Request body: held in memory when a breakpoint or a local answer needs it, else streamed.
    let want_bp = plan.bp_request && !origin.replay;
    let small = body.size_hint().upper().is_none_or(|u| u <= MAX_BREAKPOINT_BODY);
    let mut req_body = if (want_bp || plan.terminal.is_some() || plan.validate.is_some()) && small && !ws {
        match body.collect().await {
            Ok(c) => {
                let bytes = c.to_bytes();
                let cap = cfg.max_body_bytes;
                let captured = CapturedBody::new(bytes.clone(), cap);
                store.update(id, |e| e.request.body = captured);
                ReqBody::Buffered(bytes)
            }
            Err(e) => {
                let msg = format!("reading the request body failed: {}", error_chain(&*e));
                fail(eng, id, started, &msg);
                return (id, Err(e));
            }
        }
    } else {
        let s = store.clone();
        let tee = TeeBody::new(body, cfg.max_body_bytes, move |o| {
            s.update_quiet(id, |e| e.request.body = CapturedBody { raw: o.data, size: o.size, truncated: o.truncated, evicted: false });
        });
        ReqBody::Streaming(boxed(tee))
    };

    if want_bp && let ReqBody::Buffered(bytes) = &mut req_body {
        let timeout = Duration::from_secs(cfg.breakpoint_timeout_secs);
        let info = paused_view(id, Phase::Request, &method, &dest.url(), None, &parts.headers, bytes, timeout);
        store.update(id, |e| {
            e.state = ExchangeState::Paused;
            e.paused = Some(Phase::Request);
        });
        let t = Instant::now();
        let decision = eng.breakpoints.wait(info, timeout, &eng.events).await;
        blocked_ms += ms(t);
        match decision {
            Decision::Abort => {
                store.update(id, |e| {
                    e.paused = None;
                    e.state = ExchangeState::Aborted;
                    e.error = Some("aborted at the request breakpoint".into());
                    e.timing.blocked_ms = Some(blocked_ms);
                    e.timing.total_ms = Some(ms(started));
                });
                return (id, Err("aborted at a breakpoint".into()));
            }
            Decision::TimedOut => {
                store.update(id, |e| e.tag("breakpointTimeout"));
            }
            Decision::Resume(None) => {}
            Decision::Resume(Some(edit)) => {
                let mut changed = false;
                if let Some(m) = edit.method.as_deref().map(str::trim).filter(|m| !m.is_empty())
                    && let Ok(m) = Method::from_bytes(m.to_ascii_uppercase().as_bytes())
                {
                    changed |= m != parts.method;
                    parts.method = m;
                }
                if let Some(u) = edit.url.as_deref().map(str::trim).filter(|u| !u.is_empty())
                    && let Some(t) = Target::parse(u)
                {
                    changed |= t != dest;
                    dest = t;
                }
                changed |= apply_edit(&mut parts.headers, bytes, &edit);
                if changed {
                    let (m, url, hdrs, b, cap) = (parts.method.to_string(), dest.url(), header_list(&parts.headers), bytes.clone(), cfg.max_body_bytes);
                    let orig = target.url();
                    store.update(id, |e| {
                        e.request.method = m;
                        e.request.headers = hdrs;
                        e.request.body = CapturedBody::new(b, cap);
                        if url != orig {
                            e.mapped_url = Some(url);
                        }
                        e.tag("edited");
                    });
                }
            }
        }
        store.update(id, |e| {
            e.paused = None;
            e.state = ExchangeState::Pending;
            e.timing.blocked_ms = Some(blocked_ms);
        });
    }

    if plan.latency_ms > 0 {
        tokio::time::sleep(Duration::from_millis(plan.latency_ms)).await;
        blocked_ms += plan.latency_ms as f64;
        store.update(id, |e| {
            e.tag("throttled");
            e.timing.blocked_ms = Some(blocked_ms);
        });
    } else if plan.bytes_per_sec.is_some() {
        store.update(id, |e| e.tag("throttled"));
    }

    // OpenAPI request check (after edits and header rules, before anything is answered).
    if let Some(check) = plan.validate.as_ref().filter(|_| !ws) {
        let url = target.url();
        if check.conformance.covers(&url) {
            let method = parts.method.as_str().to_string();
            let ct = header_str(&parts.headers, header::CONTENT_TYPE);
            let enc = header_str(&parts.headers, header::CONTENT_ENCODING);
            let (body, have_body) = match &req_body {
                ReqBody::Buffered(b) => (body_text(b, enc.as_deref()), true),
                ReqBody::Streaming(_) => (None, false),
            };
            let conf = check.conformance.clone();
            let mut violations = off_io(move || conf.check_request(&method, &url, ct.as_deref(), body.as_deref())).await;
            if !have_body {
                // A streamed (very large) body wasn't held: only path and method were checked.
                violations.retain(|v| v.kind != devall_openapi::drift::DriftKind::InvalidRequestBody);
            }
            let blocked = check.mode == ValidateMode::Block && !violations.is_empty();
            let found = violations.clone();
            store.update(id, |e| {
                e.validation = Some(SpecValidation { rule: check.rule.clone(), spec: check.spec_name.clone(), blocked, request: found, ..Default::default() });
                if !violations.is_empty() {
                    e.tag("specViolation");
                }
                if blocked {
                    e.tag("blocked");
                }
            });
            if blocked {
                let resp = problem_response(422, "The request does not match the OpenAPI spec", check, &violations);
                let mut answer = plan.clone();
                answer.validate = None;
                return (id, respond(eng, id, &answer, resp, started, ExchangeState::Blocked, origin.replay).await);
            }
        }
    }

    if let Some(term) = &plan.terminal {
        let (resp, tag, state) = match term {
            Terminal::Block(BlockMode::Close) => {
                store.update(id, |e| {
                    e.state = ExchangeState::Blocked;
                    e.error = Some("connection closed by a block rule".into());
                    e.tag("blocked");
                    e.timing.total_ms = Some(ms(started));
                });
                return (id, Err("blocked by rule".into()));
            }
            Terminal::Block(BlockMode::Forbidden) => (text_response(StatusCode::FORBIDDEN, "blocked by a rule"), "blocked", ExchangeState::Blocked),
            Terminal::Mock { status, headers, body, delay_ms } => {
                if *delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
                    blocked_ms += *delay_ms as f64;
                }
                let has_ct = headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-type"));
                let ct = if has_ct { None } else { Some(sniff_type(body.as_bytes())) };
                (synthetic(*status, headers, ct, Bytes::from(body.clone())), "mocked", ExchangeState::Complete)
            }
            Terminal::MapLocal { file, body, status, content_type, headers, prefix } => (
                map_local(file.as_deref(), body.as_deref(), *status, content_type.as_deref(), headers, &target, prefix).await,
                "mapLocal",
                ExchangeState::Complete,
            ),
        };
        store.update(id, |e| {
            e.tag(tag);
            e.timing.blocked_ms = (blocked_ms > 0.0).then_some(blocked_ms);
        });
        return (id, respond(eng, id, &plan, resp, started, state, origin.replay).await);
    }

    if ws {
        return (id, crate::ws::handle(eng, id, parts, dest, preserve_host, started).await);
    }

    // Forward upstream.
    let original_host = parts.headers.get(header::HOST).cloned();
    let te_trailers = parts.headers.get(header::TE).is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"trailers"));
    strip_hop_by_hop(&mut parts.headers);
    parts.headers.remove(header::HOST);
    parts.headers.remove(header::EXPECT);
    if te_trailers {
        parts.headers.insert(header::TE, HeaderValue::from_static("trailers"));
    }
    if preserve_host && let Some(h) = original_host {
        parts.headers.insert(header::HOST, h);
    }
    let uri: Uri = match dest.url().parse() {
        Ok(u) => u,
        Err(e) => {
            let msg = format!("invalid upstream URL {}: {e}", dest.url());
            fail(eng, id, started, &msg);
            return (id, Ok(text_response(StatusCode::BAD_REQUEST, &msg)));
        }
    };
    parts.uri = uri;
    parts.version = Version::HTTP_11;
    let body = match req_body {
        ReqBody::Buffered(b) => full(b),
        ReqBody::Streaming(b) => b,
    };
    let upstream_req = Request::from_parts(parts, body);
    let sent = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(cfg.upstream_timeout_secs), rt.upstream.client.request(upstream_req)).await;
    let resp = match result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let msg = error_chain(&e);
            fail(eng, id, started, &msg);
            return (id, Ok(text_response(StatusCode::BAD_GATEWAY, &msg)));
        }
        Err(_) => {
            let msg = format!("no response from {} within {} s", dest.authority(), cfg.upstream_timeout_secs);
            fail(eng, id, started, &msg);
            return (id, Ok(text_response(StatusCode::GATEWAY_TIMEOUT, &msg)));
        }
    };
    let ttfb = ms(sent);
    let meta = resp.extensions().get::<ConnMeta>().cloned();
    store.update(id, |e| {
        if let Some(m) = &meta {
            e.server_addr = m.remote_addr.map(|a| a.to_string());
            if m.first_use() {
                e.timing.dns_ms = m.dns_ms;
                e.timing.connect_ms = Some(m.connect_ms);
                e.timing.tls_ms = m.tls_ms;
                e.timing.connection_reused = Some(false);
                let setup = m.dns_ms.unwrap_or(0.0) + m.connect_ms + m.tls_ms.unwrap_or(0.0);
                e.timing.wait_ms = Some((ttfb - setup).max(0.0));
            } else {
                e.timing.connection_reused = Some(true);
                e.timing.wait_ms = Some(ttfb);
            }
        } else {
            e.timing.wait_ms = Some(ttfb);
        }
    });
    let resp = resp.map(boxed);
    (id, respond(eng, id, &plan, resp, started, ExchangeState::Complete, origin.replay).await)
}

fn fail(eng: &Inner, id: ExchangeId, started: Instant, msg: &str) {
    eng.store.update(id, |e| {
        e.state = ExchangeState::Failed;
        e.error = Some(msg.to_string());
        e.paused = None;
        e.timing.total_ms = Some(ms(started));
    });
}

/// Applies response rules and the response breakpoint, records the head, and returns the
/// response with a tee (and throttle) around its body.
pub(crate) async fn respond(
    eng: &Arc<Inner>,
    id: ExchangeId,
    plan: &Plan,
    resp: Response<ProxyBody>,
    started: Instant,
    final_state: ExchangeState,
    replay: bool,
) -> Result<Response<ProxyBody>, BoxError> {
    let rt = eng.runtime();
    let cfg = &rt.cfg;
    let store = eng.store.clone();
    let (mut parts, mut body) = resp.into_parts();
    let http_version = version_str(parts.version).to_string();
    strip_hop_by_hop(&mut parts.headers);
    apply_header_ops(&mut parts.headers, &plan.resp_headers);
    let mut reason = parts
        .extensions
        .get::<hyper::ext::ReasonPhrase>()
        .map(|r| String::from_utf8_lossy(r.as_bytes()).into_owned())
        .unwrap_or_else(|| parts.status.canonical_reason().unwrap_or("").to_string());
    let t_headers = Instant::now();

    let small = body.size_hint().upper().is_none_or(|u| u <= MAX_BREAKPOINT_BODY);
    if plan.bp_response && !replay && small {
        let mut bytes = match body.collect().await {
            Ok(c) => c.to_bytes(),
            Err(e) => {
                fail(eng, id, started, &format!("reading the response body failed: {}", error_chain(&*e)));
                return Err(e);
            }
        };
        let timeout = Duration::from_secs(cfg.breakpoint_timeout_secs);
        let (method, url) = store.update(id, |e| (e.request.method.clone(), e.mapped_url.clone().unwrap_or_else(|| e.request.url.clone()))).unwrap_or_default();
        let info = paused_view(id, Phase::Response, &method, &url, Some(parts.status.as_u16()), &parts.headers, &bytes, timeout);
        let head = CapturedResponse {
            status: parts.status.as_u16(),
            reason: reason.clone(),
            http_version: http_version.clone(),
            headers: header_list(&parts.headers),
            body: CapturedBody::new(bytes.clone(), cfg.max_body_bytes),
        };
        store.update(id, |e| {
            e.response = Some(head);
            e.state = ExchangeState::Paused;
            e.paused = Some(Phase::Response);
        });
        let t = Instant::now();
        let decision = eng.breakpoints.wait(info, timeout, &eng.events).await;
        let paused_ms = ms(t);
        store.update(id, |e| {
            e.paused = None;
            e.timing.blocked_ms = Some(e.timing.blocked_ms.unwrap_or(0.0) + paused_ms);
        });
        match decision {
            Decision::Abort => {
                store.update(id, |e| {
                    e.state = ExchangeState::Aborted;
                    e.error = Some("aborted at the response breakpoint".into());
                    e.timing.total_ms = Some(ms(started));
                });
                return Err("aborted at a breakpoint".into());
            }
            Decision::TimedOut => {
                store.update(id, |e| e.tag("breakpointTimeout"));
            }
            Decision::Resume(None) => {}
            Decision::Resume(Some(edit)) => {
                let mut changed = apply_edit(&mut parts.headers, &mut bytes, &edit);
                if let Some(s) = edit.status.and_then(|s| StatusCode::from_u16(s).ok())
                    && s != parts.status
                {
                    parts.status = s;
                    reason = s.canonical_reason().unwrap_or("").to_string();
                    changed = true;
                }
                if changed {
                    store.update(id, |e| e.tag("edited"));
                }
            }
        }
        body = full(bytes);
    }

    // OpenAPI response check: only for exchanges whose request was checked (same rule, covered).
    let check = plan.validate.clone().filter(|_| store.update_quiet(id, |e| e.validation.is_some()).unwrap_or(false));
    let mut flag_check = check.clone();
    if let Some(c) = check.as_ref().filter(|c| c.mode == ValidateMode::Block) {
        let streaming = header_str(&parts.headers, header::CONTENT_TYPE).is_some_and(|t| t.to_ascii_lowercase().contains("event-stream"));
        let small = body.size_hint().upper().is_none_or(|u| u <= MAX_BREAKPOINT_BODY);
        if small && !streaming {
            let bytes = match body.collect().await {
                Ok(col) => col.to_bytes(),
                Err(e) => {
                    fail(eng, id, started, &format!("reading the response body failed: {}", error_chain(&*e)));
                    return Err(e);
                }
            };
            let status = parts.status.as_u16();
            let ct = header_str(&parts.headers, header::CONTENT_TYPE);
            let text = body_text(&bytes, header_str(&parts.headers, header::CONTENT_ENCODING).as_deref());
            let skipped = text.is_none() && !bytes.is_empty();
            let (method, url) = store.update_quiet(id, |e| (e.request.method.clone(), e.request.url.clone())).unwrap_or_default();
            let conf = c.conformance.clone();
            let violations = off_io(move || conf.check_response(&method, &url, status, ct.as_deref(), text.as_deref())).await;
            let blocked = !violations.is_empty();
            let found = violations.clone();
            store.update(id, |e| {
                if let Some(v) = e.validation.as_mut() {
                    v.response = found;
                    v.response_checked = true;
                    v.response_body_skipped = skipped;
                    v.blocked |= blocked;
                }
                if blocked {
                    e.tag("specViolation");
                    e.tag("blocked");
                }
            });
            if blocked {
                let (p2, b2) = problem_response(502, "The response does not match the OpenAPI spec", c, &violations).into_parts();
                parts.status = p2.status;
                parts.headers = p2.headers;
                reason = "Bad Gateway".into();
                body = b2;
            } else {
                body = full(bytes);
            }
            flag_check = None;
        }
    }

    let status = parts.status.as_u16();
    let head = CapturedResponse { status, reason, http_version, headers: header_list(&parts.headers), body: CapturedBody::default() };
    store.update(id, |e| {
        e.response = Some(head);
        e.state = ExchangeState::Receiving;
    });
    let s = store.clone();
    // Flag mode: validate once the body has streamed to the client, off the I/O path.
    let flag = flag_check.map(|c| (c, status, header_str(&parts.headers, header::CONTENT_TYPE), header_str(&parts.headers, header::CONTENT_ENCODING)));
    let tee = TeeBody::new(body, cfg.max_body_bytes, move |o| {
        if let Some((c, status, ct, enc)) = flag
            && o.complete
        {
            let (raw, truncated, s2) = (o.data.clone(), o.truncated, s.clone());
            let run = async move {
                let (method, url) = s2.update_quiet(id, |e| (e.request.method.clone(), e.request.url.clone())).unwrap_or_default();
                let conf = c.conformance.clone();
                let (violations, skipped) = off_io(move || {
                    let text = if truncated { None } else { body_text(&raw, enc.as_deref()) };
                    let skipped = text.is_none() && !raw.is_empty();
                    (conf.check_response(&method, &url, status, ct.as_deref(), text.as_deref()), skipped)
                })
                .await;
                s2.update(id, |e| {
                    if !violations.is_empty() {
                        e.tag("specViolation");
                    }
                    if let Some(v) = e.validation.as_mut() {
                        v.response = violations;
                        v.response_checked = true;
                        v.response_body_skipped = skipped;
                    }
                });
            };
            if let Ok(h) = tokio::runtime::Handle::try_current() {
                h.spawn(run);
            }
        }
        s.update(id, |e| {
            if let Some(r) = e.response.as_mut() {
                r.body = CapturedBody { raw: o.data, size: o.size, truncated: o.truncated, evicted: false };
            }
            e.timing.receive_ms = Some(ms(t_headers));
            e.timing.total_ms = Some(ms(started));
            if o.complete {
                e.state = final_state;
            } else {
                e.state = ExchangeState::Failed;
                e.error = o.error;
            }
        });
    });
    let body = match plan.bytes_per_sec {
        Some(bps) => boxed(ThrottleBody::new(tee, bps)),
        None => boxed(tee),
    };
    parts.version = Version::HTTP_11;
    Ok(Response::from_parts(parts, body))
}
