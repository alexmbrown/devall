//! Crate-level tests: determinism, every field type, uniqueness, references, inference, JSON
//! Schema import, every output format, the WireMock bridge, files and a performance smoke test.

use std::collections::HashSet;
use std::time::Instant;

use serde_json::{Value, json};

use crate::engine::{GenerateOptions, generate_value, generate_values, validate};
use crate::error::DataError;
use crate::model::*;
use crate::output::{create_table_sql, export_to_path, generate_string, sql_ident, sql_string, write_output};
use crate::template::{FAKER_KEYS, TemplateOptions, faker_key_kind, render_template_sample, substitute_helpers, to_response_template};
use crate::{from_json_schema, from_json_schema_text, infer_from_json, store};

fn f(name: &str, kind: FieldKind) -> Field {
    Field::new(name, kind)
}

fn schema(seed: u64, entities: Vec<Entity>) -> Schema {
    let mut s = Schema::new("test");
    s.seed = Some(seed);
    s.entities = entities;
    s
}

fn rows(s: &Schema, entity: &str) -> Vec<Value> {
    generate_values(s, &GenerateOptions::default()).unwrap().swap_remove(entity).unwrap()
}

fn people(count: u64) -> Entity {
    Entity::new(
        "users",
        count,
        vec![
            f("id", FieldKind::sequence()),
            f("firstName", FieldKind::FirstName),
            f("lastName", FieldKind::LastName),
            f("email", FieldKind::email()),
            f("age", FieldKind::int(18, 90)),
        ],
    )
}

fn json_out(s: &Schema) -> String {
    generate_string(s, &GenerateOptions::default(), &OutputFormat::json(), None).unwrap().text
}

// ---------- determinism ----------

#[test]
fn same_seed_same_data_different_seed_different_data() {
    let a = schema(1, vec![people(50)]);
    assert_eq!(json_out(&a), json_out(&a));
    let b = schema(2, vec![people(50)]);
    assert_ne!(json_out(&a), json_out(&b));
}

#[test]
fn unseeded_runs_report_their_seed_and_reproduce() {
    let mut s = schema(0, vec![people(5)]);
    s.seed = None;
    let r1 = generate_string(&s, &GenerateOptions::default(), &OutputFormat::json(), None).unwrap();
    let r2 = generate_string(&s, &GenerateOptions { seed: Some(r1.stats.seed), ..Default::default() }, &OutputFormat::json(), None).unwrap();
    assert_eq!(r1.text, r2.text);
    assert!(r1.stats.seed < (1u64 << 53), "seeds must survive a JS number round trip");
}

#[test]
fn fields_have_independent_streams() {
    let base = schema(9, vec![people(30)]);
    let mut more = base.clone();
    more.entities[0].fields.insert(1, f("bio", FieldKind::Paragraph { sentences: None }));
    let a = rows(&base, "users");
    let b = rows(&more, "users");
    for (x, y) in a.iter().zip(&b) {
        for key in ["id", "firstName", "lastName", "email", "age"] {
            assert_eq!(x[key], y[key], "adding a field changed {key}");
        }
    }
    // The first rows don't depend on the count.
    let mut bigger = base.clone();
    bigger.entities[0].count = 60;
    assert_eq!(rows(&bigger, "users")[..30], a[..]);
}

#[test]
fn preview_limit_matches_the_export_prefix() {
    let s = schema(
        3,
        vec![
            people(200),
            Entity::new("orders", 100, vec![f("id", FieldKind::uuid()), f("userId", FieldKind::reference("users", "id"))]),
        ],
    );
    let full = generate_values(&s, &GenerateOptions::default()).unwrap();
    let preview = generate_values(&s, &GenerateOptions { limit: Some(5), ..Default::default() }).unwrap();
    assert_eq!(preview["users"].len(), 5);
    assert_eq!(preview["orders"][..], full["orders"][..5]);
    assert_eq!(preview["users"][..], full["users"][..5]);
    // Only orders: users are generated (for the keys) but not emitted.
    let only = generate_values(&s, &GenerateOptions { entity: Some("orders".into()), limit: Some(5), ..Default::default() }).unwrap();
    assert_eq!(only.keys().collect::<Vec<_>>(), vec!["orders"]);
    assert_eq!(only["orders"][..], full["orders"][..5]);
}

#[test]
fn count_override_applies_to_emitted_entities() {
    let s = schema(3, vec![people(10), Entity::new("orders", 10, vec![f("userId", FieldKind::reference("users", "id"))])]);
    let out = generate_values(&s, &GenerateOptions { entity: Some("orders".into()), count_override: Some(3), ..Default::default() }).unwrap();
    assert_eq!(out["orders"].len(), 3);
    let all = generate_values(&s, &GenerateOptions { count_override: Some(2), ..Default::default() }).unwrap();
    assert_eq!(all["users"].len(), 2);
    assert_eq!(all["orders"].len(), 2);
}

// ---------- every field type ----------

fn all_types_entity() -> Entity {
    Entity::new(
        "all",
        60,
        vec![
            f("uuid", FieldKind::uuid()),
            f("uuid7", FieldKind::Uuid { version: Some(7) }),
            f("ulid", FieldKind::Ulid),
            f("seq", FieldKind::Sequence { start: Some(100), step: Some(5) }),
            f("firstName", FieldKind::FirstName),
            f("lastName", FieldKind::LastName),
            f("fullName", FieldKind::FullName),
            f("username", FieldKind::Username),
            f("email", FieldKind::Email { domain: Some("acme.test".into()) }),
            f("phone", FieldKind::Phone),
            f("url", FieldKind::Url),
            f("domain", FieldKind::Domain),
            f("ipv4", FieldKind::Ipv4),
            f("ipv6", FieldKind::Ipv6),
            f("userAgent", FieldKind::UserAgent),
            f("password", FieldKind::Password { min_length: Some(12), max_length: Some(14) }),
            f("street", FieldKind::Street),
            f("city", FieldKind::City),
            f("state", FieldKind::State),
            f("zip", FieldKind::Zip),
            f("country", FieldKind::Country),
            f("countryCode", FieldKind::CountryCode),
            f("latitude", FieldKind::Latitude),
            f("longitude", FieldKind::Longitude),
            f("company", FieldKind::Company),
            f("jobTitle", FieldKind::JobTitle),
            f("catchPhrase", FieldKind::CatchPhrase),
            f("productName", FieldKind::ProductName),
            f("price", FieldKind::Price { min: Some(10.0), max: Some(20.0), decimals: Some(2) }),
            f("currency", FieldKind::Currency),
            f("word", FieldKind::Word),
            f("sentence", FieldKind::Sentence { min_words: Some(3), max_words: Some(5) }),
            f("paragraph", FieldKind::Paragraph { sentences: Some(2) }),
            f("lorem", FieldKind::Lorem { words: Some(4) }),
            f("int", FieldKind::int(-5, 5)),
            f("float", FieldKind::Float { min: Some(1.0), max: Some(2.0), decimals: Some(3) }),
            f("bool", FieldKind::Bool { true_percent: Some(100.0) }),
            f("date", FieldKind::Date { from: Some("2021-03-01".into()), to: Some("2021-03-31".into()), format: None }),
            f("dateCustom", FieldKind::Date { from: None, to: None, format: Some("%d/%m/%Y".into()) }),
            f("datetime", FieldKind::Datetime { from: Some("2022-01-01T00:00:00Z".into()), to: Some("2022-12-31".into()), format: None }),
            f("timestamp", FieldKind::Timestamp { from: Some("2020-01-01".into()), to: Some("2020-01-02".into()), unit: None }),
            f("timestampMs", FieldKind::Timestamp { from: None, to: None, unit: Some(TimestampUnit::Ms) }),
            f("enum", FieldKind::Enum { values: vec![json!("a"), json!(2), json!(true)], weights: Some(vec![1.0, 0.0, 1.0]) }),
            f("regex", FieldKind::Regex { pattern: r"^[A-Z]{3}-\d{4}$".into() }),
            f("template", FieldKind::template("{{firstName|upper}}/{{seq}}/{{address.city|lower}}")),
            f("address", FieldKind::Object { fields: vec![f("city", FieldKind::City), f("owner", FieldKind::template("{{lastName}}"))] }),
            f("tags", FieldKind::Array { of: Box::new(FieldKind::Word), min: Some(2), max: Some(4) }),
            f("lines", FieldKind::Array { of: Box::new(FieldKind::Object { fields: vec![f("sku", FieldKind::Regex { pattern: "[A-Z]{2}[0-9]{3}".into() }), f("qty", FieldKind::int(1, 3))] }), min: Some(0), max: Some(2) }),
            f("const", FieldKind::Json { value: json!({"v": 1}) }),
        ],
    )
}

