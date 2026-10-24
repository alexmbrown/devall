//! End-to-end tests: real receivers on `127.0.0.1:0`. The forward/replay target is a second
//! receiver (another temp project), so the test sees exactly the bytes the local app would get.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::event::{EventQuery, header};
use crate::model::{ForwardSpec, HmacAlgorithm, InboxFile, ProviderKind};
use crate::verify::{SignInput, VerifyOptions, VerifyStatus, hmac, verify};
use crate::*;

const SECRET: &str = "whsec_test_secret";

struct Side {
    _dir: tempfile::TempDir,
    hub: Arc<WebhookHub>,
    rx: Receiver,
}

impl Side {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.rx.info().local_url)
    }
}

async fn side(inboxes: Vec<(&str, InboxFile)>, secrets: &[(&str, &str)]) -> Side {
    let dir = tempfile::tempdir().unwrap();
    for (id, f) in &inboxes {
        crate::inbox::save(dir.path(), id, f).unwrap();
    }
    let t = StaticTemplates { secrets: secrets.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), ..Default::default() };
    devall_core::local::isolate_for_tests();
    let hub = WebhookHub::open(dir.path(), Arc::new(t)).unwrap();
    let rx = Receiver::start(hub.clone(), ReceiverConfig { port: 0, bind_all: false }).await.unwrap();
    Side { _dir: dir, hub, rx }
}

/// The target app: one inbox at `/app` answering 202.
async fn target() -> Side {
    let mut app = InboxFile::new("App", "/app");
    app.response.status = 202;
    app.response.body = "accepted".into();
    side(vec![("app", app)], &[]).await
}

fn stripe_inbox(forward: Option<String>) -> InboxFile {
    let mut f = InboxFile::template("Stripe", ProviderKind::Stripe);
    f.secret = "{{secret.STRIPE}}".into();
    f.forward = forward.map(ForwardSpec::new);
    f
}

/// A sender that adds nothing on its own (no `Accept-Encoding`), so forwarding can be checked
/// header by header.
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().no_gzip().no_brotli().no_deflate().no_zstd().build().unwrap()
}

fn stripe_header(ts: i64, body: &[u8]) -> String {
    let sig = hex::encode(hmac(HmacAlgorithm::Sha256, SECRET.as_bytes(), &[ts.to_string().as_bytes(), b".", body]));
    format!("t={ts},v1={sig}")
}

