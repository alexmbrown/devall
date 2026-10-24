//! Guess a generator schema from sample JSON (Mockaroo-style inference): key names first
//! (`email`, `firstName`, `zip`, `createdAt`, …), then value shapes (UUIDs, ISO dates, URLs, …),
//! numbers become ranges around the observed values, and arrays of objects are merged.

use std::collections::BTreeSet;

use indexmap::IndexMap;
use serde_json::Value;

use crate::engine::parse_instant;
use crate::model::{Entity, Field, FieldKind, Schema, TimestampUnit};

/// Entities inferred from pasted JSON get this many rows unless the sample is larger.
const DEFAULT_COUNT: u64 = 10;
/// At most this many distinct short strings become an `enum`.
const MAX_ENUM: usize = 12;

/// A one-entity schema (`items`) inferred from `sample`.
pub fn infer_from_json(sample: &Value) -> Schema {
    let mut schema = Schema::new("Inferred");
    schema.entities.push(infer_entity("items", sample));
    schema
}

/// An entity named `name` inferred from `sample`: an array of objects (merged), an object, or
/// anything else (a single `value` field).
pub fn infer_entity(name: &str, sample: &Value) -> Entity {
    match sample {
        Value::Array(items) if !items.is_empty() && items.iter().all(|v| v.is_object() || v.is_null()) => {
            let objects: Vec<&Value> = items.iter().filter(|v| v.is_object()).collect();
            Entity::new(name, (items.len() as u64).max(DEFAULT_COUNT), merge_objects(&objects))
        }
        Value::Object(_) => Entity::new(name, DEFAULT_COUNT, merge_objects(&[sample])),
        Value::Array(items) => {
            let values: Vec<&Value> = items.iter().collect();
            Entity::new(name, (items.len() as u64).max(DEFAULT_COUNT), vec![Field::new("value", infer_kind("value", &values))])
        }
        other => Entity::new(name, DEFAULT_COUNT, vec![Field::new("value", infer_kind("value", &[other]))]),
    }
}

/// Fields of a set of objects, keys in first-seen order; keys missing or `null` in some objects
/// get a matching `nullPercent`.
pub(crate) fn merge_objects(objects: &[&Value]) -> Vec<Field> {
    let mut by_key: IndexMap<&str, Vec<&Value>> = IndexMap::new();
    for o in objects {
        if let Value::Object(m) = o {
            for (k, v) in m {
                by_key.entry(k.as_str()).or_default().push(v);
            }
        }
    }
    let total = objects.len().max(1);
    by_key
        .into_iter()
        .map(|(key, values)| {
            let present = values.iter().filter(|v| !v.is_null()).count();
            let mut field = Field::new(key, infer_kind(key, &values));
            if present > 0 && present < total {
                field.null_percent = ((total - present) as f64 * 100.0 / total as f64).round();
            }
            field
        })
        .collect()
}