fn s_of(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("expected a string, got {v}"))
}

#[test]
fn every_field_type_generates_plausible_values() {
    let s = schema(11, vec![all_types_entity()]);
    validate(&s).unwrap();
    let rows = rows(&s, "all");
    assert_eq!(rows.len(), 60);
    let mut words_seen = HashSet::new();
    for (i, r) in rows.iter().enumerate() {
        let u = uuid::Uuid::parse_str(s_of(&r["uuid"])).unwrap();
        assert_eq!(u.get_version_num(), 4);
        assert_eq!(uuid::Uuid::parse_str(s_of(&r["uuid7"])).unwrap().get_version_num(), 7);
        assert!(ulid::Ulid::from_string(s_of(&r["ulid"])).is_ok());
        assert_eq!(r["seq"], json!(100 + 5 * i as i64));
        let (first, last) = (s_of(&r["firstName"]), s_of(&r["lastName"]));
        assert!(!first.is_empty() && !last.is_empty());
        assert_eq!(s_of(&r["fullName"]), format!("{first} {last}"), "fullName derives from the row");
        assert!(!s_of(&r["username"]).is_empty());
        let email = s_of(&r["email"]);
        assert!(email.ends_with("@acme.test"), "{email}");
        let local = email.split('@').next().unwrap();
        let (fs, ls) = (crate::fakers::ascii_slug(first), crate::fakers::ascii_slug(last));
        assert!(local.contains(&ls) || local.contains(&fs), "{email} not derived from {first} {last}");
        assert!(s_of(&r["phone"]).chars().any(|c| c.is_ascii_digit()));
        assert!(s_of(&r["url"]).starts_with("https://"));
        assert!(s_of(&r["domain"]).contains('.'));
        assert!(s_of(&r["ipv4"]).parse::<std::net::Ipv4Addr>().is_ok());
        assert!(s_of(&r["ipv6"]).parse::<std::net::Ipv6Addr>().is_ok());
        assert!(s_of(&r["userAgent"]).len() > 10);
        assert!((12..=14).contains(&s_of(&r["password"]).chars().count()));
        for k in ["street", "city", "state", "zip", "country", "company", "jobTitle", "catchPhrase", "productName"] {
            assert!(!s_of(&r[k]).is_empty(), "{k}");
        }
        assert_eq!(s_of(&r["countryCode"]).len(), 2);
        assert!((-90.0..=90.0).contains(&r["latitude"].as_f64().unwrap()));
        assert!((-180.0..=180.0).contains(&r["longitude"].as_f64().unwrap()));
        let price = r["price"].as_f64().unwrap();
        assert!((10.0..=20.0).contains(&price));
        assert_eq!((price * 100.0).round() / 100.0, price);
        assert_eq!(s_of(&r["currency"]).len(), 3);
        words_seen.insert(s_of(&r["word"]).to_string());
        let sentence = s_of(&r["sentence"]);
        assert!(sentence.ends_with('.') && sentence.chars().next().unwrap().is_uppercase());
        assert!((3..=5).contains(&sentence.split_whitespace().count()));
        assert_eq!(s_of(&r["paragraph"]).matches('.').count(), 2);
        assert_eq!(s_of(&r["lorem"]).split_whitespace().count(), 4);
        assert!((-5..=5).contains(&r["int"].as_i64().unwrap()));
        let fl = r["float"].as_f64().unwrap();
        assert!((1.0..=2.0).contains(&fl));
        assert_eq!(r["bool"], json!(true));
        let date = s_of(&r["date"]);
        assert!(date.starts_with("2021-03-"), "{date}");
        assert!(chrono::NaiveDate::parse_from_str(s_of(&r["dateCustom"]), "%d/%m/%Y").is_ok());
        let dt = chrono::DateTime::parse_from_rfc3339(s_of(&r["datetime"])).unwrap();
        assert_eq!(dt.format("%Y").to_string(), "2022");
        let ts = r["timestamp"].as_i64().unwrap();
        assert!((1_577_836_800..=1_578_009_599).contains(&ts));
        assert!(r["timestampMs"].as_i64().unwrap() > 1_000_000_000_000);
        assert!(r["enum"] == json!("a") || r["enum"] == json!(true), "zero weight never picked");
        let code = s_of(&r["regex"]);
        assert!(code.len() == 8 && code[..3].chars().all(|c| c.is_ascii_uppercase()) && &code[3..4] == "-" && code[4..].chars().all(|c| c.is_ascii_digit()), "{code}");
        let city = s_of(&r["address"]["city"]);
        assert_eq!(s_of(&r["template"]), format!("{}/{}/{}", first.to_uppercase(), 100 + 5 * i as i64, city.to_lowercase()));
        assert_eq!(s_of(&r["address"]["owner"]), last, "nested templates see the enclosing row");
        let tags = r["tags"].as_array().unwrap();
        assert!((2..=4).contains(&tags.len()));
        let lines = r["lines"].as_array().unwrap();
        assert!(lines.len() <= 2);
        for l in lines {
            assert_eq!(s_of(&l["sku"]).len(), 5);
            assert!((1..=3).contains(&l["qty"].as_i64().unwrap()));
        }
        assert_eq!(r["const"], json!({"v": 1}));
        // Declared order is kept.
        let keys: Vec<&String> = r.as_object().unwrap().keys().collect();
        assert_eq!(keys.first().unwrap().as_str(), "uuid");
        assert_eq!(keys.last().unwrap().as_str(), "const");
    }
    assert!(words_seen.len() > 10);
}

