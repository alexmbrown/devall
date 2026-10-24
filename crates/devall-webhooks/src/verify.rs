//! Signature verification and re-signing for the providers in [`ProviderKind`].
//!
//! Every scheme signs the **raw** request body, so verification always runs on the exact bytes
//! received. Comparisons are constant time ([`subtle`]). Results explain failures in words
//! ("timestamp 512 s old (tolerance 300 s)", "signature mismatch — body was modified or the secret
//! is wrong") and describe what was signed, without ever including the secret.
//!
//! | Provider | Header(s) | Signed content | MAC / encoding |
//! |---|---|---|---|
//! | Stripe | `Stripe-Signature: t=<ts>,v1=<sig>[,v1=…]` | `<ts>.<body>` | HMAC-SHA256(secret as UTF-8), hex; any `v1` may match (rotation); `v0` ignored |
//! | GitHub | `X-Hub-Signature-256: sha256=<sig>` (legacy `X-Hub-Signature: sha1=`) | body | HMAC-SHA256 (SHA-1), hex |
//! | Shopify | `X-Shopify-Hmac-Sha256` | body | HMAC-SHA256, base64 |
//! | Slack | `X-Slack-Signature: v0=<sig>`, `X-Slack-Request-Timestamp` | `v0:<ts>:<body>` | HMAC-SHA256, hex |
//! | Twilio | `X-Twilio-Signature` | URL + form params sorted by name (`name+value…`); JSON bodies: URL with `?bodySHA256=<hex sha256(body)>` | HMAC-SHA1(auth token), base64; URL tried with and without the default port |
//! | Standard Webhooks | `webhook-id`, `webhook-timestamp`, `webhook-signature: v1,<sig> …` (or `svix-*`) | `<id>.<ts>.<body>` | HMAC-SHA256(base64-decoded `whsec_` secret), base64 |
//! | Generic HMAC | configured header | body | HMAC-SHA256/SHA-1/SHA-512, hex or base64, optional prefix |
//!
//! Timestamped schemes (Stripe, Slack, Standard Webhooks) reject old deliveries; [`sign`]
//! produces fresh headers with the current time so a replay passes again.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use subtle::ConstantTimeEq;

use crate::event::{header, is_form_content_type, set_header};
use crate::model::{GenericHmac, HmacAlgorithm, ProviderKind, SignatureEncoding};

/// The request as signed by the sender.
#[derive(Debug, Clone, Copy)]
pub struct SignInput<'a> {
    pub method: &'a str,
    /// The exact URL the sender called (only Twilio signs it).
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyStatus {
    Valid,
    /// The MAC doesn't match: modified body, wrong secret, or (Twilio) a different URL.
    Mismatch,
    /// The signature matches but its timestamp is outside the tolerance.
    Expired,
    MissingHeader,
    /// A header (or the secret) has the wrong shape.
    Malformed,
    /// No secret configured, or it could not be resolved.
    MissingSecret,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    pub provider: ProviderKind,
    pub valid: bool,
    pub status: VerifyStatus,
    /// One-line explanation.
    pub reason: String,
    /// The signature we computed, masked (`4bf1bf…3969`).
    pub expected: Option<String>,
    /// The signature the request carried.
    pub received: Option<String>,
    /// What was signed, e.g. `"1729500000." + raw body (48 bytes)`.
    pub computed_over: Option<String>,
    /// The URL used (Twilio).
    pub signed_url: Option<String>,
    /// Signed timestamp (Unix seconds), for timestamped schemes.
    pub timestamp: Option<i64>,
    /// Age of the signed timestamp when the request arrived (negative = from the future).
    pub timestamp_age_seconds: Option<i64>,
    pub tolerance_seconds: Option<u64>,
    /// Likely causes and what to do.
    pub hints: Vec<String>,
    /// Unix milliseconds.
    pub checked_at: i64,
}

impl VerifyResult {
    fn new(provider: ProviderKind, status: VerifyStatus, reason: impl Into<String>) -> Self {
        Self {
            provider,
            valid: status == VerifyStatus::Valid,
            status,
            reason: reason.into(),
            expected: None,
            received: None,
            computed_over: None,
            signed_url: None,
            timestamp: None,
            timestamp_age_seconds: None,
            tolerance_seconds: None,
            hints: Vec::new(),
            checked_at: crate::event::now_ms(),
        }
    }

