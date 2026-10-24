//! Tests for the regex, diff, query, codegen, time, network, misc and recipe ops.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::*;

const LEAF_PEM: &str = include_str!("testdata/leaf.pem");
const CA_PEM: &str = include_str!("testdata/ca.pem");

fn j(id: &str, input: &str, args: Value) -> Value {
    let out = run(id, input, &args).unwrap_or_else(|e| panic!("{id}: {e}"));
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("{id} did not return JSON ({e}): {out}"))
}

fn t(id: &str, input: &str, args: Value) -> String {
    run(id, input, &args).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn err(id: &str, input: &str, args: Value) -> String {
    match run(id, input, &args) {
        Err(UtilError::Invalid(m)) => m,
        other => panic!("{id}: expected an Invalid error, got {other:?}"),
    }
}

// ── Registry ─────────────────────────────────────────────────────────────────────────────────────

#[test]
fn registry_is_consistent_and_every_example_runs() {
    let ops = list();
    let mut ids = std::collections::HashSet::new();
    for op in &ops {
        assert!(ids.insert(op.id), "duplicate id {}", op.id);
        for a in &op.args {
            if let Some(opts) = a.kind.strip_prefix("select:") {
                assert!(opts.split('|').any(|o| o == a.default), "{}: default {:?} not among {opts}", op.id, a.default);
            }
        }
        if op.network {
            continue;
        }
        for ex in &op.examples {
            let mut args = ex.args.clone();
            if op.id == "bcrypt.hash" {
                args["cost"] = json!("4"); // keep debug-build tests fast
            }
            let out = run(op.id, ex.input, &args).unwrap_or_else(|e| panic!("example for {} failed: {e}", op.id));
            if op.output == Output::Json {
                serde_json::from_str::<Value>(&out).unwrap_or_else(|e| panic!("{} example is not JSON: {e}", op.id));
            }
        }
    }
    let net: Vec<&str> = ops.iter().filter(|o| o.network).map(|o| o.id).collect();
    assert_eq!(net, ["dns.lookup", "tls.chain"]);
    assert!(ops.iter().filter(|o| !o.no_input).all(|o| !o.examples.is_empty()), "every op with input has an example");
    // Serialized shape the UI relies on.
    let v = serde_json::to_value(info("regex.test").unwrap()).unwrap();
    assert_eq!(v["network"], false);
    assert!(v["examples"][0]["args"]["pattern"].is_string());
}

#[test]
fn missing_args_use_declared_defaults() {
    assert_eq!(j("number.base", "255", json!({}))["hex"], "0xff");
    assert_eq!(j("number.base", "255", json!({ "from": "", "to": "" }))["binary"], "0b11111111");
    assert_eq!(t("string.escape", "a\"b", json!({})), "a\\\"b");
}

// ── Regex ────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn regex_test_reports_groups_and_utf16_spans() {
    let v = j("regex.test", "née 2026-10-21 and 1999-01-02", json!({ "pattern": r"(?<y>\d{4})-(\d{2})-(?P<d>\d{2})" }));
    assert_eq!(v["matchCount"], 2);
    assert_eq!(v["groupCount"], 3);
    assert_eq!(v["groupNames"][0]["name"], "y");
    assert_eq!(v["groupNames"][1]["name"], Value::Null);
    let m = &v["matches"][0];
    assert_eq!(m["text"], "2026-10-21");
    // "née " is 4 UTF-16 units but 5 bytes.
    assert_eq!(m["start"], 4);
    assert_eq!(m["byteStart"], 5);
    assert_eq!(m["groups"][0]["text"], "2026");
    assert_eq!(m["groups"][2]["name"], "d");
    assert_eq!(m["groups"][2]["end"], 14);
    assert!(v["compileMicros"].is_u64() && v["matchMicros"].is_u64());

    // Flags and optional groups.
    let v = j("regex.test", "Foo\nfoo", json!({ "pattern": "^foo(x)?$", "flags": "im" }));
    assert_eq!(v["matchCount"], 2);
    assert_eq!(v["matches"][0]["groups"][0]["matched"], false);
    assert!(err("regex.test", "x", json!({ "pattern": "a", "flags": "q" })).contains("unknown flag"));
    assert!(err("regex.test", "x", json!({ "pattern": "(" })).contains("unclosed"));
}

#[test]
fn regex_flavors_and_backtracking_guard() {
    // Look-behind needs the pcre-like flavor.
    let e = err("regex.test", "$40", json!({ "pattern": r"(?<=\$)\d+" }));
    assert!(e.contains("pcre-like"), "{e}");
    let v = j("regex.test", "price $40, tax 5", json!({ "pattern": r"(?<=\$)\d+", "flavor": "pcre-like" }));
    assert_eq!(v["matchCount"], 1);
    assert_eq!(v["matches"][0]["text"], "40");
    // Backreference.
    let v = j("regex.test", "a hello hello b bye bye", json!({ "pattern": r"\b(\w+) \1\b", "flavor": "pcre-like" }));
    assert_eq!(v["matchCount"], 2);
    // Catastrophic backtracking is aborted, not hung.
    let evil = format!("{}!", "a".repeat(40));
    let v = j("regex.test", &evil, json!({ "pattern": r"^(?:(a+)\1?)+$", "flavor": "pcre-like" }));
    assert!(v["error"].as_str().unwrap().contains("backtrack"), "{v}");
    // Plain patterns in pcre-like mode get a linear-time hint.
    let v = j("regex.test", "abc", json!({ "pattern": "b", "flavor": "pcre-like" }));
    assert!(v["warnings"][0].as_str().unwrap().contains("linear"));
    // Empty matches are counted and flagged.
    let v = j("regex.test", "ab", json!({ "pattern": "x*" }));
    assert_eq!(v["matchCount"], 3);
    assert!(v["warnings"][0].as_str().unwrap().contains("empty"));
}

