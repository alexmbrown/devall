//! "Request → mock → randomize": turn a real JSON response body into a WireMock
//! `response-template` (Handlebars) body with the same structure, where leaf values become
//! random-data helpers.
//!
//! # Helper vocabulary
//!
//! The emitted template only uses these helpers (WireMock's built-ins plus its Faker-backed
//! `random` helper), so any WireMock-compatible engine can render it:
//!
//! | Helper | Emitted for | Renders |
//! |---|---|---|
//! | `{{randomValue type='UUID'}}` | UUIDs | a v4 UUID |
//! | `{{randomValue length=N type='ALPHANUMERIC' [uppercase=true]}}` | opaque ids, tokens, ULIDs, passwords | N letters/digits |
//! | `{{randomValue length=N type='ALPHABETIC' [uppercase=true]}}` | letter runs inside codes (`ORD-…`) | N letters |
//! | `{{randomValue length=N type='NUMERIC'}}` | digit strings (`"02134"`, `"00042"`) | N digits |
//! | `{{randomValue length=N type='HEXADECIMAL'}}` | hex strings, colours | N hex digits |
//! | `{{randomInt lower=A upper=B}}` | integers (unquoted) | an integer in `[A, B]` |
//! | `{{randomDecimal lower=A upper=B}}` | decimals (unquoted) | a decimal in `[A, B]` |
//! | `{{now}}` | ISO date-times | the current time, ISO 8601 UTC |
//! | `{{now format='yyyy-MM-dd'}}` (or `'yyyy-MM-dd HH:mm:ss'`) | dates | current date, Java pattern |
//! | `{{now format='epoch'}}` / `{{now format='unix'}}` | ms / s timestamps (unquoted) | epoch millis / seconds |
//! | `{{pickRandom 'a' 'b' …}}` | statuses, types, currencies | one of the arguments |
//! | `{{pickRandom true false}}` | booleans (unquoted) | `true` or `false` |
//! | `{{random 'Category.key'}}` | names, e-mails, addresses, … — keys in [`FAKER_KEYS`] | a Faker value |
//!
//! Strings stay quoted (`"name": "{{random 'Name.firstName'}}"`), numbers and booleans don't
//! (`"age": {{randomInt lower=18 upper=90}}`), `null` stays `null`, so the body is valid JSON once
//! rendered. Helpers only use single-quoted arguments, which keeps the template itself free of
//! characters that would need JSON escaping. With `keepIds`, `id`/`…Id`/`…_id` values keep their
//! original value so `/users/42` stays user 42 across responses.

use std::collections::HashMap;

use chrono::{Duration, Months, Utc};
use indexmap::IndexMap;
use rand::{RngExt, SeedableRng};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::generate_value;
use crate::error::{DataError, Result};
use crate::fakers::DRng;
use crate::infer::{infer_kind, is_uuid, norm_key, singular};
use crate::model::{FieldKind, TimestampUnit};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateOptions {
    /// Keep `id`, `…Id` and `…_id` values as they are (stable ids across responses).
    #[serde(default)]
    pub keep_ids: bool,
}

/// Faker keys emitted in `{{random '<key>'}}`, with what they produce. A mock engine that
/// implements the `random` helper should support at least these.
pub const FAKER_KEYS: &[(&str, &str)] = &[
    ("Name.firstName", "first name"),
    ("Name.lastName", "last name"),
    ("Name.fullName", "first and last name"),
    ("Name.username", "user name"),
    ("Internet.emailAddress", "e-mail address"),
    ("Internet.url", "URL"),
    ("Internet.domainName", "domain name"),
    ("Internet.ipV4Address", "IPv4 address"),
    ("Internet.ipV6Address", "IPv6 address"),
    ("Internet.userAgent", "browser user agent"),
    ("Address.streetAddress", "street address"),
    ("Address.city", "city"),
    ("Address.state", "state / region"),
    ("Address.zipCode", "postal code"),
    ("Address.country", "country name"),
    ("Address.countryCode", "ISO country code"),
    ("PhoneNumber.phoneNumber", "phone number"),
    ("Company.name", "company name"),
    ("Company.catchPhrase", "company slogan"),
    ("Job.title", "job title"),
    ("Commerce.productName", "product name"),
    ("Lorem.word", "one word"),
    ("Lorem.sentence", "one sentence"),
    ("Lorem.paragraph", "a few sentences"),
];

