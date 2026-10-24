//! Desktop sign-in for the hosted relay (RFC 8252-style loopback flow).
//!
//! 1. Bind `127.0.0.1:<random port>` and make a random `state`.
//! 2. Open `<signin page>?port=<port>&state=<state>` in the user's browser (the caller supplies
//!    the opener, e.g. tauri-plugin-opener).
//! 3. The hosted page signs the user in with the Firebase JS SDK and POSTs (or GETs)
//!    `refresh_token`, `state`, `uid`, `email` to `http://127.0.0.1:<port>/callback`.
//! 4. The callback is accepted only with the exact `state`; the refresh token goes straight into
//!    the OS keychain.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use devall_core::SecretStore;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::auth::store_refresh_token;
use crate::error::{RelayError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedIn {
    pub uid: Option<String>,
    pub email: Option<String>,
}

/// What the browser delivered to the loopback callback.
#[derive(Debug, Clone)]
struct Callback {
    refresh_token: String,
    uid: Option<String>,
    email: Option<String>,
}

fn random_state() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Constant-time string comparison (the state is a CSRF secret).
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

fn page(status: StatusCode, title: &str, message: &str) -> Response<Full<Bytes>> {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title><style>body{{font:15px system-ui,sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;color:#1d1d1f;background:#f7f7f8}}@media (prefers-color-scheme:dark){{body{{color:#ececef;background:#111113}}}}main{{max-width:420px;text-align:center}}</style></head><body><main><h1>{title}</h1><p>{message}</p></main></body></html>"
    );
    let mut r = Response::new(Full::new(Bytes::from(html)));
    *r.status_mut() = status;
    r.headers_mut().insert(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8".parse().expect("static header"));
    r.headers_mut().insert(hyper::header::CACHE_CONTROL, "no-store".parse().expect("static header"));
    r.headers_mut().insert("referrer-policy", "no-referrer".parse().expect("static header"));
    r
}

type Slot = Arc<Mutex<Option<oneshot::Sender<Callback>>>>;

async fn handle(req: Request<Incoming>, state: Arc<String>, slot: Slot) -> std::result::Result<Response<Full<Bytes>>, Infallible> {
    if req.uri().path() != "/callback" {
        return Ok(page(StatusCode::NOT_FOUND, "Not found", "This address only completes /dev/all sign-in."));
    }
    let query = req.uri().query().unwrap_or("").to_string();
    let form = match *req.method() {
        Method::GET => query,
        Method::POST => match Limited::new(req.into_body(), 16 * 1024).collect().await {
            Ok(b) => String::from_utf8_lossy(&b.to_bytes()).into_owned(),
            Err(_) => return Ok(page(StatusCode::PAYLOAD_TOO_LARGE, "Sign-in failed", "The callback was too large.")),
        },
        _ => return Ok(page(StatusCode::METHOD_NOT_ALLOWED, "Sign-in failed", "Unexpected request.")),
    };
    let mut got_state = None;
    let mut refresh_token = None;
    let mut uid = None;
    let mut email = None;
    let mut error = None;
    for (k, v) in url::form_urlencoded::parse(form.as_bytes()) {
        match k.as_ref() {
            "state" => got_state = Some(v.into_owned()),
            "refresh_token" => refresh_token = Some(v.into_owned()),
            "uid" => uid = Some(v.into_owned()).filter(|s| !s.is_empty()),
            "email" => email = Some(v.into_owned()).filter(|s| !s.is_empty()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if !got_state.as_deref().is_some_and(|s| same(s, &state)) {
        return Ok(page(StatusCode::BAD_REQUEST, "Sign-in failed", "This sign-in link doesn't match the one /dev/all opened. Start again from the app."));
    }
    if let Some(e) = error {
        return Ok(page(StatusCode::BAD_REQUEST, "Sign-in failed", &format!("The sign-in page reported: {}", html_escape(&e))));
    }
    let Some(refresh_token) = refresh_token.filter(|t| !t.is_empty()) else {
        return Ok(page(StatusCode::BAD_REQUEST, "Sign-in failed", "No credentials were received."));
    };
    if let Some(tx) = slot.lock().expect("slot lock").take() {
        let _ = tx.send(Callback { refresh_token, uid, email });
    }
    Ok(page(StatusCode::OK, "Signed in", "You're signed in to the /dev/all relay. You can close this tab and return to the app."))
}

fn html_escape(s: &str) -> String {
    s.chars()
        .take(200)
        .map(|c| match c {
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '&' => "&amp;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Runs the loopback sign-in. `open` receives the URL to show in the browser.
pub async fn sign_in_with_browser<F>(signin_page: &str, secrets: &dyn SecretStore, open: F, timeout: Duration) -> Result<SignedIn>
where
    F: FnOnce(&str) -> std::result::Result<(), String>,
{
    let mut page_url = url::Url::parse(signin_page).map_err(|e| RelayError::Config(format!("sign-in page url: {e}")))?;
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(random_state());
    page_url.query_pairs_mut().append_pair("port", &port.to_string()).append_pair("state", &state);

    let (tx, rx) = oneshot::channel::<Callback>();
    let slot: Slot = Arc::new(Mutex::new(Some(tx)));
    let server = {
        let state = state.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { continue };
                let state = state.clone();
                let slot = slot.clone();
                tokio::spawn(async move {
                    let svc = service_fn(move |req| handle(req, state.clone(), slot.clone()));
                    let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
                });
            }
        })
    };

    open(page_url.as_str()).map_err(|e| {
        server.abort();
        RelayError::SignIn(format!("could not open the browser: {e}"))
    })?;
    let got = tokio::time::timeout(timeout, rx).await;
    // Give the browser a moment to receive the "signed in" page before the server goes away.
    tokio::time::sleep(Duration::from_millis(200)).await;
    server.abort();
    let cb = match got {
        Ok(Ok(cb)) => cb,
        Ok(Err(_)) => return Err(RelayError::SignIn("the sign-in server stopped".into())),
        Err(_) => return Err(RelayError::Timeout("sign-in wasn't completed in the browser".into())),
    };
    store_refresh_token(secrets, &cb.refresh_token)?;
    Ok(SignedIn { uid: cb.uid, email: cb.email })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_compare_is_exact() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "ab"));
        assert!(!same("", "a"));
        assert_eq!(random_state().len(), 64);
        assert_ne!(random_state(), random_state());
    }

    #[test]
    fn escapes_error_text() {
        assert_eq!(html_escape("<b>&\""), "&lt;b&gt;&amp;&quot;");
    }
}