#[test]
fn regex_replace_split_escape() {
    let out = t("regex.replace", "2026-10-21 and 1999-01-02", json!({ "pattern": r"(?<y>\d{4})-(?<m>\d{2})-(?<d>\d{2})", "replacement": "${d}/${m}/$y" }));
    assert_eq!(out, "21/10/2026 and 02/01/1999");
    let out = t("regex.replace", "a1b2c3", json!({ "pattern": r"\d", "replacement": "#", "limit": "2" }));
    assert_eq!(out, "a#b#c3");
    let out = t("regex.replace", "foo bar", json!({ "pattern": r"(?<=foo )bar", "replacement": "[$0]", "flavor": "pcre-like" }));
    assert_eq!(out, "foo [bar]");
    assert_eq!(j("regex.split", "a, b;c ; d", json!({ "pattern": r"\s*[,;]\s*" })), json!(["a", "b", "c", "d"]));
    assert_eq!(t("regex.escape", "1+1=2? (yes)", json!({})), r"1\+1=2\? \(yes\)");
}

#[test]
fn regex_test_cases() {
    let input = r#"{"shouldMatch": ["ORD-1234", "x ORD-0001 y"], "shouldNotMatch": ["ORD-12", "ord-1234"]}"#;
    let v = j("regex.cases", input, json!({ "pattern": r"ORD-\d{4}" }));
    assert_eq!((v["passed"].as_u64(), v["failed"].as_u64()), (Some(4), Some(0)), "{v}");
    assert_eq!(v["cases"][1]["match"], "ORD-0001");
    // Case-insensitive now matches a should-not-match case.
    let v = j("regex.cases", input, json!({ "pattern": r"ORD-\d{4}", "flags": "i" }));
    assert_eq!(v["failed"], 1);
    assert_eq!(v["cases"][3]["text"], "ord-1234");
    assert_eq!(v["cases"][3]["passed"], false);
    // pcre-like flavor: look-around.
    let v = j("regex.cases", r#"{"shouldMatch": ["price $40"], "shouldNotMatch": ["40"]}"#, json!({ "pattern": r"(?<=\$)\d+", "flavor": "pcre-like" }));
    assert_eq!(v["failed"], 0, "{v}");
    assert!(err("regex.cases", "not json", json!({ "pattern": "a" })).contains("shouldMatch"));
}

fn kinds(nodes: &Value) -> Vec<String> {
    nodes.as_array().unwrap().iter().map(|n| n["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn regex_explain_walks_the_ast() {
    let v = j("regex.explain", r"^(?<user>[\w.+-]+)@ab(?:c|d){2,3}?$", json!({}));
    assert_eq!(v["source"], "ast");
    let nodes = &v["nodes"];
    assert_eq!(kinds(nodes), ["assertion", "group", "literal", "repeat", "assertion"]);
    let group = &nodes[1];
    assert!(group["description"].as_str().unwrap().contains("\"user\""));
    assert_eq!(group["start"], 1);
    let class = &group["children"][0]["children"][0];
    assert_eq!(class["kind"], "class");
    assert!(class["children"].as_array().unwrap().len() >= 4);
    // Literal runs are merged: "@ab".
    assert_eq!(nodes[2]["text"], "@ab");
    assert!(nodes[3]["description"].as_str().unwrap().contains("Between 2 and 3 times") && nodes[3]["description"].as_str().unwrap().contains("lazy"));
    let alt = &nodes[3]["children"][0]["children"][0];
    assert_eq!(alt["kind"], "alternation");
    assert_eq!(alt["children"][0]["description"], "Alternative 1: matches 'c'");

    // Look-around falls back to the backtracking parser in the pcre-like flavor.
    let v = j("regex.explain", r"(?<=\$)\d+(?!\d)", json!({ "flavor": "pcre-like" }));
    assert_eq!(v["source"], "backtracking-parser");
    let k = kinds(&v["nodes"]);
    assert_eq!(k.iter().filter(|k| *k == "lookaround").count(), 2, "{v}");
    let e = err("regex.explain", r"(?<=\$)\d+", json!({}));
    assert!(e.contains("pcre-like"));
    // x flag: whitespace ignored.
    let v = j("regex.explain", "a b # comment", json!({ "flags": "x" }));
    assert_eq!(v["nodes"][0]["text"], "a b");
}

// ── Diff ─────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn text_diff_line_word_char() {
    let v = j("diff.text", "a\nb\nc\n", json!({ "other": "a\nB\nc\nd\n" }));
    assert_eq!(v["identical"], false);
    assert_eq!(v["stats"]["inserted"], 2);
    assert_eq!(v["stats"]["deleted"], 1);
    let unified = v["unified"].as_str().unwrap();
    assert!(unified.starts_with("--- a\n+++ b\n@@ -1,3 +1,4 @@"), "{unified}");
    assert!(unified.contains("-b\n+B\n"));
    let changes = v["hunks"][0]["changes"].as_array().unwrap();
    let del = changes.iter().find(|c| c["tag"] == "delete").unwrap();
    assert_eq!(del["oldLine"], 2);
    assert_eq!(del["value"], "b");

    let v = j("diff.text", "the quick fox", json!({ "other": "the slow fox", "mode": "word" }));
    assert_eq!(v["unified"], "the [-quick-]{+slow+} fox");
    let v = j("diff.text", "abc", json!({ "other": "abd", "mode": "char" }));
    assert_eq!(v["segments"], json!([{ "tag": "equal", "value": "ab" }, { "tag": "delete", "value": "c" }, { "tag": "insert", "value": "d" }]));
    let v = j("diff.text", "same\n", json!({ "other": "same\n" }));
    assert_eq!(v["identical"], true);
    assert_eq!(v["hunks"], json!([]));
}

#[test]
fn json_diff_paths_and_patch_round_trip() {
    let a = json!({ "id": 1, "name": "Ada", "tags": ["x", "y"], "nested": { "a/b": 1, "gone": true }, "list": [{ "id": 1, "v": 1 }, { "id": 2, "v": 2 }], "n": 1 });
    let b = json!({ "id": 1, "name": "Ada K", "tags": ["x", "z", "y"], "nested": { "a/b": 2, "new": null }, "list": [{ "id": 2, "v": 3 }, { "id": 1, "v": 1 }], "n": 1.0 });
    let v = j("diff.json", &a.to_string(), json!({ "other": b.to_string() }));
    assert_eq!(v["equal"], false);
    let diffs = v["differences"].as_array().unwrap();
    let find = |p: &str| diffs.iter().find(|d| d["path"] == p).unwrap_or_else(|| panic!("no diff at {p}: {diffs:?}"));
    assert_eq!(find("$.name")["op"], "changed");
    assert_eq!(find("$.name")["old"], "Ada");
    assert_eq!(find("$.tags[1]")["op"], "added");
    assert_eq!(find("$.nested['a/b']")["pointer"], "/nested/a~1b");
    assert_eq!(find("$.nested.gone")["op"], "removed");
    assert!(diffs.iter().all(|d| d["path"] != "$.n"), "1 and 1.0 are equal");
    let patched = diff::apply_patch(&a, v["patch"].as_array().unwrap());
    assert_eq!(serde_json::to_string(&patched).unwrap().replace("1.0", "1"), serde_json::to_string(&b).unwrap().replace("1.0", "1"));

    // Ignore array order: reordered list items pair up by id.
    let v = j("diff.json", &a.to_string(), json!({ "other": b.to_string(), "ignoreArrayOrder": "yes" }));
    let diffs = v["differences"].as_array().unwrap();
    assert!(diffs.iter().any(|d| d["path"] == "$.list[1].v" && d["old"] == 2 && d["new"] == 3), "{diffs:?}");
    assert!(diffs.iter().any(|d| d["path"] == "$.tags[1]" && d["op"] == "added" && d["new"] == "z"));
    assert_eq!(v["summary"]["changed"], 3);
    let patched = diff::apply_patch(&a, v["patch"].as_array().unwrap());
    let mut want = b.clone();
    want["n"] = json!(1);
    let mut got = patched.clone();
    got["n"] = json!(1);
    assert!(j("diff.json", &got.to_string(), json!({ "other": want.to_string(), "ignoreArrayOrder": "yes" }))["equal"] == true);

    // Root type change and equal documents.
    let v = j("diff.json", "[1]", json!({ "other": "{\"a\":1}" }));
    assert_eq!(v["patch"], json!([{ "op": "replace", "path": "", "value": { "a": 1 } }]));
    assert_eq!(j("diff.json", "{\"a\":[1,2]}", json!({ "other": "{ \"a\" : [1, 2] }" }))["equal"], true);
    assert!(err("diff.json", "{}", json!({})).contains("Changed JSON"));
}

#[test]
fn json_patch_for_array_insertions_in_the_middle() {
    let a = json!([1, 2, 3, 4, 5]);
    let b = json!([1, 9, 2, 3, 5, 6]);
    let v = j("diff.json", &a.to_string(), json!({ "other": b.to_string() }));
    assert_eq!(diff::apply_patch(&a, v["patch"].as_array().unwrap()), b);
    let ops: Vec<&str> = v["differences"].as_array().unwrap().iter().map(|d| d["op"].as_str().unwrap()).collect();
    assert_eq!(ops, ["added", "removed", "added"]);
}

// ── Query ────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn jq_filters() {
    let doc = r#"{"users":[{"name":"ada","age":36},{"name":"bob","age":17}]}"#;
    assert_eq!(t("jq", doc, json!({ "filter": "[.users[] | select(.age > 18) | .name]" })), "[\n  \"ada\"\n]");
    assert_eq!(t("jq", doc, json!({ "filter": ".users[].name", "raw": "yes" })), "ada\nbob");
    assert_eq!(t("jq", doc, json!({ "filter": ".users | length" })), "2");
    assert_eq!(t("jq", "{\"b\":1,\"a\":2}", json!({})), "{\n  \"b\": 1,\n  \"a\": 2\n}");
    // A stream of inputs (NDJSON) and `jq -n` style empty input.
    assert_eq!(t("jq", "1\n2\n3", json!({ "filter": ". * 10" })), "10\n20\n30");
    assert_eq!(t("jq", "", json!({ "filter": "[limit(3; repeat(1))]" })), "[\n  1,\n  1,\n  1\n]");
    // halt must not exit the process.
    assert_eq!(t("jq", "1", json!({ "filter": "halt" })), "");
    assert!(err("jq", "1", json!({ "filter": "error(\"boom\")" })).contains("boom"));
    assert!(err("jq", "1", json!({ "filter": ".a |" })).contains("syntax"));
    assert!(err("jq", "1", json!({ "filter": "nosuchfn" })).contains("undefined"));
    assert!(err("jq", "{oops", json!({})).contains("not valid JSON"));
}

#[test]
fn jsonpath_and_jmespath() {
    let doc = r#"{"store":{"book":[{"title":"A","price":8},{"title":"B","price":12}]}}"#;
    assert_eq!(j("jsonpath", doc, json!({ "path": "$.store.book[?@.price < 10].title" })), json!(["A"]));
    assert_eq!(j("jsonpath", doc, json!({ "path": "$..title", "output": "paths" })), json!(["$['store']['book'][0]['title']", "$['store']['book'][1]['title']"]));
    let both = j("jsonpath", doc, json!({ "path": "$..price", "output": "both" }));
    assert_eq!(both[1]["pointer"], "/store/book/1/price");
    assert!(err("jsonpath", doc, json!({ "path": "$[" })).contains("invalid JSONPath"));
    assert_eq!(j("jmespath", doc, json!({ "expression": "store.book[?price > `10`].title" })), json!(["B"]));
    assert_eq!(j("jmespath", doc, json!({ "expression": "length(store.book)" })), json!(2));
    assert!(err("jmespath", doc, json!({ "expression": "store.[" })).contains("JMESPath"));
}

// ── Codegen ──────────────────────────────────────────────────────────────────────────────────────

const TYPES_SAMPLE: &str = r#"{"id":1,"userName":"ada","score":1,"tags":["a"],"createdAt":"2026-10-21T12:00:00Z",
 "orders":[{"id":"o1","total":1.5},{"id":"o2","total":2,"coupon":"X"}],"manager":null,"type":"admin","misc":[1,"x"]}"#;

