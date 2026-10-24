//! Query languages over JSON: jq (jaq), JSONPath (RFC 9535) and JMESPath.

use std::sync::mpsc;
use std::time::Duration;

use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, data};
use jaq_json::Val;
use serde_json::{Value, json};

use crate::{Result, invalid, parse_json, pretty};

/// jq programs can loop forever (`repeat(.)`, `range(1e18)`); give up waiting after this.
const JQ_TIMEOUT: Duration = Duration::from_secs(5);
const JQ_MAX_OUTPUTS: usize = 10_000;
const JQ_MAX_BYTES: usize = 32 << 20;

/// Runs a jq filter on every JSON value in `input` (empty input = `null`, like `jq -n`).
///
/// Evaluation happens on a worker thread so a runaway filter returns an error instead of
/// freezing the caller; the worker is abandoned (and keeps a core busy until it finishes).
pub fn jq(input: &str, filter: &str, raw: bool) -> Result<String> {
    let filter = if filter.trim().is_empty() { ".".to_string() } else { filter.to_string() };
    let input = input.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("devall-jq".into())
        .stack_size(32 << 20)
        .spawn(move || {
            let _ = tx.send(jq_blocking(&input, &filter, raw));
        })
        .map_err(|e| invalid(format!("cannot start jq: {e}")))?;
    rx.recv_timeout(JQ_TIMEOUT).unwrap_or_else(|_| Err(invalid("the jq filter ran for more than 5 s and was abandoned (infinite loop?)")))
}

fn offset_of(code: &str, rest: &str) -> usize {
    // Error positions are suffixes of the program text.
    (rest.as_ptr() as usize).saturating_sub(code.as_ptr() as usize).min(code.len())
}

fn near(code: &str, rest: &str) -> String {
    let at = offset_of(code, rest);
    let snippet: String = code[at..].chars().take(12).collect();
    if snippet.is_empty() { format!("at the end (position {at})") } else { format!("at position {at} near `{snippet}`") }
}

fn jq_blocking(input: &str, code: &str, raw: bool) -> Result<String> {
    let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs());
    let funs = jaq_core::funs().chain(jaq_std::funs()).chain(jaq_json::funs());
    let loader = Loader::new(defs);
    let arena = Arena::default();
    let modules = loader.load(&arena, File { code, path: () }).map_err(|errs| {
        let msgs: Vec<String> = errs
            .into_iter()
            .flat_map(|(_, e)| match e {
                jaq_core::load::Error::Io(v) => v.into_iter().map(|(p, m)| format!("cannot load {p}: {m}")).collect::<Vec<_>>(),
                jaq_core::load::Error::Lex(v) => v.into_iter().map(|(exp, rest)| format!("expected {} {}", exp.as_str(), near(code, rest))).collect(),
                jaq_core::load::Error::Parse(v) => v.into_iter().map(|(exp, rest)| format!("expected {} {}", exp.as_str(), near(code, rest))).collect(),
            })
            .collect();
        invalid(format!("jq syntax error: {}", msgs.join("; ")))
    })?;
    let filter = Compiler::default().with_funs(funs).compile(modules).map_err(|errs| {
        let msgs: Vec<String> = errs.into_iter().flat_map(|(_, v)| v.into_iter().map(|(name, undef)| format!("undefined {} `{name}`", undef.as_str()))).collect();
        invalid(format!("jq compile error: {}", msgs.join("; ")))
    })?;

    let values: Vec<Val> = if input.trim().is_empty() {
        vec![Val::Null]
    } else {
        jaq_json::read::parse_many(input.as_bytes()).collect::<std::result::Result<_, _>>().map_err(|e| invalid(format!("input is not valid JSON: {e}")))?
    };

    let mut out: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    'inputs: for v in values {
        let ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));
        for r in filter.id.run((ctx, v)) {
            match r {
                Ok(v) => {
                    let s = format_val(&v, raw);
                    bytes += s.len();
                    out.push(s);
                    if out.len() >= JQ_MAX_OUTPUTS || bytes > JQ_MAX_BYTES {
                        out.push(format!("… stopped after {} outputs", out.len()));
                        break 'inputs;
                    }
                }
                Err(exn) => match exn.get_err() {
                    Ok(e) => {
                        let after = if out.is_empty() { String::new() } else { format!(" (after {} output(s))", out.len()) };
                        return Err(invalid(format!("jq error: {e}{after}")));
                    }
                    // `halt` / `halt_error`: stop quietly with what we have.
                    Err(_) => break 'inputs,
                },
            }
        }
    }
    Ok(out.join("\n"))
}

fn format_val(v: &Val, raw: bool) -> String {
    if raw && let Val::TStr(_) | Val::BStr(_) = v {
        let quoted = v.to_string();
        if let Ok(s) = serde_json::from_str::<String>(&quoted) {
            return s;
        }
    }
    let compact = v.to_string();
    match serde_json::from_str::<Value>(&compact) {
        Ok(j) => pretty(&j),
        Err(_) => compact,
    }
}

pub fn jsonpath(input: &str, path: &str, output: &str) -> Result<Value> {
    let v = parse_json(input, "input")?;
    let path = if path.trim().is_empty() { "$" } else { path.trim() };
    let p = serde_json_path::JsonPath::parse(path).map_err(|e| invalid(format!("invalid JSONPath: {e}")))?;
    let nodes = p.query_located(&v);
    Ok(match output {
        "paths" => nodes.iter().map(|n| json!(n.location().to_string())).collect(),
        "both" => nodes.iter().map(|n| json!({ "path": n.location().to_string(), "pointer": n.location().to_json_pointer(), "value": n.node() })).collect(),
        _ => nodes.iter().map(|n| n.node().clone()).collect(),
    })
}

pub fn jmespath(input: &str, expression: &str) -> Result<Value> {
    let v = parse_json(input, "input")?;
    let expression = if expression.trim().is_empty() { "@" } else { expression.trim() };
    let expr = jmespath::compile(expression).map_err(|e| invalid(format!("invalid JMESPath: {e}")))?;
    let result = expr.search(&v).map_err(|e| invalid(format!("JMESPath error: {e}")))?;
    serde_json::to_value(&*result).map_err(|e| invalid(e.to_string()))
}
