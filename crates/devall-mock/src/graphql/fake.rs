//! Deterministic fake values for auto-mocked GraphQL fields, chosen by field name first (email,
//! name, createdAt, price, …) and scalar type second. The same seed and path always give the
//! same value, so a query returns stable data across calls.

use serde_json::{Value, json};

const FIRST: &[&str] = &[
    "Ada", "Alan", "Grace", "Linus", "Margaret", "Dennis", "Barbara", "Ken", "Radia", "Tim", "Katherine", "Guido", "Frances", "Bjarne", "Hedy", "Edsger",
    "Joan", "Donald", "Sophie", "John", "Anita", "Niklaus", "Shafi", "Yukihiro", "Lynn", "Vint",
];
const LAST: &[&str] = &[
    "Lovelace", "Turing", "Hopper", "Torvalds", "Hamilton", "Ritchie", "Liskov", "Thompson", "Perlman", "Berners-Lee", "Johnson", "van Rossum", "Allen",
    "Stroustrup", "Lamarr", "Dijkstra", "Clarke", "Knuth", "Wilson", "McCarthy", "Borg", "Wirth", "Goldwasser", "Matsumoto", "Conway", "Cerf",
];
const CITIES: &[&str] = &["London", "Lisbon", "Berlin", "Toronto", "Kyoto", "Austin", "Nairobi", "Oslo", "Melbourne", "Montréal", "Seoul", "Bogotá", "Dublin", "Zürich"];
const COUNTRIES: &[&str] = &["United Kingdom", "Portugal", "Germany", "Canada", "Japan", "United States", "Kenya", "Norway", "Australia", "South Korea", "Colombia", "Ireland"];
const STREETS: &[&str] = &["Market Street", "Baker Street", "Elm Avenue", "Harbour Road", "King's Way", "Maple Drive", "Station Road", "Rue de Rivoli", "Canal Street"];
const COMPANIES: &[&str] = &["Acme Corp", "Globex", "Initech", "Umbrella Labs", "Hooli", "Stark Industries", "Wayne Enterprises", "Soylent", "Vandelay Imports", "Cyberdyne"];
const WORDS: &[&str] = &[
    "alpha", "bright", "cloud", "delta", "ember", "forest", "garden", "harbor", "island", "jade", "kernel", "lumen", "meadow", "nova", "orbit", "pixel",
    "quartz", "river", "signal", "timber", "unity", "vector", "willow", "xenon", "yonder", "zephyr", "amber", "breeze", "comet", "drift",
];
const DOMAINS: &[&str] = &["example.com", "example.org", "example.net", "mail.test", "corp.example"];
const STATUSES: &[&str] = &["ACTIVE", "PENDING", "INACTIVE"];
const COLORS: &[&str] = &["#1f77b4", "#ff7f0e", "#2ca02c", "#d62728", "#9467bd", "#8c564b", "#e377c2", "#7f7f7f"];
const CURRENCIES: &[&str] = &["USD", "EUR", "GBP", "JPY", "CAD", "AUD", "CHF"];

fn pick<'a>(rng: &mut fastrand::Rng, items: &[&'a str]) -> &'a str {
    items[rng.usize(..items.len())]
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn words(rng: &mut fastrand::Rng, n: usize) -> String {
    (0..n).map(|_| pick(rng, WORDS)).collect::<Vec<_>>().join(" ")
}

fn sentence(rng: &mut fastrand::Rng) -> String {
    let n = rng.usize(5..11);
    format!("{}.", cap(&words(rng, n)))
}