/// The generator behind a Faker key (used to render samples locally).
pub fn faker_key_kind(key: &str) -> Option<FieldKind> {
    Some(match key {
        "Name.firstName" => FieldKind::FirstName,
        "Name.lastName" => FieldKind::LastName,
        "Name.fullName" | "Name.name" => FieldKind::FullName,
        "Name.username" => FieldKind::Username,
        "Internet.emailAddress" | "Internet.safeEmailAddress" => FieldKind::email(),
        "Internet.url" => FieldKind::Url,
        "Internet.domainName" => FieldKind::Domain,
        "Internet.ipV4Address" => FieldKind::Ipv4,
        "Internet.ipV6Address" => FieldKind::Ipv6,
        "Internet.userAgent" | "Internet.userAgentAny" => FieldKind::UserAgent,
        "Address.streetAddress" => FieldKind::Street,
        "Address.city" | "Address.cityName" => FieldKind::City,
        "Address.state" => FieldKind::State,
        "Address.zipCode" | "Address.postcode" => FieldKind::Zip,
        "Address.country" => FieldKind::Country,
        "Address.countryCode" => FieldKind::CountryCode,
        "PhoneNumber.phoneNumber" | "PhoneNumber.cellPhone" => FieldKind::Phone,
        "Company.name" => FieldKind::Company,
        "Company.catchPhrase" => FieldKind::CatchPhrase,
        "Job.title" => FieldKind::JobTitle,
        "Commerce.productName" => FieldKind::ProductName,
        "Lorem.word" => FieldKind::Word,
        "Lorem.sentence" => FieldKind::Sentence { min_words: None, max_words: None },
        "Lorem.paragraph" => FieldKind::Paragraph { sentences: None },
        _ => return None,
    })
}

/// Parses `json_text` and builds the template (see the module docs).
pub fn to_response_template_str(json_text: &str, opts: &TemplateOptions) -> Result<String> {
    let v: Value = serde_json::from_str(json_text).map_err(|e| DataError::Parse(format!("the body is not JSON: {e}")))?;
    Ok(to_response_template(&v, opts))
}

/// A pretty-printed WireMock response template with `sample`'s structure.
pub fn to_response_template(sample: &Value, opts: &TemplateOptions) -> String {
    let mut leaves: IndexMap<String, Vec<&Value>> = IndexMap::new();
    collect_leaves(sample, String::new(), &mut leaves);
    let mut b = Builder { opts, leaves, kinds: HashMap::new(), out: String::new() };
    b.value(sample, "", "", 0);
    b.out.push('\n');
    b.out
}

fn collect_leaves<'a>(v: &'a Value, path: String, out: &mut IndexMap<String, Vec<&'a Value>>) {
    match v {
        Value::Object(m) => {
            for (k, child) in m {
                collect_leaves(child, format!("{path}.{k}"), out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_leaves(child, format!("{path}[]"), out);
            }
        }
        Value::Null => {}
        leaf => out.entry(path).or_default().push(leaf),
    }
}

struct Builder<'a> {
    opts: &'a TemplateOptions,
    leaves: IndexMap<String, Vec<&'a Value>>,
    kinds: HashMap<String, FieldKind>,
    out: String,
}

