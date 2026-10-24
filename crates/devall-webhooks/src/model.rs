//! `webhooks/<slug>.webhook.yaml`: one inbox per file. An inbox is a path on the local receiver
//! (`/stripe`) with a signature scheme, a canned response and an optional forward target.
//!
//! ```yaml
//! version: 1
//! name: Stripe
//! path: /stripe
//! provider: stripe
//! secret: '{{secret.STRIPE_WEBHOOK_SECRET}}'
//! forward:
//!   url: http://localhost:3000/webhooks/stripe
//! ```
//!
//! A custom HMAC scheme uses `provider: genericHmac` plus a `genericHmac:` block (header,
//! algorithm, encoding, prefix). Secrets are always `{{secret.NAME}}` references; values live in
//! the OS keychain.

use devall_core::Extra;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
/// Stripe, Slack and Standard Webhooks libraries all default to five minutes.
pub const DEFAULT_TOLERANCE_SECONDS: u64 = 300;
pub const DEFAULT_FORWARD_TIMEOUT_MS: u64 = 10_000;

fn format_version() -> u32 {
    FORMAT_VERSION
}
fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}
fn default_status() -> u16 {
    200
}
fn default_tolerance() -> u64 {
    DEFAULT_TOLERANCE_SECONDS
}
fn is_default_tolerance(n: &u64) -> bool {
    *n == DEFAULT_TOLERANCE_SECONDS
}
fn default_forward_timeout() -> u64 {
    DEFAULT_FORWARD_TIMEOUT_MS
}
fn is_default_forward_timeout(n: &u64) -> bool {
    *n == DEFAULT_FORWARD_TIMEOUT_MS
}

/// Who signs the webhooks arriving at an inbox (decides how signatures are verified and re-signed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    /// No verification.
    #[default]
    None,
    /// `Stripe-Signature: t=…,v1=…` — HMAC-SHA256 over `t.body`.
    Stripe,
    /// `X-Hub-Signature-256: sha256=…` — HMAC-SHA256 over the body (legacy `X-Hub-Signature` SHA-1).
    Github,
    /// `X-Shopify-Hmac-Sha256` — base64 HMAC-SHA256 over the body.
    Shopify,
    /// `X-Slack-Signature: v0=…` — HMAC-SHA256 over `v0:timestamp:body`.
    Slack,
    /// `X-Twilio-Signature` — base64 HMAC-SHA1 over the public URL + sorted form parameters.
    Twilio,
    /// `webhook-id` / `webhook-timestamp` / `webhook-signature: v1,…` (also Svix's `svix-*`).
    StandardWebhooks,
    /// Any HMAC over the raw body in one header (see [`GenericHmac`]).
    GenericHmac,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 8] = [
        ProviderKind::None,
        ProviderKind::Stripe,
        ProviderKind::Github,
        ProviderKind::Shopify,
        ProviderKind::Slack,
        ProviderKind::Twilio,
        ProviderKind::StandardWebhooks,
        ProviderKind::GenericHmac,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::None => "None",
            ProviderKind::Stripe => "Stripe",
            ProviderKind::Github => "GitHub",
            ProviderKind::Shopify => "Shopify",
            ProviderKind::Slack => "Slack",
            ProviderKind::Twilio => "Twilio",
            ProviderKind::StandardWebhooks => "Standard Webhooks",
            ProviderKind::GenericHmac => "Generic HMAC",
        }
    }

    /// camelCase id as written in files.
    pub fn id(self) -> &'static str {
        match self {
            ProviderKind::None => "none",
            ProviderKind::Stripe => "stripe",
            ProviderKind::Github => "github",
            ProviderKind::Shopify => "shopify",
            ProviderKind::Slack => "slack",
            ProviderKind::Twilio => "twilio",
            ProviderKind::StandardWebhooks => "standardWebhooks",
            ProviderKind::GenericHmac => "genericHmac",
        }
    }

    /// Whether the signed content includes a timestamp (replays need re-signing).
    pub fn is_timestamped(self) -> bool {
        matches!(self, ProviderKind::Stripe | ProviderKind::Slack | ProviderKind::StandardWebhooks)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HmacAlgorithm {
    #[default]
    Sha256,
    Sha1,
    Sha512,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SignatureEncoding {
    #[default]
    Hex,
    Base64,
}

/// A custom scheme: `header: <prefix><encoding(HMAC(secret, raw body))>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GenericHmac {
    /// Header carrying the signature, e.g. `X-Signature`.
    pub header: String,
    #[serde(default)]
    pub algorithm: HmacAlgorithm,
    #[serde(default)]
    pub encoding: SignatureEncoding,
    /// Text before the signature, e.g. `sha256=`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
}

/// What the receiver answers the sender with (unless `forward.passThrough` is on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSpec {
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// Wait before answering (to test the sender's timeouts). Capped at 120 s.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay_ms: u64,
}