/// Lowercase with separators removed: `first_name`, `First-Name`, `firstName` → `firstname`.
pub(crate) fn norm_key(key: &str) -> String {
    key.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Rough singular form for array element hints (`tags` → `tag`, `categories` → `category`).
pub(crate) fn singular(key: &str) -> String {
    if let Some(s) = key.strip_suffix("ies") {
        format!("{s}y")
    } else if key.ends_with("sses") || key.ends_with("xes") {
        key[..key.len() - 2].to_string()
    } else if key.len() > 3 && key.ends_with('s') && !key.ends_with("ss") {
        key[..key.len() - 1].to_string()
    } else {
        key.to_string()
    }
}

pub(crate) fn infer_kind(key: &str, values: &[&Value]) -> FieldKind {
    let non_null: Vec<&Value> = values.iter().copied().filter(|v| !v.is_null()).collect();
    let Some(first) = non_null.first() else {
        return FieldKind::Json { value: Value::Null };
    };
    // Mixed types: go with the first value's type.
    let same: Vec<&Value> = non_null.iter().copied().filter(|v| std::mem::discriminant(*v) == std::mem::discriminant(*first)).collect();
    match first {
        Value::Object(_) => FieldKind::Object { fields: merge_objects(&same) },
        Value::Array(_) => {
            let lens: Vec<usize> = same.iter().filter_map(|v| v.as_array().map(Vec::len)).collect();
            let items: Vec<&Value> = same.iter().filter_map(|v| v.as_array()).flatten().collect();
            let min_len = lens.iter().copied().min().unwrap_or(0);
            let max_len = lens.iter().copied().max().unwrap_or(0);
            let of = if items.is_empty() { FieldKind::Word } else { infer_kind(&singular(key), &items) };
            FieldKind::Array { of: Box::new(of), min: Some(if min_len == 0 { 0 } else { 1 }), max: Some(max_len.max(3) as u32) }
        }
        Value::Bool(_) => FieldKind::Bool { true_percent: None },
        Value::Number(_) => {
            let nums: Vec<f64> = same.iter().filter_map(|v| v.as_f64()).collect();
            let all_int = same.iter().all(|v| v.is_i64() || v.is_u64());
            number_kind(key, &nums, all_int, max_decimals(&same))
        }
        Value::String(_) => {
            let strs: Vec<&str> = same.iter().filter_map(|v| v.as_str()).collect();
            string_kind(key, &strs)
        }
        Value::Null => FieldKind::Json { value: Value::Null },
    }
}

fn max_decimals(values: &[&Value]) -> u32 {
    values
        .iter()
        .filter_map(|v| v.as_number())
        .map(|n| n.to_string())
        .map(|s| if s.contains(['e', 'E']) { 2 } else { s.split_once('.').map(|(_, d)| d.len() as u32).unwrap_or(0) })
        .max()
        .unwrap_or(2)
        .min(6)
}

fn is_id_key(k: &str) -> bool {
    k == "id" || k == "uuid" || k == "guid" || k == "key" || k == "pk"
}

fn is_fk_key(raw: &str) -> bool {
    let k = norm_key(raw);
    k.len() > 2 && k.ends_with("id") && !is_id_key(&k) && !k.ends_with("valid") && !k.ends_with("paid")
}

const TIME_KEYS: &[&str] = &["timestamp", "time", "ts", "epoch", "created", "updated", "modified", "deleted", "expires", "expiry"];

fn looks_time_key(k: &str) -> bool {
    TIME_KEYS.contains(&k)
        || k.ends_with("at")
        || k.ends_with("time")
        || k.ends_with("timestamp")
        || k.ends_with("date")
        || k.starts_with("created")
        || k.starts_with("updated")
}

fn number_kind(raw_key: &str, nums: &[f64], all_int: bool, decimals: u32) -> FieldKind {
    let k = norm_key(raw_key);
    let lo = nums.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if k == "lat" || k == "latitude" {
        return FieldKind::Latitude;
    }
    if k == "lng" || k == "lon" || k == "long" || k == "longitude" {
        return FieldKind::Longitude;
    }
    if all_int && (looks_time_key(&k) || k.ends_with("epoch")) {
        if (1.0e9..1.0e10).contains(&lo) && (1.0e9..1.0e10).contains(&hi) {
            return FieldKind::Timestamp { from: None, to: None, unit: None };
        }
        if (1.0e12..1.0e13).contains(&lo) && (1.0e12..1.0e13).contains(&hi) {
            return FieldKind::Timestamp { from: None, to: None, unit: Some(TimestampUnit::Ms) };
        }
    }
    if all_int && is_id_key(&k) && lo >= 0.0 {
        let start = lo as i64;
        return FieldKind::Sequence { start: (start != 1).then_some(start), step: None };
    }
    const MONEY: &[&str] = &["price", "amount", "cost", "total", "subtotal", "balance", "fee", "salary", "revenue", "tax", "discount", "unitprice"];
    if MONEY.iter().any(|m| k == *m || k.ends_with(m)) && lo >= 0.0 && !(all_int && k.ends_with("total") && hi < 1000.0 && decimals == 0) {
        let (min, max) = float_range(lo, hi);
        return FieldKind::Price { min: Some(min.max(0.0)), max: Some(max), decimals: Some(decimals.max(2)) };
    }
    if all_int {
        if k == "age" {
            return FieldKind::Int { min: Some((lo as i64).min(18)), max: Some((hi as i64).max(90)) };
        }
        if k == "year" {
            return FieldKind::Int { min: Some((lo as i64).min(1990)), max: Some((hi as i64).max(2030)) };
        }
        let (min, max) = int_range(lo as i64, hi as i64);
        return FieldKind::Int { min: Some(min), max: Some(max) };
    }
    let (min, max) = float_range(lo, hi);
    FieldKind::Float { min: Some(min), max: Some(max), decimals: Some(decimals.max(1)) }
}

/// A range around observed integers: the observed span, or a band around a single value.
pub(crate) fn int_range(lo: i64, hi: i64) -> (i64, i64) {
    if lo < hi {
        return (lo, hi);
    }
    let v = lo;
    match v {
        0 => (0, 100),
        v if v > 0 => (v / 2, v.saturating_mul(2).max(10)),
        v => (v.saturating_mul(2), v / 2),
    }
}

pub(crate) fn float_range(lo: f64, hi: f64) -> (f64, f64) {
    if lo < hi {
        return (lo, hi);
    }
    if lo == 0.0 {
        (0.0, 100.0)
    } else if lo > 0.0 {
        (lo / 2.0, lo * 2.0)
    } else {
        (lo * 2.0, lo / 2.0)
    }
}

// ---------- strings ----------

pub(crate) fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

pub(crate) fn is_ulid(s: &str) -> bool {
    s.len() == 26 && s.bytes().all(|c| c.is_ascii_digit() || (c.is_ascii_uppercase() && !b"ILOU".contains(&c)))
}

pub(crate) fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    !local.is_empty() && !s.contains(char::is_whitespace) && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !domain.contains('@')
}

