use serde_json::{Value, json};

use super::*;

fn run0(id: &str, input: &str) -> String {
    run(id, input, &json!({})).unwrap()
}

#[test]
fn every_listed_op_runs_or_reports_invalid_input() {
    for op in list() {
        // Network ops are exercised against local servers in tests_ext.rs.
        if op.network {
            continue;
        }
        let input = if op.no_input { "" } else { "x" };
        match run(op.id, input, &json!({ "key": "k" })) {
            Ok(_) | Err(UtilError::Invalid(_)) => {}
            Err(e) => panic!("{}: {e}", op.id),
        }
    }
    assert!(matches!(run("nope", "", &json!({})), Err(UtilError::UnknownOp(_))));
}

#[test]
fn encodings() {
    assert_eq!(run0("base64.encode", "héllo?"), "aMOpbGxvPw==");
    assert_eq!(run0("base64.decode", "aMOpbGxvPw"), "héllo?");
    assert_eq!(run0("base64.decode", "aMOp bGxv\nPw=="), "héllo?");
    assert_eq!(run0("base64url.encode", "héllo?"), "aMOpbGxvPw");
    assert_eq!(run0("url.encode", "a b&c=d/é~._-"), "a%20b%26c%3Dd%2F%C3%A9~._-");
    assert_eq!(run0("url.decode", "a%20b+c%26"), "a b c&");
    assert_eq!(run0("html.encode", "<a href=\"x\">&</a>"), "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;");
    assert_eq!(run0("html.decode", "&lt;b&gt; &amp; &#39; &eacute;"), "<b> & ' é");
    assert_eq!(run0("hex.encode", "Hi"), "4869");
    assert_eq!(run0("hex.decode", "48 69"), "Hi");
    let u: Value = serde_json::from_str(&run0("url.parse", "https://u:p@x.io:8443/a/b?q=1&q=2#f")).unwrap();
    assert_eq!(u["port"], 8443);
    assert_eq!(u["password"], "••••");
    assert_eq!(u["query"][1]["value"], "2");
}

#[test]
fn hashes() {
    assert_eq!(run0("hash.md5", "abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(run0("hash.sha1", "abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(run0("hash.sha256", "abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    // RFC 4231 test case 2.
    let mac = run("hmac.sha256", "what do ya want for nothing?", &json!({ "key": "Jefe" })).unwrap();
    assert_eq!(mac, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    let b64 = run("hmac.sha256", "what do ya want for nothing?", &json!({ "key": "Jefe", "encoding": "base64" })).unwrap();
    assert_eq!(b64, "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM=");
}

#[test]
fn jwt_sign_decode_verify() {
    let token = run("jwt.sign", r#"{"sub":"42","exp":4102444800}"#, &json!({ "key": "secret", "alg": "HS256" })).unwrap();
    assert_eq!(token.split('.').count(), 3);
    let d: Value = serde_json::from_str(&run0("jwt.decode", &format!("Bearer {token}"))).unwrap();
    assert_eq!(d["header"]["alg"], "HS256");
    assert_eq!(d["payload"]["sub"], "42");
    assert_eq!(d["times"]["exp"]["iso"], "2100-01-01T00:00:00+00:00");
    assert!(d["warnings"].as_array().unwrap().is_empty());

    let ok: Value = serde_json::from_str(&run("jwt.verify", &token, &json!({ "key": "secret" })).unwrap()).unwrap();
    assert_eq!(ok["valid"], true);
    let bad: Value = serde_json::from_str(&run("jwt.verify", &token, &json!({ "key": "wrong" })).unwrap()).unwrap();
    assert_eq!(bad["valid"], false);

    // Expired and never-expiring tokens produce warnings.
    let expired = run("jwt.sign", r#"{"exp":1}"#, &json!({ "key": "s" })).unwrap();
    let d = jwt::decode_at(&expired, 100).unwrap();
    assert!(d["warnings"][0].as_str().unwrap().starts_with("Expired"));
    let forever = run("jwt.sign", r#"{"a":1}"#, &json!({ "key": "s" })).unwrap();
    let d = jwt::decode_at(&forever, 100).unwrap();
    assert!(d["warnings"][0].as_str().unwrap().contains("never expires"));

    assert!(run0_err("jwt.decode", "not-a-jwt"));
    assert!(run("jwt.sign", "[1]", &json!({ "key": "s" })).is_err());
}

fn run0_err(id: &str, input: &str) -> bool {
    run(id, input, &json!({})).is_err()
}

#[test]
fn generators() {
    let ids = run("uuid.v4", "", &json!({ "count": "3" })).unwrap();
    assert_eq!(ids.lines().count(), 3);
    assert!(ids.lines().all(|l| l.len() == 36 && l.as_bytes()[14] == b'4'));
    assert_eq!(run("uuid.v7", "", &json!({ "count": 2 })).unwrap().lines().count(), 2);
    assert_eq!(run0("ulid", "").len(), 26);
}

#[test]
fn time_conversion() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-21T12:00:00Z").unwrap().with_timezone(&chrono::Utc);
    let s = time::convert("1700000000", now).unwrap();
    assert_eq!(s["detected"], "unix seconds");
    assert_eq!(s["iso8601Utc"], "2023-11-14T22:13:20.000Z");
    let ms = time::convert("1700000000123", now).unwrap();
    assert_eq!(ms["unixMillis"], 1700000000123i64);
    let iso = time::convert("2026-10-21T13:00:00+01:00", now).unwrap();
    assert_eq!(iso["unixSeconds"], now.timestamp());
    assert_eq!(iso["relative"], "now");
    let later = time::convert("2026-10-22", now).unwrap();
    assert_eq!(later["relative"], "in 12h 0m");
    assert!(time::convert("someday", now).is_err());
}

#[test]
fn data_and_text() {
    assert_eq!(run0("json.minify", "{ \"a\" : [1, 2] }"), "{\"a\":[1,2]}");
    assert_eq!(run("json.format", "{\"a\":1}", &json!({ "indent": "4" })).unwrap(), "{\n    \"a\": 1\n}");
    assert_eq!(run0("json.toYaml", "{\"a\":[1]}"), "a:\n- 1\n");
    assert_eq!(run0("yaml.toJson", "a: [1]"), "{\n  \"a\": [\n    1\n  ]\n}");
    assert_eq!(run0("json.escape", "a\"b\n"), "\"a\\\"b\\n\"");
    assert_eq!(run0("json.unescape", "a\\\"b\\n"), "a\"b\n");
    assert_eq!(run0("case.camel", "parse HTTPResponse_v2"), "parseHttpResponseV2");
    assert_eq!(run0("case.snake", "parseHTTPResponse"), "parse_http_response");
    assert_eq!(run0("case.kebab", "Hello World"), "hello-world");
    assert_eq!(run0("case.constant", "maxRetries"), "MAX_RETRIES");
    assert_eq!(run0("case.pascal", "user-id"), "UserId");
    assert_eq!(run0("case.title", "user_id"), "User Id");
    let st: Value = serde_json::from_str(&run0("text.stats", "a b\nc")).unwrap();
    assert_eq!(st["words"], 3);
    assert_eq!(st["lines"], 2);
}
