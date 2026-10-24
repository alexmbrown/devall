//! CyberChef-lite: linear recipes of registry ops, and "magic" detection that speculatively runs
//! likely decoders (a bounded breadth-first search) and ranks the resulting recipes.

use serde_json::{Value, json};

use crate::{Result, decode_base64, info, invalid, run_depth};

const MAX_RECIPE_DEPTH: usize = 3;
const MAX_STEPS: usize = 64;

/// Steps are `[{"op": "base64.decode", "args": {…}, "disabled": false}]`; a bare string is a step
/// without args.
pub fn run_recipe(input: &str, steps: &str, depth: usize) -> Result<String> {
    if depth >= MAX_RECIPE_DEPTH {
        return Err(invalid("recipes can only nest 3 levels deep"));
    }
    let steps: Value = if steps.trim().is_empty() { json!([]) } else { serde_json::from_str(steps).map_err(|e| invalid(format!("steps must be a JSON array: {e}")))? };
    let steps = steps.as_array().ok_or_else(|| invalid("steps must be a JSON array of {\"op\", \"args\"}"))?;
    if steps.len() > MAX_STEPS {
        return Err(invalid(format!("a recipe can have at most {MAX_STEPS} steps")));
    }
    let mut cur = input.to_string();
    for (i, step) in steps.iter().enumerate() {
        let (op, args) = match step {
            Value::String(s) => (s.as_str(), json!({})),
            Value::Object(o) => {
                if o.get("disabled").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let op = o.get("op").and_then(Value::as_str).ok_or_else(|| invalid(format!("step {} has no \"op\"", i + 1)))?;
                (op, o.get("args").cloned().unwrap_or_else(|| json!({})))
            }
            _ => return Err(invalid(format!("step {} must be an object like {{\"op\": \"base64.decode\"}}", i + 1))),
        };
        if info(op).is_none() {
            return Err(invalid(format!("step {}: unknown operation {op}", i + 1)));
        }
        cur = run_depth(op, &cur, &args, depth + 1).map_err(|e| invalid(format!("step {} ({op}): {e}", i + 1)))?;
    }
    Ok(cur)
}

// ── Magic ────────────────────────────────────────────────────────────────────────────────────────

const MAX_DEPTH: usize = 3;
const MAX_NODES: usize = 60;
const MAX_CANDIDATES: usize = 8;
const PREVIEW_CHARS: usize = 240;

#[derive(Clone)]
struct Node {
    steps: Vec<&'static str>,
    output: String,
    terminal: bool,
}

fn is_b64(s: &str) -> bool {
    let c: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let body = c.trim_end_matches('=');
    body.len() >= 8 && body.len() % 4 != 1 && body.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '-' | '_'))
}

fn hex_body(s: &str) -> Option<String> {
    let t = s.trim();
    let t = t.strip_prefix("0x").unwrap_or(t);
    let c: String = t.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
    (c.len() >= 4 && c.len().is_multiple_of(2) && c.chars().all(|ch| ch.is_ascii_hexdigit())).then_some(c)
}

fn has_pct(s: &str) -> bool {
    let b = s.as_bytes();
    b.windows(3).any(|w| w[0] == b'%' && w[1].is_ascii_hexdigit() && w[2].is_ascii_hexdigit())
}

fn has_entity(s: &str) -> bool {
    s.match_indices('&').any(|(i, _)| s[i..].find(';').is_some_and(|j| (2..12).contains(&j) && s[i + 1..i + j].chars().all(|c| c.is_ascii_alphanumeric() || c == '#')))
}

fn is_jwt(s: &str) -> bool {
    let t = s.trim().trim_start_matches("Bearer ").trim();
    let parts: Vec<&str> = t.split('.').collect();
    parts.len() == 3 && parts[0].starts_with("eyJ") && parts[1].starts_with("eyJ")
}

fn is_epoch(s: &str) -> bool {
    let t = s.trim();
    (t.len() == 10 || t.len() == 13) && t.chars().all(|c| c.is_ascii_digit()) && t[..10].parse::<u64>().is_ok_and(|n| (1_000_000_000..=9_999_999_999).contains(&n))
}

