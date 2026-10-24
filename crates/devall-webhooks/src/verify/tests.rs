//! Vectors: GitHub's documented example, Slack's documented example, the Standard Webhooks /
//! Svix library test vector and twilio-python's test vectors are reproduced exactly; the others
//! were computed independently with Python's `hmac` module (see the comments) and are re-derived
//! here with direct HMAC calls rather than through [`sign`].

use super::*;
use crate::model::{GenericHmac, HmacAlgorithm, ProviderKind, SignatureEncoding};

fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn input<'a>(url: &'a str, headers: &'a [(String, String)], body: &'a [u8]) -> SignInput<'a> {
    SignInput { method: "POST", url, headers, body }
}

/// Reference time "when the request arrived".
fn at(now: i64) -> VerifyOptions {
    VerifyOptions { tolerance_seconds: 300, now, current: now }
}

fn hex_hmac256(key: &str, data: &str) -> String {
    hex::encode(hmac(HmacAlgorithm::Sha256, key.as_bytes(), &[data.as_bytes()]))
}

// ---------- GitHub ----------

#[test]
fn github_documented_vector() {
    // docs.github.com "Validating webhook deliveries" test values.
    let headers = h(&[("X-Hub-Signature-256", "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17")]);
    let r = verify(ProviderKind::Github, None, "It's a Secret to Everybody", &input("http://x/", &headers, b"Hello, World!"), at(0));
    assert!(r.valid, "{r:?}");
    assert_eq!(r.status, VerifyStatus::Valid);
    assert_eq!(r.expected.as_deref(), Some("sha256=757107…3e17"));

    let r = verify(ProviderKind::Github, None, "wrong secret", &input("http://x/", &headers, b"Hello, World!"), at(0));
    assert_eq!(r.status, VerifyStatus::Mismatch);
    assert_eq!(r.reason, "signature mismatch — body was modified or the secret is wrong");

    let r = verify(ProviderKind::Github, None, "It's a Secret to Everybody", &input("http://x/", &headers, b"Hello, World?"), at(0));
    assert_eq!(r.status, VerifyStatus::Mismatch);
}

#[test]
fn github_legacy_sha1_and_missing_header() {
    // python: hmac.new(b"It's a Secret to Everybody", b"Hello, World!", sha1).hexdigest()
    let headers = h(&[("x-hub-signature", "sha1=01dc10d0c83e72ed246219cdd91669667fe2ca59")]);
    let r = verify(ProviderKind::Github, None, "It's a Secret to Everybody", &input("http://x/", &headers, b"Hello, World!"), at(0));
    assert!(r.valid, "{r:?}");
    assert!(r.hints.iter().any(|h| h.contains("SHA-1")));

    let r = verify(ProviderKind::Github, None, "s", &input("http://x/", &[], b"{}"), at(0));
    assert_eq!(r.status, VerifyStatus::MissingHeader);
    assert!(r.reason.starts_with("missing header X-Hub-Signature-256"), "{}", r.reason);
}

// ---------- Stripe ----------

const STRIPE_BODY: &str = r#"{"id":"evt_1","type":"payment_intent.succeeded"}"#;
/// python: hmac.new(b"whsec_test_secret", b"1729500000." + body, sha256).hexdigest()
const STRIPE_SIG: &str = "4bf1bfff4edc754d418635f4c07f8c4f7dc7c06a474ae02fb3ba572144bc3969";

