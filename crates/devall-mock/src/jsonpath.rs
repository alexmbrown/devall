//! JSONPath with Jayway compatibility shims on top of RFC 9535 (`serde_json_path`).
//!
//! WireMock uses Jayway JsonPath. The common Jayway-only spellings are rewritten before parsing:
//! dashed member names (`$.first-name`), `.size()` / `.length()` (also inside filters),
//! `=~ /regex/`, `in [...]` / `nin [...]`, `empty true|false`, trailing aggregate functions
//! (`length`, `size`, `min`, `max`, `avg`, `sum`, `keys`) and filters applied directly to an object
//! root (`$[?(@.total > 5)]`). Other Jayway extensions (`subsetof`, `anyof`, regex flags…) are not
//! supported.

use std::sync::LazyLock;

use serde_json::Value;
use serde_json_path::JsonPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Aggregate {
    Length,
    Min,
    Max,
    Avg,
    Sum,
    Keys,
}

#[derive(Debug, Clone)]
pub struct JsonPathExpr {
    raw: String,
    path: JsonPath,
    definite: bool,
    aggregate: Option<Aggregate>,
    root_filter: bool,
}

static SIZE_IN_FILTER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(@(?:\.[\w$-]+|\[[^\]]*\])*)\.(?:size|length)\(\)"#).unwrap());
static REGEX_OP: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(@(?:\.[\w$-]+|\[[^\]]*\])*)\s*=~\s*/((?:\\/|[^/])*)/[a-zA-Z]*"#).unwrap());
static IN_OP: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(@(?:\.[\w$-]+|\[[^\]]*\])*)\s+(nin|in)\s+\[([^\]]*)\]"#).unwrap());
static EMPTY_OP: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(@(?:\.[\w$-]+|\[[^\]]*\])*)\s+empty\s+(true|false)"#).unwrap());
static TRAILING_FN: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"\.(length|size|min|max|avg|sum|keys)\(\)\s*$"#).unwrap());

impl JsonPathExpr {
    pub fn compile(expr: &str) -> Result<Self, String> {
        let raw = expr.trim().to_string();
        let mut text = raw.clone();
        if !text.starts_with('$') {
            text = if text.starts_with('[') { format!("${text}") } else { format!("$.{text}") };
        }
        let mut aggregate = None;
        if let Some(c) = TRAILING_FN.captures(&text) {
            aggregate = Some(match &c[1] {
                "length" | "size" => Aggregate::Length,
                "min" => Aggregate::Min,
                "max" => Aggregate::Max,
                "avg" => Aggregate::Avg,
                "sum" => Aggregate::Sum,
                _ => Aggregate::Keys,
            });
            let cut = c.get(0).map_or(text.len(), |m| m.start());
            text.truncate(cut);
            if text.is_empty() {
                text.push('$');
            }
        }
        let text = translate(&text);
        let root_filter = text.starts_with("$[?");
        let path = JsonPath::parse(&text).map_err(|e| format!("invalid JSONPath {raw:?}: {e}"))?;
        let definite = is_definite(&text);
        Ok(Self { raw, path, definite, aggregate, root_filter })
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// A definite path addresses at most one node (no wildcards, filters, slices or recursion).
    pub fn is_definite(&self) -> bool {
        self.definite || self.aggregate.is_some()
    }

    /// All matching nodes (cloned). Aggregates return a single computed value.
    pub fn eval(&self, doc: &Value) -> Vec<Value> {
        let wrapped;
        let target = if self.root_filter && doc.is_object() {
            wrapped = Value::Array(vec![doc.clone()]);
            &wrapped
        } else {
            doc
        };
        let nodes: Vec<Value> = self.path.query(target).all().into_iter().cloned().collect();
        match self.aggregate {
            None => nodes,
            Some(agg) => {
                let items: Vec<Value> = if self.definite {
                    match nodes.into_iter().next() {
                        Some(Value::Array(a)) => a,
                        Some(Value::Object(o)) if agg == Aggregate::Length || agg == Aggregate::Keys => {
                            if agg == Aggregate::Keys {
                                return vec![Value::Array(o.keys().map(|k| Value::String(k.clone())).collect())];
                            }
                            return vec![Value::from(o.len())];
                        }
                        Some(Value::String(s)) if agg == Aggregate::Length => return vec![Value::from(s.chars().count())],
                        Some(other) => vec![other],
                        None => return vec![],
                    }
                } else {
                    nodes
                };
                aggregate(agg, &items).into_iter().collect()
            }
        }
    }

    /// The value WireMock hands to a sub-matcher: the node itself for definite paths, the list of
    /// nodes otherwise. `None` when nothing matched.
    pub fn eval_value(&self, doc: &Value) -> Option<Value> {
        let nodes = self.eval(doc);
        if self.is_definite() {
            nodes.into_iter().next()
        } else if nodes.is_empty() {
            None
        } else {
            Some(Value::Array(nodes))
        }
    }
}

fn aggregate(agg: Aggregate, items: &[Value]) -> Option<Value> {
    let nums = || items.iter().filter_map(Value::as_f64);
    match agg {
        Aggregate::Length => Some(Value::from(items.len())),
        Aggregate::Keys => None,
        Aggregate::Min => nums().reduce(f64::min).map(num),
        Aggregate::Max => nums().reduce(f64::max).map(num),
        Aggregate::Sum => Some(num(nums().sum())),
        Aggregate::Avg => {
            let v: Vec<f64> = nums().collect();
            if v.is_empty() { None } else { Some(num(v.iter().sum::<f64>() / v.len() as f64)) }
        }
    }
}

fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
    }
}