#[test]
fn email_and_username_derive_from_full_name_when_alone() {
    let s = schema(5, vec![Entity::new("p", 20, vec![f("name", FieldKind::FullName), f("email", FieldKind::email()), f("login", FieldKind::Username)])]);
    for r in rows(&s, "p") {
        let name = s_of(&r["name"]);
        let last = crate::fakers::ascii_slug(name.split_whitespace().last().unwrap());
        let first = crate::fakers::ascii_slug(name.split_whitespace().next().unwrap());
        let email = s_of(&r["email"]);
        assert!(email.contains(&last) || email.contains(&first), "{email} vs {name}");
        assert!(["example.com", "example.net", "example.org"].iter().any(|d| email.ends_with(d)), "safe domain: {email}");
    }
}

#[test]
fn null_percent() {
    let s = schema(
        5,
        vec![Entity::new("n", 2000, vec![f("a", FieldKind::Word).nullable(25.0), f("never", FieldKind::Word), f("always", FieldKind::Word).nullable(100.0)])],
    );
    let r = rows(&s, "n");
    let nulls = r.iter().filter(|x| x["a"].is_null()).count();
    assert!((400..600).contains(&nulls), "{nulls}");
    assert!(r.iter().all(|x| !x["never"].is_null()));
    assert!(r.iter().all(|x| x["always"].is_null()));
}

#[test]
fn locales() {
    for (code, _) in crate::LOCALES {
        let mut s = schema(1, vec![Entity::new("p", 5, vec![f("n", FieldKind::FullName), f("e", FieldKind::email()), f("c", FieldKind::City), f("s", FieldKind::Street)])]);
        s.locale = Some(code.to_string());
        for r in rows(&s, "p") {
            assert!(!s_of(&r["n"]).is_empty());
            assert!(s_of(&r["e"]).is_ascii(), "{code}: e-mails are ASCII");
        }
    }
    let mut s = schema(1, vec![people(3)]);
    s.locale = Some("pt-BR".into());
    validate(&s).unwrap();
    s.locale = Some("xx".into());
    assert!(validate(&s).unwrap_err().to_string().contains("unknown locale"));
    let mut de = schema(1, vec![Entity::new("p", 30, vec![f("n", FieldKind::FirstName)])]);
    let en_names = rows(&de, "p");
    de.locale = Some("de".into());
    assert_ne!(rows(&de, "p"), en_names);
}

// ---------- uniqueness ----------

#[test]
fn unique_values() {
    let s = schema(
        8,
        vec![Entity::new(
            "u",
            3000,
            vec![
                f("first", FieldKind::FirstName),
                f("last", FieldKind::LastName),
                f("email", FieldKind::email()).unique(),
                f("login", FieldKind::Username).unique(),
                f("n", FieldKind::int(1, 3000)).unique(),
                f("code", FieldKind::Regex { pattern: "[A-Z]{4}".into() }).unique(),
            ],
        )],
    );
    let r = rows(&s, "u");
    for key in ["email", "login", "n", "code"] {
        let set: HashSet<String> = r.iter().map(|x| x[key].to_string()).collect();
        assert_eq!(set.len(), 3000, "{key} not unique");
    }
    // An exact-size integer range is a permutation.
    let mut ns: Vec<i64> = r.iter().map(|x| x["n"].as_i64().unwrap()).collect();
    ns.sort_unstable();
    assert_eq!(ns, (1..=3000).collect::<Vec<_>>());
}

#[test]
fn impossible_uniqueness_fails_clearly() {
    let cases = [
        (FieldKind::int(1, 10), 11),
        (FieldKind::Enum { values: vec![json!("a"), json!("b")], weights: None }, 3),
        (FieldKind::Bool { true_percent: None }, 3),
        (FieldKind::Regex { pattern: "[ab]".into() }, 3),
    ];
    for (kind, count) in cases {
        let s = schema(1, vec![Entity::new("u", count, vec![f("v", kind.clone()).unique()])]);
        match generate_values(&s, &GenerateOptions::default()) {
            Err(DataError::Unique(msg)) => assert!(msg.contains("u.v"), "{msg}"),
            other => panic!("{kind:?}: expected a unique error, got {other:?}"),
        }
    }
    // Nulls don't count against uniqueness.
    let s = schema(1, vec![Entity::new("u", 200, vec![f("v", FieldKind::int(1, 150)).unique().nullable(50.0)])]);
    assert!(generate_values(&s, &GenerateOptions::default()).is_ok());
    let s = schema(1, vec![Entity::new("u", 2, vec![f("v", FieldKind::Sequence { start: None, step: Some(0) }).unique()])]);
    assert!(matches!(validate(&s), Err(DataError::Unique(_))));
}

// ---------- references ----------

#[test]
fn refs_pick_existing_parents_in_topological_order() {
    // Declared child-first: generation must still put users first.
    let s = schema(
        4,
        vec![
            Entity::new("orders", 300, vec![f("id", FieldKind::sequence()), f("userId", FieldKind::reference("users", "id")), f("email", FieldKind::reference("users", "email"))]),
            Entity::new("users", 20, vec![f("id", FieldKind::uuid()), f("email", FieldKind::email()).unique()]),
        ],
    );
    let r = generate_values(&s, &GenerateOptions::default()).unwrap();
    assert_eq!(r.keys().collect::<Vec<_>>(), vec!["users", "orders"]);
    let ids: HashSet<&Value> = r["users"].iter().map(|u| &u["id"]).collect();
    let emails: HashSet<&Value> = r["users"].iter().map(|u| &u["email"]).collect();
    assert!(r["orders"].iter().all(|o| ids.contains(&o["userId"]) && emails.contains(&o["email"])));
    // Every parent gets used with this many children.
    let used: HashSet<&Value> = r["orders"].iter().map(|o| &o["userId"]).collect();
    assert_eq!(used.len(), 20);
}

#[test]
fn unique_refs_are_one_to_one() {
    let s = schema(
        4,
        vec![people(10), Entity::new("profiles", 10, vec![f("userId", FieldKind::reference("users", "id")).unique()])],
    );
    let r = generate_values(&s, &GenerateOptions::default()).unwrap();
    let ids: HashSet<i64> = r["profiles"].iter().map(|p| p["userId"].as_i64().unwrap()).collect();
    assert_eq!(ids, (1..=10).collect());
    let mut too_many = s.clone();
    too_many.entities[1].count = 11;
    assert!(matches!(generate_values(&too_many, &GenerateOptions::default()), Err(DataError::Unique(_))));
}

#[test]
fn self_references_pick_earlier_rows() {
    let s = schema(4, vec![Entity::new("emp", 50, vec![f("id", FieldKind::sequence()), f("managerId", FieldKind::reference("emp", "id"))])]);
    let r = rows(&s, "emp");
    assert!(r[0]["managerId"].is_null());
    for (i, e) in r.iter().enumerate().skip(1) {
        let m = e["managerId"].as_i64().unwrap();
        assert!(m >= 1 && m <= i as i64, "row {i} manager {m}");
    }
}

