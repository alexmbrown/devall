//! WireMock "string value patterns" (`equalTo`, `matches`, `equalToJson`, …) and multi-value
//! patterns (`hasExactly`, `includes`), each with an exact-match test and a 0..=1 distance used
//! to rank near misses.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::dates;
use crate::jsonpath::JsonPathExpr;
use crate::util::{b64_decode, java_regex, json_text, normalized_distance, regex_matches};
use crate::xml::{self, XNode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eval {
    pub matched: bool,
    pub distance: f64,
}

impl Eval {
    pub fn exact(matched: bool) -> Self {
        Self { matched, distance: if matched { 0.0 } else { 1.0 } }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateOp {
    Before,
    After,
    Equal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumOp {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, Clone)]
pub enum ExpectedDate {
    /// `now` plus an optional offset, evaluated at match time.
    Now(Option<dates::Offset>),
    Fixed(DateTime<Utc>),
}

#[derive(Debug, Clone)]
pub enum ValuePattern {
    EqualTo { expected: String, ci: bool },
    Contains { expected: String, ci: bool },
    DoesNotContain { expected: String, ci: bool },
    Matches { re: Box<fancy_regex::Regex>, src: String },
    DoesNotMatch { re: Box<fancy_regex::Regex>, src: String },
    Absent,
    Present,
    EqualToJson { expected: Value, ignore_array_order: bool, ignore_extra: bool },
    JsonPath { path: Box<JsonPathExpr>, sub: Option<Box<ValuePattern>> },
    EqualToXml { expected: Box<XNode>, placeholders: bool, src: String },
    XPath { expr: String, namespaces: Vec<(String, String)>, sub: Option<Box<ValuePattern>> },
    BinaryEqualTo(Vec<u8>),
    Date { op: DateOp, expected: ExpectedDate, actual_format: Option<String>, truncate_expected: Option<String>, truncate_actual: Option<String> },
    Number { op: NumOp, expected: f64 },
    And(Vec<ValuePattern>),
    Or(Vec<ValuePattern>),
    Not(Box<ValuePattern>),
    /// A matcher this engine cannot evaluate (e.g. `matchesJsonSchema`): never matches.
    Unsupported(String),
}

const OPS: &[&str] = &[
    "equalTo",
    "binaryEqualTo",
    "contains",
    "doesNotContain",
    "matches",
    "doesNotMatch",
    "absent",
    "equalToJson",
    "matchesJsonPath",
    "matchesJsonSchema",
    "equalToXml",
    "matchesXPath",
    "before",
    "after",
    "equalToDateTime",
    "equalToNumber",
    "greaterThanNumber",
    "greaterThanEqualNumber",
    "lessThanNumber",
    "lessThanEqualNumber",
    "and",
    "or",
    "not",
];

fn str_of(v: &Value) -> String {
    json_text(v)
}

fn opt_str(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).filter(|v| !v.is_null()).map(str_of)
}

fn bool_of(obj: &Map<String, Value>, key: &str) -> bool {
    match obj.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Parses `{expression, <sub-matcher>}` or a bare expression string.
fn expression_and_sub(v: &Value) -> Result<(String, Option<Box<ValuePattern>>), String> {
    match v {
        Value::String(s) => Ok((s.clone(), None)),
        Value::Object(o) => {
            let expr = o.get("expression").and_then(Value::as_str).ok_or("expression is required")?.to_string();
            let mut rest = o.clone();
            rest.remove("expression");
            rest.remove("xPathNamespaces");
            let sub = if rest.is_empty() { None } else { Some(Box::new(ValuePattern::from_json(&Value::Object(rest))?)) };
            Ok((expr, sub))
        }
        _ => Err("expected an expression string or object".into()),
    }
}

fn parse_namespaces(obj: &Map<String, Value>, inner: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for src in [obj.get("xPathNamespaces"), inner.get("xPathNamespaces")].into_iter().flatten() {
        if let Some(m) = src.as_object() {
            out.extend(m.iter().map(|(k, v)| (k.clone(), str_of(v))));
        }
    }
    out
}

impl ValuePattern {
    /// Whether `key` names a matcher (`equalTo`, `matchesJsonPath`, …).
    pub fn is_operator(key: &str) -> bool {
        OPS.contains(&key)
    }

    pub fn from_json(v: &Value) -> Result<Self, String> {
        let obj = v.as_object().ok_or_else(|| format!("matcher must be an object, got {v}"))?;
        let ci = bool_of(obj, "caseInsensitive");
        let Some((op, val)) = OPS.iter().find_map(|op| obj.get(*op).map(|val| (*op, val))) else {
            let keys: Vec<&String> = obj.keys().collect();
            return Err(format!("unknown matcher {keys:?}"));
        };
        Ok(match op {
            "equalTo" => Self::EqualTo { expected: str_of(val), ci },
            "contains" => Self::Contains { expected: str_of(val), ci },
            "doesNotContain" => Self::DoesNotContain { expected: str_of(val), ci },
            "matches" => {
                let src = str_of(val);
                Self::Matches { re: Box::new(java_regex(&src)?), src }
            }
            "doesNotMatch" => {
                let src = str_of(val);
                Self::DoesNotMatch { re: Box::new(java_regex(&src)?), src }
            }
            "absent" => {
                if matches!(val, Value::Bool(false)) {
                    Self::Present
                } else {
                    Self::Absent
                }
            }
            "binaryEqualTo" => Self::BinaryEqualTo(b64_decode(&str_of(val)).ok_or("binaryEqualTo must be base64")?),
            "equalToJson" => {
                let expected = match val {
                    Value::String(s) => serde_json::from_str(s).map_err(|e| format!("equalToJson is not valid JSON: {e}"))?,
                    other => other.clone(),
                };
                Self::EqualToJson { expected, ignore_array_order: bool_of(obj, "ignoreArrayOrder"), ignore_extra: bool_of(obj, "ignoreExtraElements") }
            }
            "matchesJsonPath" => {
                let (expr, sub) = expression_and_sub(val)?;
                Self::JsonPath { path: Box::new(JsonPathExpr::compile(&expr)?), sub }
            }
            "matchesJsonSchema" => Self::Unsupported("matchesJsonSchema".into()),
            "equalToXml" => {
                let src = str_of(val);
                Self::EqualToXml { expected: Box::new(xml::parse_tree(&src)?), placeholders: bool_of(obj, "enablePlaceholders"), src }
            }
            "matchesXPath" => {
                let (expr, sub) = expression_and_sub(val)?;
                xml::check_xpath(&expr)?;
                Self::XPath { namespaces: parse_namespaces(obj, val), expr, sub }
            }
            "before" | "after" | "equalToDateTime" => {
                let raw = str_of(val);
                let now_based = raw.trim().to_ascii_lowercase().starts_with("now");
                let mut offset = None;
                let expected = if now_based {
                    let rest = raw.trim()[3..].trim();
                    if !rest.is_empty() {
                        offset = Some(dates::parse_offset(rest).ok_or_else(|| format!("invalid date offset {rest:?}"))?);
                    }
                    ExpectedDate::Now(offset)
                } else {
                    ExpectedDate::Fixed(dates::parse_flexible(&raw).ok_or_else(|| format!("invalid date {raw:?}"))?)
                };
                let expected = match (obj.get("expectedOffset").and_then(Value::as_i64), opt_str(obj, "expectedOffsetUnit")) {
                    (Some(n), unit) => {
                        let unit = dates::parse_unit(unit.as_deref().unwrap_or("days")).ok_or("invalid expectedOffsetUnit")?;
                        let off = dates::Offset { amount: n, unit };
                        match expected {
                            ExpectedDate::Now(None) => ExpectedDate::Now(Some(off)),
                            ExpectedDate::Now(Some(o)) => ExpectedDate::Now(Some(o)),
                            ExpectedDate::Fixed(dt) => ExpectedDate::Fixed(dates::apply_offset(dt, off)),
                        }
                    }
                    _ => expected,
                };
                let op = match op {
                    "before" => DateOp::Before,
                    "after" => DateOp::After,
                    _ => DateOp::Equal,
                };
                Self::Date {
                    op,
                    expected,
                    actual_format: opt_str(obj, "actualFormat"),
                    truncate_expected: opt_str(obj, "truncateExpected"),
                    truncate_actual: opt_str(obj, "truncateActual"),
                }
            }
            "equalToNumber" | "greaterThanNumber" | "greaterThanEqualNumber" | "lessThanNumber" | "lessThanEqualNumber" => {
                let expected = match val {
                    Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
                    other => str_of(other).trim().parse().map_err(|_| format!("{op} needs a number"))?,
                };
                let op = match op {
                    "equalToNumber" => NumOp::Eq,
                    "greaterThanNumber" => NumOp::Gt,
                    "greaterThanEqualNumber" => NumOp::Gte,
                    "lessThanNumber" => NumOp::Lt,
                    _ => NumOp::Lte,
                };
                Self::Number { op, expected }
            }
            "and" | "or" => {
                let list = val.as_array().ok_or_else(|| format!("{op} needs an array"))?;
                let subs = list.iter().map(Self::from_json).collect::<Result<Vec<_>, _>>()?;
                if op == "and" { Self::And(subs) } else { Self::Or(subs) }
            }
            _ => Self::Not(Box::new(Self::from_json(val)?)),
        })
    }

    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// Short human-readable form for near-miss reports.
    pub fn describe(&self) -> String {
        let ci = |b: bool| if b { " (case-insensitive)" } else { "" };
        match self {
            Self::EqualTo { expected, ci: c } => format!("equalTo{}: {expected}", ci(*c)),
            Self::Contains { expected, ci: c } => format!("contains{}: {expected}", ci(*c)),
            Self::DoesNotContain { expected, ci: c } => format!("doesNotContain{}: {expected}", ci(*c)),
            Self::Matches { src, .. } => format!("matches: {src}"),
            Self::DoesNotMatch { src, .. } => format!("doesNotMatch: {src}"),
            Self::Absent => "absent".into(),
            Self::Present => "present".into(),
            Self::EqualToJson { expected, .. } => format!("equalToJson: {expected}"),
            Self::JsonPath { path, sub } => match sub {
                Some(s) => format!("matchesJsonPath: {} ({})", path.raw(), s.describe()),
                None => format!("matchesJsonPath: {}", path.raw()),
            },
            Self::EqualToXml { src, .. } => format!("equalToXml: {src}"),
            Self::XPath { expr, sub, .. } => match sub {
                Some(s) => format!("matchesXPath: {expr} ({})", s.describe()),
                None => format!("matchesXPath: {expr}"),
            },
            Self::BinaryEqualTo(b) => format!("binaryEqualTo: {} bytes", b.len()),
            Self::Date { op, expected, .. } => {
                let e = match expected {
                    ExpectedDate::Now(None) => "now".to_string(),
                    ExpectedDate::Now(Some(o)) => format!("now {:+} {:?}", o.amount, o.unit).to_lowercase(),
                    ExpectedDate::Fixed(d) => d.to_rfc3339(),
                };
                format!("{op:?}: {e}").replacen("Before", "before", 1).replacen("After", "after", 1).replacen("Equal", "equalToDateTime", 1)
            }
            Self::Number { op, expected } => format!("{op:?}Number: {expected}"),
            Self::And(v) => format!("and({})", v.iter().map(Self::describe).collect::<Vec<_>>().join(", ")),
            Self::Or(v) => format!("or({})", v.iter().map(Self::describe).collect::<Vec<_>>().join(", ")),
            Self::Not(p) => format!("not({})", p.describe()),
            Self::Unsupported(name) => format!("{name} (not supported by /dev/all)"),
        }
    }

    /// Exact match test (`None` = value absent).
    pub fn is_match(&self, actual: Option<&str>) -> bool {
        self.eval_inner(actual, false).matched
    }

    /// Match with a distance for near-miss ranking.
    pub fn eval(&self, actual: Option<&str>) -> Eval {
        self.eval_inner(actual, true)
    }

    /// Body matching: `binaryEqualTo` compares raw bytes, everything else sees the text.
    pub fn eval_bytes(&self, body: &[u8], with_distance: bool) -> Eval {
        if let Self::BinaryEqualTo(expected) = self {
            return Eval::exact(expected.as_slice() == body);
        }
        let text = String::from_utf8_lossy(body);
        let actual = if body.is_empty() && self.is_absent() { None } else { Some(text.as_ref()) };
        self.eval_inner(actual, with_distance)
    }

    fn eval_inner(&self, actual: Option<&str>, dist: bool) -> Eval {
        match self {
            Self::Absent => return Eval::exact(actual.is_none()),
            Self::Present => return Eval::exact(actual.is_some()),
            Self::Not(p) => return Eval::exact(!p.eval_inner(actual, false).matched),
            Self::And(subs) => {
                let evals: Vec<Eval> = subs.iter().map(|s| s.eval_inner(actual, dist)).collect();
                let matched = evals.iter().all(|e| e.matched);
                let distance = if evals.is_empty() { 0.0 } else { evals.iter().map(|e| e.distance).sum::<f64>() / evals.len() as f64 };
                return Eval { matched, distance: if matched { 0.0 } else { distance.max(0.01) } };
            }
            Self::Or(subs) => {
                let evals: Vec<Eval> = subs.iter().map(|s| s.eval_inner(actual, dist)).collect();
                let matched = evals.iter().any(|e| e.matched);
                let distance = evals.iter().map(|e| e.distance).fold(1.0, f64::min);
                return Eval { matched, distance: if matched { 0.0 } else { distance } };
            }
            Self::JsonPath { path, sub: Some(sub) } => {
                let doc = actual.and_then(|a| serde_json::from_str::<Value>(a).ok());
                let Some(doc) = doc else { return Eval::exact(false) };
                return match path.eval_value(&doc) {
                    None | Some(Value::Null) => sub.eval_inner(None, dist),
                    Some(val) => {
                        let whole = sub.eval_inner(Some(&json_text(&val)), dist);
                        if whole.matched {
                            return whole;
                        }
                        match &val {
                            Value::Array(items) if !items.is_empty() => items
                                .iter()
                                .map(|x| sub.eval_inner(Some(&json_text(x)), dist))
                                .fold(whole, |a, b| if b.matched || b.distance < a.distance { b } else { a }),
                            _ => whole,
                        }
                    }
                };
            }
            Self::XPath { expr, namespaces, sub } => {
                let Some(text) = actual else { return Eval::exact(false) };
                let Ok(result) = xml::eval_xpath(text, expr, namespaces) else { return Eval::exact(false) };
                return match sub {
                    None => Eval::exact(result.is_truthy()),
                    Some(sub) => {
                        let values = result.values();
                        if values.is_empty() {
                            return sub.eval_inner(None, dist);
                        }
                        let mut best = Eval::exact(false);
                        for item in values {
                            let mut candidates = vec![item.text.clone()];
                            if let Some(x) = &item.xml {
                                if matches!(**sub, Self::EqualToXml { .. }) {
                                    candidates.insert(0, x.clone());
                                } else {
                                    candidates.push(x.clone());
                                }
                            }
                            for c in candidates {
                                let e = sub.eval_inner(Some(&c), dist);
                                if e.matched {
                                    return e;
                                }
                                if e.distance < best.distance {
                                    best = e;
                                }
                            }
                        }
                        best
                    }
                };
            }
            _ => {}
        }
        let Some(actual) = actual else { return Eval::exact(false) };
        match self {
            Self::EqualTo { expected, ci } => {
                let matched = if *ci { expected.to_lowercase() == actual.to_lowercase() } else { expected == actual };
                if matched || !dist {
                    Eval::exact(matched)
                } else if *ci {
                    Eval { matched, distance: normalized_distance(&expected.to_lowercase(), &actual.to_lowercase()) }
                } else {
                    Eval { matched, distance: normalized_distance(expected, actual) }
                }
            }
            Self::Contains { expected, ci } => {
                Eval::exact(if *ci { actual.to_lowercase().contains(&expected.to_lowercase()) } else { actual.contains(expected.as_str()) })
            }
            Self::DoesNotContain { expected, ci } => {
                Eval::exact(if *ci { !actual.to_lowercase().contains(&expected.to_lowercase()) } else { !actual.contains(expected.as_str()) })
            }
            Self::Matches { re, .. } => Eval::exact(regex_matches(re, actual)),
            Self::DoesNotMatch { re, .. } => Eval::exact(!regex_matches(re, actual)),
            Self::EqualToJson { expected, ignore_array_order, ignore_extra } => {
                let Ok(doc) = serde_json::from_str::<Value>(actual) else { return Eval::exact(false) };
                let opts = JsonOpts { array_order: *ignore_array_order, extra: *ignore_extra };
                let matched = json_eq(expected, &doc, opts);
                if matched || !dist {
                    Eval::exact(matched)
                } else {
                    Eval { matched, distance: json_distance(expected, &doc, opts).max(0.01) }
                }
            }
            Self::JsonPath { path, sub: None } => {
                let Ok(doc) = serde_json::from_str::<Value>(actual) else { return Eval::exact(false) };
                let found = if path.is_definite() {
                    match path.eval(&doc).first() {
                        None | Some(Value::Null) => false,
                        Some(Value::Array(a)) => !a.is_empty(),
                        Some(_) => true,
                    }
                } else {
                    !path.eval(&doc).is_empty()
                };
                Eval::exact(found)
            }
            Self::EqualToXml { expected, placeholders, .. } => match xml::parse_tree(actual) {
                Ok(tree) => {
                    let matched = xml::xml_equal(expected, &tree, *placeholders);
                    if matched || !dist { Eval::exact(matched) } else { Eval { matched, distance: xml::xml_distance(expected, &tree).max(0.01) } }
                }
                Err(_) => Eval::exact(false),
            },
            Self::BinaryEqualTo(expected) => Eval::exact(expected.as_slice() == actual.as_bytes()),
            Self::Date { op, expected, actual_format, truncate_expected, truncate_actual } => {
                let now = Utc::now();
                let mut exp = match expected {
                    ExpectedDate::Now(off) => off.map_or(now, |o| dates::apply_offset(now, o)),
                    ExpectedDate::Fixed(d) => *d,
                };
                let parsed = match actual_format {
                    Some(f) => dates::parse_with_format(actual, f),
                    None => dates::parse_flexible(actual),
                };
                let Some(mut act) = parsed else { return Eval::exact(false) };
                if let Some(t) = truncate_expected {
                    exp = dates::truncate(exp, t).unwrap_or(exp);
                }
                if let Some(t) = truncate_actual {
                    act = dates::truncate(act, t).unwrap_or(act);
                }
                Eval::exact(match op {
                    DateOp::Before => act < exp,
                    DateOp::After => act > exp,
                    DateOp::Equal => act == exp,
                })
            }
            Self::Number { op, expected } => {
                let Ok(n) = actual.trim().trim_end_matches(['d', 'D', 'f', 'F']).parse::<f64>() else { return Eval::exact(false) };
                Eval::exact(match op {
                    NumOp::Eq => n == *expected,
                    NumOp::Gt => n > *expected,
                    NumOp::Gte => n >= *expected,
                    NumOp::Lt => n < *expected,
                    NumOp::Lte => n <= *expected,
                })
            }
            Self::Unsupported(_) => Eval::exact(false),
            Self::Absent | Self::Present | Self::Not(_) | Self::And(_) | Self::Or(_) | Self::JsonPath { .. } | Self::XPath { .. } => {
                unreachable!("handled above")
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct JsonOpts {
    array_order: bool,
    extra: bool,
}

const PLACEHOLDER: &str = "${json-unit.";

fn placeholder(expected: &Value) -> Option<(&str, &str)> {
    let s = expected.as_str()?.strip_prefix(PLACEHOLDER)?;
    let (name, rest) = s.split_once('}')?;
    Some((name, rest))
}

fn is_ignore(expected: &Value) -> bool {
    matches!(placeholder(expected), Some(("ignore" | "ignore-element", _)))
}

fn json_eq(expected: &Value, actual: &Value, o: JsonOpts) -> bool {
    if let Some((name, rest)) = placeholder(expected) {
        match name {
            "ignore" | "ignore-element" => return true,
            "any-string" => return actual.is_string(),
            "any-number" => return actual.is_number(),
            "any-boolean" => return actual.is_boolean(),
            "regex" => {
                let text = match actual {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => return false,
                };
                return java_regex(rest).is_ok_and(|re| regex_matches(&re, &text));
            }
            _ => {}
        }
    }
    match (expected, actual) {
        (Value::Number(a), Value::Number(b)) => a == b || a.as_f64() == b.as_f64(),
        (Value::Array(e), Value::Array(a)) => {
            if (!o.extra && e.len() != a.len()) || a.len() < e.len() {
                return false;
            }
            if !o.array_order {
                return e.iter().zip(a).all(|(x, y)| json_eq(x, y, o));
            }
            let mut used = vec![false; a.len()];
            e.iter().all(|x| {
                if let Some(i) = (0..a.len()).find(|&i| !used[i] && json_eq(x, &a[i], o)) {
                    used[i] = true;
                    true
                } else {
                    false
                }
            })
        }
        (Value::Object(e), Value::Object(a)) => {
            if !o.extra && a.keys().any(|k| !e.contains_key(k)) {
                return false;
            }
            e.iter().all(|(k, ev)| match a.get(k) {
                Some(av) => json_eq(ev, av, o),
                None => is_ignore(ev),
            })
        }
        _ => expected == actual,
    }
}

fn json_size(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(json_size).sum::<usize>(),
        Value::Object(o) => 1 + o.values().map(json_size).sum::<usize>(),
        _ => 1,
    }
}

fn json_diff(expected: &Value, actual: &Value, o: JsonOpts) -> usize {
    if json_eq(expected, actual, o) {
        return 0;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            let mut d = 0;
            for (k, ev) in e {
                d += a.get(k).map_or_else(|| json_size(ev), |av| json_diff(ev, av, o));
            }
            if !o.extra {
                d += a.iter().filter(|(k, _)| !e.contains_key(*k)).map(|(_, v)| json_size(v)).sum::<usize>();
            }
            d.max(1)
        }
        (Value::Array(e), Value::Array(a)) => {
            let mut d = 0;
            for (i, ev) in e.iter().enumerate() {
                d += a.get(i).map_or_else(|| json_size(ev), |av| json_diff(ev, av, o));
            }
            if !o.extra {
                d += a.iter().skip(e.len()).map(json_size).sum::<usize>();
            }
            d.max(1)
        }
        _ => json_size(expected),
    }
}

fn json_distance(expected: &Value, actual: &Value, o: JsonOpts) -> f64 {
    let total = json_size(expected).max(json_size(actual)).max(1);
    (json_diff(expected, actual, o) as f64 / total as f64).min(1.0)
}

/// A pattern applied to a multi-valued field (query parameter, header, cookie, form field).
#[derive(Debug, Clone)]
pub enum MultiPattern {
    /// Matches when any value matches (or, for `absent`, when there are no values).
    Single(ValuePattern),
    HasExactly(Vec<ValuePattern>),
    Includes(Vec<ValuePattern>),
}

impl MultiPattern {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let obj = v.as_object().ok_or_else(|| format!("matcher must be an object, got {v}"))?;
        for (key, ctor) in [("hasExactly", 0), ("includes", 1)] {
            if let Some(list) = obj.get(key) {
                let subs = list.as_array().ok_or_else(|| format!("{key} needs an array"))?.iter().map(ValuePattern::from_json).collect::<Result<Vec<_>, _>>()?;
                return Ok(if ctor == 0 { Self::HasExactly(subs) } else { Self::Includes(subs) });
            }
        }
        ValuePattern::from_json(v).map(Self::Single)
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Single(p) => p.describe(),
            Self::HasExactly(v) => format!("hasExactly({})", v.iter().map(ValuePattern::describe).collect::<Vec<_>>().join(", ")),
            Self::Includes(v) => format!("includes({})", v.iter().map(ValuePattern::describe).collect::<Vec<_>>().join(", ")),
        }
    }

    pub fn eval(&self, values: &[&str], dist: bool) -> Eval {
        let any = |p: &ValuePattern| values.iter().any(|v| p.is_match(Some(v)));
        match self {
            Self::HasExactly(subs) => Eval::exact(values.len() == subs.len() && subs.iter().all(any)),
            Self::Includes(subs) => Eval::exact(subs.iter().all(any)),
            Self::Single(p) => {
                if values.is_empty() {
                    return p.eval_inner(None, dist);
                }
                if p.is_absent() {
                    return Eval::exact(false);
                }
                let mut best = Eval::exact(false);
                for v in values {
                    let e = p.eval_inner(Some(v), dist);
                    if e.matched {
                        return e;
                    }
                    if e.distance < best.distance {
                        best = e;
                    }
                }
                best
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(v: Value) -> ValuePattern {
        ValuePattern::from_json(&v).unwrap()
    }

    fn m(v: Value, actual: &str) -> bool {
        p(v).is_match(Some(actual))
    }

    #[test]
    fn string_matchers() {
        assert!(m(json!({"equalTo": "abc"}), "abc"));
        assert!(!m(json!({"equalTo": "abc"}), "ABC"));
        assert!(m(json!({"equalTo": "abc", "caseInsensitive": true}), "ABC"));
        assert!(m(json!({"equalTo": 42}), "42"));
        assert!(m(json!({"contains": "ell"}), "hello"));
        assert!(m(json!({"doesNotContain": "xyz"}), "hello"));
        assert!(!m(json!({"doesNotContain": "ell"}), "hello"));
        assert!(m(json!({"matches": "[a-z]+-\\d{3}"}), "abc-123"));
        assert!(!m(json!({"matches": "[a-z]+"}), "abc-123"));
        assert!(m(json!({"doesNotMatch": "\\d+"}), "abc"));
        assert!(p(json!({"absent": true})).is_match(None));
        assert!(!m(json!({"absent": true}), "x"));
        assert!(!p(json!({"equalTo": "x"})).is_match(None));
        assert!(ValuePattern::from_json(&json!({"bogus": 1})).is_err());
        assert!(ValuePattern::from_json(&json!({"matches": "("})).is_err());
    }

    #[test]
    fn logical() {
        let and = json!({"and": [{"contains": "a"}, {"contains": "b"}]});
        assert!(m(and.clone(), "ab"));
        assert!(!m(and, "a"));
        assert!(m(json!({"or": [{"equalTo": "x"}, {"equalTo": "y"}]}), "y"));
        assert!(m(json!({"not": {"contains": "secret"}}), "public"));
        assert!(p(json!({"not": {"equalTo": "x"}})).is_match(None));
    }

    #[test]
    fn json_matchers() {
        let e = json!({"equalToJson": {"a": 1, "b": [1, 2]}});
        assert!(m(e.clone(), r#"{"b":[1,2],"a":1}"#));
        assert!(!m(e.clone(), r#"{"a":1,"b":[2,1]}"#));
        assert!(!m(e, r#"{"a":1,"b":[1,2],"c":3}"#));
        assert!(m(json!({"equalToJson": "{\"a\":1}", "ignoreExtraElements": true}), r#"{"a":1,"c":3}"#));
        assert!(m(json!({"equalToJson": {"b": [1, 2]}, "ignoreArrayOrder": true}), r#"{"b":[2,1]}"#));
        assert!(m(json!({"equalToJson": {"id": "${json-unit.any-string}", "n": "${json-unit.any-number}", "x": "${json-unit.ignore}"}}), r#"{"id":"abc","n":5,"x":[1]}"#));
        assert!(!m(json!({"equalToJson": {"id": "${json-unit.any-string}"}}), r#"{"id":5}"#));
        assert!(m(json!({"equalToJson": {"code": "${json-unit.regex}[A-Z]{3}"}}), r#"{"code":"ABC"}"#));
        assert!(!m(json!({"equalToJson": {"a": 1}}), "not json"));
        let near = p(json!({"equalToJson": {"a": 1, "b": 2, "c": 3}})).eval(Some(r#"{"a":1,"b":2,"c":4}"#));
        assert!(!near.matched && near.distance < 0.5);

        assert!(m(json!({"matchesJsonPath": "$.name"}), r#"{"name":"x"}"#));
        assert!(!m(json!({"matchesJsonPath": "$.name"}), r#"{"other":"x"}"#));
        assert!(!m(json!({"matchesJsonPath": "$.list"}), r#"{"list":[]}"#));
        assert!(m(json!({"matchesJsonPath": "$.things[?(@.n > 2)]"}), r#"{"things":[{"n":1},{"n":3}]}"#));
        assert!(m(json!({"matchesJsonPath": {"expression": "$.name", "equalTo": "bob"}}), r#"{"name":"bob"}"#));
        assert!(m(json!({"matchesJsonPath": {"expression": "$.tags", "contains": "b"}}), r#"{"tags":["a","b"]}"#));
        assert!(m(json!({"matchesJsonPath": {"expression": "$.tags[*]", "equalTo": "b"}}), r#"{"tags":["a","b"]}"#));
        assert!(m(json!({"matchesJsonPath": {"expression": "$.missing", "absent": true}}), r#"{"name":"bob"}"#));
        assert!(m(json!({"matchesJsonPath": {"expression": "$.o", "equalToJson": {"x": 1}}}), r#"{"o":{"x":1}}"#));
        assert!(m(json!({"matchesJsonPath": {"expression": "$.n", "greaterThanNumber": 5}}), r#"{"n":7}"#));
    }

    #[test]
    fn xml_matchers() {
        assert!(m(json!({"equalToXml": "<a><b>1</b></a>"}), "<a>\n  <b>1</b>\n</a>"));
        assert!(!m(json!({"equalToXml": "<a><b>1</b></a>"}), "<a><b>2</b></a>"));
        assert!(m(json!({"equalToXml": "<a><b>${xmlunit.ignore}</b></a>", "enablePlaceholders": true}), "<a><b>2</b></a>"));
        let xml = "<todo-list><todo-item>wash dishes</todo-item></todo-list>";
        assert!(m(json!({"matchesXPath": "/todo-list[count(todo-item) = 1]"}), xml));
        assert!(m(json!({"matchesXPath": "//todo-item"}), xml));
        assert!(!m(json!({"matchesXPath": "//nothing"}), xml));
        assert!(m(json!({"matchesXPath": {"expression": "//todo-item/text()", "contains": "wash"}}), xml));
        assert!(m(json!({"matchesXPath": {"expression": "//todo-item", "equalTo": "wash dishes"}}), xml));
        assert!(m(json!({"matchesXPath": {"expression": "//todo-item", "equalToXml": "<todo-item>wash dishes</todo-item>"}}), xml));
        assert!(m(
            json!({"matchesXPath": {"expression": "//s:Body/text()", "equalTo": "x"}, "xPathNamespaces": {"s": "urn:soap"}}),
            "<e:Env xmlns:e=\"urn:soap\"><e:Body>x</e:Body></e:Env>"
        ));
    }

    #[test]
    fn dates_and_numbers() {
        assert!(m(json!({"before": "2030-01-01T00:00:00Z"}), "2024-05-01T10:00:00Z"));
        assert!(m(json!({"after": "now -1 days"}), &Utc::now().to_rfc3339()));
        assert!(!m(json!({"after": "now +1 days"}), &Utc::now().to_rfc3339()));
        assert!(m(json!({"equalToDateTime": "2024-05-01T00:00:00Z", "actualFormat": "dd/MM/yyyy"}), "01/05/2024"));
        assert!(m(json!({"equalToDateTime": "2024-05-01T00:00:00Z", "truncateActual": "first day of month"}), "2024-05-17T13:00:00Z"));
        assert!(m(json!({"before": "now", "expectedOffset": 1, "expectedOffsetUnit": "days"}), &Utc::now().to_rfc3339()));
        assert!(!m(json!({"before": "2020-01-01T00:00:00Z"}), "garbage"));
        assert!(m(json!({"equalToNumber": 5}), "5.0"));
        assert!(m(json!({"greaterThanNumber": "5"}), "6"));
        assert!(m(json!({"lessThanEqualNumber": 5}), "5"));
        assert!(!m(json!({"lessThanNumber": 5}), "abc"));
    }

    #[test]
    fn binary_and_multi() {
        let b = p(json!({"binaryEqualTo": "AQID"}));
        assert!(b.eval_bytes(&[1, 2, 3], false).matched);
        assert!(!b.eval_bytes(&[1, 2], false).matched);
        let any = MultiPattern::from_json(&json!({"equalTo": "b"})).unwrap();
        assert!(any.eval(&["a", "b"], false).matched);
        assert!(!any.eval(&[], false).matched);
        let exact = MultiPattern::from_json(&json!({"hasExactly": [{"equalTo": "a"}, {"equalTo": "b"}]})).unwrap();
        assert!(exact.eval(&["b", "a"], false).matched);
        assert!(!exact.eval(&["a", "b", "c"], false).matched);
        let inc = MultiPattern::from_json(&json!({"includes": [{"equalTo": "a"}]})).unwrap();
        assert!(inc.eval(&["a", "z"], false).matched);
        assert!(!inc.eval(&["z"], false).matched);
        let absent = MultiPattern::from_json(&json!({"absent": true})).unwrap();
        assert!(absent.eval(&[], false).matched);
        assert!(!absent.eval(&["x"], false).matched);
        let unsupported = p(json!({"matchesJsonSchema": {"type": "object"}}));
        assert!(!unsupported.is_match(Some("{}")));
    }
}