pub(crate) fn is_date(s: &str) -> bool {
    s.len() == 10 && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

pub(crate) fn is_datetime(s: &str) -> bool {
    s.len() >= 16 && s.as_bytes().get(10).is_some_and(|c| *c == b'T' || *c == b' ') && is_date(&s[..10]) && parse_instant(s, false).is_some()
}

pub(crate) fn is_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://")) && s.len() > 10 && !s.contains(char::is_whitespace)
}

pub(crate) fn is_ipv4(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>().is_ok()
}

pub(crate) fn is_ipv6(s: &str) -> bool {
    s.contains(':') && s.parse::<std::net::Ipv6Addr>().is_ok()
}

fn is_phone(s: &str) -> bool {
    let digits = s.chars().filter(char::is_ascii_digit).count();
    (7..=15).contains(&digits) && s.chars().all(|c| c.is_ascii_digit() || " +-().x".contains(c)) && s.chars().any(|c| " +-().".contains(c))
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

fn capitalized_words(s: &str) -> bool {
    s.split_whitespace().all(|w| w.chars().next().is_some_and(char::is_uppercase) && w.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\''))
}

/// Kind for a key alone (used for JSON Schema strings without a format, too).
pub(crate) fn string_kind_for_key(raw_key: &str) -> Option<FieldKind> {
    let k = norm_key(raw_key);
    let any = |list: &[&str]| list.contains(&k.as_str());
    let ends = |list: &[&str]| list.iter().any(|s| k.ends_with(s));
    Some(if any(&["email", "mail", "emailaddress", "useremail", "contactemail"]) || k.ends_with("email") {
        FieldKind::email()
    } else if any(&["firstname", "givenname", "fname", "forename"]) {
        FieldKind::FirstName
    } else if any(&["lastname", "surname", "familyname", "lname"]) {
        FieldKind::LastName
    } else if any(&["fullname", "displayname", "contactname", "authorname", "customername", "personname"]) {
        FieldKind::FullName
    } else if any(&["username", "login", "handle", "nickname", "nick", "screenname"]) {
        FieldKind::Username
    } else if any(&["phone", "mobile", "tel", "telephone", "cell", "fax", "phonenumber", "mobilenumber"]) || ends(&["phone", "phonenumber"]) {
        FieldKind::Phone
    } else if any(&["url", "website", "homepage", "link", "href", "uri", "site", "webpage"])
        || ends(&["url", "avatar", "image", "photo", "picture", "thumbnail", "logo", "icon"])
        || any(&["avatar", "image", "img", "photo", "picture", "thumbnail", "logo", "icon"])
    {
        FieldKind::Url
    } else if any(&["domain", "hostname", "host", "domainname"]) {
        FieldKind::Domain
    } else if any(&["ip", "ipaddress", "ipv4", "clientip", "remoteip", "remoteaddr"]) {
        FieldKind::Ipv4
    } else if any(&["ipv6"]) {
        FieldKind::Ipv6
    } else if any(&["useragent", "ua"]) {
        FieldKind::UserAgent
    } else if any(&["password", "passwd", "pwd", "pass"]) {
        FieldKind::Password { min_length: None, max_length: None }
    } else if any(&["street", "address", "address1", "addressline", "addressline1", "streetaddress", "line1", "street1"]) {
        FieldKind::Street
    } else if any(&["city", "town", "locality"]) {
        FieldKind::City
    } else if any(&["state", "province", "region", "county", "statename"]) {
        FieldKind::State
    } else if any(&["zip", "zipcode", "postcode", "postalcode", "postal"]) {
        FieldKind::Zip
    } else if any(&["country", "countryname", "nation"]) {
        FieldKind::Country
    } else if any(&["countrycode", "countryiso", "iso2", "cc"]) {
        FieldKind::CountryCode
    } else if any(&["company", "companyname", "organization", "organisation", "org", "employer", "business", "vendor", "brand"]) {
        FieldKind::Company
    } else if any(&["jobtitle", "job", "position", "occupation", "profession"]) {
        FieldKind::JobTitle
    } else if any(&["catchphrase", "slogan", "tagline", "motto"]) {
        FieldKind::CatchPhrase
    } else if any(&["product", "productname", "itemname"]) {
        FieldKind::ProductName
    } else if any(&["currency", "currencycode"]) {
        FieldKind::Currency
    } else if any(&["description", "bio", "about", "summary", "body", "content", "text", "comment", "comments", "message", "note", "notes", "details", "review"]) {
        FieldKind::Paragraph { sentences: Some(2) }
    } else if any(&["title", "subject", "headline", "heading", "caption"]) {
        FieldKind::Sentence { min_words: Some(3), max_words: Some(8) }
    } else {
        return None;
    })
}