#[test]
fn json_to_types_infers_and_merges_shapes() {
    let ts = t("json.toTypes", TYPES_SAMPLE, json!({}));
    assert!(ts.contains("export interface Root {"), "{ts}");
    assert!(ts.contains("  orders: Order[];"));
    assert!(ts.contains("export interface Order {\n  id: string;\n  total: number;\n  coupon?: string;\n}"), "{ts}");
    assert!(ts.contains("  manager: null;"));
    assert!(ts.contains("  misc: unknown[];"));

    let rs = t("json.toTypes", TYPES_SAMPLE, json!({ "language": "Rust (serde)", "root": "user" }));
    assert!(rs.contains("pub struct User {"), "{rs}");
    assert!(rs.contains("    #[serde(rename = \"userName\")]\n    pub user_name: String,"));
    assert!(rs.contains("    #[serde(rename = \"type\")]\n    pub type_: String,"));
    assert!(rs.contains("    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    pub coupon: Option<String>,"));
    assert!(rs.contains("use serde_json::Value;"));

    let go = t("json.toTypes", TYPES_SAMPLE, json!({ "language": "Go" }));
    assert!(go.contains("\tID "), "{go}");
    assert!(go.contains("`json:\"coupon,omitempty\"`"));
    assert!(go.contains("*string"));

    let py = t("json.toTypes", TYPES_SAMPLE, json!({ "language": "Python (dataclasses)" }));
    assert!(py.find("class Order:").unwrap() < py.find("class Root:").unwrap(), "dependencies first");
    assert!(py.contains("    coupon: Optional[str] = None"));
    assert!(py.contains("    user_name: str  # JSON: \"userName\""));
    let pd = t("json.toTypes", TYPES_SAMPLE, json!({ "language": "Python (pydantic)" }));
    assert!(pd.contains("user_name: str = Field(alias=\"userName\")"));
    assert!(pd.contains("model_config = ConfigDict(populate_by_name=True)"));

    let java = t("json.toTypes", TYPES_SAMPLE, json!({ "language": "Java records" }));
    assert!(java.contains("public record Order(") && java.contains("List<Order> orders") && java.contains("        String type,") && java.contains("double total"), "{java}");
    let java = t("json.toTypes", r#"{"user_id":1,"class":"x"}"#, json!({ "language": "Java records" }));
    assert!(java.contains("@JsonProperty(\"user_id\") long userId") && java.contains("@JsonProperty(\"class\") String class_"), "{java}");
    let cs = t("json.toTypes", TYPES_SAMPLE, json!({ "language": "C#" }));
    assert!(cs.contains("[JsonPropertyName(\"userName\")]\n    public required string UserName { get; set; }"), "{cs}");
    assert!(cs.contains("public string? Coupon { get; set; }"));
    let kt = t("json.toTypes", TYPES_SAMPLE, json!({ "language": "Kotlin" }));
    assert!(kt.contains("val coupon: String? = null,") && kt.contains("data class Root("), "{kt}");
    let sw = t("json.toTypes", TYPES_SAMPLE, json!({ "language": "Swift" }));
    assert!(sw.contains("struct Order: Codable {") && sw.contains("let coupon: String?") && sw.contains("enum JSONValue"), "{sw}");

    let schema: Value = serde_json::from_str(&t("json.toTypes", TYPES_SAMPLE, json!({ "language": "JSON Schema" }))).unwrap();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["createdAt"]["format"], "date-time");
    assert_eq!(schema["properties"]["orders"]["items"]["$ref"], "#/$defs/Order");
    assert_eq!(schema["$defs"]["Order"]["required"], json!(["id", "total"]));
    assert_eq!(schema["properties"]["manager"]["type"], "null");

    // Root arrays get an element type and an alias; identical shapes are shared.
    let ts = t("json.toTypes", r#"[{"a":{"x":1}},{"a":{"x":2},"b":{"x":3}}]"#, json!({ "root": "Items" }));
    assert!(ts.contains("export type Items = ItemsItem[];"), "{ts}");
    assert_eq!(ts.matches("export interface").count(), 2, "{ts}");
}

// ── Time ─────────────────────────────────────────────────────────────────────────────────────────

fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&chrono::Utc)
}