fn pad(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Characters that can sit inside a single-quoted Handlebars literal inside a JSON string.
fn safe_literal(s: &str) -> bool {
    !s.is_empty() && s.len() <= 60 && !s.chars().any(|c| matches!(c, '\'' | '"' | '\\' | '{' | '}') || c.is_control())
}

fn id_key(key: &str) -> bool {
    let k = norm_key(key);
    k == "id" || k == "uuid" || k == "guid" || (k.len() > 2 && k.ends_with("id") && !k.ends_with("valid") && !k.ends_with("paid"))
}

fn faker(key: &str) -> String {
    format!("{{{{random '{key}'}}}}")
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{x}") }
}

impl Builder<'_> {
    fn value(&mut self, v: &Value, key: &str, path: &str, indent: usize) {
        match v {
            Value::Object(m) if m.is_empty() => self.out.push_str("{}"),
            Value::Object(m) => {
                self.out.push_str("{\n");
                for (i, (k, child)) in m.iter().enumerate() {
                    pad(&mut self.out, indent + 2);
                    self.out.push_str(&json_str(k));
                    self.out.push_str(": ");
                    self.value(child, k, &format!("{path}.{k}"), indent + 2);
                    if i + 1 < m.len() {
                        self.out.push(',');
                    }
                    self.out.push('\n');
                }
                pad(&mut self.out, indent);
                self.out.push('}');
            }
            Value::Array(items) if items.is_empty() => self.out.push_str("[]"),
            Value::Array(items) => {
                self.out.push_str("[\n");
                let item_key = singular(key);
                let item_path = format!("{path}[]");
                for (i, child) in items.iter().enumerate() {
                    pad(&mut self.out, indent + 2);
                    self.value(child, &item_key, &item_path, indent + 2);
                    if i + 1 < items.len() {
                        self.out.push(',');
                    }
                    self.out.push('\n');
                }
                pad(&mut self.out, indent);
                self.out.push(']');
            }
            Value::Null => self.out.push_str("null"),
            leaf => {
                let text = self.leaf(leaf, key, path);
                self.out.push_str(&text);
            }
        }
    }

    fn kind_at(&mut self, key: &str, path: &str, v: &Value) -> FieldKind {
        if let Some(k) = self.kinds.get(path) {
            return k.clone();
        }
        let same: Vec<&Value> = self.leaves.get(path).map(|vs| vs.iter().copied().filter(|x| std::mem::discriminant(*x) == std::mem::discriminant(v)).collect()).unwrap_or_else(|| vec![v]);
        let kind = infer_kind(key, &same);
        self.kinds.insert(path.to_string(), kind.clone());
        kind
    }

    fn leaf(&mut self, v: &Value, key: &str, path: &str) -> String {
        if self.opts.keep_ids && id_key(key) && !v.as_str().is_some_and(|s| s.contains("{{")) {
            return v.to_string();
        }
        match v {
            Value::Bool(_) => "{{pickRandom true false}}".into(),
            Value::Number(_) => self.number(v, key, path),
            Value::String(s) => format!("\"{}\"", self.string(s, key, path, v)),
            other => other.to_string(),
        }
    }

    fn number(&mut self, v: &Value, key: &str, path: &str) -> String {
        match self.kind_at(key, path, v) {
            FieldKind::Timestamp { unit: Some(TimestampUnit::Ms), .. } => "{{now format='epoch'}}".into(),
            FieldKind::Timestamp { .. } => "{{now format='unix'}}".into(),
            FieldKind::Sequence { start, .. } => {
                let lo = start.unwrap_or(1).max(1);
                format!("{{{{randomInt lower={lo} upper={}}}}}", lo.saturating_add(9999))
            }
            FieldKind::Int { min, max } => format!("{{{{randomInt lower={} upper={}}}}}", min.unwrap_or(0), max.unwrap_or(1000)),
            FieldKind::Latitude => "{{randomDecimal lower=-90 upper=90}}".into(),
            FieldKind::Longitude => "{{randomDecimal lower=-180 upper=180}}".into(),
            FieldKind::Price { min, max, .. } | FieldKind::Float { min, max, .. } => {
                format!("{{{{randomDecimal lower={} upper={}}}}}", fmt_num(min.unwrap_or(0.0)), fmt_num(max.unwrap_or(1000.0)))
            }
            _ => v.to_string(),
        }
    }

    /// The inside of a JSON string: helpers and/or literal text (already JSON-escaped).
    fn string(&mut self, s: &str, key: &str, path: &str, v: &Value) -> String {
        if s.contains("{{") {
            return format!("{{{{randomValue length={} type='ALPHANUMERIC'}}}}", s.chars().count().clamp(1, 64));
        }
        let literal = || {
            let j = json_str(s);
            j[1..j.len() - 1].to_string()
        };
        match self.kind_at(key, path, v) {
            FieldKind::Uuid { .. } => "{{randomValue type='UUID'}}".into(),
            FieldKind::Ulid => "{{randomValue length=26 type='ALPHANUMERIC' uppercase=true}}".into(),
            FieldKind::Email { .. } => faker("Internet.emailAddress"),
            FieldKind::FirstName => faker("Name.firstName"),
            FieldKind::LastName => faker("Name.lastName"),
            FieldKind::FullName => faker("Name.fullName"),
            FieldKind::Username => faker("Name.username"),
            FieldKind::Phone => faker("PhoneNumber.phoneNumber"),
            FieldKind::Url => faker("Internet.url"),
            FieldKind::Domain => faker("Internet.domainName"),
            FieldKind::Ipv4 => faker("Internet.ipV4Address"),
            FieldKind::Ipv6 => faker("Internet.ipV6Address"),
            FieldKind::UserAgent => faker("Internet.userAgent"),
            FieldKind::Password { .. } => format!("{{{{randomValue length={} type='ALPHANUMERIC'}}}}", s.chars().count().clamp(8, 64)),
            FieldKind::Street => faker("Address.streetAddress"),
            FieldKind::City => faker("Address.city"),
            FieldKind::State => faker("Address.state"),
            FieldKind::Zip => faker("Address.zipCode"),
            FieldKind::Country => faker("Address.country"),
            FieldKind::CountryCode => faker("Address.countryCode"),
            FieldKind::Company => faker("Company.name"),
            FieldKind::JobTitle => faker("Job.title"),
            FieldKind::CatchPhrase => faker("Company.catchPhrase"),
            FieldKind::ProductName => faker("Commerce.productName"),
            FieldKind::Currency => "{{pickRandom 'USD' 'EUR' 'GBP' 'JPY' 'CAD' 'AUD'}}".into(),
            FieldKind::Word => faker("Lorem.word"),
            FieldKind::Sentence { .. } | FieldKind::Lorem { .. } => faker("Lorem.sentence"),
            FieldKind::Paragraph { .. } => faker("Lorem.paragraph"),
            FieldKind::Date { .. } => "{{now format='yyyy-MM-dd'}}".into(),
            FieldKind::Datetime { format, .. } => match format.as_deref() {
                Some(f) if f.starts_with("%Y-%m-%d %H:%M") => "{{now format='yyyy-MM-dd HH:mm:ss'}}".into(),
                _ => "{{now}}".into(),
            },
            FieldKind::Enum { values, .. } => {
                let opts: Vec<&str> = values.iter().filter_map(Value::as_str).collect();
                if opts.len() >= 2 && opts.len() == values.len() && opts.iter().all(|o| safe_literal(o)) {
                    let args: Vec<String> = opts.iter().map(|o| format!("'{o}'")).collect();
                    format!("{{{{pickRandom {}}}}}", args.join(" "))
                } else {
                    literal()
                }
            }
            FieldKind::Regex { .. } => token_template(s).unwrap_or_else(literal),
            _ => literal(),
        }
    }
}