impl Default for ResponseSpec {
    fn default() -> Self {
        Self { status: 200, headers: IndexMap::new(), body: String::new(), delay_ms: 0 }
    }
}

impl ResponseSpec {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Forwarding of each received event to the local app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSpec {
    /// Target URL, e.g. `http://localhost:3000/webhooks/stripe`. May use `{{variables}}`. The
    /// received query string is appended when the URL has none.
    pub url: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Send the original `Host` header instead of the target's.
    #[serde(default, skip_serializing_if = "is_false")]
    pub preserve_host: bool,
    #[serde(default = "default_forward_timeout", skip_serializing_if = "is_default_forward_timeout")]
    pub timeout_ms: u64,
    /// Wait for the local app and answer the sender with its response (needed for Slack slash
    /// commands, Twilio TwiML, …) instead of the inbox's canned `response`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pass_through: bool,
}

impl ForwardSpec {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), enabled: true, preserve_host: false, timeout_ms: DEFAULT_FORWARD_TIMEOUT_MS, pass_through: false }
    }
}

/// `webhooks/<slug>.webhook.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxFile {
    #[serde(default = "format_version")]
    pub version: u32,
    pub name: String,
    /// Receiver path, e.g. `/stripe`. Sub-paths (`/stripe/connect`) route here too unless another
    /// inbox matches more exactly.
    pub path: String,
    #[serde(default)]
    pub provider: ProviderKind,
    /// Only for `provider: genericHmac`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic_hmac: Option<GenericHmac>,
    /// Signing secret, normally `{{secret.NAME}}` (resolved with the active environment).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret: String,
    #[serde(default, skip_serializing_if = "ResponseSpec::is_default")]
    pub response: ResponseSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward: Option<ForwardSpec>,
    /// The exact public URL the provider calls (e.g. a stable tunnel URL). Twilio signs it, so
    /// verification needs it; an exposed tunnel supplies it for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// Maximum age of signed timestamps (Stripe, Slack, Standard Webhooks).
    #[serde(default = "default_tolerance", skip_serializing_if = "is_default_tolerance")]
    pub tolerance_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl InboxFile {
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            name: name.into(),
            path: path.into(),
            provider: ProviderKind::None,
            generic_hmac: None,
            secret: String::new(),
            response: ResponseSpec::default(),
            forward: None,
            public_url: None,
            tolerance_seconds: DEFAULT_TOLERANCE_SECONDS,
            description: None,
            extra: Extra::new(),
        }
    }

    /// A new inbox for `provider` with the conventional secret name and path.
    pub fn template(name: &str, provider: ProviderKind) -> Self {
        let slug = devall_core::fsutil::slugify(name);
        let mut f = Self::new(name.trim(), format!("/{slug}"));
        f.provider = provider;
        let info = provider_info(provider);
        if let Some(secret) = info.secret_name {
            f.secret = format!("{{{{secret.{secret}}}}}");
        }
        if provider == ProviderKind::GenericHmac {
            f.generic_hmac = Some(GenericHmac { header: "X-Signature".into(), prefix: "sha256=".into(), ..Default::default() });
        }
        f
    }
}

/// Static help for a provider, shown in the inbox editor.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub kind: ProviderKind,
    pub label: &'static str,
    /// Headers that carry the signature (and its timestamp/id).
    pub signature_headers: &'static [&'static str],
    /// Conventional secret name for `{{secret.NAME}}`.
    pub secret_name: Option<&'static str>,
    /// Where to find the secret.
    pub secret_hint: &'static str,
    /// One-line description of the algorithm.
    pub algorithm: &'static str,
    pub timestamped: bool,
    pub docs_url: Option<&'static str>,
}