    /// For an inbox whose secret is empty or unresolvable.
    pub fn missing_secret(provider: ProviderKind, reason: impl Into<String>) -> Self {
        let mut r = Self::new(provider, VerifyStatus::MissingSecret, reason);
        r.hints.push("Secrets live in the OS keychain: add it on the Environments page (or in a git-ignored .env) and reference it as {{secret.NAME}}.".into());
        r
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VerifyOptions {
    /// Maximum signed-timestamp age, seconds.
    pub tolerance_seconds: u64,
    /// Reference time (Unix seconds), normally when the request arrived.
    pub now: i64,
    /// Wall-clock time (Unix seconds); used to warn that a replay without re-signing would fail.
    pub current: i64,
}

impl VerifyOptions {
    pub fn at(received_ms: i64, tolerance_seconds: u64) -> Self {
        Self { tolerance_seconds, now: received_ms.div_euclid(1000), current: chrono::Utc::now().timestamp() }
    }
}

// ---------- primitives ----------

fn mac<M: Mac + KeyInit>(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut m = <M as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().to_vec()
}

/// HMAC over the concatenation of `parts`.
pub fn hmac(alg: HmacAlgorithm, key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    match alg {
        HmacAlgorithm::Sha256 => mac::<Hmac<sha2::Sha256>>(key, parts),
        HmacAlgorithm::Sha1 => mac::<Hmac<sha1::Sha1>>(key, parts),
        HmacAlgorithm::Sha512 => mac::<Hmac<sha2::Sha512>>(key, parts),
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Standard or URL-safe base64, padding optional.
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    STANDARD.decode(s).or_else(|_| STANDARD_NO_PAD.decode(s)).or_else(|_| URL_SAFE.decode(s)).or_else(|_| URL_SAFE_NO_PAD.decode(s)).ok()
}

fn decode(enc: SignatureEncoding, s: &str) -> Option<Vec<u8>> {
    match enc {
        SignatureEncoding::Hex => hex::decode(s.trim()).ok(),
        SignatureEncoding::Base64 => b64_decode(s),
    }
}

fn encode(enc: SignatureEncoding, bytes: &[u8]) -> String {
    match enc {
        SignatureEncoding::Hex => hex::encode(bytes),
        SignatureEncoding::Base64 => b64(bytes),
    }
}

/// `4bf1bf…3969`: enough to compare by eye, not enough to reuse. A scheme prefix (`sha256=`,
/// `v0=`, `v1,`) stays readable.
pub fn mask(sig: &str) -> String {
    if let Some(i) = sig.find(['=', ',']).filter(|i| *i < 10 && *i + 1 < sig.len()) {
        return format!("{}{}", &sig[..=i], mask(&sig[i + 1..]));
    }
    let chars: Vec<char> = sig.chars().collect();
    if chars.len() <= 12 {
        return "•".repeat(chars.len().clamp(1, 8));
    }
    format!("{}…{}", chars[..6].iter().collect::<String>(), chars[chars.len() - 4..].iter().collect::<String>())
}

fn body_desc(body: &[u8]) -> String {
    format!("raw body ({} bytes)", body.len())
}

fn quote_prefix(p: &str) -> String {
    format!("\"{}\"", p.replace('"', "\\\""))
}

/// Header names a provider signs with (for replacing/removing them when converting events).
pub fn signature_headers(kind: ProviderKind, generic: Option<&GenericHmac>) -> Vec<String> {
    match kind {
        ProviderKind::StandardWebhooks => {
            ["webhook-id", "webhook-timestamp", "webhook-signature", "svix-id", "svix-timestamp", "svix-signature"].map(String::from).to_vec()
        }
        ProviderKind::GenericHmac => generic.map(|g| vec![g.header.clone()]).filter(|v| !v[0].is_empty()).unwrap_or_default(),
        k => crate::model::provider_info(k).signature_headers.iter().map(|s| s.to_string()).collect(),
    }
}

// ---------- verification ----------

/// What one provider computed for one secret.
struct Check {
    matched: bool,
    expected: String,
    received: String,
    computed_over: String,
    timestamp: Option<i64>,
    /// Allowed skew into the future too (Slack, Standard Webhooks); Stripe only rejects old ones.
    symmetric_tolerance: bool,
    signed_url: Option<String>,
    hints: Vec<String>,
}

type Failure = (VerifyStatus, String);

fn missing(name: &str, what: &str) -> Failure {
    (VerifyStatus::MissingHeader, format!("missing header {name} — this request wasn't signed by {what}, or a proxy stripped the header"))
}

fn malformed(msg: impl Into<String>) -> Failure {
    (VerifyStatus::Malformed, msg.into())
}

fn parse_ts(s: &str, header: &str) -> Result<i64, Failure> {
    s.trim().parse::<i64>().map_err(|_| malformed(format!("{header} timestamp {s:?} is not a Unix timestamp")))
}

fn check(kind: ProviderKind, generic: Option<&GenericHmac>, secret: &str, input: &SignInput) -> Result<Check, Failure> {
    match kind {
        ProviderKind::None => Err((VerifyStatus::Malformed, "this inbox has no signature scheme".into())),
        ProviderKind::Stripe => check_stripe(secret, input),
        ProviderKind::Github => check_github(secret, input),
        ProviderKind::Shopify => check_shopify(secret, input),
        ProviderKind::Slack => check_slack(secret, input),
        ProviderKind::Twilio => check_twilio(secret, input),
        ProviderKind::StandardWebhooks => check_standard(secret, input),
        ProviderKind::GenericHmac => check_generic(generic, secret, input),
    }
}

fn check_stripe(secret: &str, input: &SignInput) -> Result<Check, Failure> {
    let h = header(input.headers, "stripe-signature").ok_or_else(|| missing("Stripe-Signature", "Stripe"))?;
    let (mut t, mut v1, mut v0) = (None, Vec::new(), false);
    for part in h.split(',') {
        let Some((k, v)) = part.split_once('=') else { continue };
        match k.trim() {
            "t" if t.is_none() => t = Some(v.trim()),
            "v1" => v1.push(v.trim()),
            "v0" => v0 = true,
            _ => {}
        }
    }
    let t = t.ok_or_else(|| malformed("Stripe-Signature has no t= timestamp"))?;
    let ts = parse_ts(t, "Stripe-Signature t=")?;
    if v1.is_empty() {
        return Err(malformed(if v0 { "Stripe-Signature has only a v0 signature (legacy test scheme); v1 is required" } else { "Stripe-Signature has no v1= signature" }));
    }
    let expected = hmac(HmacAlgorithm::Sha256, secret.as_bytes(), &[t.as_bytes(), b".", input.body]);
    let matched = v1.iter().any(|s| hex::decode(s).is_ok_and(|d| ct_eq(&d, &expected)));
    let mut hints = Vec::new();
    if !matched {
        if v1.len() > 1 {
            hints.push(format!("{} v1 signatures were sent (secret rotation); none matches this secret.", v1.len()));
        }
        if !secret.starts_with("whsec_") {
            hints.push("Stripe endpoint secrets start with whsec_ — this looks like an API key or another value.".into());
        }
        hints.push("`stripe listen` prints its own whsec_… that differs from the Dashboard endpoint's secret; each endpoint (and test/live mode) has its own.".into());
    }
    Ok(Check {
        matched,
        expected: hex::encode(&expected),
        received: v1.join(", "),
        computed_over: format!("{} + {}", quote_prefix(&format!("{t}.")), body_desc(input.body)),
        timestamp: Some(ts),
        symmetric_tolerance: false,
        signed_url: None,
        hints,
    })
}

fn check_github(secret: &str, input: &SignInput) -> Result<Check, Failure> {
    let (name, alg, prefix, value) = if let Some(v) = header(input.headers, "x-hub-signature-256") {
        ("X-Hub-Signature-256", HmacAlgorithm::Sha256, "sha256=", v)
    } else if let Some(v) = header(input.headers, "x-hub-signature") {
        ("X-Hub-Signature", HmacAlgorithm::Sha1, "sha1=", v)
    } else {
        return Err(missing("X-Hub-Signature-256", "GitHub (set a secret on the webhook so GitHub signs deliveries)"));
    };
    let sig = value.trim().strip_prefix(prefix).ok_or_else(|| malformed(format!("{name} should start with {prefix}")))?;
    let expected = hmac(alg, secret.as_bytes(), &[input.body]);
    let matched = hex::decode(sig).is_ok_and(|d| ct_eq(&d, &expected));
    let mut hints = Vec::new();
    if !matched {
        hints.push("Use the secret entered in the webhook's settings on GitHub (it can't be viewed again — reset it if unsure).".into());
    }
    if alg == HmacAlgorithm::Sha1 {
        hints.push("Only the legacy SHA-1 header was sent; GitHub recommends verifying X-Hub-Signature-256.".into());
    }
    Ok(Check {
        matched,
        expected: format!("{prefix}{}", hex::encode(&expected)),
        received: value.trim().to_string(),
        computed_over: body_desc(input.body),
        timestamp: None,
        symmetric_tolerance: false,
        signed_url: None,
        hints,
    })
}

fn check_shopify(secret: &str, input: &SignInput) -> Result<Check, Failure> {
    let value = header(input.headers, "x-shopify-hmac-sha256").ok_or_else(|| missing("X-Shopify-Hmac-Sha256", "Shopify"))?;
    let expected = hmac(HmacAlgorithm::Sha256, secret.as_bytes(), &[input.body]);
    let matched = b64_decode(value).is_some_and(|d| ct_eq(&d, &expected));
    let mut hints = Vec::new();
    if !matched {
        hints.push("Use the app's client secret (API secret key), not an access token. A rotated secret can take up to an hour to apply.".into());
    }
    Ok(Check {
        matched,
        expected: b64(&expected),
        received: value.trim().to_string(),
        computed_over: body_desc(input.body),
        timestamp: None,
        symmetric_tolerance: false,
        signed_url: None,
        hints,
    })
}

fn check_slack(secret: &str, input: &SignInput) -> Result<Check, Failure> {
    let ts_s = header(input.headers, "x-slack-request-timestamp").ok_or_else(|| missing("X-Slack-Request-Timestamp", "Slack"))?;
    let value = header(input.headers, "x-slack-signature").ok_or_else(|| missing("X-Slack-Signature", "Slack"))?;
    let ts = parse_ts(ts_s, "X-Slack-Request-Timestamp")?;
    let sig = value.trim().strip_prefix("v0=").ok_or_else(|| malformed("X-Slack-Signature should start with v0="))?;
    let ts_s = ts_s.trim();
    let expected = hmac(HmacAlgorithm::Sha256, secret.as_bytes(), &[b"v0:", ts_s.as_bytes(), b":", input.body]);
    let matched = hex::decode(sig).is_ok_and(|d| ct_eq(&d, &expected));
    let mut hints = Vec::new();
    if !matched {
        hints.push("Use the Signing Secret from Basic Information → App Credentials (not the deprecated verification token).".into());
    }
    Ok(Check {
        matched,
        expected: format!("v0={}", hex::encode(&expected)),
        received: value.trim().to_string(),
        computed_over: format!("{} + {}", quote_prefix(&format!("v0:{ts_s}:")), body_desc(input.body)),
        timestamp: Some(ts),
        symmetric_tolerance: true,
        signed_url: None,
        hints,
    })
}

/// `(scheme://authority, rest)` without parsing/normalizing (Twilio signs the URL string as is).
fn split_origin(url: &str) -> Option<(&str, &str, &str)> {
    let scheme_end = url.find("://")?;
    let after = &url[scheme_end + 3..];
    let auth_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    Some((&url[..scheme_end], &after[..auth_end], &after[auth_end..]))
}

/// `(host, port)` of an authority, IPv6-aware.
fn split_port(authority: &str) -> (&str, Option<&str>) {
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let prefix_len = authority.len() - hostport.len();
    let colon = if hostport.starts_with('[') { hostport.find("]:").map(|i| i + 1) } else { hostport.rfind(':') };
    match colon {
        Some(i) => (&authority[..prefix_len + i], Some(&hostport[i + 1..])),
        None => (authority, None),
    }
}

/// The URL with and without the scheme's default port, as Twilio's own validators try both.
fn twilio_url_variants(url: &str) -> Vec<String> {
    let Some((scheme, authority, rest)) = split_origin(url) else { return vec![url.to_string()] };
    let (host, port) = split_port(authority);
    let default = match scheme.to_ascii_lowercase().as_str() {
        "https" => "443",
        "http" => "80",
        _ => return vec![url.to_string()],
    };
    let without = format!("{scheme}://{host}{rest}");
    let with = format!("{scheme}://{host}:{}{rest}", port.unwrap_or(default));
    let mut out = vec![url.to_string()];
    for v in [without, with] {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

fn query_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let q = url.split_once('?')?.1;
    let q = q.split('#').next().unwrap_or(q);
    q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v))
}

/// Twilio's data string: URL followed by `name+value` for each POST parameter, names sorted,
/// duplicate values sorted and de-duplicated (as twilio-python / twilio-node do).
fn twilio_data(url: &str, params: &[(String, String)]) -> String {
    let mut names: Vec<&str> = params.iter().map(|(k, _)| k.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let mut s = url.to_string();
    for n in names {
        let mut vals: Vec<&str> = params.iter().filter(|(k, _)| k == n).map(|(_, v)| v.as_str()).collect();
        vals.sort_unstable();
        vals.dedup();
        for v in vals {
            s.push_str(n);
            s.push_str(v);
        }
    }
    s
}

fn twilio_params(input: &SignInput) -> Vec<(String, String)> {
    if input.method.eq_ignore_ascii_case("POST") && is_form_content_type(header(input.headers, "content-type")) {
        url::form_urlencoded::parse(input.body).into_owned().collect()
    } else {
        Vec::new()
    }
}

fn check_twilio(secret: &str, input: &SignInput) -> Result<Check, Failure> {
    let value = header(input.headers, "x-twilio-signature").ok_or_else(|| missing("X-Twilio-Signature", "Twilio"))?;
    let received = b64_decode(value);
    let body_hash = query_param(input.url, "bodySHA256");
    let mut hints = Vec::new();
    let mut body_ok = true;
    let params = if let Some(expected_hash) = body_hash {
        let actual = hex::encode(sha2::Sha256::digest(input.body));
        body_ok = expected_hash.eq_ignore_ascii_case(&actual);
        if !body_ok {
            hints.push(format!("The body's SHA-256 ({}) doesn't match the bodySHA256 query parameter ({}) — the body was modified.", mask(&actual), mask(expected_hash)));
        }
        Vec::new()
    } else {
        twilio_params(input)
    };
    let sign = |url: &str| hmac(HmacAlgorithm::Sha1, secret.as_bytes(), &[twilio_data(url, &params).as_bytes()]);
    let variants = twilio_url_variants(input.url);
    let matched_url = variants.iter().find(|u| received.as_deref().is_some_and(|r| ct_eq(r, &sign(u))));
    let matched = body_ok && matched_url.is_some();
    if matched_url.is_none() {
        // Common URL mistakes, so the explanation can say exactly what differs.
        let mut alts = Vec::new();
        if let Some(rest) = input.url.strip_prefix("http://") {
            alts.push(format!("https://{rest}"));
        } else if let Some(rest) = input.url.strip_prefix("https://") {
            alts.push(format!("http://{rest}"));
        }
        let (path_part, query_part) = match input.url.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (input.url, None),
        };
        let toggled = if path_part.ends_with('/') { path_part.trim_end_matches('/').to_string() } else { format!("{path_part}/") };
        alts.push(match query_part {
            Some(q) => format!("{toggled}?{q}"),
            None => toggled,
        });
        for alt in alts {
            if twilio_url_variants(&alt).iter().any(|u| received.as_deref().is_some_and(|r| ct_eq(r, &sign(u)))) {
                hints.push(format!("The signature matches {alt} — Twilio signed that exact URL; use it as the inbox's publicUrl (or reconstruct it with X-Forwarded-Proto/Host in your app)."));
            }
        }
        let host = split_origin(input.url).map(|(_, a, _)| split_port(a).0.to_ascii_lowercase()).unwrap_or_default();
        if host == "127.0.0.1" || host == "localhost" || host == "[::1]" || host.starts_with("192.168.") || host.starts_with("10.") {
            hints.push("Twilio signs the public URL it called, but only the local URL is known for this event — set the inbox's publicUrl to the URL configured in the Twilio console, or expose the inbox through a tunnel.".into());
        } else {
            hints.push("Twilio signs the exact URL configured in the console (scheme, host, port, path and query); decoding or re-encoding it breaks validation.".into());
        }
        hints.push("Use the account's primary Auth Token (API keys can't validate signatures).".into());
    }
    let shown_url = matched_url.cloned().unwrap_or_else(|| input.url.to_string());
    let expected = sign(&shown_url);
    let computed_over = if body_hash.is_some() {
        format!("URL {shown_url} (JSON body checked through bodySHA256)")
    } else if params.is_empty() {
        format!("URL {shown_url} (no POST parameters)")
    } else {
        let mut names: Vec<&str> = params.iter().map(|(k, _)| k.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        let shown: Vec<&str> = names.iter().take(8).copied().collect();
        format!("URL {shown_url} + {} sorted POST parameters ({}{})", params.len(), shown.join(", "), if names.len() > 8 { ", …" } else { "" })
    };
    Ok(Check {
        matched,
        expected: b64(&expected),
        received: value.trim().to_string(),
        computed_over,
        timestamp: None,
        symmetric_tolerance: false,
        signed_url: Some(shown_url),
        hints,
    })
}

/// Standard Webhooks / Svix header names in use: (`id`, `timestamp`, `signature`).
fn standard_names(headers: &[(String, String)]) -> (&'static str, &'static str, &'static str) {
    if header(headers, "webhook-signature").is_none() && header(headers, "svix-signature").is_some() {
        ("svix-id", "svix-timestamp", "svix-signature")
    } else {
        ("webhook-id", "webhook-timestamp", "webhook-signature")
    }
}

/// `whsec_<base64>` → key bytes.
pub fn standard_key(secret: &str) -> Option<Vec<u8>> {
    let s = secret.trim();
    let s = s.strip_prefix("whsec_").unwrap_or(s);
    b64_decode(s).filter(|k| !k.is_empty())
}

fn check_standard(secret: &str, input: &SignInput) -> Result<Check, Failure> {
    let (id_h, ts_h, sig_h) = standard_names(input.headers);
    let id = header(input.headers, id_h).ok_or_else(|| missing(id_h, "a Standard Webhooks sender"))?;
    let ts_s = header(input.headers, ts_h).ok_or_else(|| missing(ts_h, "a Standard Webhooks sender"))?;
    let value = header(input.headers, sig_h).ok_or_else(|| missing(sig_h, "a Standard Webhooks sender"))?;
    let ts = parse_ts(ts_s, ts_h)?;
    let key = standard_key(secret).ok_or_else(|| malformed("the secret isn't whsec_ followed by base64 — copy it again from the sender's endpoint settings"))?;
    let sigs: Vec<&str> = value.split_whitespace().filter_map(|e| e.strip_prefix("v1,")).collect();
    if sigs.is_empty() {
        return Err(malformed(format!("{sig_h} has no v1,<signature> entry (v1a asymmetric signatures aren't supported)")));
    }
    let ts_s = ts_s.trim();
    let expected = hmac(HmacAlgorithm::Sha256, &key, &[id.trim().as_bytes(), b".", ts_s.as_bytes(), b".", input.body]);
    let matched = sigs.iter().any(|s| b64_decode(s).is_some_and(|d| ct_eq(&d, &expected)));
    let mut hints = Vec::new();
    if !matched {
        if sigs.len() > 1 {
            hints.push(format!("{} v1 signatures were sent (secret rotation); none matches this secret.", sigs.len()));
        }
        hints.push("The secret is base64-decoded after the whsec_ prefix; use the endpoint's signing secret exactly as shown by the sender.".into());
    }
    Ok(Check {
        matched,
        expected: format!("v1,{}", b64(&expected)),
        received: value.trim().to_string(),
        computed_over: format!("{} + {}", quote_prefix(&format!("{}.{ts_s}.", id.trim())), body_desc(input.body)),
        timestamp: Some(ts),
        symmetric_tolerance: true,
        signed_url: None,
        hints,
    })
}

fn check_generic(generic: Option<&GenericHmac>, secret: &str, input: &SignInput) -> Result<Check, Failure> {
    let g = generic.filter(|g| !g.header.trim().is_empty()).ok_or_else(|| malformed("generic HMAC needs a signature header name"))?;
    let value = header(input.headers, g.header.trim()).ok_or_else(|| missing(g.header.trim(), "the configured sender"))?.trim();
    let sig = if g.prefix.is_empty() {
        value
    } else if value.len() >= g.prefix.len() && value[..g.prefix.len()].eq_ignore_ascii_case(&g.prefix) {
        &value[g.prefix.len()..]
    } else {
        return Err(malformed(format!("{} should start with {}", g.header, g.prefix)));
    };
    let expected = hmac(g.algorithm, secret.as_bytes(), &[input.body]);
    let matched = decode(g.encoding, sig).is_some_and(|d| ct_eq(&d, &expected));
    let mut hints = Vec::new();
    if !matched && decode(g.encoding, sig).is_some_and(|d| d.len() != expected.len()) {
        hints.push(format!("The received signature has a different length than {:?} produces — check the algorithm and encoding.", g.algorithm));
    }
    Ok(Check {
        matched,
        expected: format!("{}{}", g.prefix, encode(g.encoding, &expected)),
        received: value.to_string(),
        computed_over: body_desc(input.body),
        timestamp: None,
        symmetric_tolerance: false,
        signed_url: None,
        hints,
    })
}

/// Verifies `input` against `secret` (already resolved). Never fails: problems are described in
/// the result.
pub fn verify(kind: ProviderKind, generic: Option<&GenericHmac>, secret: &str, input: &SignInput, opts: VerifyOptions) -> VerifyResult {
    if secret.is_empty() {
        return VerifyResult::missing_secret(kind, "no signing secret configured");
    }
    let c = match check(kind, generic, secret, input) {
        Ok(c) => c,
        Err((status, reason)) => return VerifyResult::new(kind, status, reason),
    };
    let mut r = VerifyResult::new(kind, VerifyStatus::Valid, "");
    r.expected = Some(mask(&c.expected));
    r.received = Some(c.received.clone());
    r.computed_over = Some(c.computed_over.clone());
    r.signed_url = c.signed_url.clone();
    r.hints = c.hints.clone();
    if let Some(ts) = c.timestamp {
        r.timestamp = Some(ts);
        r.timestamp_age_seconds = Some(opts.now - ts);
        r.tolerance_seconds = Some(opts.tolerance_seconds);
    }
    if !c.matched {
        r.status = VerifyStatus::Mismatch;
        r.valid = false;
        r.reason = match kind {
            ProviderKind::Twilio => "signature mismatch — the URL or parameters differ from what Twilio signed, or the auth token is wrong".into(),
            _ => "signature mismatch — body was modified or the secret is wrong".into(),
        };
        if secret.trim() != secret
            && let Ok(t) = check(kind, generic, secret.trim(), input)
            && t.matched
        {
            r.hints.insert(0, "The signature matches once leading/trailing whitespace is removed from the secret — fix the stored value.".into());
        }
        if kind != ProviderKind::Twilio {
            r.hints.push("Verify the raw bytes: frameworks that parse and re-serialize JSON (e.g. a body parser running before the webhook route) change the payload.".into());
        }
        return r;
    }
    if let Some(ts) = c.timestamp {
        let age = opts.now - ts;
        let tol = opts.tolerance_seconds as i64;
        if age > tol {
            r.status = VerifyStatus::Expired;
            r.valid = false;
            r.reason = format!("timestamp {age} s old (tolerance {tol} s) — the signature itself is valid");
            r.hints.insert(0, "Replay with “Re-sign” to send it with a fresh timestamp and signature.".into());
            return r;
        }
        if c.symmetric_tolerance && -age > tol {
            r.status = VerifyStatus::Expired;
            r.valid = false;
            r.reason = format!("timestamp {} s in the future (tolerance {tol} s) — check the system clock", -age);
            return r;
        }
        r.reason = if age >= 0 { format!("signature valid (signed {age} s before arrival)") } else { format!("signature valid (timestamp {} s ahead of this clock)", -age) };
        let replay_age = opts.current - ts;
        if replay_age > tol {
            r.hints.push(format!("Replaying it now without re-signing would be rejected: the signature is {} old.", human_secs(replay_age)));
        }
    } else {
        r.reason = "signature valid".into();
    }
    r
}

fn human_secs(s: i64) -> String {
    match s {
        s if s < 120 => format!("{s} s"),
        s if s < 7200 => format!("{} min", s / 60),
        s if s < 172_800 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

// ---------- re-signing ----------

#[derive(Debug, Clone, Default)]
pub struct SignOptions {
    /// Unix seconds; defaults to now.
    pub now: Option<i64>,
}

/// Fresh signature headers for a (possibly edited) request.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resigned {
    /// Headers to set (replacing any with the same name).
    pub headers: Vec<(String, String)>,
    /// New URL when the scheme signs the URL and it had to change (Twilio `bodySHA256`).
    pub url: Option<String>,
    pub computed_over: String,
}

impl Resigned {
    pub fn apply(&self, headers: &mut Vec<(String, String)>) {
        for (k, v) in &self.headers {
            set_header(headers, k, v);
        }
    }
}

fn replace_query_param(url: &str, name: &str, value: &str) -> String {
    let Some((base, rest)) = url.split_once('?') else { return url.to_string() };
    let (query, frag) = match rest.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (rest, None),
    };
    let q: Vec<String> = query.split('&').map(|kv| if kv.split_once('=').is_some_and(|(k, _)| k == name) { format!("{name}={value}") } else { kv.to_string() }).collect();
    let mut out = format!("{base}?{}", q.join("&"));
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// Signs `input` as `kind` would right now. `input.url` is where the request will be sent (Twilio
/// signs it). Existing message ids (Standard Webhooks) are kept so receivers can deduplicate.
pub fn sign(kind: ProviderKind, generic: Option<&GenericHmac>, secret: &str, input: &SignInput, opts: &SignOptions) -> Result<Resigned, String> {
    if secret.is_empty() {
        return Err("no signing secret".into());
    }
    let now = opts.now.unwrap_or_else(|| chrono::Utc::now().timestamp());
    let body = input.body;
    Ok(match kind {
        ProviderKind::None => return Err("this inbox has no signature scheme to re-sign with".into()),
        ProviderKind::Stripe => {
            let t = now.to_string();
            let sig = hex::encode(hmac(HmacAlgorithm::Sha256, secret.as_bytes(), &[t.as_bytes(), b".", body]));
            Resigned { headers: vec![("Stripe-Signature".into(), format!("t={t},v1={sig}"))], url: None, computed_over: format!("{} + {}", quote_prefix(&format!("{t}.")), body_desc(body)) }
        }
        ProviderKind::Github => {
            let mut headers = vec![("X-Hub-Signature-256".into(), format!("sha256={}", hex::encode(hmac(HmacAlgorithm::Sha256, secret.as_bytes(), &[body]))))];
            if header(input.headers, "x-hub-signature").is_some() {
                headers.push(("X-Hub-Signature".into(), format!("sha1={}", hex::encode(hmac(HmacAlgorithm::Sha1, secret.as_bytes(), &[body])))));
            }
            Resigned { headers, url: None, computed_over: body_desc(body) }
        }
        ProviderKind::Shopify => Resigned {
            headers: vec![("X-Shopify-Hmac-Sha256".into(), b64(&hmac(HmacAlgorithm::Sha256, secret.as_bytes(), &[body])))],
            url: None,
            computed_over: body_desc(body),
        },
        ProviderKind::Slack => {
            let t = now.to_string();
            let sig = hex::encode(hmac(HmacAlgorithm::Sha256, secret.as_bytes(), &[b"v0:", t.as_bytes(), b":", body]));
            Resigned {
                headers: vec![("X-Slack-Request-Timestamp".into(), t.clone()), ("X-Slack-Signature".into(), format!("v0={sig}"))],
                url: None,
                computed_over: format!("{} + {}", quote_prefix(&format!("v0:{t}:")), body_desc(body)),
            }
        }
        ProviderKind::Twilio => {
            let (url, params) = if query_param(input.url, "bodySHA256").is_some() {
                (replace_query_param(input.url, "bodySHA256", &hex::encode(sha2::Sha256::digest(body))), Vec::new())
            } else {
                (input.url.to_string(), twilio_params(input))
            };
            let sig = b64(&hmac(HmacAlgorithm::Sha1, secret.as_bytes(), &[twilio_data(&url, &params).as_bytes()]));
            let changed = (url != input.url).then(|| url.clone());
            Resigned { headers: vec![("X-Twilio-Signature".into(), sig)], url: changed, computed_over: format!("URL {url} + {} sorted POST parameters", params.len()) }
        }
        ProviderKind::StandardWebhooks => {
            let (id_h, ts_h, sig_h) = standard_names(input.headers);
            let key = standard_key(secret).ok_or("the secret isn't whsec_ followed by base64")?;
            let id = header(input.headers, id_h).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("msg_{}", ulid::Ulid::generate()));
            let t = now.to_string();
            let sig = b64(&hmac(HmacAlgorithm::Sha256, &key, &[id.as_bytes(), b".", t.as_bytes(), b".", body]));
            Resigned {
                headers: vec![(id_h.into(), id.clone()), (ts_h.into(), t.clone()), (sig_h.into(), format!("v1,{sig}"))],
                url: None,
                computed_over: format!("{} + {}", quote_prefix(&format!("{id}.{t}.")), body_desc(body)),
            }
        }
        ProviderKind::GenericHmac => {
            let g = generic.filter(|g| !g.header.trim().is_empty()).ok_or("generic HMAC needs a signature header name")?;
            let sig = encode(g.encoding, &hmac(g.algorithm, secret.as_bytes(), &[body]));
            Resigned { headers: vec![(g.header.trim().to_string(), format!("{}{sig}", g.prefix))], url: None, computed_over: body_desc(body) }
        }
    })
}

#[cfg(test)]
mod tests;