/// `ORD-2024-00042` → `{{randomValue length=3 type='ALPHABETIC' uppercase=true}}-{{randomValue length=4 type='NUMERIC'}}-…`.
/// Hex strings become one HEXADECIMAL helper. `None` when the string has nothing to randomize.
fn token_template(s: &str) -> Option<String> {
    if is_uuid(s) {
        return Some("{{randomValue type='UUID'}}".into());
    }
    let body = s.strip_prefix('#').unwrap_or(s);
    let has_digit = body.chars().any(|c| c.is_ascii_digit());
    let has_alpha_hex = body.chars().any(|c| matches!(c, 'a'..='f' | 'A'..='F'));
    if body.len() >= 6 && has_digit && has_alpha_hex && body.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!("{}{{{{randomValue length={} type='HEXADECIMAL'}}}}", if s.starts_with('#') { "#" } else { "" }, body.len()));
    }
    #[derive(PartialEq, Clone, Copy)]
    enum C {
        Upper,
        Lower,
        Digit,
        Other,
    }
    let class = |c: char| match c {
        'A'..='Z' => C::Upper,
        'a'..='z' => C::Lower,
        '0'..='9' => C::Digit,
        _ => C::Other,
    };
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let c = class(chars[i]);
        let mut j = i;
        while j < chars.len() && class(chars[j]) == c {
            j += 1;
        }
        let n = j - i;
        match c {
            C::Upper => out.push_str(&format!("{{{{randomValue length={n} type='ALPHABETIC' uppercase=true}}}}")),
            C::Lower => out.push_str(&format!("{{{{randomValue length={n} type='ALPHABETIC'}}}}")),
            C::Digit => out.push_str(&format!("{{{{randomValue length={n} type='NUMERIC'}}}}")),
            C::Other => {
                let lit: String = chars[i..j].iter().collect();
                if lit.contains(['{', '}']) {
                    return None;
                }
                let esc = json_str(&lit);
                out.push_str(&esc[1..esc.len() - 1]);
            }
        }
        changed |= c != C::Other;
        i = j;
    }
    changed.then_some(out)
}