fn is_json_doc(s: &str) -> bool {
    let t = s.trim_start();
    (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<Value>(s).is_ok()
}

fn is_cron(s: &str) -> bool {
    let t = s.trim();
    let n = t.split_whitespace().count();
    (t.starts_with('@') || ((5..=7).contains(&n) && t.contains(['*', '/', '?']) && t.chars().all(|c| c.is_ascii_alphanumeric() || " */,-?#".contains(c))))
        && crate::cron::explain(t, "UTC", 1, chrono::Utc::now()).is_ok()
}

fn is_ip_or_cidr(s: &str) -> bool {
    let t = s.trim();
    !t.contains(char::is_whitespace) && t.len() <= 64 && (t.contains('.') || t.contains(':')) && crate::net::cidr(t, "").is_ok()
}

/// Ops worth trying on this text, most specific first. Terminal ops produce structured results.
fn candidates(s: &str) -> Vec<(&'static str, bool)> {
    let t = s.trim();
    let mut out = Vec::new();
    if t.is_empty() {
        return out;
    }
    if is_jwt(t) {
        out.push(("jwt.decode", true));
    }
    if is_epoch(t) || (t.len() >= 16 && t.as_bytes()[4] == b'-' && t.as_bytes()[7] == b'-' && t.as_bytes()[10] == b'T') {
        out.push(("time.convert", true));
    }
    if is_json_doc(t) {
        out.push(("json.format", true));
    }
    if t.contains("-----BEGIN CERTIFICATE-----") {
        out.push(("x509.decode", true));
    }
    if (t.starts_with("http://") || t.starts_with("https://")) && !t.contains(char::is_whitespace) {
        out.push(("url.parse", true));
    }
    if is_ip_or_cidr(t) {
        out.push(("cidr.calc", true));
    }
    if is_cron(t) {
        out.push(("cron.explain", true));
    }
    if is_b64(t) {
        match decode_base64(t) {
            Ok(b) if b.starts_with(&[0x1f, 0x8b]) || (b.len() > 2 && b[0] == 0x78 && matches!(b[1], 0x01 | 0x5e | 0x9c | 0xda)) => out.push(("gzip.decompress", false)),
            Ok(_) => out.push(("base64.decode", false)),
            Err(_) => {}
        }
    }
    if let Some(h) = hex_body(t) {
        out.push(if h.to_ascii_lowercase().starts_with("1f8b") { ("gzip.decompress", false) } else { ("hex.decode", false) });
    }
    if has_pct(t) {
        out.push(("url.decode", false));
    }
    if has_entity(t) {
        out.push(("html.decode", false));
    }
    if t.contains("\\n") || t.contains("\\\"") || t.contains("\\u") || t.contains("\\t") {
        out.push(("json.unescape", false));
    }
    out
}

struct Quality {
    printable: f64,
    replacement: f64,
    wordy: f64,
}

fn quality(s: &str) -> Quality {
    let total = s.chars().count().max(1) as f64;
    let mut printable = 0usize;
    let mut replacement = 0usize;
    let mut wordy = 0usize;
    for c in s.chars() {
        if c == '\u{fffd}' {
            replacement += 1;
        } else if !c.is_control() || matches!(c, '\n' | '\r' | '\t') {
            printable += 1;
        }
        if c.is_alphanumeric() || c == ' ' || ".,:;!?'\"-_/(){}[]@\n".contains(c) {
            wordy += 1;
        }
    }
    Quality { printable: printable as f64 / total, replacement: replacement as f64 / total, wordy: wordy as f64 / total }
}

fn score(n: &Node) -> (f64, Vec<String>) {
    let q = quality(&n.output);
    let mut reasons = Vec::new();
    let mut s = 60.0 * q.printable + 20.0 * q.wordy - 80.0 * q.replacement;
    if q.replacement > 0.0 {
        reasons.push(format!("{:.0}% invalid UTF-8", q.replacement * 100.0));
    } else {
        reasons.push("valid UTF-8".into());
    }
    reasons.push(format!("{:.0}% printable", q.printable * 100.0));
    if n.terminal {
        s += 25.0;
        reasons.push(format!("recognized format ({})", n.steps.last().copied().unwrap_or("")));
    } else if is_json_doc(&n.output) {
        s += 20.0;
        reasons.push("decodes to JSON".into());
    }
    if n.output.trim().chars().count() < 3 {
        s -= 30.0;
    }
    s -= 3.0 * n.steps.len() as f64;
    ((s * 10.0).round() / 10.0, reasons)
}

fn label(op: &str) -> &'static str {
    match op {
        "base64.decode" => "Base64",
        "hex.decode" => "Hex",
        "url.decode" => "URL-encoding",
        "html.decode" => "HTML entities",
        "json.unescape" => "JSON string escapes",
        "gzip.decompress" => "Gzip",
        "jwt.decode" => "JWT",
        "time.convert" => "Timestamp",
        "json.format" => "JSON",
        "x509.decode" => "X.509 certificate",
        "url.parse" => "URL",
        "cidr.calc" => "IP / CIDR",
        "cron.explain" => "Cron expression",
        _ => "?",
    }
}

pub fn detect(input: &str) -> Value {
    let mut frontier = vec![Node { steps: vec![], output: input.to_string(), terminal: false }];
    let mut found: Vec<Node> = Vec::new();
    let mut explored = 0usize;
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for node in frontier {
            for (op, terminal) in candidates(&node.output) {
                if explored >= MAX_NODES {
                    break;
                }
                explored += 1;
                let Ok(out) = run_depth(op, &node.output, &json!({}), 1) else { continue };
                if out == node.output || out.trim().is_empty() {
                    continue;
                }
                let mut steps = node.steps.clone();
                steps.push(op);
                let child = Node { steps, output: out, terminal };
                // Keep decoding only text that still looks like text.
                if !terminal && quality(&child.output).replacement < 0.05 {
                    next.push(child.clone());
                }
                found.push(child);
            }
        }
        frontier = next;
    }
    let mut ranked: Vec<(f64, Vec<String>, Node)> = found
        .into_iter()
        .map(|n| {
            let (s, r) = score(&n);
            (s, r, n)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.steps.len().cmp(&b.2.steps.len())));
    let mut seen = std::collections::HashSet::new();
    let candidates: Vec<Value> = ranked
        .into_iter()
        .filter(|(_, _, n)| seen.insert(n.output.clone()))
        .take(MAX_CANDIDATES)
        .map(|(score, reasons, n)| {
            let preview: String = n.output.chars().take(PREVIEW_CHARS).collect();
            json!({
                "recipe": n.steps.iter().map(|op| json!({ "op": op, "args": {} })).collect::<Vec<_>>(),
                "label": n.steps.iter().map(|op| label(op)).collect::<Vec<_>>().join(" → "),
                "score": score,
                "reasons": reasons,
                "preview": preview,
                "truncated": n.output.chars().count() > PREVIEW_CHARS,
            })
        })
        .collect();
    let q = quality(input);
    json!({
        "input": { "length": input.chars().count(), "printableRatio": (q.printable * 100.0).round() / 100.0 },
        "explored": explored,
        "candidates": candidates,
    })
}