#[test]
fn cron_next_runs_and_descriptions() {
    let now = at("2026-10-21T12:07:00Z"); // a Wednesday
    let v = cron::explain("*/15 9-17 * * MON-FRI", "UTC", 3, now).unwrap();
    assert_eq!(v["nextRuns"][0]["utc"], "2026-10-21T12:15:00Z");
    assert_eq!(v["nextRuns"][2]["utc"], "2026-10-21T12:45:00Z");
    assert_eq!(v["fields"]["dayOfWeek"], "MON-FRI");
    assert!(!v["description"].as_str().unwrap().is_empty());

    let v = cron::explain("0 0 9 * * *", "America/New_York", 1, now).unwrap();
    assert_eq!(v["timezone"], "America/New_York");
    assert_eq!(v["nextRuns"][0]["utc"], "2026-10-21T13:00:00Z"); // 09:00 EDT
    assert_eq!(v["fields"]["second"], "0");
    assert_eq!(v["nextRuns"][0]["display"], "2026-10-21 09:00:00 EDT");

    let v = cron::explain("@daily", "UTC", 2, now).unwrap();
    assert_eq!(v["normalized"], "0 0 * * *");
    assert_eq!(v["nextRuns"][1]["utc"], "2026-10-23T00:00:00Z");
    let v = cron::explain("0 0 L * *", "UTC", 1, now).unwrap();
    assert_eq!(v["nextRuns"][0]["utc"], "2026-10-31T00:00:00Z");
    let v = cron::explain("0 0 1 * MON", "UTC", 1, now).unwrap();
    assert!(v["warnings"][0].as_str().unwrap().contains("EITHER"));
    let v = cron::explain("0 0 30 2 *", "UTC", 3, now).unwrap();
    assert_eq!(v["nextRuns"], json!([]));
    assert!(cron::explain("61 * * * *", "UTC", 1, now).is_err());
    assert!(cron::explain("* * * * *", "Mars/Olympus", 1, now).is_err());
}