#[test]
fn schema_errors() {
    let cyc = schema(
        1,
        vec![
            Entity::new("a", 1, vec![f("id", FieldKind::sequence()), f("b", FieldKind::reference("b", "id"))]),
            Entity::new("b", 1, vec![f("id", FieldKind::sequence()), f("a", FieldKind::reference("a", "id"))]),
        ],
    );
    let err = validate(&cyc).unwrap_err();
    assert!(matches!(err, DataError::Cycle(_)), "{err}");
    assert!(err.to_string().contains("a → b → a") || err.to_string().contains("b → a → b"), "{err}");

    let field_cycle = schema(1, vec![Entity::new("e", 1, vec![f("x", FieldKind::template("{{y}}")), f("y", FieldKind::template("{{x}}"))])]);
    assert!(matches!(validate(&field_cycle), Err(DataError::Cycle(_))));
    let selfie = schema(1, vec![Entity::new("e", 1, vec![f("x", FieldKind::template("{{x}}"))])]);
    assert!(matches!(validate(&selfie), Err(DataError::Cycle(_))));

    let bad = |kind: FieldKind| validate(&schema(1, vec![Entity::new("e", 1, vec![f("v", kind)])])).unwrap_err().to_string();
    assert!(bad(FieldKind::reference("nope", "id")).contains("unknown entity"));
    assert!(bad(FieldKind::reference("e", "nope")).contains("unknown field"));
    assert!(bad(FieldKind::template("{{missing}}")).contains("unknown field 'missing'"));
    assert!(bad(FieldKind::template("{{v|shout}}")).contains("unknown template filter"));
    assert!(bad(FieldKind::int(5, 1)).contains("min must not exceed max"));
    assert!(bad(FieldKind::Regex { pattern: "(".into() }).contains("invalid regex"));
    assert!(bad(FieldKind::Date { from: Some("yesterday".into()), to: None, format: None }).contains("cannot parse date"));
    assert!(bad(FieldKind::Date { from: None, to: None, format: Some("%Q".into()) }).contains("invalid date format"));
    assert!(bad(FieldKind::Enum { values: vec![], weights: None }).contains("at least one value"));
    assert!(bad(FieldKind::Enum { values: vec![json!(1)], weights: Some(vec![1.0, 2.0]) }).contains("weights"));
    assert!(bad(FieldKind::Uuid { version: Some(5) }).contains("UUID version"));

    let dup = schema(1, vec![Entity::new("e", 1, vec![f("a", FieldKind::Word), f("a", FieldKind::Word)])]);
    assert!(validate(&dup).unwrap_err().to_string().contains("duplicate field"));
    let dup = schema(1, vec![people(1), people(1)]);
    assert!(validate(&dup).unwrap_err().to_string().contains("duplicate entity"));
    let mut nulls = schema(1, vec![people(1)]);
    nulls.entities[0].fields[0].null_percent = 120.0;
    assert!(validate(&nulls).is_err());
    let s = schema(1, vec![people(1)]);
    assert!(matches!(generate_values(&s, &GenerateOptions { entity: Some("ghosts".into()), ..Default::default() }), Err(DataError::UnknownEntity(_))));
}

// ---------- inference ----------

#[test]
fn infers_types_from_sample_json() {
    let sample = json!([
        {
            "id": 1,
            "uuid": "3f2b8c4e-9a1d-4e6f-8b2a-1c3d5e7f9a0b",
            "firstName": "Ada",
            "last_name": "Lovelace",
            "email": "ada@example.com",
            "createdAt": "2024-05-01T10:20:30Z",
            "birthday": "1990-12-10",
            "price": 19.99,
            "status": "active",
            "active": true,
            "score": 7,
            "website": "https://ada.dev",
            "zip": "02134",
            "orderRef": "ORD-2024-00042",
            "nickname": null,
            "address": { "city": "London", "country": "GB", "lat": 51.5 },
            "tags": ["a", "b"],
            "items": [{ "sku": "X1", "qty": 2 }, { "sku": "X2", "qty": 5 }]
        },
        {
            "id": 2,
            "uuid": "8a4f2c1e-1b2d-4c3e-9f8a-7b6c5d4e3f2a",
            "firstName": "Alan",
            "last_name": "Turing",
            "email": "alan@example.com",
            "createdAt": "2024-06-01T08:00:00Z",
            "birthday": "1985-06-23",
            "price": 5.5,
            "status": "disabled",
            "active": false,
            "score": 9,
            "website": "https://alan.dev",
            "zip": "10001",
            "orderRef": "ORD-2024-00043",
            "nickname": "prof",
            "address": { "city": "Manchester", "country": "GB", "lat": 53.4 },
            "tags": [],
            "items": []
        }
    ]);
    let schema = infer_from_json(&sample);
    let e = &schema.entities[0];
    let kind = |name: &str| &e.fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name}")).kind;
    assert!(matches!(kind("id"), FieldKind::Sequence { .. }));
    assert!(matches!(kind("uuid"), FieldKind::Uuid { .. }));
    assert!(matches!(kind("firstName"), FieldKind::FirstName));
    assert!(matches!(kind("last_name"), FieldKind::LastName));
    assert!(matches!(kind("email"), FieldKind::Email { .. }));
    assert!(matches!(kind("createdAt"), FieldKind::Datetime { format: None, .. }));
    assert!(matches!(kind("birthday"), FieldKind::Date { .. }));
    assert!(matches!(kind("price"), FieldKind::Price { min: Some(m), max: Some(x), .. } if *m == 5.5 && *x == 19.99));
    match kind("status") {
        FieldKind::Enum { values, .. } => assert_eq!(values, &vec![json!("active"), json!("disabled")]),
        other => panic!("{other:?}"),
    }
    assert!(matches!(kind("active"), FieldKind::Bool { .. }));
    assert!(matches!(kind("score"), FieldKind::Int { min: Some(7), max: Some(9) }));
    assert!(matches!(kind("website"), FieldKind::Url));
    assert!(matches!(kind("zip"), FieldKind::Zip));
    assert!(matches!(kind("orderRef"), FieldKind::Regex { pattern } if pattern == "[A-Z]{3}-[0-9]{4}-[0-9]{5}"));
    assert_eq!(e.fields.iter().find(|f| f.name == "nickname").unwrap().null_percent, 50.0);
    match kind("address") {
        FieldKind::Object { fields } => {
            assert!(matches!(fields[0].kind, FieldKind::City));
            assert!(matches!(fields[1].kind, FieldKind::CountryCode));
            assert!(matches!(fields[2].kind, FieldKind::Latitude));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(kind("tags"), FieldKind::Array { min: Some(0), .. }));
    match kind("items") {
        FieldKind::Array { of, .. } => assert!(matches!(&**of, FieldKind::Object { fields } if fields.len() == 2)),
        other => panic!("{other:?}"),
    }
    // The inferred schema generates.
    let mut s = schema.clone();
    s.seed = Some(1);
    let out = generate_values(&s, &GenerateOptions::default()).unwrap();
    assert_eq!(out["items"].len(), 10);
    // A single object infers too.
    let one = infer_from_json(&json!({"name": "Grace Hopper", "count": 3}));
    assert!(matches!(one.entities[0].fields[0].kind, FieldKind::FullName));
}