#[test]
fn stripe_vector_rotation_and_tolerance() {
    assert_eq!(hex_hmac256("whsec_test_secret", &format!("1729500000.{STRIPE_BODY}")), STRIPE_SIG);
    // Rotation: an unknown v1 first, the right one second, and a v0 to ignore.
    let hv = format!("t=1729500000,v1={},v1={STRIPE_SIG},v0=deadbeef", "0".repeat(64));
    let headers = h(&[("Stripe-Signature", &hv)]);
    let i = input("http://x/", &headers, STRIPE_BODY.as_bytes());

    let r = verify(ProviderKind::Stripe, None, "whsec_test_secret", &i, at(1_729_500_010));
    assert!(r.valid, "{r:?}");
    assert_eq!(r.timestamp_age_seconds, Some(10));
    assert_eq!(r.computed_over.as_deref(), Some("\"1729500000.\" + raw body (48 bytes)"));

    // Signature fine, but 512 s old.
    let r = verify(ProviderKind::Stripe, None, "whsec_test_secret", &i, at(1_729_500_512));
    assert_eq!(r.status, VerifyStatus::Expired);
    assert!(!r.valid);
    assert_eq!(r.reason, "timestamp 512 s old (tolerance 300 s) — the signature itself is valid");
    assert!(r.hints[0].contains("Re-sign"));

    // Stripe libraries don't reject future timestamps.
    assert!(verify(ProviderKind::Stripe, None, "whsec_test_secret", &i, at(1_729_499_000)).valid);

    // Wrong secret → mismatch with Stripe-specific hints.
    let r = verify(ProviderKind::Stripe, None, "sk_test_123", &i, at(1_729_500_010));
    assert_eq!(r.status, VerifyStatus::Mismatch);
    assert!(r.hints.iter().any(|h| h.contains("whsec_")), "{:?}", r.hints);
    assert!(r.hints.iter().any(|h| h.contains("rotation")), "{:?}", r.hints);
}

#[test]
fn stripe_malformed_headers() {
    for (hv, want) in [("v1=abc", "no t="), ("t=abc,v1=00", "not a Unix timestamp"), ("t=1,v0=00", "only a v0")] {
        let headers = h(&[("stripe-signature", hv)]);
        let r = verify(ProviderKind::Stripe, None, "whsec_x", &input("http://x/", &headers, b"{}"), at(1));
        assert_eq!(r.status, VerifyStatus::Malformed, "{hv}");
        assert!(r.reason.contains(want), "{hv}: {}", r.reason);
    }
}

#[test]
fn secret_whitespace_is_diagnosed() {
    let hv = format!("t=1729500000,v1={STRIPE_SIG}");
    let headers = h(&[("Stripe-Signature", &hv)]);
    let r = verify(ProviderKind::Stripe, None, "whsec_test_secret\n", &input("http://x/", &headers, STRIPE_BODY.as_bytes()), at(1_729_500_000));
    assert_eq!(r.status, VerifyStatus::Mismatch);
    assert!(r.hints[0].contains("whitespace"), "{:?}", r.hints);
}

// ---------- Shopify ----------