pub fn provider_info(kind: ProviderKind) -> ProviderInfo {
    let (headers, secret, hint, algorithm, docs): (&'static [&'static str], Option<&'static str>, &'static str, &'static str, Option<&'static str>) =
        match kind {
            ProviderKind::None => (&[], None, "No signature verification.", "—", None),
            ProviderKind::Stripe => (
                &["Stripe-Signature"],
                Some("STRIPE_WEBHOOK_SECRET"),
                "Dashboard → Developers → Webhooks → your endpoint → Signing secret (whsec_…). `stripe listen` prints its own whsec_ that differs from the Dashboard one.",
                "HMAC-SHA256(secret, \"<t>.<raw body>\") as hex in v1=; several v1 during rotation",
                Some("https://docs.stripe.com/webhooks/signature"),
            ),
            ProviderKind::Github => (
                &["X-Hub-Signature-256", "X-Hub-Signature"],
                Some("GITHUB_WEBHOOK_SECRET"),
                "Repository/organization/app → Settings → Webhooks → Secret (you choose it when creating the webhook).",
                "sha256=HMAC-SHA256(secret, raw body) as hex (legacy X-Hub-Signature: sha1=)",
                Some("https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries"),
            ),
            ProviderKind::Shopify => (
                &["X-Shopify-Hmac-Sha256"],
                Some("SHOPIFY_API_SECRET"),
                "Partner Dashboard / Dev Dashboard → your app → Client credentials → Client secret (API secret key). Rotated secrets can take up to an hour to apply.",
                "base64(HMAC-SHA256(secret, raw body))",
                Some("https://shopify.dev/docs/apps/build/webhooks/subscribe/https"),
            ),
            ProviderKind::Slack => (
                &["X-Slack-Signature", "X-Slack-Request-Timestamp"],
                Some("SLACK_SIGNING_SECRET"),
                "api.slack.com/apps → your app → Basic Information → App Credentials → Signing Secret (not the deprecated verification token).",
                "v0=HMAC-SHA256(secret, \"v0:<timestamp>:<raw body>\") as hex",
                Some("https://docs.slack.dev/authentication/verifying-requests-from-slack"),
            ),
            ProviderKind::Twilio => (
                &["X-Twilio-Signature"],
                Some("TWILIO_AUTH_TOKEN"),
                "Twilio Console → Account → API keys & tokens → Auth Token (primary). Twilio signs the exact public URL it called, so set publicUrl or expose the inbox through a tunnel.",
                "base64(HMAC-SHA1(auth token, public URL + sorted form params)); JSON bodies add ?bodySHA256=",
                Some("https://www.twilio.com/docs/usage/webhooks/webhooks-security"),
            ),
            ProviderKind::StandardWebhooks => (
                &["webhook-id", "webhook-timestamp", "webhook-signature"],
                Some("WEBHOOK_SECRET"),
                "The sender's endpoint settings (Svix, Clerk, Resend, …): a secret that starts with whsec_ followed by base64.",
                "v1,base64(HMAC-SHA256(base64-decoded secret, \"<id>.<timestamp>.<raw body>\"))",
                Some("https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md"),
            ),
            ProviderKind::GenericHmac => (
                &[],
                Some("WEBHOOK_SECRET"),
                "The shared secret configured in the sending service.",
                "<prefix><hex|base64(HMAC(secret, raw body))> in one header",
                None,
            ),
        };
    ProviderInfo {
        kind,
        label: kind.label(),
        signature_headers: headers,
        secret_name: secret,
        secret_hint: hint,
        algorithm,
        timestamped: kind.is_timestamped(),
        docs_url: docs,
    }
}

pub fn providers() -> Vec<ProviderInfo> {
    ProviderKind::ALL.iter().map(|k| provider_info(*k)).collect()
}

/// A problem with an inbox definition. Inboxes with issues still load (best effort).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// Dotted field path, e.g. `forward.url`.
    pub field: String,
    pub message: String,
}

fn issue(field: &str, message: impl Into<String>) -> Issue {
    Issue { field: field.into(), message: message.into() }
}