// ---------- JSON Schema / OpenAPI ----------

#[test]
fn imports_json_schema() {
    let doc = json!({
        "title": "User",
        "type": "object",
        "required": ["id", "email"],
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "email": { "type": "string", "format": "email" },
            "age": { "type": "integer", "minimum": 18, "exclusiveMaximum": 66 },
            "score": { "type": "number", "minimum": 0, "maximum": 1 },
            "role": { "enum": ["admin", "user", null] },
            "code": { "type": "string", "pattern": "^[A-Z]{2}-\\d{3}$" },
            "nick": { "type": "string", "minLength": 3, "maxLength": 5 },
            "born": { "type": "string", "format": "date" },
            "seen": { "type": ["string", "null"], "format": "date-time" },
            "home": { "$ref": "#/$defs/Address" },
            "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 2 },
            "meta": { "allOf": [ { "properties": { "a": { "type": "boolean" } } }, { "properties": { "b": { "const": 7 } } } ] },
            "pet": { "oneOf": [ { "type": "null" }, { "type": "string", "enum": ["cat", "dog"] } ] }
        },
        "$defs": {
            "Address": { "type": "object", "properties": { "city": { "type": "string" }, "zip": { "type": "string" } } }
        }
    });
    let schema = from_json_schema(&doc).unwrap();
    assert_eq!(schema.entities[0].name, "User");
    let mut s = schema.clone();
    s.seed = Some(3);
    let rows = generate_values(&s, &GenerateOptions { count_override: Some(80), ..Default::default() }).unwrap().swap_remove("User").unwrap();
    for r in &rows {
        assert!(uuid::Uuid::parse_str(s_of(&r["id"])).is_ok());
        assert!(s_of(&r["email"]).contains('@'));
        assert!((18..=65).contains(&r["age"].as_i64().unwrap()));
        assert!((0.0..=1.0).contains(&r["score"].as_f64().unwrap()));
        assert!(r["role"].is_null() || r["role"] == json!("admin") || r["role"] == json!("user"));
        let code = s_of(&r["code"]);
        assert!(code.len() == 6 && code.as_bytes()[2] == b'-', "{code}");
        assert!((3..=5).contains(&s_of(&r["nick"]).len()));
        assert!(chrono::NaiveDate::parse_from_str(s_of(&r["born"]), "%Y-%m-%d").is_ok());
        assert!(r["seen"].is_null() || chrono::DateTime::parse_from_rfc3339(s_of(&r["seen"])).is_ok());
        assert!(r["home"]["city"].is_string() && r["home"]["zip"].is_string());
        assert!((1..=2).contains(&r["tags"].as_array().unwrap().len()));
        assert!(r["meta"]["a"].is_boolean());
        assert_eq!(r["meta"]["b"], json!(7));
        assert!(r["pet"].is_null() || r["pet"] == json!("cat") || r["pet"] == json!("dog"));
    }
    assert!(rows.iter().any(|r| r["role"].is_null()), "nullable enums produce nulls");
    assert!(matches!(schema.entities[0].fields.iter().find(|f| f.name == "home").unwrap().kind, FieldKind::Object { .. }));
}

#[test]
fn imports_openapi_components() {
    let yaml = r#"
openapi: 3.0.3
info: { title: Shop, version: "1" }
paths: {}
components:
  schemas:
    Customer:
      type: object
      properties:
        id: { type: integer }
        name: { type: string }
        email: { type: string, format: email }
        nickname: { type: string, nullable: true }
    Order:
      type: object
      properties:
        id: { type: string, format: uuid }
        customer: { $ref: '#/components/schemas/Customer' }
        lines:
          type: array
          items: { $ref: '#/components/schemas/Line' }
    Line:
      type: object
      properties:
        sku: { type: string }
        price: { type: number, minimum: 1, maximum: 9 }
    Status:
      type: string
      enum: [a, b]
"#;
    let schema = from_json_schema_text(yaml).unwrap();
    assert_eq!(schema.name, "Shop");
    let names: Vec<&str> = schema.entities.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["Customer", "Order", "Line"]);
    let customer = &schema.entities[0];
    assert!(matches!(customer.fields[0].kind, FieldKind::Sequence { .. }));
    assert!(matches!(customer.fields[1].kind, FieldKind::FullName));
    assert_eq!(customer.fields[3].null_percent, 10.0);
    let mut s = schema.clone();
    s.seed = Some(2);
    let out = generate_values(&s, &GenerateOptions::default()).unwrap();
    let order = &out["Order"][0];
    assert!(order["customer"]["email"].as_str().unwrap().contains('@'));
    for line in order["lines"].as_array().unwrap() {
        assert!((1.0..=9.0).contains(&line["price"].as_f64().unwrap()));
    }
    assert!(from_json_schema_text("not: [valid").is_err());
    assert!(from_json_schema_text("openapi: 3.0.0\ninfo: {title: x}\n").is_err());
}

// ---------- outputs ----------

fn tricky() -> Schema {
    schema(
        6,
        vec![
            Entity::new(
                "people",
                3,
                vec![
                    f("id", FieldKind::sequence()),
                    f("name", FieldKind::Enum { values: vec![json!("O'Brien, \"Ann\"\nback\\slash")], weights: None }),
                    f("ok", FieldKind::Bool { true_percent: Some(100.0) }),
                    f("missing", FieldKind::Json { value: Value::Null }),
                    f("address", FieldKind::Object { fields: vec![f("city", FieldKind::Enum { values: vec![json!("Paris")], weights: None }), f("geo", FieldKind::Object { fields: vec![f("lat", FieldKind::Json { value: json!(1.5) })] })] }),
                    f("tags", FieldKind::Json { value: json!(["x", "y"]) }),
                ],
            ),
            Entity::new("pets", 2, vec![f("owner", FieldKind::reference("people", "id")), f("weird col", FieldKind::Json { value: json!("a`b\"c") })]),
        ],
    )
}

fn render(s: &Schema, format: OutputFormat, entity: Option<&str>) -> String {
    generate_string(s, &GenerateOptions { entity: entity.map(String::from), ..Default::default() }, &format, None).unwrap().text
}