// ---------- local rendering (samples, tests) ----------

/// Replaces every `{{…}}` (or `{{{…}}}`) in `template` with `f(inner, in_json_string)`, where
/// `in_json_string` says whether the mustache sits inside a JSON string literal of the
/// surrounding document. The replacement is inserted verbatim.
pub fn substitute_helpers(template: &str, mut f: impl FnMut(&str, bool) -> String) -> String {
    let mut out = String::with_capacity(template.len());
    let mut in_string = false;
    let mut rest = template;
    while !rest.is_empty() {
        if rest.starts_with("{{") {
            let triple = rest.starts_with("{{{");
            let (open, close) = if triple { ("{{{", "}}}") } else { ("{{", "}}") };
            if let Some(end) = rest[open.len()..].find(close) {
                let inner = &rest[open.len()..open.len() + end];
                out.push_str(&f(inner.trim(), in_string));
                rest = &rest[open.len() + end + close.len()..];
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or_default();
        let len = c.len_utf8();
        if in_string && c == '\\' {
            let esc_len = rest[1..].chars().next().map(char::len_utf8).unwrap_or(0);
            out.push_str(&rest[..1 + esc_len]);
            rest = &rest[1 + esc_len..];
            continue;
        }
        if c == '"' {
            in_string = !in_string;
        }
        out.push(c);
        rest = &rest[len..];
    }
    out
}

/// Renders a template the way a WireMock-compatible engine would, using this crate's
/// generators (`now` uses the real clock). Values inside JSON strings are JSON-escaped, so the
/// result of a template from [`to_response_template`] is valid JSON (pretty-printed). Unknown
/// helpers are left untouched.
pub fn render_template_sample(template: &str, seed: u64) -> String {
    let mut rng = DRng::seed_from_u64(seed);
    let rendered = substitute_helpers(template, |inner, in_string| match render_helper(inner, &mut rng) {
        Some(v) if in_string => {
            let j = json_str(&v);
            j[1..j.len() - 1].to_string()
        }
        Some(v) => v,
        None => format!("{{{{{inner}}}}}"),
    });
    match serde_json::from_str::<Value>(&rendered) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or(rendered),
        Err(_) => rendered,
    }
}

#[derive(Debug, PartialEq)]
enum Arg {
    Str(String),
    Raw(String),
}

impl Arg {
    fn text(&self) -> &str {
        match self {
            Arg::Str(s) | Arg::Raw(s) => s,
        }
    }
}

/// `name pos 'quoted' key=value key='quoted'` → (name, positional, named).
fn tokenize(inner: &str) -> (String, Vec<Arg>, HashMap<String, Arg>) {
    let mut tokens: Vec<(Option<String>, Arg)> = Vec::new();
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let mut key = None;
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' && chars[i] != '\'' && chars[i] != '"' {
            i += 1;
        }
        if i < chars.len() && chars[i] == '=' {
            key = Some(chars[start..i].iter().collect::<String>());
            i += 1;
        } else {
            i = start;
        }
        let arg = if i < chars.len() && (chars[i] == '\'' || chars[i] == '"') {
            let q = chars[i];
            let s = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != q {
                i += 1;
            }
            let text: String = chars[s..i.min(chars.len())].iter().collect();
            i += 1;
            Arg::Str(text)
        } else {
            let s = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            Arg::Raw(chars[s..i].iter().collect())
        };
        tokens.push((key, arg));
    }
    let mut iter = tokens.into_iter();
    let name = iter.next().map(|(_, a)| a.text().to_string()).unwrap_or_default();
    let mut positional = Vec::new();
    let mut named = HashMap::new();
    for (k, a) in iter {
        match k {
            Some(k) => {
                named.insert(k, a);
            }
            None => positional.push(a),
        }
    }
    (name, positional, named)
}