fn uuid(rng: &mut fastrand::Rng) -> String {
    let mut b = [0u8; 16];
    for x in &mut b {
        *x = rng.u8(..);
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// A date-time in 2024–2026 (deterministic, no clock).
fn datetime(rng: &mut fastrand::Rng) -> String {
    format!("{}-{:02}-{:02}T{:02}:{:02}:{:02}Z", rng.u32(2024..=2026), rng.u32(1..=12), rng.u32(1..=28), rng.u32(0..24), rng.u32(0..60), rng.u32(0..60))
}

fn person(rng: &mut fastrand::Rng) -> (String, String) {
    (pick(rng, FIRST).to_string(), pick(rng, LAST).to_string())
}

fn email(rng: &mut fastrand::Rng) -> String {
    let (f, l) = person(rng);
    format!("{}.{}@{}", f.to_ascii_lowercase(), l.to_ascii_lowercase().replace([' ', '-'], ""), pick(rng, DOMAINS))
}

/// Normalised field name: lower case without `_`/`-`.
fn norm(name: &str) -> String {
    name.chars().filter(|c| *c != '_' && *c != '-').collect::<String>().to_ascii_lowercase()
}

fn ends(n: &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|s| n.ends_with(s))
}

/// A value for a field of scalar type `scalar` named `field`.
pub fn scalar(scalar: &str, field: &str, rng: &mut fastrand::Rng) -> Value {
    let n = norm(field);
    match scalar {
        "Int" => json!(int_for(&n, rng)),
        "Float" => json!(float_for(&n, rng)),
        "Boolean" => json!(rng.bool()),
        "ID" => {
            if n == "id" || n.ends_with("id") {
                json!(uuid(rng))
            } else {
                json!(rng.u32(1..100_000).to_string())
            }
        }
        "String" => json!(string_for(&n, rng)),
        other => custom(other, &n, rng),
    }
}

fn int_for(n: &str, rng: &mut fastrand::Rng) -> i64 {
    if ends(n, &["age"]) {
        rng.i64(18..90)
    } else if ends(n, &["year"]) {
        rng.i64(1990..2031)
    } else if ends(n, &["count", "total", "quantity", "qty", "size", "length", "number", "num"]) {
        rng.i64(0..100)
    } else if ends(n, &["price", "amount", "cost", "balance"]) {
        rng.i64(1..1000)
    } else if ends(n, &["rating", "stars"]) {
        rng.i64(1..6)
    } else if ends(n, &["percent", "percentage", "progress"]) {
        rng.i64(0..101)
    } else if ends(n, &["port"]) {
        rng.i64(1024..65536)
    } else {
        rng.i64(0..1000)
    }
}

fn float_for(n: &str, rng: &mut fastrand::Rng) -> f64 {
    let v = if ends(n, &["latitude", "lat"]) {
        rng.f64() * 180.0 - 90.0
    } else if ends(n, &["longitude", "lng", "lon"]) {
        rng.f64() * 360.0 - 180.0
    } else if ends(n, &["rating", "score"]) {
        1.0 + rng.f64() * 4.0
    } else if ends(n, &["percent", "percentage", "ratio", "progress"]) {
        rng.f64() * 100.0
    } else {
        1.0 + rng.f64() * 999.0
    };
    (v * 100.0).round() / 100.0
}

fn string_for(n: &str, rng: &mut fastrand::Rng) -> String {
    if ends(n, &["email", "mail"]) {
        email(rng)
    } else if n == "firstname" || n == "givenname" {
        pick(rng, FIRST).into()
    } else if n == "lastname" || n == "surname" || n == "familyname" {
        pick(rng, LAST).into()
    } else if ends(n, &["username", "login", "handle", "nickname"]) {
        let (f, l) = person(rng);
        format!("{}{}", f.to_ascii_lowercase(), &l.to_ascii_lowercase()[..1])
    } else if n == "name" || ends(n, &["fullname", "displayname", "author", "owner"]) {
        let (f, l) = person(rng);
        format!("{f} {l}")
    } else if ends(n, &["phone", "mobile", "tel", "fax"]) {
        format!("+1 555 {:03} {:04}", rng.u32(0..1000), rng.u32(0..10000))
    } else if ends(n, &["city", "town"]) {
        pick(rng, CITIES).into()
    } else if ends(n, &["country"]) {
        pick(rng, COUNTRIES).into()
    } else if ends(n, &["street", "address", "address1", "line1"]) {
        format!("{} {}", rng.u32(1..400), pick(rng, STREETS))
    } else if ends(n, &["zip", "zipcode", "postcode", "postalcode"]) {
        format!("{:05}", rng.u32(10000..99999))
    } else if ends(n, &["company", "organization", "organisation", "employer"]) {
        pick(rng, COMPANIES).into()
    } else if ends(n, &["avatar", "image", "photo", "picture", "thumbnail", "logo", "icon"]) {
        format!("https://picsum.photos/seed/{}/200", rng.u32(1..10_000))
    } else if ends(n, &["url", "uri", "link", "website", "homepage", "href"]) {
        format!("https://{}/{}", pick(rng, DOMAINS), pick(rng, WORDS))
    } else if ends(n, &["at", "date", "time", "timestamp", "on"]) && !ends(n, &["format", "status"]) {
        datetime(rng)
    } else if ends(n, &["uuid", "guid"]) {
        uuid(rng)
    } else if ends(n, &["slug", "key", "code"]) {
        format!("{}-{}", pick(rng, WORDS), pick(rng, WORDS))
    } else if ends(n, &["status", "state"]) {
        pick(rng, STATUSES).into()
    } else if ends(n, &["color", "colour"]) {
        pick(rng, COLORS).into()
    } else if ends(n, &["currency"]) {
        pick(rng, CURRENCIES).into()
    } else if ends(n, &["ip", "ipaddress"]) {
        format!("10.{}.{}.{}", rng.u8(..), rng.u8(..), rng.u8(1..255))
    } else if ends(n, &["token", "hash", "secret", "sha"]) {
        (0..32).map(|_| format!("{:x}", rng.u8(..16))).collect()
    } else if ends(n, &["title", "headline", "subject", "label"]) {
        cap(&{
            let k = rng.usize(2..5);
            words(rng, k)
        })
    } else if ends(n, &["description", "summary", "bio", "about", "body", "content", "text", "message", "comment", "note", "notes"]) {
        let k = rng.usize(1..3);
        (0..k).map(|_| sentence(rng)).collect::<Vec<_>>().join(" ")
    } else if ends(n, &["version"]) {
        format!("{}.{}.{}", rng.u32(0..5), rng.u32(0..20), rng.u32(0..50))
    } else if ends(n, &["language", "locale", "lang"]) {
        pick(rng, &["en-GB", "en-US", "pt-PT", "de-DE", "ja-JP", "fr-CA"]).into()
    } else {
        let k = rng.usize(1..3);
        words(rng, k)
    }
}

fn custom(scalar: &str, n: &str, rng: &mut fastrand::Rng) -> Value {
    let s = scalar.to_ascii_lowercase();
    if s.contains("datetime") || s.contains("timestamp") || s == "instant" {
        json!(datetime(rng))
    } else if s == "date" || s == "localdate" {
        json!(datetime(rng)[..10])
    } else if s == "time" || s == "localtime" {
        json!(datetime(rng)[11..19])
    } else if s.contains("json") || s == "object" || s == "map" || s == "any" {
        json!({ pick(rng, WORDS): pick(rng, WORDS) })
    } else if s.contains("url") || s.contains("uri") {
        json!(format!("https://{}/{}", pick(rng, DOMAINS), pick(rng, WORDS)))
    } else if s.contains("email") {
        json!(email(rng))
    } else if s.contains("uuid") || s.contains("guid") {
        json!(uuid(rng))
    } else if s.contains("bigint") || s == "long" || s.contains("int") {
        json!(int_for(n, rng))
    } else if s.contains("decimal") || s.contains("money") || s.contains("float") || s.contains("number") {
        json!(float_for(n, rng))
    } else if s == "upload" || s == "file" {
        Value::Null
    } else {
        json!(string_for(n, rng))
    }
}

/// FNV-1a over the seed and a path: one stable random stream per field position.
pub fn rng_for(seed: u64, path: &str) -> fastrand::Rng {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
    for b in path.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    fastrand::Rng::with_seed(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_follow_names_and_are_deterministic() {
        let v = |t: &str, f: &str| scalar(t, f, &mut rng_for(42, f));
        assert!(v("String", "email").as_str().unwrap().contains('@'));
        assert!(v("String", "createdAt").as_str().unwrap().ends_with('Z'));
        assert!(v("String", "avatarUrl").as_str().unwrap().starts_with("https://"));
        assert_eq!(v("ID", "id").as_str().unwrap().len(), 36);
        assert!((18..90).contains(&v("Int", "age").as_i64().unwrap()));
        assert!(v("Boolean", "active").is_boolean());
        assert!(v("DateTime", "x").as_str().unwrap().contains('T'));
        assert!(v("JSON", "meta").is_object());
        assert_eq!(v("String", "first_name"), v("String", "first_name"));
        assert_ne!(scalar("String", "name", &mut rng_for(1, "a.name")), scalar("String", "name", &mut rng_for(1, "b.name")));
    }
}