fn is_definite(text: &str) -> bool {
    if text.contains("..") || text.contains('*') || text.contains("[?") {
        return false;
    }
    // Any bracket with a union or slice makes the path indefinite.
    let mut depth = 0;
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '[' => depth += 1,
                ']' => depth -= 1,
                ',' | ':' if depth > 0 => return false,
                _ => {}
            },
        }
    }
    true
}

/// Rewrites Jayway spellings into RFC 9535.
fn translate(text: &str) -> String {
    let s = SIZE_IN_FILTER.replace_all(text, |c: &regex::Captures| format!("length({})", &c[1]));
    let s = REGEX_OP.replace_all(&s, |c: &regex::Captures| format!("match({}, '{}')", &c[1], c[2].replace("\\/", "/").replace('\'', "\\'")));
    let s = IN_OP.replace_all(&s, |c: &regex::Captures| {
        let lhs = &c[1];
        let items: Vec<&str> = c[3].split(',').map(str::trim).filter(|x| !x.is_empty()).collect();
        if items.is_empty() {
            return if &c[2] == "in" { "false".into() } else { "true".into() };
        }
        if &c[2] == "in" {
            format!("({})", items.iter().map(|v| format!("{lhs} == {v}")).collect::<Vec<_>>().join(" || "))
        } else {
            format!("({})", items.iter().map(|v| format!("{lhs} != {v}")).collect::<Vec<_>>().join(" && "))
        }
    });
    let s = EMPTY_OP.replace_all(&s, |c: &regex::Captures| {
        if &c[2] == "true" { format!("length({}) == 0", &c[1]) } else { format!("length({}) > 0", &c[1]) }
    });
    bracket_dashed_members(&s)
}

/// `.first-name` → `['first-name']` (RFC 9535 shorthand names cannot contain `-` or start with a
/// digit); leaves quoted strings and decimal numbers alone.
fn bracket_dashed_members(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    let mut quote: Option<char> = None;
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '$';
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            out.push(c);
            i += 1;
            continue;
        }
        if c == '.' {
            // Decimal number? (the run of identifier chars before the dot is all digits)
            let mut j = out.len();
            let bytes = out.as_bytes();
            while j > 0 && (bytes[j - 1] as char).is_ascii_alphanumeric() {
                j -= 1;
            }
            let before = &out[j..];
            let numeric = !before.is_empty() && before.chars().all(|ch| ch.is_ascii_digit()) && (j == 0 || !matches!(bytes[j - 1], b'.' | b'$' | b'@' | b'_' | b'-'));
            if numeric {
                out.push(c);
                i += 1;
                continue;
            }
            let mut dots = 1;
            if chars.get(i + 1) == Some(&'.') {
                dots = 2;
            }
            let start = i + dots;
            let mut end = start;
            while end < chars.len() && is_ident(chars[end]) {
                end += 1;
            }
            let ident: String = chars[start..end].iter().collect();
            let needs_bracket = !ident.is_empty()
                && chars.get(end) != Some(&'(')
                && (ident.contains('-') || ident.contains('$') || ident.starts_with(|ch: char| ch.is_ascii_digit()));
            if needs_bracket {
                if dots == 2 {
                    out.push_str("..");
                }
                out.push_str(&format!("['{}']", ident.replace('\'', "\\'")));
            } else {
                for _ in 0..dots {
                    out.push('.');
                }
                out.push_str(&ident);
            }
            i = end;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval(expr: &str, doc: &Value) -> Vec<Value> {
        JsonPathExpr::compile(expr).unwrap().eval(doc)
    }

    #[test]
    fn basics() {
        let doc = json!({"name": "bob", "first-name": "Bob", "things": [{"id": 1, "kind": "a"}, {"id": 2, "kind": "b"}], "price": 10.5});
        assert_eq!(eval("$.name", &doc), vec![json!("bob")]);
        assert_eq!(eval("name", &doc), vec![json!("bob")]);
        assert_eq!(eval("$.first-name", &doc), vec![json!("Bob")]);
        assert_eq!(eval("$.things[?(@.id == 2)].kind", &doc), vec![json!("b")]);
        assert_eq!(eval("$.things[?(@.id > 1.5)].kind", &doc), vec![json!("b")]);
        assert_eq!(eval("$.things.length()", &doc), vec![json!(2)]);
        assert_eq!(eval("$.things.size()", &doc), vec![json!(2)]);
        assert_eq!(eval("$.things[*].id.max()", &doc), vec![json!(2)]);
        assert_eq!(eval("$[?(@.price > 10)].name", &doc), vec![json!("bob")]);
        assert!(eval("$[?(@.price > 100)]", &doc).is_empty());
        assert_eq!(eval("$.things[?(@.kind in ['a', 'c'])].id", &doc), vec![json!(1)]);
        assert_eq!(eval("$.things[?(@.kind =~ /b.*/)].id", &doc), vec![json!(2)]);
        assert_eq!(eval("$[?(@.things.size() == 2)].name", &doc), vec![json!("bob")]);
        assert!(JsonPathExpr::compile("$.things[?(@.id == 2)]").unwrap().eval_value(&doc).unwrap().is_array());
        assert!(JsonPathExpr::compile("$.name").unwrap().is_definite());
        assert!(!JsonPathExpr::compile("$..name").unwrap().is_definite());
        assert!(JsonPathExpr::compile("$[").is_err());
    }
}