#[test]
fn json_and_ndjson_outputs() {
    let s = tricky();
    let multi: Value = serde_json::from_str(&render(&s, OutputFormat::json(), None)).unwrap();
    assert_eq!(multi["people"].as_array().unwrap().len(), 3);
    assert_eq!(multi["people"][0]["name"], json!("O'Brien, \"Ann\"\nback\\slash"));
    let single: Value = serde_json::from_str(&render(&s, OutputFormat::json(), Some("pets"))).unwrap();
    assert_eq!(single.as_array().unwrap().len(), 2);
    let compact = render(&s, OutputFormat::Json { pretty: false }, None);
    assert!(!compact.contains('\n'));
    assert_eq!(serde_json::from_str::<Value>(&compact).unwrap(), multi);
    let nd = render(&s, OutputFormat::Ndjson, Some("people"));
    let lines: Vec<Value> = nd.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines, multi["people"].as_array().unwrap().clone());
    // Empty entities stay valid.
    let empty = schema(1, vec![Entity::new("none", 0, vec![f("a", FieldKind::Word)])]);
    assert_eq!(serde_json::from_str::<Value>(&render(&empty, OutputFormat::json(), None)).unwrap(), json!([]));
    let none = schema(1, vec![]);
    assert_eq!(serde_json::from_str::<Value>(&render(&none, OutputFormat::json(), None)).unwrap(), json!({}));
}

#[test]
fn yaml_output_round_trips() {
    let s = tricky();
    let json: Value = serde_json::from_str(&render(&s, OutputFormat::json(), None)).unwrap();
    let yaml: Value = serde_yaml_ng::from_str(&render(&s, OutputFormat::Yaml, None)).unwrap();
    assert_eq!(yaml, json);
    let single: Value = serde_yaml_ng::from_str(&render(&s, OutputFormat::Yaml, Some("people"))).unwrap();
    assert_eq!(single, json["people"]);
    let empty = schema(1, vec![Entity::new("none", 0, vec![]), people(1)]);
    let v: Value = serde_yaml_ng::from_str(&render(&empty, OutputFormat::Yaml, None)).unwrap();
    assert_eq!(v["none"], json!([]));
}

#[test]
fn csv_output_flattens_and_escapes() {
    let s = tricky();
    let csv = render(&s, OutputFormat::csv(), Some("people"));
    let mut lines = csv.split('\n');
    assert_eq!(lines.next().unwrap(), "id,name,ok,missing,address.city,address.geo.lat,tags");
    assert!(csv.contains("1,\"O'Brien, \"\"Ann\"\"\nback\\slash\",true,,Paris,1.5,\"[\"\"x\"\",\"\"y\"\"]\"\n"), "{csv}");
    let tsv = render(&s, OutputFormat::Csv { delimiter: Some("\\t".into()), header: false }, Some("pets"));
    assert_eq!(tsv.lines().count(), 2);
    assert!(tsv.lines().all(|l| l.contains('\t')));
    let multi = render(&s, OutputFormat::csv(), None);
    assert!(multi.starts_with("# people\n") && multi.contains("\n\n# pets\nowner,weird col\n"));
    let bad = generate_string(&s, &GenerateOptions::default(), &OutputFormat::Csv { delimiter: Some("ab".into()), header: true }, None);
    assert!(bad.is_err());
}

#[test]
fn sql_escaping_per_dialect() {
    let mut out = String::new();
    sql_string("O'Brien \\ \n\0end", SqlDialect::Postgres, &mut out);
    assert_eq!(out, "'O''Brien \\ \nend'");
    out.clear();
    sql_string("O'Brien \\ \n\0\u{1a}", SqlDialect::Mysql, &mut out);
    assert_eq!(out, "'O''Brien \\\\ \\n\\0\\Z'");
    out.clear();
    sql_string("it's", SqlDialect::Sqlite, &mut out);
    assert_eq!(out, "'it''s'");
    assert_eq!(sql_ident("we\"ird", SqlDialect::Postgres), "\"we\"\"ird\"");
    assert_eq!(sql_ident("we`ird", SqlDialect::Mysql), "`we``ird`");

    let s = tricky();
    let pg = render(&s, OutputFormat::sql(SqlDialect::Postgres), None);
    assert!(pg.contains("INSERT INTO \"people\" (\"id\", \"name\", \"ok\", \"missing\", \"address\", \"tags\") VALUES\n  (1, 'O''Brien, \"Ann\"\nback\\slash', TRUE, NULL, '{\"city\":\"Paris\",\"geo\":{\"lat\":1.5}}', '[\"x\",\"y\"]'),"), "{pg}");
    assert!(pg.find("\"people\"").unwrap() < pg.find("\"pets\"").unwrap(), "parents first");
    assert!(pg.contains("\"weird col\""));
    let my = render(&s, OutputFormat::sql(SqlDialect::Mysql), None);
    assert!(my.contains("'O''Brien, \"Ann\"\\nback\\\\slash'"), "{my}");
    assert!(my.contains("`weird col`") && my.contains("'a`b\"c'"));
    let lite = render(&s, OutputFormat::sql(SqlDialect::Sqlite), Some("people"));
    assert!(lite.contains(", 1, NULL, "), "sqlite booleans are 1/0: {lite}");
    // One statement per row.
    let per_row = render(&s, OutputFormat::Sql { dialect: SqlDialect::Postgres, create_table: false, batch_size: Some(1) }, Some("people"));
    assert_eq!(per_row.matches("INSERT INTO").count(), 3);
    assert_eq!(per_row.matches(';').count(), 3);
    let batched = render(&s, OutputFormat::sql(SqlDialect::Postgres), Some("people"));
    assert_eq!(batched.matches("INSERT INTO").count(), 1);
    let ms = render(&s, OutputFormat::sql(SqlDialect::Sqlserver), Some("people"));
    assert!(ms.contains("INSERT INTO [people] ([id], [name], [ok]"), "{ms}");
    assert!(ms.contains("N'O''Brien") && ms.contains(", 1, NULL, "), "{ms}");
    assert_eq!(sql_ident("we]ird", SqlDialect::Sqlserver), "[we]]ird]");
}

#[test]
fn create_table_statements() {
    let s = schema(
        1,
        vec![
            Entity::new(
                "users",
                1,
                vec![
                    f("id", FieldKind::uuid()),
                    f("email", FieldKind::email()).unique(),
                    f("age", FieldKind::int(1, 2)).nullable(10.0),
                    f("meta", FieldKind::Object { fields: vec![] }),
                    f("born", FieldKind::Date { from: None, to: None, format: None }),
                    f("price", FieldKind::Price { min: None, max: None, decimals: None }),
                ],
            ),
            Entity::new("orders", 1, vec![f("id", FieldKind::sequence()), f("userId", FieldKind::reference("users", "id"))]),
        ],
    );
    let pg = create_table_sql(&s, &s.entities[0], SqlDialect::Postgres);
    assert!(pg.contains("\"id\" UUID NOT NULL PRIMARY KEY"), "{pg}");
    assert!(pg.contains("\"email\" TEXT NOT NULL UNIQUE"));
    assert!(pg.contains("\"age\" BIGINT,") || pg.contains("\"age\" BIGINT\n"));
    assert!(pg.contains("\"meta\" JSONB NOT NULL"));
    assert!(pg.contains("\"born\" DATE NOT NULL"));
    assert!(pg.contains("\"price\" NUMERIC(16, 2) NOT NULL"));
    let my = create_table_sql(&s, &s.entities[1], SqlDialect::Mysql);
    assert!(my.contains("`userId` CHAR(36) NOT NULL"), "{my}");
    assert!(my.contains("FOREIGN KEY (`userId`) REFERENCES `users` (`id`)"));
    let ms = create_table_sql(&s, &s.entities[0], SqlDialect::Sqlserver);
    assert!(ms.starts_with("IF OBJECT_ID(N'users', N'U') IS NULL\nCREATE TABLE [users] ("), "{ms}");
    assert!(ms.contains("[id] UNIQUEIDENTIFIER NOT NULL PRIMARY KEY") && ms.contains("[meta] NVARCHAR(MAX) NOT NULL") && ms.contains("[price] DECIMAL(16, 2)"), "{ms}");
    let full = render(&s, OutputFormat::Sql { dialect: SqlDialect::Sqlite, create_table: true, batch_size: None }, None);
    assert!(full.find("CREATE TABLE IF NOT EXISTS \"users\"").unwrap() < full.find("CREATE TABLE IF NOT EXISTS \"orders\"").unwrap());
}

