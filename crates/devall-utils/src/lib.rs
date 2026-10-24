//! A registry of small, pure, typed operations. The same registry backs the Utilities screen,
//! inline decoding in other views, the CLI (`devall util <op>`), recipes and, later, MCP tools.
//!
//! Every op takes a text `input` plus a JSON object of string `args` and returns text (or pretty
//! JSON when `OpInfo::output` is `Json`). Missing args fall back to the defaults declared in
//! [`list`], so callers (CLI, recipes) can omit them.

mod case;
mod codegen;
mod color;
mod compare;
mod cron;
mod diff;
mod jwt;
mod magic;
mod misc;
mod net;
mod query;
mod regexp;
mod time;
mod unicode;
mod x509;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{KeyInit, Mac};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Digest;

#[derive(Debug, thiserror::Error)]
pub enum UtilError {
    #[error("unknown operation: {0}")]
    UnknownOp(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, UtilError>;

pub(crate) fn invalid(msg: impl Into<String>) -> UtilError {
    UtilError::Invalid(msg.into())
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Output {
    Text,
    Json,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArgInfo {
    pub name: &'static str,
    pub label: &'static str,
    /// `text`, `secret`, `number`, `select:<a>|<b>|…`, `textarea`
    pub kind: &'static str,
    pub default: &'static str,
}

/// A ready-to-run sample for the UI's "Example" button.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub input: &'static str,
    /// Argument overrides (object of strings); empty object when the defaults are fine.
    pub args: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub output: Output,
    /// Whether the op ignores its input (generators).
    pub no_input: bool,
    pub args: Vec<ArgInfo>,
    /// Talks to the network (DNS, TLS). The UI badges these and only runs them on request.
    pub network: bool,
    pub examples: Vec<Example>,
}

impl OpInfo {
    fn ex(mut self, input: &'static str) -> Self {
        self.examples.push(Example { input, args: json!({}) });
        self
    }

    fn ex_args(mut self, input: &'static str, args: Value) -> Self {
        self.examples.push(Example { input, args });
        self
    }

    fn net(mut self) -> Self {
        self.network = true;
        self
    }
}

const fn arg(name: &'static str, label: &'static str, kind: &'static str, default: &'static str) -> ArgInfo {
    ArgInfo { name, label, kind, default }
}

macro_rules! op {
    ($id:expr, $name:expr, $cat:expr, $desc:expr, $out:ident) => {
        op!($id, $name, $cat, $desc, $out, false, vec![])
    };
    ($id:expr, $name:expr, $cat:expr, $desc:expr, $out:ident, $noin:expr, $args:expr) => {
        OpInfo {
            id: $id,
            name: $name,
            category: $cat,
            description: $desc,
            output: Output::$out,
            no_input: $noin,
            args: $args,
            network: false,
            examples: Vec::new(),
        }
    };
}

const JWT_ALGS: &str = "select:HS256|HS384|HS512|RS256|RS384|RS512|PS256|PS384|PS512|ES256|ES384|EdDSA";
const REGEX_FLAVOR: &str = "select:rust|pcre-like";
const YES_NO: &str = "select:no|yes";
const NO_YES_DEFAULT_YES: &str = "select:yes|no";

const SAMPLE_JWT: &str = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkFkYSBMb3ZlbGFjZSIsImlhdCI6MTcwMDAwMDAwMCwiZXhwIjo0MTAyNDQ0ODAwfQ.ztJg2XTxHbh8LcYWXKXFBzQ12QzE0hb8lvSogRg8vrE";
const SAMPLE_JSON: &str = r#"{
  "id": 42,
  "name": "Ada Lovelace",
  "email": "ada@example.com",
  "active": true,
  "score": 9.5,
  "tags": ["admin", "beta"],
  "address": { "city": "London", "zip": "N1 9GU" },
  "orders": [
    { "id": "o-1", "total": 19.99, "items": 2 },
    { "id": "o-2", "total": 5, "items": 1, "coupon": "WELCOME" }
  ],
  "manager": null
}"#;
const SAMPLE_JSON_B: &str = r#"{
  "id": 42,
  "name": "Ada King",
  "email": "ada@example.com",
  "active": false,
  "score": 9.5,
  "tags": ["beta", "admin", "staff"],
  "address": { "city": "London" },
  "orders": [
    { "id": "o-1", "total": 21.5, "items": 2 },
    { "id": "o-2", "total": 5, "items": 1, "coupon": "WELCOME" }
  ],
  "manager": { "id": 7 }
}"#;
const SAMPLE_LOG: &str = "2026-10-21 12:00:01 INFO  user=ada id=42 GET /api/users 200 12ms\n2026-10-21 12:00:02 WARN  user=bob id=7 POST /api/orders 429 3ms\n2026-10-21 12:00:05 ERROR user=eve id=13 GET /admin 403 1ms";
// A throwaway test chain (leaf CN=localhost + its private test CA) so the example decodes offline.
const SAMPLE_CERT: &str = concat!(include_str!("testdata/leaf.pem"), include_str!("testdata/ca.pem"));

/// Every operation, in display order.
pub fn list() -> Vec<OpInfo> {
    registry().to_vec()
}

fn registry() -> &'static [OpInfo] {
    static OPS: std::sync::OnceLock<Vec<OpInfo>> = std::sync::OnceLock::new();
    OPS.get_or_init(build_registry)
}

