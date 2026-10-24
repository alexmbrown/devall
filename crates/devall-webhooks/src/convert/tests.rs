use super::*;
use crate::model::{GenericHmac, HmacAlgorithm, ProviderKind, SignatureEncoding};
use crate::verify::{SignInput, SignOptions, VerifyOptions, sign, verify};
use devall_script::{PreRequestInput, RequestView, ScriptEngine, ScriptSource};

fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn event(path: &str, ct: &str, body: &str, extra: &[(&str, &str)]) -> WebhookEvent {
    let mut headers = h(&[("host", "abc.trycloudflare.com"), ("content-type", ct), ("content-length", "99"), ("x-forwarded-for", "1.2.3.4"), ("user-agent", "Stripe/1.0")]);
    headers.extend(h(extra));
    WebhookEvent {
        id: "01J".into(),
        inbox: Some("in".into()),
        received_at: 1_729_500_000_000,
        method: "POST".into(),
        url: format!("http://abc.trycloudflare.com{path}?a=1"),
        path: path.into(),
        query: "a=1".into(),
        headers,
        body: body.as_bytes().to_vec(),
        body_size: body.len(),
        label: Some("payment_intent.succeeded".into()),
        ..Default::default()
    }
}

fn inbox(kind: ProviderKind) -> InboxFile {
    let mut f = InboxFile::template("In", kind);
    f.secret = "{{secret.WH_SECRET}}".into();
    f.forward = Some(crate::model::ForwardSpec::new("http://localhost:3000/hooks"));
    f
}

#[test]
fn request_file_keeps_exact_body_and_templated_signatures() {
    let body = "{\"id\":\"evt_1\",  \"type\":\"payment_intent.succeeded\"}";
    let e = event("/stripe", "application/json", body, &[("stripe-signature", "t=1,v1=00")]);
    let f = inbox(ProviderKind::Stripe);
    let r = to_request_file(&e, Some(&f), None).unwrap();
    assert_eq!(r.name, "payment_intent.succeeded");
    assert_eq!(r.url, "http://localhost:3000/hooks");
    assert_eq!(r.params, vec![KeyValue::new("a", "1")]);
    assert_eq!(r.body, Body::Json { content: body.into() }, "not pretty-printed");
    let names: Vec<&str> = r.headers.iter().map(|k| k.name.as_str()).collect();
    assert_eq!(names, ["content-type", "user-agent", "stripe-signature"], "host, length and proxy headers dropped");
    assert_eq!(r.headers[2].value, "{{webhookSignature}}");
    let script = r.scripts.as_ref().unwrap().pre_request.clone();
    assert!(script.contains("pm.secrets.get(\"WH_SECRET\")"), "{script}");
    assert!(!script.contains("whsec"), "never the value");
    assert_eq!(r.tests.len(), 1);
    assert!(r.docs.unwrap().contains("{{secret.WH_SECRET}}"));

    // Saved through the project API.
    let dir = tempfile::tempdir().unwrap();
    let p = devall_core::Project::init(dir.path(), "P").unwrap();
    let rel = save_as_request(&p, "webhooks", &e, Some(&f), None).unwrap();
    assert_eq!(rel, "webhooks/payment-intent-succeeded.req.yaml");
    assert_eq!(p.read_request(&rel).unwrap().body, Body::Json { content: body.into() });

    // Binary bodies are refused (fixtures handle them).
    let mut bin = e.clone();
    bin.body = vec![0xff, 0xfe];
    assert!(to_request_file(&bin, Some(&f), None).is_err());
    // No inbox: original URL, captured headers, no script.
    let plain = to_request_file(&e, None, Some("http://localhost:9/x?b=2")).unwrap();
    assert_eq!((plain.url.as_str(), plain.params[0].name.as_str()), ("http://localhost:9/x", "b"));
    assert!(plain.scripts.is_none());
    assert_eq!(plain.headers.iter().find(|k| k.name == "stripe-signature").unwrap().value, "t=1,v1=00");
}

/// Runs the generated pre-request script in the real sandbox, puts its variables into the
/// templated headers and checks the result with the verifier.
fn script_signs_correctly(kind: ProviderKind, generic: Option<GenericHmac>, secret: &str, ct: &str, body: &str, extra: &[(&str, &str)]) {
    let e = event("/hook", ct, body, extra);
    let mut f = inbox(kind);
    f.generic_hmac = generic.clone();
    let r = to_request_file(&e, Some(&f), None).unwrap();
    let script = r.scripts.clone().unwrap().pre_request;
    let headers: Vec<(String, String)> = r.headers.iter().map(|k| (k.name.clone(), k.value.clone())).collect();
    let body_text = match &r.body {
        Body::Json { content } => content.clone(),
        Body::Text { content, .. } => content.clone(),
        other => panic!("{other:?}"),
    };
    let input = PreRequestInput {
        request: RequestView {
            method: r.method.clone(),
            url: r.url.clone(),
            query: r.params.iter().map(|k| (k.name.clone(), k.value.clone())).collect(),
            headers: headers.clone(),
            body: Some(body_text.clone()),
        },
        secrets: [("WH_SECRET".to_string(), secret.to_string())].into_iter().collect(),
        ..Default::default()
    };
    let out = ScriptEngine::new().run_pre_request(&[ScriptSource::new("request", script.clone())], &input);
    assert!(out.error.is_none(), "{kind:?}: {:?}\n{script}", out.error);
    let sig = out.runtime.get(SIGNATURE_VAR).cloned().unwrap_or_else(|| panic!("{kind:?}: no signature set"));
    let ts = out.runtime.get(TIMESTAMP_VAR).cloned().unwrap_or_default();
    let resolved: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.clone(), v.replace("{{webhookSignature}}", &sig).replace("{{webhookTimestamp}}", &ts))).collect();
    let url = format!("{}?a=1", r.url);
    let now = chrono::Utc::now().timestamp();
    let v = verify(kind, generic.as_ref(), secret, &SignInput { method: "POST", url: &url, headers: &resolved, body: body_text.as_bytes() }, VerifyOptions { tolerance_seconds: 300, now, current: now });
    assert!(v.valid, "{kind:?}: {v:?}\n{script}");
}