#[test]
fn truncated_previews() {
    let s = schema(1, vec![people(1000)]);
    let r = generate_string(&s, &GenerateOptions::default(), &OutputFormat::json(), Some(2000)).unwrap();
    assert!(r.truncated);
    assert!(r.text.len() <= 2000);
    let r = generate_string(&s, &GenerateOptions { limit: Some(2), ..Default::default() }, &OutputFormat::json(), Some(1 << 20)).unwrap();
    assert!(!r.truncated);
    assert_eq!(r.stats.rows["users"], 2);
    assert_eq!(serde_json::from_str::<Value>(&r.text).unwrap().as_array().unwrap().len(), 2);
}

#[test]
fn exports_to_files() {
    let dir = tempfile::tempdir().unwrap();
    let s = tricky();
    let res = export_to_path(&s, &GenerateOptions::default(), &OutputFormat::json(), &dir.path().join("out.json")).unwrap();
    assert_eq!(res.files.len(), 1);
    assert_eq!(res.rows, 5);
    let text = std::fs::read_to_string(dir.path().join("out.json")).unwrap();
    assert_eq!(res.bytes, text.len() as u64);
    assert!(serde_json::from_str::<Value>(&text).unwrap().is_object());
    let res = export_to_path(&s, &GenerateOptions::default(), &OutputFormat::csv(), &dir.path().join("seed.csv")).unwrap();
    assert_eq!(res.files.len(), 2);
    let people = std::fs::read_to_string(dir.path().join("seed.people.csv")).unwrap();
    let pets = std::fs::read_to_string(dir.path().join("seed.pets.csv")).unwrap();
    assert!(people.starts_with("id,"));
    assert!(pets.starts_with("owner,"));
    // No temp files left behind.
    let leftovers = std::fs::read_dir(dir.path()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")).count();
    assert_eq!(leftovers, 0);
}

// ---------- WireMock bridge ----------

fn sample_body() -> Value {
    json!({
        "id": 42,
        "user": {
            "id": "3f2b8c4e-9a1d-4e6f-8b2a-1c3d5e7f9a0b",
            "firstName": "Ada",
            "lastName": "Lovelace",
            "email": "ada@example.com",
            "phone": "+1 (555) 010-9999",
            "address": { "street": "1 Main St", "city": "London", "country": "United Kingdom", "zip": "02134" },
            "company": "Analytical Engines Ltd",
            "bio": "Wrote the first program. Loved \"poetical science\".",
            "website": "https://ada.dev"
        },
        "orders": [
            { "orderId": "ORD-2024-00042", "status": "shipped", "total": 19.99, "paid": true, "createdAt": "2024-05-01T10:20:30Z", "on": "2024-05-01" },
            { "orderId": "ORD-2024-00043", "status": "pending", "total": 5.5, "paid": false, "createdAt": "2024-05-02T10:20:30Z", "on": "2024-05-02" }
        ],
        "count": 2,
        "ratio": 0.75,
        "updated": 1714558830,
        "tags": ["a", "b"],
        "color": "#a1b2c3",
        "next": null,
        "empty": {},
        "none": [],
        "weird": "{{not a helper}}"
    })
}

#[test]
fn template_bridge_keeps_structure_and_valid_json() {
    let sample = sample_body();
    let template = to_response_template(&sample, &TemplateOptions::default());
    // Strings are quoted, numbers and booleans are not.
    assert!(template.contains("\"firstName\": \"{{random 'Name.firstName'}}\""), "{template}");
    assert!(template.contains("\"email\": \"{{random 'Internet.emailAddress'}}\""));
    assert!(template.contains("\"city\": \"{{random 'Address.city'}}\""));
    assert!(template.contains("\"id\": \"{{randomValue type='UUID'}}\""));
    assert!(template.contains("\"status\": \"{{pickRandom "), "{template}");
    assert!(template.contains("\"paid\": {{pickRandom true false}}"));
    assert!(template.contains("\"total\": {{randomDecimal lower=5.5 upper=19.99}}"));
    assert!(template.contains("\"count\": {{randomInt "));
    assert!(template.contains("\"createdAt\": \"{{now}}\""));
    assert!(template.contains("\"on\": \"{{now format='yyyy-MM-dd'}}\""));
    assert!(template.contains("\"updated\": {{now format='unix'}}"));
    assert!(template.contains("\"orderId\": \"{{randomValue length=3 type='ALPHABETIC' uppercase=true}}-{{randomValue length=4 type='NUMERIC'}}-{{randomValue length=5 type='NUMERIC'}}\""), "{template}");
    assert!(template.contains("\"color\": \"#{{randomValue length=6 type='HEXADECIMAL'}}\""));
    assert!(template.contains("\"next\": null"));
    assert!(template.contains("\"empty\": {}") && template.contains("\"none\": []"));
    // Every Faker key emitted is in the documented vocabulary.
    let keys: Vec<&str> = FAKER_KEYS.iter().map(|k| k.0).collect();
    for cap in template.split("{{random '").skip(1) {
        let key = &cap[..cap.find('\'').unwrap()];
        assert!(keys.contains(&key), "undocumented key {key}");
    }

    // Substituting dummy values yields JSON with the original shape.
    let dummy = substitute_helpers(&template, |inner, in_string| match (in_string, inner.starts_with("pickRandom true")) {
        (true, _) => "x".into(),
        (false, true) => "true".into(),
        (false, false) => "1".into(),
    });
    let parsed: Value = serde_json::from_str(&dummy).unwrap_or_else(|e| panic!("{e}\n{dummy}"));
    assert_eq!(shape(&parsed), shape(&sample));

    // A locally rendered sample is valid JSON with real-looking values.
    let rendered: Value = serde_json::from_str(&render_template_sample(&template, 1)).unwrap();
    assert_eq!(shape(&rendered), shape(&sample));
    assert!(rendered["user"]["email"].as_str().unwrap().contains('@'));
    assert!(uuid::Uuid::parse_str(rendered["user"]["id"].as_str().unwrap()).is_ok());
    let total = rendered["orders"][0]["total"].as_f64().unwrap();
    assert!((5.5..=19.99).contains(&total));
    assert!(["shipped", "pending"].contains(&rendered["orders"][0]["status"].as_str().unwrap()));
    let code = rendered["orders"][0]["orderId"].as_str().unwrap();
    assert_eq!(code.len(), "ORD-2024-00042".len());
    assert_ne!(rendered["user"]["firstName"], json!("Ada"));
}

/// Keys and JSON types, ignoring values (numbers/strings/bools collapse to their type).
fn shape(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), shape(v))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(shape).collect()),
        Value::String(_) => json!("s"),
        Value::Number(_) => json!(0),
        Value::Bool(_) => json!(false),
        Value::Null => Value::Null,
    }
}