fn string_kind(raw_key: &str, strs: &[&str]) -> FieldKind {
    let k = norm_key(raw_key);
    let all = |p: fn(&str) -> bool| !strs.is_empty() && strs.iter().all(|s| p(s));

    // Unambiguous value shapes win over key names.
    if all(is_uuid) {
        let v7 = strs.iter().all(|s| s.as_bytes()[14] == b'7');
        return FieldKind::Uuid { version: v7.then_some(7) };
    }
    if all(is_ulid) {
        return FieldKind::Ulid;
    }
    if all(is_email) {
        return FieldKind::email();
    }
    if all(is_datetime) {
        let format = datetime_format(strs[0]);
        return FieldKind::Datetime { from: None, to: None, format };
    }
    if all(is_date) {
        return FieldKind::Date { from: None, to: None, format: None };
    }

    // Small closed sets on status-like keys (or clearly repeated values) become enums.
    let distinct: BTreeSet<&str> = strs.iter().copied().collect();
    const ENUM_KEYS: &[&str] = &["status", "type", "kind", "category", "gender", "level", "tier", "plan", "role", "priority", "stage", "visibility", "method", "mode", "state", "sex", "size", "env", "environment", "channel", "source"];
    let enum_key = ENUM_KEYS.contains(&k.as_str()) || k.ends_with("status") || k.ends_with("type");
    // `state: "California"` is an address, `state: "active"` is a status.
    let address_state = k == "state" && strs.iter().all(|s| capitalized_words(s));
    let short = strs.iter().all(|s| s.chars().count() <= 40 && word_count(s) <= 3);
    let repeated = strs.len() >= 4 && distinct.len() * 2 <= strs.len() && string_kind_for_key(raw_key).is_none();
    if short && distinct.len() <= MAX_ENUM && (enum_key || repeated) && !address_state {
        return FieldKind::Enum { values: distinct.into_iter().map(|s| Value::String(s.to_string())).collect(), weights: None };
    }

    if is_fk_key(raw_key) || is_id_key(&k) {
        return id_like(strs);
    }
    if k == "name" || (k.ends_with("name") && !k.ends_with("username")) {
        let words = strs.iter().map(|s| word_count(s)).max().unwrap_or(1);
        if string_kind_for_key(raw_key).is_none() || k == "name" {
            return if strs.iter().all(|s| capitalized_words(s)) {
                match words {
                    1 => FieldKind::FirstName,
                    2 => FieldKind::FullName,
                    _ => FieldKind::ProductName,
                }
            } else {
                FieldKind::Lorem { words: Some(words as u32) }
            };
        }
    }
    if let Some(kind) = string_kind_for_key(raw_key) {
        // Key says "country" but the values are ISO codes.
        if matches!(kind, FieldKind::Country) && all(|s| s.len() == 2 && s.chars().all(|c| c.is_ascii_uppercase())) {
            return FieldKind::CountryCode;
        }
        if matches!(kind, FieldKind::Paragraph { .. }) {
            let words = strs.iter().map(|s| word_count(s)).max().unwrap_or(0);
            return if words <= 1 {
                FieldKind::Word
            } else if words <= 16 {
                FieldKind::Sentence { min_words: Some((words as u32 / 2).max(2)), max_words: Some(words as u32) }
            } else {
                FieldKind::Paragraph { sentences: Some((words as u32 / 12).clamp(1, 10)) }
            };
        }
        return kind;
    }
    if all(is_url) {
        return FieldKind::Url;
    }
    if all(is_ipv4) {
        return FieldKind::Ipv4;
    }
    if all(is_ipv6) {
        return FieldKind::Ipv6;
    }
    if all(|s| s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())) {
        return FieldKind::Regex { pattern: "#[0-9a-f]{6}".into() };
    }
    if all(is_phone) {
        return FieldKind::Phone;
    }
    if all(|s| !s.is_empty() && s.len() <= 20 && s.chars().all(|c| c.is_ascii_digit())) {
        return FieldKind::Regex { pattern: digits_pattern(strs) };
    }
    if all(|s| !s.is_empty() && !s.contains(char::is_whitespace) && s.chars().any(|c| c.is_ascii_digit()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        return FieldKind::Regex { pattern: token_pattern(strs[0]) };
    }
    let words = strs.iter().map(|s| word_count(s)).max().unwrap_or(1);
    match words {
        0 | 1 => FieldKind::Word,
        2..=16 => FieldKind::Sentence { min_words: Some((words as u32 / 2).max(2)), max_words: Some(words as u32) },
        _ => FieldKind::Paragraph { sentences: Some((words as u32 / 12).clamp(1, 10)) },
    }
}