fn render_helper(inner: &str, rng: &mut DRng) -> Option<String> {
    let (name, positional, named) = tokenize(inner);
    let num = |k: &str, default: f64| named.get(k).and_then(|a| a.text().parse::<f64>().ok()).unwrap_or(default);
    match name.as_str() {
        "randomValue" => {
            let ty = named.get("type").map(|a| a.text().to_ascii_uppercase()).unwrap_or_else(|| "ALPHANUMERIC".into());
            let len = num("length", 36.0).clamp(1.0, 4096.0) as usize;
            let upper = named.get("uppercase").is_some_and(|a| a.text() == "true");
            let alphabet: &[u8] = match ty.as_str() {
                "UUID" => return Some(uuid::Builder::from_random_bytes(rng.random::<u128>().to_le_bytes()).into_uuid().to_string()),
                "ALPHABETIC" => b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
                "NUMERIC" => b"0123456789",
                "HEXADECIMAL" => b"0123456789abcdef",
                "ALPHANUMERIC_AND_SYMBOLS" => b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!#$%&()*+,-./:;<=>?@[]^_|~",
                _ => b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            };
            let s: String = (0..len).map(|_| alphabet[rng.random_range(0..alphabet.len())] as char).collect();
            Some(if upper { s.to_ascii_uppercase() } else { s })
        }
        "randomInt" => {
            let lo = num("lower", i32::MIN as f64) as i64;
            let hi = num("upper", i32::MAX as f64) as i64;
            Some(rng.random_range(lo.min(hi)..=hi.max(lo)).to_string())
        }
        "randomDecimal" => {
            let lo = num("lower", 0.0);
            let hi = num("upper", 1.0);
            let x = if lo < hi { rng.random_range(lo..=hi) } else { lo };
            Some(fmt_num(crate::fakers::round_to(x, 4)))
        }
        "pickRandom" => {
            if positional.is_empty() {
                return Some(String::new());
            }
            Some(positional[rng.random_range(0..positional.len())].text().to_string())
        }
        "random" => {
            let key = positional.first()?.text();
            let kind = faker_key_kind(key)?;
            let v = generate_value(&kind, rng.random::<u64>() >> 11, None).ok()?;
            Some(match v {
                Value::String(s) => s,
                other => other.to_string(),
            })
        }
        "now" => Some(render_now(named.get("format").map(Arg::text), named.get("offset").map(Arg::text))),
        _ => None,
    }
}