#[test]
fn template_bridge_can_keep_ids() {
    let template = to_response_template(&sample_body(), &TemplateOptions { keep_ids: true });
    assert!(template.contains("\"id\": 42"));
    assert!(template.contains("\"id\": \"3f2b8c4e-9a1d-4e6f-8b2a-1c3d5e7f9a0b\""));
    assert!(template.contains("\"orderId\": \"ORD-2024-00042\""));
    assert!(!template.contains("\"paid\": false"), "paid is not an id");
    let rendered: Value = serde_json::from_str(&render_template_sample(&template, 9)).unwrap();
    assert_eq!(rendered["id"], json!(42));
    // Top-level arrays and scalars work too.
    let t = to_response_template(&json!([1, "x@y.io", true]), &TemplateOptions::default());
    let v: Value = serde_json::from_str(&substitute_helpers(&t, |_, s| if s { "q".into() } else { "2".into() })).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
}

#[test]
fn faker_keys_all_render() {
    for (key, _) in FAKER_KEYS {
        let kind = faker_key_kind(key).unwrap_or_else(|| panic!("{key}"));
        let v = generate_value(&kind, 3, None).unwrap();
        assert!(v.as_str().is_some_and(|s| !s.is_empty()), "{key}: {v}");
        let out = render_template_sample(&format!("\"{{{{random '{key}'}}}}\""), 1);
        assert!(serde_json::from_str::<Value>(&out).unwrap().is_string(), "{key}");
    }
    // Built-ins.
    let t = "{\"a\": {{randomInt lower=3 upper=3}}, \"b\": \"{{randomValue length=4 type='NUMERIC'}}\", \"c\": {{now format='epoch'}}, \"d\": \"{{now format='yyyy-MM-dd' offset='-2 days'}}\", \"e\": \"{{unknownHelper}}\"}";
    let v: Value = serde_json::from_str(&render_template_sample(t, 1)).unwrap();
    assert_eq!(v["a"], json!(3));
    assert_eq!(v["b"].as_str().unwrap().len(), 4);
    assert!(v["c"].as_i64().unwrap() > 1_700_000_000_000);
    assert_eq!(v["d"].as_str().unwrap().len(), 10);
    assert_eq!(v["e"], json!("{{unknownHelper}}"));
}

// ---------- files & format ----------

#[test]
fn schema_file_format_is_flat_yaml() {
    let s = store::starter_schema("Shop");
    let yaml = devall_core::fsutil::to_yaml(&s).unwrap();
    assert!(yaml.starts_with("version: 1\nname: Shop\nseed: 42\nentities:\n"), "{yaml}");
    assert!(yaml.contains("  - name: email\n    type: email\n    unique: true\n"), "{yaml}");
    assert!(yaml.contains("    type: ref\n    entity: users\n    field: id\n"));
    assert!(yaml.contains("    nullPercent: 70.0\n"));
    let back: Schema = serde_yaml_ng::from_str(&yaml).unwrap();
    assert_eq!(back, s);
    // Hand-written files with defaults omitted parse.
    let hand: Schema = serde_yaml_ng::from_str("name: x\nentities:\n  - name: e\n    fields:\n      - {name: a, type: int, max: 5}\n      - {name: b, type: ulid}\n      - {name: c, type: array, of: {type: object, fields: [{name: d, type: word}]}}\n").unwrap();
    assert_eq!(hand.version, 1);
    assert_eq!(hand.entities[0].count, 10);
    assert_eq!(hand.entities[0].fields[0].kind, FieldKind::Int { min: None, max: Some(5) });
    validate(&hand).unwrap();
    // Unknown keys survive a round trip.
    let extra: Schema = serde_yaml_ng::from_str("name: x\nfuture: 1\nentities: []\n").unwrap();
    assert!(devall_core::fsutil::to_yaml(&extra).unwrap().contains("future: 1"));
    assert!(crate::json_schema()["$id"].as_str().unwrap().ends_with("data.schema.json"));
}

#[test]
fn store_operations() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(store::list(root).unwrap().is_empty());
    let a = store::create(root, "Users & Orders").unwrap();
    assert_eq!(a, "users-orders.data.yaml");
    assert_eq!(store::create(root, "Users & Orders").unwrap(), "users-orders-2.data.yaml");
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    std::fs::write(root.join("data/nested/broken.data.yaml"), "entities: [").unwrap();
    std::fs::write(root.join("data/readme.txt"), "ignored").unwrap();
    let list = store::list(root).unwrap();
    assert_eq!(list.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["nested/broken.data.yaml", "users-orders-2.data.yaml", "users-orders.data.yaml"]);
    assert!(list[0].error.is_some());
    assert_eq!(list[2].name, "Users & Orders");
    assert_eq!(list[2].entities, vec!["users", "orders"]);
    let mut s = store::read(root, &a).unwrap();
    s.seed = Some(7);
    store::save(root, &a, &s).unwrap();
    assert_eq!(store::read(root, &a).unwrap().seed, Some(7));
    assert!(store::read(root, "../devall.yaml").is_err());
    assert!(store::read(root, "../x.data.yaml").is_err());
    assert!(store::save(root, "x.yaml", &s).is_err());
    assert!(store::delete(root, "ghost.data.yaml").is_err());
    // The starter schema generates.
    let out = generate_values(&s, &GenerateOptions::default()).unwrap();
    assert_eq!(out["orders"].len(), 100);
}

// ---------- performance ----------

#[test]
fn hundred_thousand_rows_stream_quickly() {
    let s = schema(
        1,
        vec![Entity::new(
            "users",
            100_000,
            vec![
                f("id", FieldKind::sequence()),
                f("uuid", FieldKind::uuid()),
                f("firstName", FieldKind::FirstName),
                f("lastName", FieldKind::LastName),
                f("email", FieldKind::email()).unique(),
                f("age", FieldKind::int(18, 90)),
                f("createdAt", FieldKind::Datetime { from: None, to: None, format: None }),
                f("status", FieldKind::Enum { values: vec![json!("a"), json!("b")], weights: None }),
            ],
        )],
    );
    let start = Instant::now();
    let mut sink = std::io::sink();
    let stats = write_output(&s, &GenerateOptions::default(), &OutputFormat::csv(), &mut sink).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(stats.rows["users"], 100_000);
    // Generous for unoptimized test builds; release builds take a fraction of this.
    assert!(elapsed.as_secs_f64() < 30.0, "100k rows took {elapsed:?}");
    eprintln!("100k rows → CSV in {elapsed:?}");
}