pub fn validate(f: &InboxFile) -> Vec<Issue> {
    let mut out = Vec::new();
    if f.name.trim().is_empty() {
        out.push(issue("name", "name is required"));
    }
    if !f.path.starts_with('/') {
        out.push(issue("path", "path must start with /"));
    } else if f.path.starts_with(crate::receiver::INTERNAL_PREFIX) {
        out.push(issue("path", format!("{} is reserved", crate::receiver::INTERNAL_PREFIX)));
    } else if f.path.contains(['?', '#', ' ']) {
        out.push(issue("path", "path cannot contain ?, # or spaces"));
    }
    if f.provider == ProviderKind::GenericHmac {
        match &f.generic_hmac {
            Some(g) if !g.header.trim().is_empty() => {}
            _ => out.push(issue("genericHmac.header", "generic HMAC needs the signature header name")),
        }
    }
    if f.provider != ProviderKind::None {
        if f.secret.trim().is_empty() {
            out.push(issue("secret", "set the signing secret, e.g. {{secret.WEBHOOK_SECRET}}, to verify signatures"));
        } else if !f.secret.contains("{{") {
            out.push(issue("secret", "the secret is written in plain text in a project file — store it in the keychain and use {{secret.NAME}}"));
        }
    }
    if !(100..=599).contains(&f.response.status) {
        out.push(issue("response.status", "status must be between 100 and 599"));
    }
    if let Some(fw) = &f.forward {
        let u = fw.url.trim();
        if u.is_empty() {
            out.push(issue("forward.url", "forward URL is required"));
        } else if !u.contains("{{") && !(u.starts_with("http://") || u.starts_with("https://")) {
            out.push(issue("forward.url", "forward URL must start with http:// or https://"));
        } else if !u.contains("{{") && url::Url::parse(u).is_err() {
            out.push(issue("forward.url", "forward URL is not a valid URL"));
        }
        if fw.timeout_ms == 0 {
            out.push(issue("forward.timeoutMs", "timeout must be greater than 0"));
        }
    }
    if let Some(p) = &f.public_url
        && !p.trim().is_empty()
        && url::Url::parse(p.trim()).is_err()
    {
        out.push(issue("publicUrl", "public URL is not a valid absolute URL"));
    }
    out
}

/// JSON Schema of the inbox file format.
pub fn inbox_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(InboxFile)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trip_keeps_defaults_out() {
        let mut f = InboxFile::template("Stripe", ProviderKind::Stripe);
        f.forward = Some(ForwardSpec::new("http://localhost:3000/webhooks/stripe"));
        let text = devall_core::fsutil::to_yaml(&f).unwrap();
        assert!(text.contains("provider: stripe"), "{text}");
        assert!(text.contains("secret: '{{secret.STRIPE_WEBHOOK_SECRET}}'"), "{text}");
        assert!(!text.contains("toleranceSeconds"), "{text}");
        assert!(!text.contains("enabled"), "{text}");
        assert!(!text.contains("response"), "{text}");
        let back: InboxFile = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn generic_hmac_block() {
        let text = "name: Custom\npath: /custom\nprovider: genericHmac\ngenericHmac:\n  header: X-Sig\n  algorithm: sha512\n  encoding: base64\nsecret: '{{secret.X}}'\nfuture: 1\n";
        let f: InboxFile = serde_yaml_ng::from_str(text).unwrap();
        let g = f.generic_hmac.clone().unwrap();
        assert_eq!(g.algorithm, HmacAlgorithm::Sha512);
        assert_eq!(g.encoding, SignatureEncoding::Base64);
        assert_eq!(f.tolerance_seconds, DEFAULT_TOLERANCE_SECONDS);
        assert!(validate(&f).is_empty(), "{:?}", validate(&f));
        // Unknown keys survive.
        assert!(devall_core::fsutil::to_yaml(&f).unwrap().contains("future: 1"));
    }

    #[test]
    fn validation() {
        let mut f = InboxFile::new("", "stripe");
        f.provider = ProviderKind::Stripe;
        f.secret = "whsec_plain".into();
        f.forward = Some(ForwardSpec::new("localhost:3000"));
        f.response.status = 42;
        let fields: Vec<String> = validate(&f).into_iter().map(|i| i.field).collect();
        assert_eq!(fields, ["name", "path", "secret", "response.status", "forward.url"]);
        let ok = InboxFile::template("GitHub", ProviderKind::Github);
        assert!(validate(&ok).is_empty());
        assert_eq!(ok.path, "/github");
    }

    #[test]
    fn schema_generates() {
        let s = inbox_schema();
        assert!(s.to_string().contains("genericHmac"));
    }
}