/// Polls until `pred` holds for the stored event.
async fn wait_event(hub: &WebhookHub, id: &str, pred: impl Fn(&WebhookEvent) -> bool) -> WebhookEvent {
    let start = Instant::now();
    loop {
        if let Some(e) = hub.store().get(id).unwrap()
            && pred(&e)
        {
            return e;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for event {id}: {:?}", hub.store().get(id).unwrap());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn only_event(hub: &WebhookHub) -> WebhookEvent {
    let start = Instant::now();
    loop {
        let page = hub.store().list(&EventQuery::default()).unwrap();
        if let Some(s) = page.events.first() {
            return hub.store().get(&s.id).unwrap().unwrap();
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no event arrived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_verify_and_forward_byte_for_byte() {
    let app = target().await;
    let a = side(vec![("stripe", stripe_inbox(Some(app.url("/app/stripe"))))], &[("STRIPE", SECRET)]).await;
    let mut notices = a.hub.store().subscribe();

    // Odd but valid bytes: spacing, unicode, CRLF, trailing newline.
    let body = "{\"id\": \"evt_1\",\r\n  \"type\":\"payment_intent.succeeded\", \"note\": \"h\u{e9}llo \u{2713}\"}\n".as_bytes().to_vec();
    let resp = client()
        .post(a.url("/stripe?a=1&b=%2F"))
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Stripe-Signature", stripe_header(now(), &body))
        .header("X-Dup", "1")
        .header("X-Dup", "2")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();

    let e = wait_event(&a.hub, &id, |e| e.verification.is_some() && e.forward.is_some()).await;
    assert!(matches!(notices.recv().await.unwrap(), StoreNotice::Added { .. }));
    assert_eq!(e.inbox.as_deref(), Some("stripe"));
    assert_eq!(e.label.as_deref(), Some("payment_intent.succeeded"));
    assert_eq!((e.path.as_str(), e.query.as_str()), ("/stripe", "a=1&b=%2F"));
    assert_eq!(e.body, body);
    assert_eq!(e.response_status, Some(200));
    assert!(e.remote_addr.as_deref().unwrap().starts_with("127.0.0.1:"));
    let dups: Vec<&str> = e.headers.iter().filter(|(k, _)| k == "x-dup").map(|(_, v)| v.as_str()).collect();
    assert_eq!(dups, ["1", "2"], "duplicates kept in order");
    let v = e.verification.clone().unwrap();
    assert!(v.valid, "{v:?}");
    let f = e.forward.clone().unwrap();
    assert_eq!(f.status, Some(202), "{f:?}");
    assert_eq!(f.body_excerpt.as_deref(), Some("accepted"));
    assert!(f.duration_ms > 0.0);

    // What the app received: same bytes, headers and query; Host replaced; proxy headers added.
    let got = only_event(&app.hub).await;
    assert_eq!(got.body, body);
    assert_eq!((got.path.as_str(), got.query.as_str()), ("/app/stripe", "a=1&b=%2F"));
    assert_eq!(got.header("stripe-signature"), e.header("stripe-signature"));
    assert_eq!(got.header("content-type"), Some("application/json; charset=utf-8"));
    assert_eq!(got.headers.iter().filter(|(k, _)| k == "x-dup").count(), 2);
    assert_eq!(got.header("host"), Some(app.url("").trim_start_matches("http://")));
    assert_eq!(got.header("x-forwarded-for"), Some("127.0.0.1"));
    assert!(got.header("accept-encoding").is_none(), "no injected Accept-Encoding");
    let check = verify(
        ProviderKind::Stripe,
        None,
        SECRET,
        &SignInput { method: "POST", url: &got.url, headers: &got.headers, body: &got.body },
        VerifyOptions::at(got.received_at, 300),
    );
    assert!(check.valid, "the app can verify the forwarded request: {check:?}");

    // Listing + live updates.
    let page = a.hub.store().list(&EventQuery { inbox: Some("stripe".into()), ..Default::default() }).unwrap();
    assert_eq!(page.events[0].forward_status, Some(202));
    assert!(page.events[0].signature.as_ref().unwrap().valid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_signature_then_resign_on_replay() {
    let app = target().await;
    let a = side(vec![("stripe", stripe_inbox(None))], &[("STRIPE", SECRET)]).await;
    let body = br#"{"id":"evt_2","type":"invoice.paid"}"#.to_vec();
    let resp = client().post(a.url("/stripe")).header("Stripe-Signature", stripe_header(now() - 1000, &body)).body(body.clone()).send().await.unwrap();
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();
    let e = wait_event(&a.hub, &id, |e| e.verification.is_some()).await;
    let v = e.verification.unwrap();
    assert_eq!(v.status, VerifyStatus::Expired);
    assert!(v.reason.starts_with("timestamp 100") && v.reason.contains("s old (tolerance 300 s)"), "{}", v.reason);

    let templates = a.hub.templates();
    // Replay as is: the target gets the stale signature.
    let plain = a.hub.replay(&id, ReplayOptions { target: Some(app.url("/app/x")), ..Default::default() }, templates.clone()).await.unwrap();
    assert_eq!(plain.replay_of.as_deref(), Some(id.as_str()));
    assert_eq!(plain.source, EventSource::Replay);
    assert_eq!(plain.forward.as_ref().unwrap().status, Some(202));
    assert_eq!(plain.verification.as_ref().unwrap().status, VerifyStatus::Expired);

    // Re-signed: fresh timestamp, valid at the target.
    let signed = a.hub.replay(&id, ReplayOptions { target: Some(app.url("/app/x")), resign: true, ..Default::default() }, templates.clone()).await.unwrap();
    assert!(signed.verification.as_ref().unwrap().valid, "{:?}", signed.verification);
    assert_ne!(signed.header("stripe-signature"), e.headers.iter().find(|(k, _)| k == "stripe-signature").map(|(_, v)| v.as_str()));
    let start = Instant::now();
    let received = loop {
        let page = app.hub.store().list(&EventQuery::default()).unwrap();
        if page.total >= 2 {
            break app.hub.store().get(&page.events[0].id).unwrap().unwrap();
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let at_app = verify(ProviderKind::Stripe, None, SECRET, &SignInput { method: "POST", url: &received.url, headers: &received.headers, body: &received.body }, VerifyOptions::at(received.received_at, 300));
    assert!(at_app.valid, "{at_app:?}");

    // Edited body: re-signing keeps it valid, not re-signing breaks it.
    let edits = ReplayEdits { body: Some(r#"{"id":"evt_2","type":"invoice.paid","edited":true}"#.into()), ..Default::default() };
    let ok = a.hub.replay(&id, ReplayOptions { target: Some(app.url("/app/x")), edits: Some(edits.clone()), resign: true, ..Default::default() }, templates.clone()).await.unwrap();
    assert!(ok.verification.unwrap().valid);
    assert_eq!(ok.body, br#"{"id":"evt_2","type":"invoice.paid","edited":true}"#);
    let bad = a.hub.replay(&id, ReplayOptions { target: Some(app.url("/app/x")), edits: Some(edits), resign: false, ..Default::default() }, templates.clone()).await.unwrap();
    assert_eq!(bad.verification.unwrap().status, VerifyStatus::Mismatch);

    // cURL with a fresh signature; the original stays untouched.
    let curl = a.hub.curl(&id, &ReplayOptions { target: Some("http://localhost:3000/hook".into()), resign: true, ..Default::default() }, templates).await.unwrap();
    let stale = e.headers.iter().find(|(k, _)| k == "stripe-signature").map(|(_, v)| v.clone()).unwrap();
    assert!(curl.contains("http://localhost:3000/hook") && curl.contains("stripe-signature: t=") && !curl.contains(&stale), "{curl}");
    assert_eq!(a.hub.store().list(&EventQuery { inbox: Some("stripe".into()), ..Default::default() }).unwrap().total, 5);

    // Re-verify on demand with the stored event.
    let again = a.hub.verify(&id, None).await.unwrap();
    assert_eq!(again.status, VerifyStatus::Expired);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pass_through_catch_all_missing_secret_and_delay() {
    let mut app_inbox = InboxFile::new("Slack app", "/slack");
    app_inbox.response.status = 201;
    app_inbox.response.body = r#"{"text":"deploying"}"#.into();
    app_inbox.response.headers.insert("X-App".into(), "yes".into());
    let app = side(vec![("slack", app_inbox)], &[]).await;

    let mut slack = InboxFile::template("Slack", ProviderKind::Slack);
    let mut fw = ForwardSpec::new(app.url("/slack"));
    fw.pass_through = true;
    slack.forward = Some(fw);
    slack.secret = "{{secret.MISSING}}".into();
    let mut slow = InboxFile::new("Slow", "/slow");
    slow.response.delay_ms = 300;
    let mut looped = InboxFile::new("Loop", "/loop");
    looped.forward = Some(ForwardSpec::new("http://localhost:{{port}}/loop"));
    let a = side(vec![("slack", slack), ("slow", slow), ("loop", looped)], &[]).await;
    a.hub.set_templates(Arc::new(StaticTemplates { vars: [("port".to_string(), a.rx.port().to_string())].into_iter().collect(), ..Default::default() }));

    // Pass-through: the sender gets the app's response.
    let resp = client().post(a.url("/slack")).header("Content-Type", "application/x-www-form-urlencoded").body("command=%2Fdeploy&text=prod").send().await.unwrap();
    assert_eq!(resp.status(), 201);
    assert_eq!(resp.headers().get("x-app").unwrap(), "yes");
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();
    assert_eq!(resp.text().await.unwrap(), r#"{"text":"deploying"}"#);
    let e = wait_event(&a.hub, &id, |e| e.verification.is_some() && e.forward.is_some()).await;
    assert_eq!(e.response_status, Some(201));
    assert_eq!(e.label.as_deref(), Some("/deploy"));
    let v = e.verification.unwrap();
    assert_eq!(v.status, VerifyStatus::MissingSecret);
    assert!(v.reason.contains("secret MISSING is not set"), "{}", v.reason);

    // Catch-all.
    let resp = client().put(a.url("/nowhere")).body("x").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();
    let e = a.hub.store().get(&id).unwrap().unwrap();
    assert_eq!((e.inbox, e.method.as_str()), (None, "PUT"));
    assert_eq!(a.hub.store().list(&EventQuery { inbox: Some(String::new()), ..Default::default() }).unwrap().total, 1);
    assert!(a.hub.verify(&id, None).await.is_err(), "no scheme to verify");

    // Internal health path is not captured.
    let resp = client().get(a.url("/__devall/health")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(a.hub.store().list(&EventQuery::default()).unwrap().total, 2);

    // Delay.
    let start = Instant::now();
    client().post(a.url("/slow")).send().await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(290));

    // A forward target pointing back at the receiver is refused instead of looping.
    let resp = client().post(a.url("/loop")).body("{}").send().await.unwrap();
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();
    let e = wait_event(&a.hub, &id, |e| e.forward.is_some()).await;
    assert!(e.forward.unwrap().error.unwrap().contains("would loop"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn twilio_public_url_from_config_and_tunnel_headers() {
    let token = "12345";
    let mut configured = InboxFile::template("Twilio", ProviderKind::Twilio);
    configured.secret = "{{secret.TWILIO}}".into();
    configured.public_url = Some("https://hooks.example.com/sms".into());
    let mut tunneled = InboxFile::template("Twilio 2", ProviderKind::Twilio);
    tunneled.path = "/twilio2".into();
    tunneled.secret = "{{secret.TWILIO}}".into();
    let a = side(vec![("twilio", configured), ("twilio2", tunneled)], &[("TWILIO", token)]).await;
    let form = "To=%2B18005551212&From=%2B14158675309&Body=Hi";
    let sig = |url: &str| {
        let mut data = url.to_string();
        let mut params: Vec<(String, String)> = url::form_urlencoded::parse(form.as_bytes()).into_owned().collect();
        params.sort();
        for (k, v) in params {
            data.push_str(&k);
            data.push_str(&v);
        }
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, hmac(HmacAlgorithm::Sha1, token.as_bytes(), &[data.as_bytes()]))
    };
    let send = |path: &str, signed_url: &str, extra: Vec<(&'static str, String)>| {
        let mut rb = client().post(a.url(path)).header("Content-Type", "application/x-www-form-urlencoded").header("X-Twilio-Signature", sig(signed_url)).body(form);
        for (k, v) in extra {
            rb = rb.header(k, v);
        }
        rb.send()
    };

    let resp = send("/twilio?x=1", "https://hooks.example.com/sms?x=1", vec![]).await.unwrap();
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();
    let e = wait_event(&a.hub, &id, |e| e.verification.is_some()).await;
    assert_eq!(e.public_url.as_deref(), Some("https://hooks.example.com/sms?x=1"));
    assert!(e.verification.unwrap().valid);

    // Through a tunnel: Host + X-Forwarded-Proto give the URL Twilio called.
    let resp = send("/twilio2", "https://abc.trycloudflare.com/twilio2", vec![("X-Forwarded-Host", "abc.trycloudflare.com".into()), ("X-Forwarded-Proto", "https".into())]).await.unwrap();
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();
    let e = wait_event(&a.hub, &id, |e| e.verification.is_some()).await;
    assert_eq!(e.public_url.as_deref(), Some("https://abc.trycloudflare.com/twilio2"));
    assert!(e.verification.unwrap().valid);

    // Signed for another URL → mismatch, explained.
    let resp = send("/twilio2", "https://other.example.com/twilio2", vec![]).await.unwrap();
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();
    let v = wait_event(&a.hub, &id, |e| e.verification.is_some()).await.verification.unwrap();
    assert_eq!(v.status, VerifyStatus::Mismatch);
    assert!(v.hints.iter().any(|h| h.contains("publicUrl")), "{:?}", v.hints);

    // A session tunnel URL ("Expose inbox") is used when the request carries only local headers.
    a.hub.set_public_base(Some("https://session.trycloudflare.com".into()));
    let resp = send("/twilio2", "https://session.trycloudflare.com/twilio2", vec![]).await.unwrap();
    let id = resp.headers().get("x-devall-event-id").unwrap().to_str().unwrap().to_string();
    assert!(wait_event(&a.hub, &id, |e| e.verification.is_some()).await.verification.unwrap().valid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ingest_from_relay_routes_verifies_and_forwards() {
    let app = target().await;
    let dir = tempfile::tempdir().unwrap();
    crate::inbox::save(dir.path(), "stripe", &stripe_inbox(Some(app.url("/app/relay")))).unwrap();
    let t = StaticTemplates { secrets: [("STRIPE".to_string(), SECRET.to_string())].into_iter().collect(), ..Default::default() };
    // No receiver running: the relay feeds the hub directly.
    devall_core::local::isolate_for_tests();
    let hub = WebhookHub::open(dir.path(), Arc::new(t)).unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    hub.set_on_received(Some(Arc::new(move |e: &WebhookEvent| s2.lock().unwrap().push((e.path.clone(), e.response_status)))));
    let body = br#"{"id":"evt_3","type":"charge.refunded"}"#.to_vec();
    let at = (now() - 60) * 1000;
    let incoming = IncomingEvent {
        method: "POST".into(),
        url: "https://relay.devall.dev/p/abc/stripe".into(),
        path: "/stripe".into(),
        headers: vec![("Stripe-Signature".into(), stripe_header(now() - 60, &body)), ("Content-Type".into(), "application/json".into())],
        body: body.clone(),
        received_at: at,
        source: EventSource::Relay,
        public_url: Some("https://relay.devall.dev/p/abc/stripe".into()),
        external_id: Some("relay-evt-1".into()),
        ..Default::default()
    };
    let e = hub.ingest(incoming.clone()).await.unwrap();
    // At-least-once redelivery of the same relay event is a no-op.
    assert_eq!(hub.ingest(incoming).await.unwrap().id, e.id);
    assert_eq!((e.inbox.as_deref(), e.source, e.received_at), (Some("stripe"), EventSource::Relay, at));
    let e = wait_event(&hub, &e.id, |e| e.verification.is_some() && e.forward.is_some()).await;
    assert!(e.verification.unwrap().valid, "verified against the arrival time at the relay");
    assert_eq!(e.forward.unwrap().status, Some(202));
    assert_eq!(only_event(&app.hub).await.body, body);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(app.hub.store().list(&EventQuery::default()).unwrap().total, 1, "forwarded once");
    assert_eq!(hub.store().list(&EventQuery::default()).unwrap().total, 1);
    assert_eq!(*seen.lock().unwrap(), vec![("/stripe".to_string(), None)], "hook fired once (not for the redelivery)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_bodies_are_truncated_and_rejected() {
    let a = side(vec![("stripe", stripe_inbox(None))], &[("STRIPE", SECRET)]).await;
    let big = vec![b'a'; crate::event::MAX_BODY_BYTES + 10];
    let resp = client().post(a.url("/stripe")).body(big).send().await;
    // The server may close the connection before reading everything; either way it's recorded.
    if let Ok(r) = resp {
        assert_eq!(r.status(), 413);
    }
    let e = only_event(&a.hub).await;
    assert!(e.body_truncated);
    assert_eq!(e.body.len(), crate::event::MAX_BODY_BYTES);
    assert_eq!(e.response_status, Some(413));
}

#[tokio::test]
async fn port_in_use_is_explained() {
    let a = side(vec![], &[]).await;
    let dir = tempfile::tempdir().unwrap();
    devall_core::local::isolate_for_tests();
    let hub = WebhookHub::open(dir.path(), Arc::new(NoTemplates)).unwrap();
    let err = Receiver::start(hub, ReceiverConfig { port: a.rx.port(), bind_all: false }).await.err().unwrap();
    assert!(err.to_string().contains("already in use"), "{err}");
    assert_eq!(header(&[("A".to_string(), "1".to_string())], "a"), Some("1"));
    let port = a.rx.port();
    a.rx.stop().await;
    assert!(a.hub.local_base().is_none());
    // The port is free again.
    let again = Receiver::start(a.hub.clone(), ReceiverConfig { port, bind_all: false }).await.unwrap();
    assert!(again.is_running());
    again.stop().await;
}