#[test]
fn generated_scripts_produce_valid_signatures() {
    let json = r#"{"id":"evt_1","type":"invoice.paid","amount":4200,"note":"héllo ✓"}"#;
    script_signs_correctly(ProviderKind::Stripe, None, "whsec_test_secret", "application/json", json, &[("stripe-signature", "t=1,v1=00")]);
    script_signs_correctly(ProviderKind::Github, None, "gh-secret", "application/json", json, &[("x-hub-signature-256", "sha256=00"), ("x-hub-signature", "sha1=00")]);
    script_signs_correctly(ProviderKind::Shopify, None, "shpss", "application/json", json, &[("x-shopify-hmac-sha256", "AA==")]);
    script_signs_correctly(ProviderKind::Slack, None, "slack-secret", "application/x-www-form-urlencoded", "command=%2Fdeploy&text=prod+now", &[("x-slack-request-timestamp", "1"), ("x-slack-signature", "v0=00")]);
    script_signs_correctly(
        ProviderKind::Twilio,
        None,
        "12345",
        "application/x-www-form-urlencoded",
        "To=%2B18005551212&Body=Hi+there&From=%2B14158675309&MediaUrl0=a&MediaUrl0=b",
        &[("x-twilio-signature", "old")],
    );
    script_signs_correctly(
        ProviderKind::StandardWebhooks,
        None,
        "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
        "application/json",
        json,
        &[("webhook-id", "msg_1"), ("webhook-timestamp", "1"), ("webhook-signature", "v1,AA==")],
    );
    let g = GenericHmac { header: "X-Signature".into(), algorithm: HmacAlgorithm::Sha512, encoding: SignatureEncoding::Base64, prefix: "sha512=".into() };
    script_signs_correctly(ProviderKind::GenericHmac, Some(g), "s3", "application/json", json, &[("x-signature", "sha512=AA==")]);
}

#[test]
fn secret_expressions() {
    assert_eq!(secret_expr("{{secret.STRIPE}}"), "pm.secrets.get(\"STRIPE\")");
    assert_eq!(secret_expr(" {{ secret.A_B }} "), "pm.secrets.get(\"A_B\")");
    assert_eq!(secret_expr("{{webhookSecret}}"), "pm.variables.replaceIn(\"{{webhookSecret}}\")");
    assert!(resign_script(&InboxFile::new("x", "/x")).is_none());
}

#[test]
fn curl_and_fixture() {
    let body = r#"{"id":"evt_1","type":"invoice.paid"}"#;
    let e = event("/stripe", "application/json", body, &[("stripe-signature", "t=1,v1=00"), ("authorization", "Basic abc")]);
    let f = inbox(ProviderKind::Stripe);

    let req = OutboundRequest { method: "POST".into(), url: "http://localhost:3000/x".into(), headers: h(&[("content-type", "application/json"), ("stripe-signature", "t=5,v1=ab")]), body: body.as_bytes().to_vec(), timeout_ms: 1 };
    let c = curl_for(&req);
    assert!(c.starts_with("curl") && c.contains("-X POST") && c.contains("http://localhost:3000/x"), "{c}");
    assert!(c.contains("-H 'stripe-signature: t=5,v1=ab'"), "{c}");
    assert!(c.contains(&format!("--data-raw '{body}'")), "{c}");
    assert!(!c.contains("--location"));

    let fx = to_fixture(&e, Some(&f));
    assert_eq!(fx.body_encoding, "json");
    assert_eq!(fx.body["type"], "invoice.paid");
    assert_eq!(fx.signature_headers, ["Stripe-Signature"]);
    assert!(!fx.headers.iter().any(|(k, _)| k == "stripe-signature" || k == "host"));
    assert_eq!(fx.headers.iter().find(|(k, _)| k == "authorization").unwrap().1, "••••••");

    let dir = tempfile::tempdir().unwrap();
    let rel = save_fixture(dir.path(), &e, Some(&f)).unwrap();
    assert_eq!(rel, "webhooks/fixtures/payment-intent-succeeded.json");
    let rel2 = save_fixture(dir.path(), &e, Some(&f)).unwrap();
    assert_eq!(rel2, "webhooks/fixtures/payment-intent-succeeded-2.json");
    let text = std::fs::read_to_string(dir.path().join(&rel)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["provider"], "stripe");
    assert_eq!(v["bodyEncoding"], "json");

    let mut bin = e.clone();
    bin.body = vec![0xff, 0xfe, 0];
    assert_eq!(to_fixture(&bin, None).body_encoding, "base64");
}

#[test]
fn resigned_request_matches_independent_hmac() {
    // Sanity link between sign() and the generated script's algorithm for Stripe.
    let body = br#"{"a":1}"#;
    let r = sign(ProviderKind::Stripe, None, "whsec_x", &SignInput { method: "POST", url: "http://x/", headers: &[], body }, &SignOptions { now: Some(100) }).unwrap();
    let expected = hex::encode(crate::verify::hmac(HmacAlgorithm::Sha256, b"whsec_x", &[b"100.", body]));
    assert_eq!(r.headers[0].1, format!("t=100,v1={expected}"));
}