#[test]
fn time_difference() {
    let now = at("2026-10-21T12:00:00Z");
    let v = time::diff("2026-01-01T00:00:00Z", "2026-10-21T12:34:56.5Z", now).unwrap();
    assert_eq!(v["breakdown"], json!({ "days": 293, "hours": 12, "minutes": 34, "seconds": 56, "milliseconds": 500 }));
    assert_eq!(v["iso8601"], "P293DT12H34M56.500S");
    assert_eq!(v["calendar"], "0 years, 9 months, 20 days, 12:34:56");
    assert_eq!(v["direction"], "forward");
    let v = time::diff("1700000000", "", now).unwrap();
    assert_eq!(v["to"], "2026-10-21T12:00:00.000Z");
    let v = time::diff("2026-10-22", "2026-10-21", now).unwrap();
    assert_eq!(v["direction"], "backward");
    assert_eq!(v["totalSeconds"], -86_400);
    assert_eq!(v["human"], "1d");
}

// ── Certificates & network ───────────────────────────────────────────────────────────────────────

#[test]
fn x509_decodes_pem_chains_and_der() {
    let now = at("2026-10-22T00:00:00Z").timestamp();
    let v = x509::decode(&format!("{LEAF_PEM}{CA_PEM}"), now).unwrap();
    assert_eq!(v["count"], 2);
    let leaf = &v["certificates"][0];
    assert_eq!(leaf["subjectFields"]["commonName"], "localhost");
    assert_eq!(leaf["issuerFields"]["commonName"], "devall Test CA");
    assert_eq!(leaf["subjectAltNames"], json!(["DNS:localhost", "IP:127.0.0.1"]));
    assert_eq!(leaf["fingerprints"]["sha256"], "61:79:6F:BA:28:38:9E:FD:1E:93:C3:01:FA:5C:A2:3A:96:EF:48:38:0A:BC:9D:31:97:49:E3:D1:67:E3:9B:36");
    assert_eq!(leaf["fingerprints"]["sha1"], "AA:0A:8C:15:F1:08:A7:3C:89:AA:4B:47:B4:96:61:3C:E7:3A:A7:3F");
    assert_eq!(leaf["serial"], "64:4D:D5:BB:D8:4A:19:A6:AB:02:15:13:A7:1B:69:1F:58:9C:9E:08");
    assert_eq!(leaf["publicKey"]["curve"], "P-256");
    assert_eq!(leaf["publicKey"]["bits"], 256);
    assert_eq!(leaf["validity"]["status"], "valid");
    assert_eq!(leaf["extendedKeyUsage"], json!(["serverAuth"]));
    assert_eq!(leaf["isCa"], false);
    assert!(leaf["signatureAlgorithm"].as_str().unwrap().contains("ecdsa"), "{}", leaf["signatureAlgorithm"]);
    let ca = &v["certificates"][1];
    assert_eq!(ca["isCa"], true);
    assert_eq!(ca["selfSigned"], true);
    assert_eq!(leaf["authorityKeyId"], ca["subjectKeyId"]);
    // Linked chain: only the "root included" note.
    assert_eq!(v["chainWarnings"].as_array().unwrap().len(), 1);
    // Reversed order is flagged.
    let v = x509::decode(&format!("{CA_PEM}{LEAF_PEM}"), now).unwrap();
    assert!(v["chainWarnings"][0].as_str().unwrap().contains("not issued by"));

    // Base64 DER without PEM armour, and expiry in the far future.
    let b64: String = LEAF_PEM.lines().filter(|l| !l.starts_with("-----")).collect();
    let far = at("2200-01-01T00:00:00Z").timestamp();
    let v = x509::decode(&b64, far).unwrap();
    assert_eq!(v["certificates"][0]["validity"]["status"], "expired");
    assert!(v["certificates"][0]["warnings"][0].as_str().unwrap().starts_with("EXPIRED"));
    assert!(x509::decode("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----", now).is_err());
    assert!(x509::decode("bm90IGEgY2VydA==", now).is_err());
}

/// A local TLS server presenting a `localhost` leaf + its CA, both generated for this run (no
/// private key in the repo), for `connections` handshakes. Returns the CA to trust.
fn tls_server(connections: usize) -> (u16, std::thread::JoinHandle<()>, rustls::pki_types::CertificateDer<'static>) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose};
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.distinguished_name.push(DnType::CommonName, "devall test CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca, ca_key);
    let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut leaf = CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    leaf.distinguished_name.push(DnType::CommonName, "localhost");
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf.use_authority_key_identifier_extension = true;
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer).unwrap();
    let certs = vec![leaf_cert.der().clone(), ca_cert.der().clone()];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = Arc::new(rustls::ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(certs, key).unwrap());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        for _ in 0..connections {
            let Ok((mut sock, _)) = listener.accept() else { return };
            sock.set_read_timeout(Some(Duration::from_secs(10))).ok();
            let mut conn = rustls::ServerConnection::new(config.clone()).unwrap();
            while conn.is_handshaking() {
                if conn.complete_io(&mut sock).is_err() {
                    break;
                }
            }
            let _ = conn.complete_io(&mut sock);
        }
    });
    (port, handle, ca_cert.der().clone())
}

#[test]
fn tls_chain_against_a_local_server() {
    let (port, server, ca) = tls_server(2);
    // Trusting the test CA: verified.
    let v = net::tls_chain_with("127.0.0.1", port, Some(vec![ca])).unwrap();
    assert_eq!(v["verified"], true, "{v}");
    assert_eq!(v["protocol"], "TLSv1.3");
    assert_eq!(v["certificates"].as_array().unwrap().len(), 2);
    assert_eq!(v["certificates"][0]["subjectFields"]["commonName"], "localhost");
    // Through the registry with the OS trust store: the private CA is unknown.
    let v = j("tls.chain", &format!("https://127.0.0.1:{port}/some/path"), json!({}));
    assert_eq!(v["verified"], false, "{v}");
    assert!(v["verificationError"].as_str().is_some());
    assert_eq!(v["port"], port);
    server.join().unwrap();
    assert!(err("tls.chain", "", json!({})).contains("host"));
}

#[test]
fn dns_lookup_validates_before_touching_the_network() {
    assert!(err("dns.lookup", "  ", json!({})).contains("host name"));
    assert!(err("dns.lookup", "example.com", json!({ "type": "XYZ" })).contains("unsupported"));
}