fn build_registry() -> Vec<OpInfo> {
    vec![
        // ── Encoding ───────────────────────────────────────────────────────────────────────────
        op!("base64.encode", "Base64 encode", "Encoding", "Text → Base64 (standard alphabet, padded)", Text).ex("Hello, /dev/all!"),
        op!("base64.decode", "Base64 decode", "Encoding", "Base64 or Base64URL → text (padding optional)", Text).ex("SGVsbG8sIC9kZXYvYWxsIQ=="),
        op!("base64url.encode", "Base64URL encode", "Encoding", "Text → URL-safe Base64 without padding", Text).ex("subjects?_d=1"),
        op!("url.encode", "URL encode", "Encoding", "Percent-encode a URL component", Text).ex("name=Ada Lovelace&role=admin/owner"),
        op!("url.decode", "URL decode", "Encoding", "Decode %XX sequences and +", Text).ex("name%3DAda%20Lovelace%26role%3Dadmin"),
        op!("url.parse", "Parse URL", "Encoding", "Split a URL into scheme, host, path, query parameters…", Json)
            .ex("https://user:pw@api.example.com:8443/v1/users?page=2&sort=name#top"),
        op!("html.encode", "HTML entities encode", "Encoding", "Escape <, >, &, quotes", Text).ex("<a href=\"/x\">Tom & Jerry</a>"),
        op!("html.decode", "HTML entities decode", "Encoding", "Unescape named and numeric entities", Text).ex("&lt;b&gt;caf&eacute; &amp; cr&#232;me&lt;/b&gt;"),
        op!("hex.encode", "Hex encode", "Encoding", "Text → hexadecimal bytes", Text).ex("Hi!"),
        op!("hex.decode", "Hex decode", "Encoding", "Hexadecimal → text", Text).ex("48 65 6c 6c 6f"),
        op!("gzip.compress", "Gzip compress", "Encoding", "Text → gzip → Base64", Text, false, vec![arg("level", "Level (0–9)", "number", "6")])
            .ex("Hello Hello Hello Hello Hello Hello"),
        op!("gzip.decompress", "Gzip decompress", "Encoding", "Base64 (or hex) gzip/zlib/deflate → text", Text).ex("H4sIAAAAAAAA//NIzcnJV/DARwIAOz1tpyMAAAA="),
        op!(
            "string.escape",
            "String escape",
            "Encoding",
            "Escape or unescape text for JSON, JS, Java, C, SQL, regex, shell, CSV or XML",
            Text,
            false,
            vec![arg("mode", "Language", "select:json|js|java|c|sql|regex|shell|csv|xml", "json"), arg("direction", "Direction", "select:escape|unescape", "escape")]
        )
        .ex("He said \"hi\"\n\tand left — it's done."),
        op!(
            "number.base",
            "Number base",
            "Encoding",
            "Binary, octal, decimal, hex and any base 2–36 (up to 128-bit)",
            Json,
            false,
            vec![arg("from", "Input base (auto = 0x/0o/0b prefix)", "text", "auto"), arg("to", "Extra output base (2–36)", "number", "36")]
        )
        .ex("0xFF_FF")
        .ex_args("zz", json!({ "from": "36" })),
        // ── Data ───────────────────────────────────────────────────────────────────────────────
        op!("json.format", "JSON format", "Data", "Pretty-print JSON", Text, false, vec![arg("indent", "Indent", "select:2|4|tab", "2")]).ex(r#"{"a":1,"b":[true,null,"x"]}"#),
        op!("json.minify", "JSON minify", "Data", "Remove whitespace", Text).ex(SAMPLE_JSON),
        op!("json.toYaml", "JSON → YAML", "Data", "Convert JSON to YAML", Text).ex(SAMPLE_JSON),
        op!("yaml.toJson", "YAML → JSON", "Data", "Convert YAML to pretty JSON", Text).ex("name: Ada\ntags: [admin, beta]\naddress:\n  city: London\n"),
        op!("json.escape", "JSON string escape", "Data", "Text → JSON string literal", Text).ex("line 1\nline \"2\""),
        op!("json.unescape", "JSON string unescape", "Data", "JSON string literal → text", Text).ex(r#""line 1\nline \"2\" é""#),
        // ── Query ──────────────────────────────────────────────────────────────────────────────
        op!("jq", "jq", "Query", "Run a jq filter (jaq engine) over JSON or a stream of JSON values", Text, false, vec![arg("filter", "Filter", "text", "."), arg("raw", "Raw strings (-r)", YES_NO, "no")])
            .ex_args(SAMPLE_JSON, json!({ "filter": ".orders | map(select(.total > 10)) | length" }))
            .ex_args(SAMPLE_JSON, json!({ "filter": "{name, city: .address.city, tags: (.tags | join(\",\"))}" })),
        op!(
            "jsonpath",
            "JSONPath",
            "Query",
            "RFC 9535 JSONPath query; returns matched values or their normalized paths",
            Json,
            false,
            vec![arg("path", "Path", "text", "$"), arg("output", "Output", "select:values|paths|both", "values")]
        )
        .ex_args(SAMPLE_JSON, json!({ "path": "$.orders[?@.total > 10].id" }))
        .ex_args(SAMPLE_JSON, json!({ "path": "$..id", "output": "both" })),
        op!("jmespath", "JMESPath", "Query", "JMESPath expression (AWS CLI --query syntax)", Json, false, vec![arg("expression", "Expression", "text", "@")])
            .ex_args(SAMPLE_JSON, json!({ "expression": "orders[?total > `10`].{id: id, total: total}" })),
        // ── Regex ──────────────────────────────────────────────────────────────────────────────
        op!(
            "regex.test",
            "Regex tester",
            "Regex",
            "Matches, numbered and named groups with spans, and timing. Rust flavor is linear-time; pcre-like adds look-around and backreferences.",
            Json,
            false,
            vec![arg("pattern", "Pattern", "text", ""), arg("flags", "Flags (i m s x U)", "text", ""), arg("flavor", "Flavor", REGEX_FLAVOR, "rust")]
        )
        .ex_args(SAMPLE_LOG, json!({ "pattern": r"(?<level>INFO|WARN|ERROR)\s+user=(?<user>\w+).*?(?<status>\d{3})", "flags": "m" }))
        .ex_args("price: $40, cost: $25, tax: 5", json!({ "pattern": r"(?<=\$)\d+", "flavor": "pcre-like" })),
        op!(
            "regex.replace",
            "Regex replace",
            "Regex",
            "Replace matches; use $1, ${name} and $$ in the replacement",
            Text,
            false,
            vec![
                arg("pattern", "Pattern", "text", ""),
                arg("replacement", "Replacement", "text", ""),
                arg("flags", "Flags (i m s x U)", "text", ""),
                arg("flavor", "Flavor", REGEX_FLAVOR, "rust"),
                arg("limit", "Max replacements (0 = all)", "number", "0"),
            ]
        )
        .ex_args("2026-10-21\n1999-01-02", json!({ "pattern": r"(?<y>\d{4})-(?<m>\d{2})-(?<d>\d{2})", "replacement": "${d}/${m}/${y}" })),
        op!(
            "regex.cases",
            "Regex test cases",
            "Regex",
            "Check should-match / should-not-match strings against a pattern (input: JSON {shouldMatch, shouldNotMatch})",
            Json,
            false,
            vec![arg("pattern", "Pattern", "text", ""), arg("flags", "Flags (i m s x U)", "text", ""), arg("flavor", "Flavor", REGEX_FLAVOR, "rust")]
        )
        .ex_args(r#"{"shouldMatch": ["ORD-1234", "order ORD-0001"], "shouldNotMatch": ["ORD-12", "ord-1234"]}"#, json!({ "pattern": r"ORD-\d{4}\b" })),
        op!(
            "regex.split",
            "Regex split",
            "Regex",
            "Split text on a pattern → JSON array",
            Json,
            false,
            vec![arg("pattern", "Pattern", "text", ""), arg("flags", "Flags (i m s x U)", "text", ""), arg("flavor", "Flavor", REGEX_FLAVOR, "rust")]
        )
        .ex_args("a, b;c ; d", json!({ "pattern": r"\s*[,;]\s*" })),
        op!(
            "regex.explain",
            "Regex explain",
            "Regex",
            "Readable, nested explanation of a pattern (input = the pattern)",
            Json,
            false,
            vec![arg("flags", "Flags (i m s x U)", "text", ""), arg("flavor", "Flavor", REGEX_FLAVOR, "rust")]
        )
        .ex(r"^(?<user>[\w.+-]+)@(?<domain>[\w-]+(?:\.[\w-]+)+)$")
        .ex_args(r"(?<=\$)\d+(?:\.\d{2})?(?!\d)", json!({ "flavor": "pcre-like" })),
        op!("regex.escape", "Regex escape", "Regex", "Escape text so it matches literally", Text).ex("1+1=2? (yes) [really] $5.00"),
        // ── Diff ───────────────────────────────────────────────────────────────────────────────
        op!(
            "diff.text",
            "Text diff",
            "Diff",
            "Unified diff plus JSON hunks; line, word or character granularity",
            Json,
            false,
            vec![arg("other", "Changed text", "textarea", ""), arg("mode", "Granularity", "select:line|word|char", "line"), arg("context", "Context lines", "number", "3")]
        )
        .ex_args("The quick brown fox\njumps over\nthe lazy dog.\n", json!({ "other": "The quick red fox\njumps over\nthe lazy cat.\nThe end.\n" })),
        op!(
            "diff.json",
            "JSON semantic diff",
            "Diff",
            "Added / removed / changed paths with old and new values, plus an RFC 6902 JSON Patch. Key order never matters.",
            Json,
            false,
            vec![arg("other", "Changed JSON", "textarea", ""), arg("ignoreArrayOrder", "Ignore array order", YES_NO, "no")]
        )
        .ex_args(SAMPLE_JSON, json!({ "other": SAMPLE_JSON_B }))
        .ex_args(SAMPLE_JSON, json!({ "other": SAMPLE_JSON_B, "ignoreArrayOrder": "yes" })),
        op!(
            "text.compare",
            "String compare",
            "Diff",
            "Are two strings really equal? Case/whitespace/Unicode-normalized equality, lengths, first difference, invisible and look-alike characters, SHA-256",
            Json,
            false,
            vec![arg("other", "String B", "textarea", "")]
        )
        .ex_args("api_key = \u{feff}sk-live-42\u{a0}", json!({ "other": "api_key = sk-live-42 " }))
        .ex_args("p\u{430}ypal.com", json!({ "other": "paypal.com" }))
        .ex_args("Caf\u{e9}\r\n", json!({ "other": "Cafe\u{301}\n" })),
        // ── Codegen ────────────────────────────────────────────────────────────────────────────
        op!(
            "json.toTypes",
            "JSON → types",
            "Codegen",
            "Infer types from a JSON sample (nested objects, merged array shapes, optional and nullable fields)",
            Text,
            false,
            vec![
                arg(
                    "language",
                    "Language",
                    "select:TypeScript|Rust (serde)|Go|Python (dataclasses)|Python (pydantic)|Java records|C#|Kotlin|Swift|JSON Schema",
                    "TypeScript"
                ),
                arg("root", "Root type name", "text", "Root"),
            ]
        )
        .ex(SAMPLE_JSON)
        .ex_args(SAMPLE_JSON, json!({ "language": "Rust (serde)", "root": "User" })),
        // ── JWT ────────────────────────────────────────────────────────────────────────────────
        op!("jwt.decode", "JWT decode", "JWT", "Header, payload, timestamps and expiry warnings. Never leaves your machine.", Json).ex(SAMPLE_JWT),
        op!(
            "jwt.verify",
            "JWT verify",
            "JWT",
            "Check the signature (and exp/nbf) with a secret or PEM public key",
            Json,
            false,
            vec![arg("key", "Secret or PEM public key", "secret", ""), arg("alg", "Algorithm (blank = from header)", "text", "")]
        )
        .ex_args(SAMPLE_JWT, json!({ "key": "your-256-bit-secret" })),
        op!(
            "jwt.sign",
            "JWT sign",
            "JWT",
            "Sign a JSON payload with a secret or PEM private key",
            Text,
            false,
            vec![arg("key", "Secret or PEM private key", "secret", ""), arg("alg", "Algorithm", JWT_ALGS, "HS256")]
        )
        .ex_args(r#"{"sub":"42","name":"Ada","exp":4102444800}"#, json!({ "key": "your-256-bit-secret" })),
        // ── Hashing ────────────────────────────────────────────────────────────────────────────
        op!("hash.md5", "MD5", "Hashing", "MD5 digest (hex) — not for security", Text).ex("abc"),
        op!("hash.sha1", "SHA-1", "Hashing", "SHA-1 digest (hex)", Text).ex("abc"),
        op!("hash.sha256", "SHA-256", "Hashing", "SHA-256 digest (hex)", Text).ex("abc"),
        op!("hash.sha512", "SHA-512", "Hashing", "SHA-512 digest (hex)", Text).ex("abc"),
        op!("hash.sha3", "SHA-3 / Keccak", "Hashing", "SHA3-224/256/384/512 or Keccak-256 (Ethereum) digest (hex)", Text, false, vec![arg("variant", "Variant", "select:256|224|384|512|keccak256", "256")])
            .ex("abc"),
        op!("hash.blake3", "BLAKE3", "Hashing", "BLAKE3 digest (hex), any output length", Text, false, vec![arg("length", "Output bytes", "number", "32")]).ex("abc"),
        op!(
            "hmac.sha256",
            "HMAC-SHA256",
            "Hashing",
            "Keyed hash, e.g. to check webhook signatures",
            Text,
            false,
            vec![arg("key", "Key", "secret", ""), arg("encoding", "Output", "select:hex|base64", "hex")]
        )
        .ex_args("what do ya want for nothing?", json!({ "key": "Jefe" })),
        op!(
            "hmac.sha512",
            "HMAC-SHA512",
            "Hashing",
            "Keyed hash (SHA-512)",
            Text,
            false,
            vec![arg("key", "Key", "secret", ""), arg("encoding", "Output", "select:hex|base64", "hex")]
        )
        .ex_args("what do ya want for nothing?", json!({ "key": "Jefe" })),
        op!("bcrypt.hash", "bcrypt hash", "Hashing", "Hash a password with bcrypt (cost 4–16)", Text, false, vec![arg("cost", "Cost", "number", "10")]).ex("correct horse battery staple"),
        op!("bcrypt.verify", "bcrypt verify", "Hashing", "Check a password against a bcrypt hash", Json, false, vec![arg("hash", "bcrypt hash", "text", "")]).ex_args(
            "correct horse battery staple",
            json!({ "hash": "$2b$04$6OlOK4.782O66KrqqpMaIu2rJIzJTW5O.x2oCXnmQ9.lleB16FPeq" })
        ),
        // ── Generators ─────────────────────────────────────────────────────────────────────────
        op!("uuid.v4", "UUID v4", "Generators", "Random UUIDs", Text, true, vec![arg("count", "How many", "number", "1")]),
        op!("uuid.v7", "UUID v7", "Generators", "Time-ordered UUIDs", Text, true, vec![arg("count", "How many", "number", "1")]),
        op!("ulid", "ULID", "Generators", "Sortable 26-char identifiers", Text, true, vec![arg("count", "How many", "number", "1")]),
        op!(
            "password.generate",
            "Password generator",
            "Generators",
            "Cryptographically random passwords with entropy estimate",
            Json,
            true,
            vec![
                arg("length", "Length", "number", "20"),
                arg("count", "How many", "number", "5"),
                arg("lower", "a–z", NO_YES_DEFAULT_YES, "yes"),
                arg("upper", "A–Z", NO_YES_DEFAULT_YES, "yes"),
                arg("digits", "0–9", NO_YES_DEFAULT_YES, "yes"),
                arg("symbols", "Symbols", NO_YES_DEFAULT_YES, "yes"),
                arg("excludeAmbiguous", "Exclude look-alikes (Il1O0)", YES_NO, "no"),
            ]
        ),
        op!(
            "lorem",
            "Lorem ipsum",
            "Generators",
            "Placeholder words, sentences or paragraphs",
            Text,
            true,
            vec![arg("unit", "Unit", "select:paragraphs|sentences|words", "paragraphs"), arg("count", "How many", "number", "3")]
        ),
        op!(
            "qr.generate",
            "QR code",
            "Generators",
            "Text or URL → QR code (SVG)",
            Text,
            false,
            vec![arg("ecLevel", "Error correction", "select:M|L|Q|H", "M"), arg("size", "Min size (px)", "number", "256")]
        )
        .ex("https://example.com/devall"),
        op!(
            "totp.generate",
            "TOTP code",
            "Generators",
            "Current one-time code from a Base32 secret or otpauth:// URI",
            Json,
            false,
            vec![arg("digits", "Digits", "select:6|8", "6"), arg("period", "Period (s)", "number", "30"), arg("algorithm", "Algorithm", "select:SHA1|SHA256|SHA512", "SHA1")]
        )
        .ex("JBSWY3DPEHPK3PXP")
        .ex("otpauth://totp/devall:ada@example.com?secret=JBSWY3DPEHPK3PXP&issuer=devall&digits=8"),
        // ── Time ───────────────────────────────────────────────────────────────────────────────
        op!("time.convert", "Timestamp converter", "Time", "Epoch s/ms/µs/ns, ISO 8601, RFC 2822 or `now` → every format", Json).ex("1700000000").ex("2026-10-21T12:00:00+02:00"),
        op!("time.diff", "Time difference", "Time", "Duration between two timestamps (any format time.convert accepts)", Json, false, vec![arg("other", "Other time", "text", "now")])
            .ex_args("2026-01-01T00:00:00Z", json!({ "other": "2026-10-21T12:34:56Z" })),
        op!(
            "cron.explain",
            "Cron expression",
            "Time",
            "Describe a cron expression (5/6/7 fields, @daily…) and list the next runs in a time zone",
            Json,
            false,
            vec![arg("tz", "Time zone (IANA)", "text", "UTC"), arg("count", "Next runs", "number", "10")]
        )
        .ex("*/15 9-17 * * MON-FRI")
        .ex_args("0 30 2 L * *", json!({ "tz": "Europe/Berlin" }))
        .ex("@weekly"),
        // ── Text ───────────────────────────────────────────────────────────────────────────────
        op!("case.camel", "camelCase", "Text", "Convert to camelCase", Text).ex("parse HTTP response_v2"),
        op!("case.pascal", "PascalCase", "Text", "Convert to PascalCase", Text).ex("parse HTTP response_v2"),
        op!("case.snake", "snake_case", "Text", "Convert to snake_case", Text).ex("parseHTTPResponseV2"),
        op!("case.kebab", "kebab-case", "Text", "Convert to kebab-case", Text).ex("parseHTTPResponseV2"),
        op!("case.constant", "CONSTANT_CASE", "Text", "Convert to CONSTANT_CASE", Text).ex("maxRetryCount"),
        op!("case.title", "Title Case", "Text", "Convert to Title Case", Text).ex("user_profile-settings"),
        op!("slug", "Slugify", "Text", "Text → URL slug (accents folded)", Text, false, vec![arg("separator", "Separator", "select:-|_", "-"), arg("lowercase", "Lowercase", NO_YES_DEFAULT_YES, "yes")])
            .ex("Crème Brûlée: 10 Tips & Tricks (2026)!"),
        op!("text.stats", "Text stats", "Text", "Characters, bytes, words, lines", Json).ex("The quick brown fox\njumps over the lazy dog."),
        op!("unicode.inspect", "Unicode inspector", "Text", "Code points, names, UTF-8/16 bytes; flags invisible, bidi and look-alike characters", Json)
            .ex("Pay\u{200b}Pal ра\u{301}ypal ✓ é")
            .ex("admin\u{202e}txt.exe"),
        // ── Network ────────────────────────────────────────────────────────────────────────────
        op!("cidr.calc", "CIDR calculator", "Network", "IPv4/IPv6 network, mask, broadcast, host range and count", Json, false, vec![arg("contains", "Contains IP? (optional)", "text", "")])
            .ex_args("192.168.10.37/26", json!({ "contains": "192.168.10.70" }))
            .ex("2001:db8:abcd::/48"),
        op!("ip.info", "IP address info", "Network", "Classify an IP (private, loopback, link-local, documentation…) and show its forms", Json).ex("100.64.12.7").ex("fe80::1ff:fe23:4567:890a"),
        op!(
            "dns.lookup",
            "DNS lookup",
            "Network",
            "A / AAAA / CNAME / MX / TXT / NS records via the system resolver config. Network call.",
            Json,
            false,
            vec![arg("type", "Record type", "select:ALL|A|AAAA|CNAME|MX|TXT|NS|SOA|CAA", "ALL")]
        )
        .net()
        .ex("example.com"),
        op!(
            "tls.chain",
            "TLS certificate chain",
            "Network",
            "Connect to host[:port], decode the presented chain and validate it against the OS trust store. Network call.",
            Json
        )
        .net()
        .ex("example.com:443"),
        op!("x509.decode", "X.509 certificate", "Network", "PEM / Base64 DER certificate(s) → subject, SANs, validity, key, fingerprints, extensions", Json).ex(SAMPLE_CERT),
        // ── Web & system ───────────────────────────────────────────────────────────────────────
        op!("http.status", "HTTP status codes", "Web & system", "Look up a status code (or search by text / 4xx class)", Json).ex("429").ex("teapot"),
        op!("useragent.parse", "User-agent parser", "Web & system", "Browser, version, OS and device category", Json)
            .ex("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36"),
        op!("color.convert", "Color converter", "Web & system", "HEX / RGB / HSL / HSV / OKLCH and WCAG contrast against a second colour", Json, false, vec![arg("against", "Contrast against", "text", "#ffffff")])
            .ex("#3b82f6")
            .ex_args("oklch(0.7 0.15 150)", json!({ "against": "black" })),
        op!("chmod.calc", "chmod calculator", "Web & system", "Octal ↔ symbolic Unix permissions (incl. setuid/setgid/sticky)", Json).ex("755").ex("-rwsr-x---").ex("u=rwx,g=rx,o="),
        // ── Recipes ────────────────────────────────────────────────────────────────────────────
        op!("detect.magic", "Magic detect", "Recipes", "Try likely decoders (Base64, hex, URL, gzip, JWT, epoch, JSON…) and rank candidate recipes", Json)
            .ex("H4sIAAAAAAAC/6tWKi1OLVKyUkpMSVTSUSrKz0kFc3Iz84Dc/Gwlq5Ki0tRaACSanD0nAAAA")
            .ex("JTdCJTIyaWQlMjIlM0E0MiUyQyUyMm5hbWUlMjIlM0ElMjJBZGElMjIlN0Q=")
            .ex(SAMPLE_JWT),
        op!(
            "recipe.run",
            "Run recipe",
            "Recipes",
            "Chain operations: steps = JSON array of {\"op\", \"args\"}",
            Text,
            false,
            vec![arg("steps", "Steps (JSON)", "textarea", "[]")]
        )
        .ex_args("eyJ1c2VyIjoiYWRhIiwicm9sZSI6ImFkbWluIn0", json!({ "steps": r#"[{"op":"base64.decode"},{"op":"json.format","args":{"indent":"4"}}]"# })),
    ]
}

pub fn info(id: &str) -> Option<OpInfo> {
    registry().iter().find(|o| o.id == id).cloned()
}

pub(crate) fn arg_str<'a>(args: &'a Value, name: &str) -> &'a str {
    args.get(name).and_then(Value::as_str).unwrap_or("")
}

/// Numeric argument given as a JSON number or numeric string.
pub(crate) fn arg_num(args: &Value, name: &str) -> Option<f64> {
    match args.get(name) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn arg_usize(args: &Value, name: &str, default: usize, min: usize, max: usize) -> usize {
    arg_num(args, name).map_or(default, |n| if n.is_finite() && n >= 0.0 { n as usize } else { default }).clamp(min, max)
}

/// `yes` / `true` / `1` / `on` (or JSON `true`).
pub(crate) fn arg_bool(args: &Value, name: &str) -> bool {
    match args.get(name) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on"),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => false,
    }
}

fn arg_count(args: &Value) -> usize {
    arg_usize(args, "count", 1, 1, 1000)
}

pub(crate) fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

pub(crate) fn parse_json(input: &str, what: &str) -> Result<Value> {
    serde_json::from_str(input).map_err(|e| invalid(format!("{what} is not valid JSON: {e}")))
}

/// Lenient Base64: standard or URL-safe alphabet, padding optional, whitespace ignored.
pub fn decode_base64(input: &str) -> Result<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let normalized = cleaned.replace('-', "+").replace('_', "/");
    let trimmed = normalized.trim_end_matches('=');
    let padded = format!("{trimmed}{}", "=".repeat((4 - trimmed.len() % 4) % 4));
    STANDARD.decode(padded).map_err(|e| invalid(format!("not valid Base64: {e}")))
}

pub(crate) fn bytes_to_text(b: Vec<u8>) -> String {
    match String::from_utf8(b) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Fills missing args with the defaults declared in [`list`], so every caller sees the same values.
fn with_defaults(id: &str, args: &Value) -> Value {
    let mut out = match args {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    if let Some(op) = registry().iter().find(|o| o.id == id) {
        for a in &op.args {
            let missing = match out.get(a.name) {
                None | Some(Value::Null) => true,
                // An empty text box means "use the default" for numbers and selects.
                Some(Value::String(s)) => s.is_empty() && (a.kind == "number" || a.kind.starts_with("select:")),
                _ => false,
            };
            if missing {
                out.insert(a.name.into(), Value::String(a.default.into()));
            }
        }
    }
    Value::Object(out)
}

/// Runs operation `id` on `input` with JSON-object `args`.
pub fn run(id: &str, input: &str, args: &Value) -> Result<String> {
    run_depth(id, input, args, 0)
}

pub(crate) fn run_depth(id: &str, input: &str, args: &Value, depth: usize) -> Result<String> {
    let args = &with_defaults(id, args);
    Ok(match id {
        "base64.encode" => STANDARD.encode(input),
        "base64.decode" => bytes_to_text(decode_base64(input)?),
        "base64url.encode" => URL_SAFE_NO_PAD.encode(input),
        "url.encode" => percent_encoding::utf8_percent_encode(input, percent_encoding::NON_ALPHANUMERIC)
            .to_string()
            .replace("%2D", "-")
            .replace("%2E", ".")
            .replace("%5F", "_")
            .replace("%7E", "~"),
        "url.decode" => percent_encoding::percent_decode_str(&input.replace('+', " ")).decode_utf8_lossy().into_owned(),
        "url.parse" => pretty(&parse_url(input.trim())?),
        "html.encode" => html_escape::encode_quoted_attribute(input).into_owned(),
        "html.decode" => html_escape::decode_html_entities(input).into_owned(),
        "hex.encode" => hex::encode(input),
        "hex.decode" => {
            let cleaned: String = input.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            bytes_to_text(hex::decode(cleaned).map_err(|e| invalid(format!("not valid hex: {e}")))?)
        }
        "gzip.compress" => misc::gzip_compress(input, arg_usize(args, "level", 6, 0, 9) as u32)?,
        "gzip.decompress" => misc::gzip_decompress(input)?,
        "string.escape" => misc::string_escape(input, arg_str(args, "mode"), arg_str(args, "direction") == "unescape")?,
        "number.base" => pretty(&misc::number_base(input, arg_str(args, "from"), arg_usize(args, "to", 36, 2, 36) as u32)?),
        "jwt.decode" => pretty(&jwt::decode(input.trim())?),
        "jwt.verify" => pretty(&jwt::verify(input.trim(), arg_str(args, "key"), arg_str(args, "alg"))?),
        "jwt.sign" => jwt::sign(input, arg_str(args, "key"), arg_str(args, "alg"))?,
        "hash.md5" => hex::encode(md5::Md5::digest(input.as_bytes())),
        "hash.sha1" => hex::encode(sha1::Sha1::digest(input.as_bytes())),
        "hash.sha256" => hex::encode(sha2::Sha256::digest(input.as_bytes())),
        "hash.sha512" => hex::encode(sha2::Sha512::digest(input.as_bytes())),
        "hash.sha3" => misc::sha3(input, arg_str(args, "variant"))?,
        "hash.blake3" => misc::blake3_hex(input, arg_usize(args, "length", 32, 1, 1024)),
        "hmac.sha256" => {
            let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(arg_str(args, "key").as_bytes()).map_err(|e| invalid(e.to_string()))?;
            m.update(input.as_bytes());
            encode_digest(&m.finalize().into_bytes(), arg_str(args, "encoding"))
        }
        "hmac.sha512" => {
            let mut m = hmac::Hmac::<sha2::Sha512>::new_from_slice(arg_str(args, "key").as_bytes()).map_err(|e| invalid(e.to_string()))?;
            m.update(input.as_bytes());
            encode_digest(&m.finalize().into_bytes(), arg_str(args, "encoding"))
        }
        "bcrypt.hash" => misc::bcrypt_hash(input, arg_usize(args, "cost", 10, 0, 99) as u32)?,
        "bcrypt.verify" => pretty(&misc::bcrypt_verify(input, arg_str(args, "hash"))?),
        "uuid.v4" => (0..arg_count(args)).map(|_| uuid::Uuid::new_v4().to_string()).collect::<Vec<_>>().join("\n"),
        "uuid.v7" => (0..arg_count(args)).map(|_| uuid::Uuid::now_v7().to_string()).collect::<Vec<_>>().join("\n"),
        "ulid" => (0..arg_count(args)).map(|_| ulid::Ulid::generate().to_string()).collect::<Vec<_>>().join("\n"),
        "password.generate" => pretty(&misc::password_generate(args)?),
        "lorem" => misc::lorem(arg_str(args, "unit"), arg_usize(args, "count", 3, 1, 500))?,
        "qr.generate" => misc::qr_svg(input, arg_str(args, "ecLevel"), arg_usize(args, "size", 256, 64, 4096) as u32)?,
        "totp.generate" => pretty(&misc::totp(input, args, chrono::Utc::now().timestamp())?),
        "time.convert" => pretty(&time::convert(input.trim(), chrono::Utc::now())?),
        "time.diff" => pretty(&time::diff(input.trim(), arg_str(args, "other"), chrono::Utc::now())?),
        "cron.explain" => pretty(&cron::explain(input, arg_str(args, "tz"), arg_usize(args, "count", 10, 1, 100), chrono::Utc::now())?),
        "json.format" => {
            let v = parse_json(input, "input")?;
            match arg_str(args, "indent") {
                "4" => indent_json(&v, b"    "),
                "tab" => indent_json(&v, b"\t"),
                _ => pretty(&v),
            }
        }
        "json.minify" => serde_json::to_string(&parse_json(input, "input")?).unwrap_or_default(),
        "json.toYaml" => serde_yaml_ng::to_string(&parse_json(input, "input")?).map_err(|e| invalid(e.to_string()))?,
        "yaml.toJson" => {
            let v: Value = serde_yaml_ng::from_str(input).map_err(|e| invalid(format!("not valid YAML: {e}")))?;
            pretty(&v)
        }
        "json.escape" => serde_json::to_string(input).unwrap_or_default(),
        "json.unescape" => {
            let t = input.trim();
            let quoted = if t.starts_with('"') { t.to_string() } else { format!("\"{t}\"") };
            serde_json::from_str::<String>(&quoted).map_err(|e| invalid(format!("not a valid JSON string: {e}")))?
        }
        "json.toTypes" => codegen::generate(input, arg_str(args, "language"), arg_str(args, "root"))?,
        "jq" => query::jq(input, arg_str(args, "filter"), arg_bool(args, "raw"))?,
        "jsonpath" => pretty(&query::jsonpath(input, arg_str(args, "path"), arg_str(args, "output"))?),
        "jmespath" => pretty(&query::jmespath(input, arg_str(args, "expression"))?),
        "regex.test" => pretty(&regexp::test(input, &regexp::Opts::from_args(args))?),
        "regex.replace" => regexp::replace(input, &regexp::Opts::from_args(args), arg_str(args, "replacement"), arg_usize(args, "limit", 0, 0, usize::MAX))?,
        "regex.split" => pretty(&regexp::split(input, &regexp::Opts::from_args(args))?),
        "regex.cases" => pretty(&regexp::cases(input, &regexp::Opts::from_args(args))?),
        "regex.explain" => pretty(&regexp::explain(input, arg_str(args, "flags"), arg_str(args, "flavor"))?),
        "regex.escape" => regex::escape(input),
        "diff.text" => pretty(&diff::text(input, arg_str(args, "other"), arg_str(args, "mode"), arg_usize(args, "context", 3, 0, 1000))?),
        "diff.json" => pretty(&diff::json(input, arg_str(args, "other"), arg_bool(args, "ignoreArrayOrder"))?),
        "text.compare" => pretty(&compare::compare(input, arg_str(args, "other"))),
        "case.camel" => case::camel(input),
        "case.pascal" => case::pascal(input),
        "case.snake" => case::joined(input, "_", false),
        "case.kebab" => case::joined(input, "-", false),
        "case.constant" => case::joined(input, "_", true),
        "case.title" => case::title(input),
        "slug" => misc::slug(input, arg_str(args, "separator"), arg_str(args, "lowercase") != "no"),
        "text.stats" => pretty(&json!({
            "characters": input.chars().count(),
            "bytes": input.len(),
            "words": input.split_whitespace().count(),
            "lines": if input.is_empty() { 0 } else { input.lines().count() },
        })),
        "unicode.inspect" => pretty(&unicode::inspect(input)),
        "cidr.calc" => pretty(&net::cidr(input.trim(), arg_str(args, "contains").trim())?),
        "ip.info" => pretty(&net::ip_info(input.trim())?),
        "dns.lookup" => pretty(&net::dns_lookup(input.trim(), arg_str(args, "type"))?),
        "tls.chain" => pretty(&net::tls_chain(input.trim())?),
        "x509.decode" => pretty(&x509::decode(input, chrono::Utc::now().timestamp())?),
        "http.status" => pretty(&misc::http_status(input.trim())?),
        "useragent.parse" => pretty(&misc::user_agent(input.trim())),
        "color.convert" => pretty(&color::convert(input.trim(), arg_str(args, "against").trim())?),
        "chmod.calc" => pretty(&misc::chmod(input.trim())?),
        "detect.magic" => pretty(&magic::detect(input)),
        "recipe.run" => magic::run_recipe(input, arg_str(args, "steps"), depth)?,
        other => return Err(UtilError::UnknownOp(other.to_string())),
    })
}

fn encode_digest(bytes: &[u8], encoding: &str) -> String {
    if encoding == "base64" { STANDARD.encode(bytes) } else { hex::encode(bytes) }
}

fn indent_json(v: &Value, indent: &[u8]) -> String {
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(indent);
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    v.serialize(&mut ser).ok();
    String::from_utf8(buf).unwrap_or_default()
}

fn parse_url(input: &str) -> Result<Value> {
    let u = url::Url::parse(input).map_err(|e| invalid(format!("not a valid absolute URL: {e}")))?;
    let query: Vec<Value> = u.query_pairs().map(|(k, v)| json!({ "name": k, "value": v })).collect();
    Ok(json!({
        "scheme": u.scheme(),
        "username": (!u.username().is_empty()).then(|| u.username()),
        "password": u.password().map(|_| "••••"),
        "host": u.host_str(),
        "port": u.port_or_known_default(),
        "path": u.path(),
        "pathSegments": u.path_segments().map(|s| s.collect::<Vec<_>>()),
        "query": query,
        "fragment": u.fragment(),
        "origin": u.origin().ascii_serialization(),
    }))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ext;