#[test]
fn shopify_vector() {
    // python: base64(hmac.new(b"shpss_secret", b'{"id":820982911946154508}', sha256).digest())
    let headers = h(&[("X-Shopify-Hmac-Sha256", "jqbalbO3Z8mDmrF2/+wzM3KYvbgqzVBF969vgiIy80I=")]);
    let body = br#"{"id":820982911946154508}"#;
    assert!(verify(ProviderKind::Shopify, None, "shpss_secret", &input("http://x/", &headers, body), at(0)).valid);
    let r = verify(ProviderKind::Shopify, None, "shpss_secret", &input("http://x/", &headers, br#"{"id":820982911946154509}"#), at(0));
    assert_eq!(r.status, VerifyStatus::Mismatch);
    assert!(r.hints.iter().any(|h| h.contains("client secret")));
}

// ---------- Slack ----------

const SLACK_BODY: &str = "token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.47445629121.803a0bc887a14d10d2c447fce8b6703c";

#[test]
fn slack_documented_vector_and_symmetric_tolerance() {
    // Slack's "Verifying requests from Slack" walkthrough.
    let headers = h(&[("X-Slack-Request-Timestamp", "1531420618"), ("X-Slack-Signature", "v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503")]);
    let i = input("http://x/", &headers, SLACK_BODY.as_bytes());
    let secret = "8f742231b10e8888abcd99yyyzzz85a5";
    assert!(verify(ProviderKind::Slack, None, secret, &i, at(1_531_420_618)).valid);
    let r = verify(ProviderKind::Slack, None, secret, &i, at(1_531_420_618 + 301));
    assert_eq!(r.status, VerifyStatus::Expired);
    assert_eq!(r.reason, "timestamp 301 s old (tolerance 300 s) — the signature itself is valid");
    let r = verify(ProviderKind::Slack, None, secret, &i, at(1_531_420_618 - 400));
    assert_eq!(r.status, VerifyStatus::Expired);
    assert!(r.reason.contains("in the future"), "{}", r.reason);

    let r = verify(ProviderKind::Slack, None, secret, &input("http://x/", &headers[1..], SLACK_BODY.as_bytes()), at(0));
    assert_eq!(r.status, VerifyStatus::MissingHeader);
    assert!(r.reason.contains("X-Slack-Request-Timestamp"));
}

// ---------- Twilio ----------

/// twilio-python's RequestValidator tests (token `12345`).
const TWILIO_URL: &str = "https://mycompany.com/myapp.php?foo=1&bar=2";
const TWILIO_FORM: &str = "CallSid=CA1234567890ABCDE&Caller=%2B14158675309&Digits=1234&From=%2B14158675309&To=%2B18005551212";

fn twilio_headers(sig: &str) -> Vec<(String, String)> {
    h(&[("Content-Type", "application/x-www-form-urlencoded"), ("X-Twilio-Signature", sig)])
}

#[test]
fn twilio_form_vector_and_url_sensitivity() {
    let headers = twilio_headers("RSOYDt4T1cUTdK1PDd93/VVr8B8=");
    let r = verify(ProviderKind::Twilio, None, "12345", &input(TWILIO_URL, &headers, TWILIO_FORM.as_bytes()), at(0));
    assert!(r.valid, "{r:?}");
    assert_eq!(r.signed_url.as_deref(), Some(TWILIO_URL));
    assert!(r.computed_over.as_deref().unwrap().contains("5 sorted POST parameters (CallSid, Caller, Digits, From, To)"), "{:?}", r.computed_over);

    // Twilio's validators accept the URL with an explicit default port too.
    let with_port = "https://mycompany.com:443/myapp.php?foo=1&bar=2";
    assert!(verify(ProviderKind::Twilio, None, "12345", &input(with_port, &headers, TWILIO_FORM.as_bytes()), at(0)).valid);

    // Any other URL change breaks it — and the explanation names the URL that would match.
    let http = "http://mycompany.com/myapp.php?foo=1&bar=2";
    let r = verify(ProviderKind::Twilio, None, "12345", &input(http, &headers, TWILIO_FORM.as_bytes()), at(0));
    assert_eq!(r.status, VerifyStatus::Mismatch);
    assert!(r.reason.contains("URL"));
    assert!(r.hints.iter().any(|h| h.contains(&format!("matches {TWILIO_URL}"))), "{:?}", r.hints);

    let reordered = "https://mycompany.com/myapp.php?bar=2&foo=1";
    assert!(!verify(ProviderKind::Twilio, None, "12345", &input(reordered, &headers, TWILIO_FORM.as_bytes()), at(0)).valid);
    let local = "http://127.0.0.1:4590/myapp.php?foo=1&bar=2";
    let r = verify(ProviderKind::Twilio, None, "12345", &input(local, &headers, TWILIO_FORM.as_bytes()), at(0));
    assert!(r.hints.iter().any(|h| h.contains("publicUrl")), "{:?}", r.hints);

    // Parameter order on the wire doesn't matter (they are sorted), values do.
    let shuffled = "To=%2B18005551212&Digits=1234&CallSid=CA1234567890ABCDE&From=%2B14158675309&Caller=%2B14158675309";
    assert!(verify(ProviderKind::Twilio, None, "12345", &input(TWILIO_URL, &headers, shuffled.as_bytes()), at(0)).valid);
    let changed = TWILIO_FORM.replace("1234&", "1235&");
    assert!(!verify(ProviderKind::Twilio, None, "12345", &input(TWILIO_URL, &headers, changed.as_bytes()), at(0)).valid);
}

#[test]
fn twilio_json_body_uses_body_sha256() {
    let body = r#"{"property": "value", "boolean": true}"#;
    let url = format!("{TWILIO_URL}&bodySHA256=0a1ff7634d9ab3b95db5c9a2dfe9416e41502b283a80c7cf19632632f96e6620");
    let headers = h(&[("Content-Type", "application/json"), ("X-Twilio-Signature", "a9nBmqA0ju/hNViExpshrM61xv4=")]);
    let r = verify(ProviderKind::Twilio, None, "12345", &input(&url, &headers, body.as_bytes()), at(0));
    assert!(r.valid, "{r:?}");
    let r = verify(ProviderKind::Twilio, None, "12345", &input(&url, &headers, br#"{"property": "value", "boolean": false}"#), at(0));
    assert_eq!(r.status, VerifyStatus::Mismatch);
    assert!(r.hints.iter().any(|h| h.contains("bodySHA256")), "{:?}", r.hints);
}

#[test]
fn twilio_url_variants() {
    assert_eq!(super::twilio_url_variants("https://a.com/x?y=1"), ["https://a.com/x?y=1", "https://a.com:443/x?y=1"]);
    assert_eq!(super::twilio_url_variants("http://[::1]:8080/x"), ["http://[::1]:8080/x", "http://[::1]/x"]);
    assert_eq!(super::twilio_url_variants("https://u:p@a.com:443"), ["https://u:p@a.com:443", "https://u:p@a.com"]);
}

// ---------- Standard Webhooks ----------

#[test]
fn standard_webhooks_spec_vector() {
    // The Standard Webhooks / Svix libraries' signing test vector.
    let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    let body = br#"{"test": 2432232314}"#;
    let headers = h(&[("webhook-id", "msg_p5jXN8AQM9LWM0D4loKWxJek"), ("webhook-timestamp", "1614265330"), ("webhook-signature", "v1,bm9wZQ== v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=")]);
    let r = verify(ProviderKind::StandardWebhooks, None, secret, &input("http://x/", &headers, body), at(1_614_265_330));
    assert!(r.valid, "{r:?}");
    assert_eq!(r.computed_over.as_deref(), Some("\"msg_p5jXN8AQM9LWM0D4loKWxJek.1614265330.\" + raw body (20 bytes)"));

    // Svix-branded headers work the same.
    let svix: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.replace("webhook-", "svix-"), v.clone())).collect();
    assert!(verify(ProviderKind::StandardWebhooks, None, secret, &input("http://x/", &svix, body), at(1_614_265_330)).valid);

    // Too old / too new.
    assert_eq!(verify(ProviderKind::StandardWebhooks, None, secret, &input("http://x/", &headers, body), at(1_614_265_330 + 600)).status, VerifyStatus::Expired);
    assert_eq!(verify(ProviderKind::StandardWebhooks, None, secret, &input("http://x/", &headers, body), at(1_614_265_330 - 600)).status, VerifyStatus::Expired);

    // A secret that isn't base64.
    let r = verify(ProviderKind::StandardWebhooks, None, "whsec_***", &input("http://x/", &headers, body), at(1_614_265_330));
    assert_eq!(r.status, VerifyStatus::Malformed);
}

// ---------- Generic ----------

#[test]
fn generic_hmac_variants() {
    // python: hmac.new(b"generic-secret", b'{"a":1}', sha512).hexdigest() / sha1 base64
    let sha512 = "120c58023c72b10b814e631dec46baf9007723f57f292dcbe3027fe6d2321ba65aee8f568abe17f69e0db20e7cfa7a8f347ec9689aab080f832853c4c8fb76fb";
    let g = GenericHmac { header: "X-Sig".into(), algorithm: HmacAlgorithm::Sha512, encoding: SignatureEncoding::Hex, prefix: "sha512=".into() };
    let hv = format!("SHA512={}", sha512.to_uppercase());
    let headers = h(&[("x-sig", &hv)]);
    assert!(verify(ProviderKind::GenericHmac, Some(&g), "generic-secret", &input("http://x/", &headers, br#"{"a":1}"#), at(0)).valid);

    let g = GenericHmac { header: "X-Sig".into(), algorithm: HmacAlgorithm::Sha1, encoding: SignatureEncoding::Base64, prefix: String::new() };
    let headers = h(&[("X-Sig", "5hAzfgn/9amQExvBz/FCW1yPOaA=")]);
    assert!(verify(ProviderKind::GenericHmac, Some(&g), "generic-secret", &input("http://x/", &headers, br#"{"a":1}"#), at(0)).valid);

    // Wrong algorithm → length hint.
    let g256 = GenericHmac { algorithm: HmacAlgorithm::Sha256, ..g };
    let r = verify(ProviderKind::GenericHmac, Some(&g256), "generic-secret", &input("http://x/", &headers, br#"{"a":1}"#), at(0));
    assert_eq!(r.status, VerifyStatus::Mismatch);
    assert!(r.hints.iter().any(|h| h.contains("different length")));
}

#[test]
fn missing_secret() {
    let r = verify(ProviderKind::Stripe, None, "", &input("http://x/", &[], b""), at(0));
    assert_eq!(r.status, VerifyStatus::MissingSecret);
    assert!(!r.valid);
}

// ---------- re-sign → verify round trips ----------

fn round_trip(kind: ProviderKind, generic: Option<&GenericHmac>, secret: &str, url: &str, original: Vec<(String, String)>, body: &[u8]) {
    let now = chrono::Utc::now().timestamp();
    let resigned = sign(kind, generic, secret, &SignInput { method: "POST", url, headers: &original, body }, &SignOptions::default()).unwrap();
    let mut headers = original.clone();
    resigned.apply(&mut headers);
    let url = resigned.url.clone().unwrap_or_else(|| url.to_string());
    let r = verify(kind, generic, secret, &SignInput { method: "POST", url: &url, headers: &headers, body }, VerifyOptions { tolerance_seconds: 300, now, current: now });
    assert!(r.valid, "{kind:?}: {r:?}");
}

#[test]
fn resign_then_verify_round_trips_for_every_provider() {
    let edited = br#"{"id":"evt_1","type":"payment_intent.succeeded","edited":true}"#;
    // A stale Stripe header is replaced (not duplicated).
    round_trip(ProviderKind::Stripe, None, "whsec_test_secret", "http://localhost:3000/hook", h(&[("Stripe-Signature", &format!("t=1729500000,v1={STRIPE_SIG}"))]), edited);
    round_trip(ProviderKind::Github, None, "gh", "http://localhost/", h(&[("X-Hub-Signature", "sha1=00"), ("X-Hub-Signature-256", "sha256=00")]), edited);
    round_trip(ProviderKind::Shopify, None, "shp", "http://localhost/", vec![], edited);
    round_trip(ProviderKind::Slack, None, "slack", "http://localhost/", h(&[("X-Slack-Request-Timestamp", "1531420618"), ("X-Slack-Signature", "v0=00")]), SLACK_BODY.as_bytes());
    round_trip(ProviderKind::Twilio, None, "12345", "http://localhost:3000/sms?x=1", twilio_headers("old"), TWILIO_FORM.as_bytes());
    let json_url = format!("http://localhost:3000/t?bodySHA256={}", "0".repeat(64));
    round_trip(ProviderKind::Twilio, None, "12345", &json_url, h(&[("Content-Type", "application/json"), ("X-Twilio-Signature", "old")]), edited);
    round_trip(ProviderKind::StandardWebhooks, None, "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw", "http://localhost/", h(&[("webhook-id", "msg_1"), ("webhook-timestamp", "1"), ("webhook-signature", "v1,AAAA")]), edited);
    round_trip(ProviderKind::StandardWebhooks, None, "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw", "http://localhost/", vec![], edited);
    let g = GenericHmac { header: "X-Signature".into(), prefix: "sha256=".into(), ..Default::default() };
    round_trip(ProviderKind::GenericHmac, Some(&g), "s", "http://localhost/", vec![], edited);
}

#[test]
fn resign_details() {
    let headers = h(&[("Stripe-Signature", "t=1,v1=00"), ("stripe-signature", "dup")]);
    let r = sign(ProviderKind::Stripe, None, "whsec_test_secret", &input("http://x/", &headers, STRIPE_BODY.as_bytes()), &SignOptions { now: Some(1_729_500_000) }).unwrap();
    // Independently: Python vector for t=1729500000.
    assert_eq!(r.headers, h(&[("Stripe-Signature", &format!("t=1729500000,v1={STRIPE_SIG}"))]));
    let mut hs = headers.clone();
    r.apply(&mut hs);
    assert_eq!(hs.len(), 1);

    // Standard Webhooks keeps the message id and header family.
    let svix = h(&[("svix-id", "msg_keep"), ("svix-timestamp", "1"), ("svix-signature", "v1,x")]);
    let r = sign(ProviderKind::StandardWebhooks, None, "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw", &input("http://x/", &svix, b"{}"), &SignOptions { now: Some(5) }).unwrap();
    assert_eq!(r.headers[0], ("svix-id".to_string(), "msg_keep".to_string()));
    assert_eq!(r.headers[1], ("svix-timestamp".to_string(), "5".to_string()));

    // Twilio JSON: bodySHA256 is recomputed in the URL.
    let url = format!("https://h/t?a=1&bodySHA256={}#f", "0".repeat(64));
    let r = sign(ProviderKind::Twilio, None, "12345", &input(&url, &[], br#"{"property": "value", "boolean": true}"#), &SignOptions::default()).unwrap();
    assert_eq!(r.url.as_deref(), Some("https://h/t?a=1&bodySHA256=0a1ff7634d9ab3b95db5c9a2dfe9416e41502b283a80c7cf19632632f96e6620#f"));

    assert!(sign(ProviderKind::None, None, "x", &input("http://x/", &[], b""), &SignOptions::default()).is_err());
    assert!(sign(ProviderKind::Stripe, None, "", &input("http://x/", &[], b""), &SignOptions::default()).is_err());
}

#[test]
fn masking() {
    assert_eq!(mask("4bf1bfff4edc754d418635f4c07f8c4f7dc7c06a474ae02fb3ba572144bc3969"), "4bf1bf…3969");
    assert_eq!(mask("short"), "•••••");
    assert_eq!(mask("v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="), "v1,g0hM9S…1OE=");
    assert_eq!(mask("jqbalbO3Z8mDmrF2/+wzM3KYvbgqzVBF969vgiIy80I="), "jqbalb…80I=");
}

#[test]
fn replay_warning_for_old_but_valid_events() {
    let hv = format!("t=1729500000,v1={STRIPE_SIG}");
    let headers = h(&[("Stripe-Signature", &hv)]);
    let r = verify(
        ProviderKind::Stripe,
        None,
        "whsec_test_secret",
        &input("http://x/", &headers, STRIPE_BODY.as_bytes()),
        VerifyOptions { tolerance_seconds: 300, now: 1_729_500_001, current: 1_729_500_000 + 3 * 3600 },
    );
    assert!(r.valid);
    assert!(r.hints.iter().any(|h| h.contains("3 h old")), "{:?}", r.hints);
}