#[test]
fn cidr_and_ip_info() {
    let v = j("cidr.calc", "192.168.10.37/26", json!({ "contains": "192.168.10.70" }));
    assert_eq!(v["network"], "192.168.10.0");
    assert_eq!(v["broadcast"], "192.168.10.63");
    assert_eq!(v["netmask"], "255.255.255.192");
    assert_eq!(v["wildcard"], "0.0.0.63");
    assert_eq!(v["firstHost"], "192.168.10.1");
    assert_eq!(v["lastHost"], "192.168.10.62");
    assert_eq!(v["usableHosts"], 62);
    assert_eq!(v["scope"], "private");
    assert_eq!(v["contains"]["inNetwork"], false);
    assert!(v["note"].as_str().unwrap().contains("host inside"));
    let v = j("cidr.calc", "10.0.0.1 255.255.255.0", json!({}));
    assert_eq!(v["cidr"], "10.0.0.0/24");
    assert_eq!(j("cidr.calc", "10.0.0.0/31", json!({}))["usableHosts"], 2);
    assert_eq!(j("cidr.calc", "8.8.8.8", json!({}))["totalAddresses"], 1);
    let v = j("cidr.calc", "2001:db8:abcd::/48", json!({ "contains": "2001:db8:abcd:12::1" }));
    assert_eq!(v["totalAddresses"], "1208925819614629174706176");
    assert_eq!(v["lastHost"], "2001:db8:abcd:ffff:ffff:ffff:ffff:ffff");
    assert_eq!(v["subnets64"], "65536");
    assert_eq!(v["contains"]["inNetwork"], true);
    assert!(err("cidr.calc", "10.0.0.0/33", json!({})).contains("too long"));
    assert!(err("cidr.calc", "10.0.0.0 255.0.255.0", json!({})).contains("contiguous"));

    assert_eq!(j("ip.info", "100.64.12.7", json!({}))["scope"], "private");
    assert_eq!(j("ip.info", "100.64.12.7", json!({}))["tags"][0], "shared (carrier-grade NAT)");
    assert_eq!(j("ip.info", "8.8.8.8", json!({}))["scope"], "public");
    assert_eq!(j("ip.info", "127.0.0.1", json!({}))["reverseDns"], "1.0.0.127.in-addr.arpa");
    assert_eq!(j("ip.info", "192.168.1.1", json!({}))["integer"], 3_232_235_777u32);
    let v = j("ip.info", "[fe80::1%eth0]", json!({}));
    assert_eq!(v["scope"], "link-local");
    assert_eq!(v["expanded"], "fe80:0000:0000:0000:0000:0000:0000:0001");
    assert_eq!(j("ip.info", "::ffff:1.2.3.4", json!({}))["embeddedIpv4"], "1.2.3.4");
    assert_eq!(j("ip.info", "2001:db8::1", json!({}))["scope"], "documentation");
    assert_eq!(j("ip.info", "2606:4700::1111", json!({}))["scope"], "public");
    assert!(err("ip.info", "999.1.1.1", json!({})).contains("not an IPv4"));
}

// ── Encoding & misc ──────────────────────────────────────────────────────────────────────────────

#[test]
fn gzip_round_trip_and_formats() {
    let text = "hello hello hello ✓";
    let gz = t("gzip.compress", text, json!({}));
    assert!(gz.starts_with("H4sI"));
    assert_eq!(t("gzip.decompress", &gz, json!({})), text);
    let bytes = decode_base64(&gz).unwrap();
    assert_eq!(t("gzip.decompress", &hex::encode(&bytes), json!({})), text);
    // zlib stream.
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut z, b"zlib!").unwrap();
    assert_eq!(t("gzip.decompress", &base64::engine::general_purpose::STANDARD.encode(z.finish().unwrap()), json!({})), "zlib!");
    assert!(run("gzip.decompress", "aGVsbG8=", &json!({})).is_err());
}

#[test]
fn number_bases() {
    let v = j("number.base", "0xFF_FF", json!({}));
    assert_eq!(v["decimal"], "65535");
    assert_eq!(v["binaryGrouped"], "1111 1111 1111 1111");
    assert_eq!(v["octal"], "0o177777");
    assert_eq!(v["base36"], "1ekf");
    assert_eq!(j("number.base", "zz", json!({ "from": "36" }))["decimal"], "1295");
    assert_eq!(j("number.base", "0b1010", json!({ "to": "3" }))["base3"], "101");
    let v = j("number.base", "-1", json!({}));
    assert_eq!(v["hex"], "-0x1");
    assert_eq!(v["twosComplement64"], "0xffffffffffffffff");
    assert_eq!(j("number.base", "340282366920938463463374607431768211455", json!({}))["bits"], 128);
    assert!(err("number.base", "12", json!({ "from": "2" })).contains("base-2"));
    assert!(err("number.base", "xyz!", json!({})).contains("base"));
}

#[test]
fn colors_and_contrast() {
    let v = j("color.convert", "#3b82f6", json!({}));
    assert_eq!(v["rgb"], "rgb(59 130 246)");
    assert_eq!(v["hsl"], "hsl(217.2 91.2% 59.8%)");
    assert_eq!(v["contrast"]["against"], "#ffffff");
    assert_eq!(v["contrast"]["ratio"], 3.68);
    assert_eq!(v["contrast"]["aaNormalText"], false);
    assert_eq!(v["contrast"]["aaLargeText"], true);
    // OKLCH round-trips back to the same hex.
    let oklch = v["oklch"].as_str().unwrap().to_string();
    assert_eq!(j("color.convert", &oklch, json!({}))["hex"], "#3b82f6");
    assert_eq!(j("color.convert", "black", json!({ "against": "white" }))["contrast"]["ratio"], 21.0);
    assert_eq!(j("color.convert", "rebeccapurple", json!({}))["hex"], "#663399");
    assert_eq!(j("color.convert", "rgb(255 0 0 / 50%)", json!({}))["hexAlpha"], "#ff000080");
    assert_eq!(j("color.convert", "hsl(120, 100%, 25%)", json!({}))["hex"], "#008000");
    assert_eq!(j("color.convert", "hsv(0 100% 100%)", json!({}))["hex"], "#ff0000");
    assert_eq!(j("color.convert", "#f00", json!({}))["nearestNamed"], json!({ "name": "red", "exact": true }));
    assert_eq!(j("color.convert", "oklch(0.9 0.4 150)", json!({}))["outOfSrgbGamut"], true);
    assert!(run("color.convert", "nope", &json!({})).is_err());
}