fn render_now(format: Option<&str>, offset: Option<&str>) -> String {
    let mut t = Utc::now();
    if let Some(off) = offset {
        let mut parts = off.split_whitespace();
        if let (Some(n), Some(unit)) = (parts.next().and_then(|n| n.parse::<i64>().ok()), parts.next()) {
            let unit = unit.to_ascii_lowercase();
            let unit = unit.trim_end_matches('s');
            t = match unit {
                "second" => t + Duration::seconds(n),
                "minute" => t + Duration::minutes(n),
                "hour" => t + Duration::hours(n),
                "day" => t + Duration::days(n),
                "week" => t + Duration::weeks(n),
                "month" if n >= 0 => t.checked_add_months(Months::new(n as u32)).unwrap_or(t),
                "month" => t.checked_sub_months(Months::new(n.unsigned_abs() as u32)).unwrap_or(t),
                "year" => t + Duration::days(365 * n),
                _ => t,
            };
        }
    }
    match format {
        None => t.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        Some("epoch") => t.timestamp_millis().to_string(),
        Some("unix") => t.timestamp().to_string(),
        Some(java) => t.format(&java_to_strftime(java)).to_string(),
    }
}

/// The common subset of Java date patterns (`yyyy-MM-dd'T'HH:mm:ss.SSSXXX`) as strftime.
fn java_to_strftime(p: &str) -> String {
    let chars: Vec<char> = p.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                if chars[i] == '%' {
                    out.push('%');
                }
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        let piece = match (c, n) {
            ('y', 2) => "%y",
            ('y', _) => "%Y",
            ('M', 1 | 2) => "%m",
            ('M', 3) => "%b",
            ('M', _) => "%B",
            ('d', _) => "%d",
            ('H', _) => "%H",
            ('h', _) => "%I",
            ('m', _) => "%M",
            ('s', _) => "%S",
            ('S', _) => "%3f",
            ('a', _) => "%p",
            ('E', 1..=3) => "%a",
            ('E', _) => "%A",
            ('X', 1) | ('Z', _) => "%z",
            ('X', _) => "%:z",
            ('%', _) => "%%",
            _ => {
                for _ in 0..n {
                    out.push(c);
                }
                i += n;
                continue;
            }
        };
        out.push_str(piece);
        i += n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_patterns() {
        assert_eq!(java_to_strftime("yyyy-MM-dd'T'HH:mm:ss.SSSXXX"), "%Y-%m-%dT%H:%M:%S.%3f%:z");
        assert_eq!(java_to_strftime("dd/MM/yy"), "%d/%m/%y");
    }

    #[test]
    fn tokenizer() {
        let (name, pos, named) = tokenize("randomValue length=5 type='NUMERIC' uppercase=true");
        assert_eq!(name, "randomValue");
        assert!(pos.is_empty());
        assert_eq!(named["type"], Arg::Str("NUMERIC".into()));
        assert_eq!(named["length"], Arg::Raw("5".into()));
        let (_, pos, _) = tokenize("pickRandom 'a b' true 3");
        assert_eq!(pos, vec![Arg::Str("a b".into()), Arg::Raw("true".into()), Arg::Raw("3".into())]);
    }

    #[test]
    fn token_templates() {
        assert_eq!(
            token_template("ORD-42").unwrap(),
            "{{randomValue length=3 type='ALPHABETIC' uppercase=true}}-{{randomValue length=2 type='NUMERIC'}}"
        );
        assert_eq!(token_template("#a1b2c3").unwrap(), "#{{randomValue length=6 type='HEXADECIMAL'}}");
        assert_eq!(token_template("--"), None);
    }
}