fn id_like(strs: &[&str]) -> FieldKind {
    if strs.iter().all(|s| is_uuid(s)) {
        return FieldKind::uuid();
    }
    if strs.iter().all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())) {
        return FieldKind::Regex { pattern: digits_pattern(strs) };
    }
    FieldKind::Regex { pattern: token_pattern(strs.first().copied().unwrap_or("x")) }
}

fn digits_pattern(strs: &[&str]) -> String {
    let lo = strs.iter().map(|s| s.len()).min().unwrap_or(1);
    let hi = strs.iter().map(|s| s.len()).max().unwrap_or(1);
    if lo == hi { format!("[0-9]{{{lo}}}") } else { format!("[0-9]{{{lo},{hi}}}") }
}

/// A pattern with the same shape as `s`: letters stay letters (same case), digits stay digits,
/// separators stay put. `ORD-2024-00042` → `[A-Z]{3}-[0-9]{4}-[0-9]{5}`.
pub(crate) fn token_pattern(s: &str) -> String {
    #[derive(PartialEq, Clone, Copy)]
    enum C {
        Upper,
        Lower,
        Digit,
        Other(char),
    }
    let class = |c: char| {
        if c.is_ascii_uppercase() {
            C::Upper
        } else if c.is_ascii_lowercase() {
            C::Lower
        } else if c.is_ascii_digit() {
            C::Digit
        } else {
            C::Other(c)
        }
    };
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = class(chars[i]);
        let mut j = i;
        while j < chars.len() && class(chars[j]) == c {
            j += 1;
        }
        let n = j - i;
        match c {
            C::Upper => out.push_str("[A-Z]"),
            C::Lower => out.push_str("[a-z]"),
            C::Digit => out.push_str("[0-9]"),
            C::Other(ch) => {
                for _ in 0..n {
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ' ' {
                        out.push(ch);
                    } else {
                        out.push('\\');
                        out.push(ch);
                    }
                }
                i = j;
                continue;
            }
        }
        if n > 1 {
            out.push_str(&format!("{{{n}}}"));
        }
        i = j;
    }
    if out.is_empty() { "[a-z0-9]{8}".into() } else { out }
}

/// strftime matching an observed ISO datetime, or `None` for the default `…Z` form.
fn datetime_format(s: &str) -> Option<String> {
    let sep = if s.as_bytes()[10] == b' ' { " " } else { "T" };
    let frac = s.find('.').map(|i| s[i + 1..].chars().take_while(char::is_ascii_digit).count()).unwrap_or(0);
    let tz = if s.ends_with('Z') {
        "Z"
    } else if s.len() > 19 && (s[19..].contains('+') || s[19..].rfind('-').is_some()) {
        "%:z"
    } else {
        ""
    };
    let has_secs = s.len() >= 19 && s.as_bytes()[16] == b':';
    let mut f = format!("%Y-%m-%d{sep}%H:%M");
    if has_secs {
        f.push_str(":%S");
    }
    match frac {
        0 => {}
        3 => f.push_str("%.3f"),
        6 => f.push_str("%.6f"),
        9 => f.push_str("%.9f"),
        _ => f.push_str("%.f"),
    }
    f.push_str(if tz == "%:z" { "+00:00" } else { tz });
    if f == "%Y-%m-%dT%H:%M:%SZ" { None } else { Some(f) }
}