#[test]
fn unicode_inspector_flags_tricks() {
    let v = j("unicode.inspect", "Pay\u{200b}Pal раypal é\u{202e}", json!({}));
    let w: Vec<&str> = v["warnings"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
    assert!(w.iter().any(|w| w.contains("bidirectional")), "{w:?}");
    assert!(w.iter().any(|w| w.contains("invisible") && w.contains("U+200B")));
    assert!(w.iter().any(|w| w.contains("Mixed scripts") && w.contains("Cyrillic + Latin")));
    assert!(w.iter().any(|w| w.contains("р (U+0440) ≈ p")));
    let e = v["characters"].as_array().unwrap().iter().find(|c| c["codePoint"] == "U+00E9").unwrap();
    assert_eq!(e["name"], "LATIN SMALL LETTER E WITH ACUTE");
    assert_eq!(e["utf8"], "C3 A9");
    assert_eq!(e["utf16"], "00E9");
    let v = j("unicode.inspect", "😀\n", json!({}));
    assert_eq!(v["counts"], json!({ "codePoints": 2, "utf16Units": 3, "utf8Bytes": 5, "approxGraphemes": 2, "lines": 1, "nonAscii": 1 }));
    assert_eq!(v["characters"][0]["utf16"], "D83D DE00");
    assert_eq!(v["characters"][1]["name"], "LINE FEED");
}

#[test]
fn passwords_and_lorem() {
    let v = j("password.generate", "", json!({ "length": "16", "count": "3", "symbols": "no" }));
    let pws = v["passwords"].as_array().unwrap();
    assert_eq!(pws.len(), 3);
    for p in pws {
        let p = p.as_str().unwrap();
        assert_eq!(p.len(), 16);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(p.chars().any(|c| c.is_ascii_lowercase()) && p.chars().any(|c| c.is_ascii_uppercase()) && p.chars().any(|c| c.is_ascii_digit()));
    }
    assert_eq!(v["alphabetSize"], 62);
    assert_eq!(v["entropyBits"], 95.3);
    let v = j("password.generate", "", json!({ "length": "8", "upper": "no", "digits": "no", "symbols": "no", "excludeAmbiguous": "yes" }));
    assert!(v["passwords"][0].as_str().unwrap().chars().all(|c| c.is_ascii_lowercase() && c != 'l' && c != 'o'));
    assert!(err("password.generate", "", json!({ "lower": "no", "upper": "no", "digits": "no", "symbols": "no" })).contains("at least one"));

    assert_eq!(t("lorem", "", json!({ "unit": "words", "count": "7" })).split(' ').count(), 7);
    let p = t("lorem", "", json!({ "unit": "paragraphs", "count": "2" }));
    assert!(p.starts_with("Lorem ipsum dolor sit amet"));
    assert_eq!(p.split("\n\n").count(), 2);
    let s = t("lorem", "", json!({ "unit": "sentences", "count": "3" }));
    assert_eq!(s.matches('.').count(), 3);
}

#[test]
fn hashes_bcrypt_totp() {
    assert_eq!(t("hash.sha3", "abc", json!({})), "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532");
    assert_eq!(t("hash.sha3", "", json!({ "variant": "keccak256" })), "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470");
    assert_eq!(t("hash.blake3", "abc", json!({})), "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85");
    assert_eq!(t("hash.blake3", "abc", json!({ "length": "64" })).len(), 128);

    let h = t("bcrypt.hash", "s3cret", json!({ "cost": "4" }));
    assert!(h.starts_with("$2b$04$"));
    assert_eq!(j("bcrypt.verify", "s3cret", json!({ "hash": h }))["valid"], true);
    assert_eq!(j("bcrypt.verify", "wrong", json!({ "hash": h }))["valid"], false);
    assert!(err("bcrypt.hash", "x", json!({ "cost": "31" })).contains("between 4 and 16"));
    assert!(err("bcrypt.verify", "x", json!({ "hash": "nope" })).contains("bcrypt"));

    // RFC 6238 appendix B vectors (SHA-1, 8 digits).
    let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
    assert_eq!(misc::totp_at(secret, 59, 8, "SHA1").unwrap(), "94287082");
    assert_eq!(misc::totp_at(secret, 1_111_111_109, 8, "SHA1").unwrap(), "07081804");
    assert_eq!(misc::totp_at(secret, 1_234_567_890, 8, "SHA1").unwrap(), "89005924");
    assert_eq!(misc::totp_at("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA", 59, 8, "SHA256").unwrap(), "46119246");
    let v = misc::totp(&format!("otpauth://totp/devall:ada?secret={secret}&digits=8&issuer=devall"), &json!({ "digits": "6", "period": "30", "algorithm": "SHA1" }), 59).unwrap();
    assert_eq!(v["code"], "94287082");
    assert_eq!(v["secondsLeft"], 1);
    assert_eq!(v["issuer"], "devall");
    assert!(err("totp.generate", "not base32!", json!({})).contains("Base32"));
}

#[test]
fn reference_and_text_helpers() {
    let v = j("http.status", "404", json!({}));
    assert_eq!(v["name"], "Not Found");
    assert_eq!(v["class"], "Client error");
    assert!(v["reference"].as_str().unwrap().contains("9110"));
    assert_eq!(j("http.status", "teapot", json!({}))[0]["code"], 418);
    assert!(j("http.status", "5xx", json!({})).as_array().unwrap().iter().all(|s| s["code"].as_u64().unwrap() / 100 == 5));
    assert_eq!(j("http.status", "299", json!({}))["class"], "Success");

    let ua = j("useragent.parse", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36", json!({}));
    assert_eq!(ua["browser"], "Chrome");
    assert_eq!(ua["engine"], "Blink");
    assert_eq!(ua["device"], "desktop");
    assert_eq!(ua["isMobile"], false);
    let bot = j("useragent.parse", "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)", json!({}));
    assert_eq!(bot["isBot"], true);

    let v = j("chmod.calc", "755", json!({}));
    assert_eq!(v["symbolic"], "rwxr-xr-x");
    assert_eq!(v["symbolicCommand"], "chmod u=rwx,g=rx,o=rx <file>");
    assert_eq!(j("chmod.calc", "-rwsr-x---", json!({}))["octal"], "4750");
    assert_eq!(j("chmod.calc", "u=rwx,g=rx,o=", json!({}))["octal"], "750");
    assert_eq!(j("chmod.calc", "1777", json!({}))["symbolic"], "rwxrwxrwt");
    assert_eq!(j("chmod.calc", "a+x", json!({}))["octal"], "111");
    assert!(run("chmod.calc", "rwxq", &json!({})).is_err());

    let svg = t("qr.generate", "https://example.com", json!({ "ecLevel": "H" }));
    assert!(svg.contains("<svg") && svg.contains("</svg>"));
    assert!(run("qr.generate", "", &json!({})).is_err());

    assert_eq!(t("slug", "Crème Brûlée: 10 Tips & Tricks (2026)!", json!({})), "creme-brulee-10-tips-and-tricks-2026");
    assert_eq!(t("slug", "Hello World", json!({ "separator": "_", "lowercase": "no" })), "Hello_World");
    assert_eq!(t("slug", "don't stop", json!({})), "dont-stop");
}

#[test]
fn string_escape_round_trips() {
    let sample = "He said \"hi\"\n\tit's 5\\5 — ok\u{1}";
    for mode in ["json", "js", "java", "c", "sql", "regex", "shell", "csv", "xml"] {
        let esc = t("string.escape", sample, json!({ "mode": mode }));
        let back = t("string.escape", &esc, json!({ "mode": mode, "direction": "unescape" }));
        assert_eq!(back, sample, "{mode}: {esc}");
    }
    assert_eq!(t("string.escape", "it's", json!({ "mode": "shell" })), "'it'\\''s'");
    assert_eq!(t("string.escape", "a,b", json!({ "mode": "csv" })), "\"a,b\"");
    assert_eq!(t("string.escape", "<a & 'b'>", json!({ "mode": "xml" })), "&lt;a &amp; &apos;b&apos;&gt;");
    assert_eq!(t("string.escape", "\\uD83D\\uDE00 \\x41\\101", json!({ "mode": "js", "direction": "unescape" })), "😀 AA");
}

// ── Recipes & magic ──────────────────────────────────────────────────────────────────────────────

#[test]
fn recipes_chain_operations() {
    let steps = r#"[{"op":"base64.decode"},{"op":"json.format","args":{"indent":"4"}},{"op":"hash.sha256","disabled":true}]"#;
    assert_eq!(t("recipe.run", "eyJhIjoxfQ", json!({ "steps": steps })), "{\n    \"a\": 1\n}");
    assert_eq!(t("recipe.run", "abc", json!({ "steps": r#"["hex.encode","base64.encode"]"# })), "NjE2MjYz");
    assert_eq!(t("recipe.run", "same", json!({})), "same");
    let e = err("recipe.run", "%%%", json!({ "steps": r#"[{"op":"url.decode"},{"op":"base64.decode"}]"# }));
    assert!(e.starts_with("step 2 (base64.decode):"), "{e}");
    assert!(err("recipe.run", "x", json!({ "steps": r#"[{"op":"nope"}]"# })).contains("unknown operation"));
    assert!(err("recipe.run", "x", json!({ "steps": "{}" })).contains("array"));
    // Nested recipes are allowed but bounded.
    let inner = json!([{ "op": "recipe.run", "args": { "steps": r#"["hex.encode"]"# } }]).to_string();
    assert_eq!(t("recipe.run", "a", json!({ "steps": inner })), "61");
    let deep = json!([{ "op": "recipe.run", "args": { "steps": json!([{ "op": "recipe.run", "args": { "steps": json!([{ "op": "recipe.run", "args": { "steps": "[]" } }]).to_string() } }]).to_string() } }]).to_string();
    assert!(err("recipe.run", "a", json!({ "steps": deep })).contains("nest"));
}

fn best_recipe(input: &str) -> Vec<String> {
    let v = magic::detect(input);
    v["candidates"][0]["recipe"].as_array().unwrap_or_else(|| panic!("no candidates: {v}")).iter().map(|s| s["op"].as_str().unwrap().to_string()).collect()
}

#[test]
fn magic_ranks_likely_decodings() {
    let gz = t("gzip.compress", r#"{"user":"ada","role":"admin"}"#, json!({}));
    assert_eq!(best_recipe(&gz), ["gzip.decompress", "json.format"]);
    let b64_url_json = t("base64.encode", "%7B%22id%22%3A42%7D", json!({}));
    assert_eq!(best_recipe(&b64_url_json), ["base64.decode", "url.decode", "json.format"]);
    let jwt = t("jwt.sign", r#"{"sub":"1"}"#, json!({ "key": "k" }));
    assert_eq!(best_recipe(&jwt), ["jwt.decode"]);
    assert_eq!(best_recipe("1700000000"), ["time.convert"]);
    assert_eq!(best_recipe("48656c6c6f2c20776f726c6421"), ["hex.decode"]);
    assert_eq!(best_recipe("*/5 * * * *"), ["cron.explain"]);
    assert_eq!(best_recipe("10.0.0.0/8"), ["cidr.calc"]);
    let v = magic::detect(&gz);
    assert!(v["candidates"][0]["preview"].as_str().unwrap().contains("\"ada\""));
    assert!(v["candidates"][0]["score"].as_f64().unwrap() > v["candidates"].as_array().unwrap().last().unwrap()["score"].as_f64().unwrap() || v["candidates"].as_array().unwrap().len() == 1);
    // Every suggested recipe replays to its preview through recipe.run.
    for c in v["candidates"].as_array().unwrap() {
        let out = t("recipe.run", &gz, json!({ "steps": c["recipe"].to_string() }));
        assert!(out.starts_with(c["preview"].as_str().unwrap()));
    }
    assert_eq!(magic::detect("hello world")["candidates"], json!([]));
}
