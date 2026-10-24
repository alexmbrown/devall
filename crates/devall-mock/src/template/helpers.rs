//! WireMock / Handlebars.java helpers. Each helper works inline, as a sub-expression (returning
//! typed JSON) and, where it makes sense, as a block (`{{#trim}} … {{/trim}}`,
//! `{{#eq a b}} … {{else}} … {{/eq}}`). Helper failures render as `[ERROR: …]` like WireMock.

use std::collections::BTreeMap;

use chrono::Utc;
use handlebars::{Context, Handlebars, Helper, HelperDef, HelperResult, Output, RenderContext, RenderError, Renderable, ScopedJson};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Map, Value, json};

use crate::dates;
use crate::jsonpath::JsonPathExpr;
use crate::util::{b64_decode, b64_encode, find_regex, java_regex, json_text, regex_matches};
use crate::xml;

type Res = Result<Value, String>;

pub(super) struct Args {
    pub(super) params: Vec<Value>,
    pub(super) hash: BTreeMap<String, Value>,
    block: Option<String>,
}

impl Args {
    pub(super) fn get(&self, i: usize) -> Option<&Value> {
        self.params.get(i).filter(|v| !v.is_null())
    }
    pub(super) fn str(&self, i: usize) -> Option<String> {
        self.get(i).map(json_text)
    }
    /// First positional argument, or the rendered block content for block usage.
    fn input(&self) -> Option<String> {
        self.block.clone().or_else(|| self.str(0))
    }
    fn h(&self, k: &str) -> Option<&Value> {
        self.hash.get(k).filter(|v| !v.is_null())
    }
    fn hs(&self, k: &str) -> Option<String> {
        self.h(k).map(json_text)
    }
    fn hb(&self, k: &str) -> bool {
        match self.h(k) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
            Some(Value::Number(n)) => n.as_f64() != Some(0.0),
            _ => false,
        }
    }
    fn hf(&self, k: &str) -> Option<f64> {
        self.h(k).and_then(num)
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn number_value(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 { json!(f as i64) } else { serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number) }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty() && s != "false",
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::Array(a) => !a.is_empty(),
        Value::Object(_) => true,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// Arrays render Java-style (`[a, b]`), objects as JSON.
    Value,
    /// Arrays and objects render as JSON.
    Json,
    /// Boolean result; as a block renders the main or inverse section.
    Cond,
}

pub(super) fn render_value(v: &Value, mode: Mode) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) if mode != Mode::Json => format!("[{}]", a.iter().map(|x| render_value(x, Mode::Json)).collect::<Vec<_>>().join(", ")),
        Value::Array(_) | Value::Object(_) => v.to_string(),
        other => other.to_string(),
    }
}

pub(super) fn collect(h: &Helper<'_>, block: Option<String>) -> Args {
    Args {
        params: h.params().iter().map(|p| p.value().clone()).collect(),
        hash: h.hash().iter().map(|(k, v)| ((*k).to_string(), v.value().clone())).collect(),
        block,
    }
}

struct Fx {
    f: fn(&Args) -> Res,
    mode: Mode,
}

pub(super) fn error_text(e: &str) -> String {
    format!("[ERROR: {e}]")
}

impl HelperDef for Fx {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        let v = (self.f)(&collect(h, None)).unwrap_or_else(|e| Value::String(error_text(&e)));
        Ok(ScopedJson::Derived(v))
    }

    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        r: &'reg Handlebars<'reg>,
        ctx: &'rc Context,
        rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        if let Some(t) = h.template() {
            if self.mode == Mode::Cond {
                let v = (self.f)(&collect(h, None)).unwrap_or(Value::Bool(false));
                let section = if truthy(&v) { Some(t) } else { h.inverse() };
                if let Some(s) = section {
                    s.render(r, ctx, rc, out)?;
                }
                return Ok(());
            }
            let inner = t.renders(r, ctx, rc)?;
            let text = match (self.f)(&collect(h, Some(inner))) {
                Ok(v) => render_value(&v, self.mode),
                Err(e) => error_text(&e),
            };
            out.write(&text)?;
            return Ok(());
        }
        let text = match (self.f)(&collect(h, None)) {
            Ok(v) => render_value(&v, self.mode),
            Err(e) => error_text(&e),
        };
        out.write(&text)?;
        Ok(())
    }
}

/// Helpers that may store their result in a template variable.
#[derive(Clone, Copy)]
enum VarKind {
    Assign,
    RegexExtract,
    FormData,
    ParseJson,
    Val,
}

struct VarHelper(VarKind);

fn assign_var(ctx: &Context, rc: &mut RenderContext<'_, '_>, name: &str, value: Value) {
    let mut data = rc.context().map_or_else(|| ctx.data().clone(), |c| c.data().clone());
    if let Value::Object(o) = &mut data {
        o.insert(name.to_string(), value);
    }
    rc.set_context(Context::from(data));
}

impl VarHelper {
    /// Returns (value to output or return, variable name to assign to).
    fn compute(&self, a: &Args) -> (Res, Option<String>) {
        match self.0 {
            VarKind::Assign => (Ok(Value::String(a.block.clone().or_else(|| a.str(1)).unwrap_or_default())), a.str(0)),
            VarKind::RegexExtract => {
                let input = a.str(0).unwrap_or_default();
                let Some(pattern) = a.str(1) else { return (Err("regexExtract needs a regex".into()), None) };
                let re = match find_regex(&pattern) {
                    Ok(r) => r,
                    Err(e) => return (Err(e), None),
                };
                let var = a.str(2);
                let caps = re.captures(&input).ok().flatten();
                let Some(caps) = caps else {
                    return (a.h("default").cloned().map_or_else(|| Err(format!("Nothing matched {pattern}")), Ok), None);
                };
                match var {
                    Some(name) => {
                        let groups: Vec<Value> = (1..caps.len()).map(|i| Value::String(caps.get(i).map_or("", |m| m.as_str()).to_string())).collect();
                        (Ok(Value::Array(groups)), Some(name))
                    }
                    None => (Ok(Value::String(caps.get(0).map_or("", |m| m.as_str()).to_string())), None),
                }
            }
            VarKind::FormData => {
                let text = a.str(0).unwrap_or_default();
                let decode = a.hb("urlDecode");
                let mut map: Map<String, Value> = Map::new();
                for part in text.split('&').filter(|p| !p.is_empty()) {
                    let (k, v) = part.split_once('=').unwrap_or((part, ""));
                    let dec = |s: &str| if decode { url_decode(s) } else { s.to_string() };
                    let (k, v) = (dec(k), dec(v));
                    match map.get_mut(&k) {
                        None => {
                            map.insert(k, Value::String(v));
                        }
                        Some(Value::Array(arr)) => arr.push(Value::String(v)),
                        Some(existing) => {
                            let first = existing.take();
                            *existing = json!([first, v]);
                        }
                    }
                }
                (Ok(Value::Object(map)), a.str(1))
            }
            VarKind::ParseJson => {
                let (text, var) = match &a.block {
                    Some(b) => (b.clone(), a.str(0)),
                    None => (a.str(0).unwrap_or_default(), a.str(1)),
                };
                (serde_json::from_str::<Value>(&text).map_err(|e| format!("invalid JSON: {e}")), var)
            }
            VarKind::Val => {
                let v = a.get(0).cloned().or_else(|| a.h("default").cloned()).or_else(|| a.h("or").cloned()).unwrap_or(Value::Null);
                (Ok(v), a.hs("assign"))
            }
        }
    }
}

impl HelperDef for VarHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        ctx: &'rc Context,
        rc: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        let (res, var) = self.compute(&collect(h, None));
        let v = res.unwrap_or_else(|e| Value::String(error_text(&e)));
        if let Some(name) = var {
            assign_var(ctx, rc, &name, v);
            return Ok(ScopedJson::Derived(Value::String(String::new())));
        }
        Ok(ScopedJson::Derived(v))
    }

    fn call<'reg: 'rc, 'rc>(
        &self,
        h: &Helper<'rc>,
        r: &'reg Handlebars<'reg>,
        ctx: &'rc Context,
        rc: &mut RenderContext<'reg, 'rc>,
        out: &mut dyn Output,
    ) -> HelperResult {
        let block = match h.template() {
            Some(t) => Some(t.renders(r, ctx, rc)?),
            None => None,
        };
        let (res, var) = self.compute(&collect(h, block));
        match (res, var) {
            (Ok(v), Some(name)) => assign_var(ctx, rc, &name, v),
            (Ok(v), None) => out.write(&render_value(&v, Mode::Json))?,
            (Err(e), _) => out.write(&error_text(&e))?,
        }
        Ok(())
    }
}

fn url_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(&s.replace('+', " ")).decode_utf8_lossy().into_owned()
}

// ---------------------------------------------------------------- string helpers

fn s_upper(a: &Args) -> Res {
    Ok(Value::String(a.input().unwrap_or_default().to_uppercase()))
}
fn s_lower(a: &Args) -> Res {
    Ok(Value::String(a.input().unwrap_or_default().to_lowercase()))
}
fn capitalize_word(w: &str, rest_lower: bool) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &if rest_lower { c.as_str().to_lowercase() } else { c.as_str().to_string() },
        None => String::new(),
    }
}
fn s_capitalize(a: &Args) -> Res {
    let fully = a.hb("fully");
    let text = a.input().unwrap_or_default();
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            out.push_str(&capitalize_word(&word, fully));
            word.clear();
            out.push(ch);
        } else {
            word.push(ch);
        }
    }
    out.push_str(&capitalize_word(&word, fully));
    Ok(Value::String(out))
}
fn s_capitalize_first(a: &Args) -> Res {
    Ok(Value::String(capitalize_word(&a.input().unwrap_or_default(), false)))
}
fn s_trim(a: &Args) -> Res {
    Ok(Value::String(a.input().unwrap_or_default().trim().to_string()))
}
fn s_abbreviate(a: &Args) -> Res {
    let text = a.str(0).unwrap_or_default();
    let n = a.get(1).and_then(num).unwrap_or(0.0).max(4.0) as usize;
    if text.chars().count() <= n {
        return Ok(Value::String(text));
    }
    Ok(Value::String(format!("{}...", text.chars().take(n - 3).collect::<String>())))
}
fn pad(a: &Args, align: char) -> Res {
    let text = a.str(0).unwrap_or_default();
    let size = a.hf("size").or_else(|| a.get(1).and_then(num)).unwrap_or(0.0) as usize;
    let p = a.hs("pad").and_then(|p| p.chars().next()).unwrap_or(' ');
    let len = text.chars().count();
    if len >= size {
        return Ok(Value::String(text));
    }
    let total = size - len;
    let (l, r) = match align {
        'l' => (0, total),
        'r' => (total, 0),
        _ => (total / 2, total - total / 2),
    };
    Ok(Value::String(format!("{}{text}{}", p.to_string().repeat(l), p.to_string().repeat(r))))
}
fn s_center(a: &Args) -> Res {
    pad(a, 'c')
}
fn s_ljust(a: &Args) -> Res {
    pad(a, 'l')
}
fn s_rjust(a: &Args) -> Res {
    pad(a, 'r')
}
fn s_cut(a: &Args) -> Res {
    let text = a.str(0).unwrap_or_default();
    let what = a.str(1).unwrap_or_else(|| " ".into());
    Ok(Value::String(text.replace(&what, "")))
}
fn s_default_if_empty(a: &Args) -> Res {
    let v = a.get(0).cloned().unwrap_or(Value::Null);
    if truthy(&v) { Ok(v) } else { Ok(a.get(1).cloned().unwrap_or(Value::String(String::new()))) }
}
fn s_replace(a: &Args) -> Res {
    let text = a.input().unwrap_or_default();
    let (from, to) = if a.block.is_some() { (a.str(0), a.str(1)) } else { (a.str(1), a.str(2)) };
    Ok(Value::String(text.replace(&from.unwrap_or_default(), &to.unwrap_or_default())))
}
fn s_substring(a: &Args) -> Res {
    let chars: Vec<char> = a.str(0).unwrap_or_default().chars().collect();
    let start = (a.get(1).and_then(num).unwrap_or(0.0).max(0.0) as usize).min(chars.len());
    let end = a.get(2).and_then(num).map_or(chars.len(), |e| (e.max(0.0) as usize).min(chars.len())).max(start);
    Ok(Value::String(chars[start..end].iter().collect()))
}
fn s_strip_tags(a: &Args) -> Res {
    static TAGS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"<[^>]*>").unwrap());
    Ok(Value::String(TAGS.replace_all(&a.input().unwrap_or_default(), "").into_owned()))
}
fn s_slugify(a: &Args) -> Res {
    Ok(Value::String(devall_core::fsutil::slugify(&a.input().unwrap_or_default())))
}
fn s_word_wrap(a: &Args) -> Res {
    let text = a.str(0).unwrap_or_default();
    let width = a.get(1).and_then(num).unwrap_or(80.0).max(1.0) as usize;
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    Ok(Value::String(lines.join("\n")))
}
fn s_yesno(a: &Args) -> Res {
    let v = a.get(0);
    let out = match v {
        None => a.hs("maybe").unwrap_or_else(|| "maybe".into()),
        Some(v) if truthy(v) => a.hs("yes").unwrap_or_else(|| "yes".into()),
        Some(_) => a.hs("no").unwrap_or_else(|| "no".into()),
    };
    Ok(Value::String(out))
}
fn s_split(a: &Args) -> Res {
    let text = a.str(0).unwrap_or_default();
    let sep = a.str(1).unwrap_or_else(|| ",".into());
    Ok(Value::Array(text.split(sep.as_str()).map(|s| Value::String(s.to_string())).collect()))
}
fn s_string_format(a: &Args) -> Res {
    let fmt = a.str(0).unwrap_or_default();
    let args: Vec<&Value> = a.params.iter().skip(1).collect();
    let mut out = String::new();
    let mut idx = 0;
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut spec = String::new();
        while j < chars.len() && !chars[j].is_ascii_alphabetic() && chars[j] != '%' {
            spec.push(chars[j]);
            j += 1;
        }
        let Some(&conv) = chars.get(j) else { break };
        match conv {
            '%' => out.push('%'),
            'n' => out.push('\n'),
            's' | 'S' => {
                let s = args.get(idx).map(|v| render_value(v, Mode::Value)).unwrap_or_default();
                idx += 1;
                out.push_str(&if conv == 'S' { s.to_uppercase() } else { s });
            }
            'd' => {
                let n = args.get(idx).and_then(|v| num(v)).unwrap_or(0.0);
                idx += 1;
                out.push_str(&format!("{}", n as i64));
            }
            'f' => {
                let n = args.get(idx).and_then(|v| num(v)).unwrap_or(0.0);
                idx += 1;
                let prec = spec.split('.').nth(1).and_then(|p| p.parse::<usize>().ok()).unwrap_or(6);
                out.push_str(&format!("{n:.prec$}"));
            }
            other => {
                out.push('%');
                out.push_str(&spec);
                out.push(other);
            }
        }
        i = j + 1;
    }
    Ok(Value::String(out))
}

// ---------------------------------------------------------------- encoding

fn e_base64(a: &Args) -> Res {
    let text = a.input().unwrap_or_default();
    if a.hb("decode") {
        let bytes = b64_decode(&text).ok_or("invalid base64")?;
        return Ok(Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    }
    let mut enc = b64_encode(text.as_bytes());
    if a.h("padding").is_some() && !a.hb("padding") {
        enc = enc.trim_end_matches('=').to_string();
    }
    Ok(Value::String(enc))
}
fn e_url_encode(a: &Args) -> Res {
    let text = a.input().unwrap_or_default();
    if a.hb("decode") {
        return Ok(Value::String(url_decode(&text)));
    }
    // Java URLEncoder: space → '+', keeps `*-._`.
    let enc = utf8_percent_encode(&text, NON_ALPHANUMERIC).to_string().replace("%20", "+").replace("%2A", "*").replace("%2D", "-").replace("%2E", ".").replace("%5F", "_");
    Ok(Value::String(enc))
}

// ---------------------------------------------------------------- collections & numbers

fn list_args(a: &Args, skip: usize) -> Vec<Value> {
    let rest: Vec<Value> = a.params.iter().skip(skip).cloned().collect();
    if rest.len() == 1
        && let Value::Array(items) = &rest[0]
    {
        return items.clone();
    }
    rest
}
fn c_size(a: &Args) -> Res {
    Ok(json!(match a.get(0) {
        Some(Value::Array(v)) => v.len(),
        Some(Value::Object(o)) => o.len(),
        Some(Value::String(s)) => s.chars().count(),
        Some(_) => 1,
        None => 0,
    }))
}
fn c_join(a: &Args) -> Res {
    let sep = a.str(0).unwrap_or_else(|| ",".into());
    let items = list_args(a, 1);
    let body = items.iter().map(|v| render_value(v, Mode::Json)).collect::<Vec<_>>().join(&sep);
    Ok(Value::String(format!("{}{body}{}", a.hs("prefix").unwrap_or_default(), a.hs("suffix").unwrap_or_default())))
}
fn c_array(a: &Args) -> Res {
    Ok(Value::Array(a.params.clone()))
}
fn c_range(a: &Args) -> Res {
    let start = a.get(0).and_then(num).unwrap_or(0.0) as i64;
    let end = a.get(1).and_then(num).unwrap_or(start as f64) as i64;
    if (end - start).abs() > 100_000 {
        return Err("range too large".into());
    }
    let v: Vec<Value> = if start <= end { (start..=end).map(Value::from).collect() } else { (end..=start).rev().map(Value::from).collect() };
    Ok(Value::Array(v))
}
fn c_pick_random(a: &Args) -> Res {
    let mut items = list_args(a, 0);
    if items.is_empty() {
        return Err("pickRandom needs at least one value".into());
    }
    match a.hf("count") {
        Some(c) => {
            fastrand::shuffle(&mut items);
            items.truncate((c.max(0.0) as usize).min(items.len()));
            Ok(Value::Array(items))
        }
        None => Ok(items[fastrand::usize(..items.len())].clone()),
    }
}
fn c_array_add(a: &Args) -> Res {
    let mut items = match a.get(0) {
        Some(Value::Array(v)) => v.clone(),
        _ => Vec::new(),
    };
    let item = a.get(1).cloned().unwrap_or(Value::Null);
    match a.h("position") {
        Some(Value::String(s)) if s == "start" => items.insert(0, item),
        Some(Value::String(s)) if s == "end" => items.push(item),
        Some(p) => {
            let i = num(p).unwrap_or(items.len() as f64).max(0.0) as usize;
            items.insert(i.min(items.len()), item);
        }
        None => items.push(item),
    }
    Ok(Value::Array(items))
}
fn c_array_remove(a: &Args) -> Res {
    let mut items = match a.get(0) {
        Some(Value::Array(v)) => v.clone(),
        _ => Vec::new(),
    };
    if items.is_empty() {
        return Ok(Value::Array(items));
    }
    let i = match a.h("position") {
        Some(Value::String(s)) if s == "start" => 0,
        Some(Value::String(s)) if s == "end" => items.len() - 1,
        Some(p) => num(p).unwrap_or(0.0).max(0.0) as usize,
        None => items.len() - 1,
    };
    if i < items.len() {
        items.remove(i);
    }
    Ok(Value::Array(items))
}
fn n_math(a: &Args) -> Res {
    let x = a.get(0).and_then(num).ok_or("math: left operand is not a number")?;
    let op = a.str(1).ok_or("math: missing operator")?;
    let y = a.get(2).and_then(num).ok_or("math: right operand is not a number")?;
    let r = match op.as_str() {
        "+" => x + y,
        "-" => x - y,
        "*" => x * y,
        "/" => {
            if y == 0.0 {
                return Err("division by zero".into());
            }
            x / y
        }
        "%" => {
            if y == 0.0 {
                return Err("division by zero".into());
            }
            x % y
        }
        other => return Err(format!("unknown operator {other}")),
    };
    Ok(number_value(r))
}
fn n_random_int(a: &Args) -> Res {
    let lo = a.hf("lower").map_or(i64::from(i32::MIN), |v| v as i64);
    let hi = a.hf("upper").map_or(i64::from(i32::MAX), |v| v as i64);
    if hi < lo {
        return Err("upper must be greater than lower".into());
    }
    Ok(json!(fastrand::i64(lo..=hi)))
}
fn n_random_decimal(a: &Args) -> Res {
    let lo = a.hf("lower").unwrap_or(f64::from(i32::MIN));
    let hi = a.hf("upper").unwrap_or(f64::from(i32::MAX));
    if hi < lo {
        return Err("upper must be greater than lower".into());
    }
    let v = lo + fastrand::f64() * (hi - lo);
    Ok(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number))
}
fn n_random_value(a: &Args) -> Res {
    let kind = a.hs("type").unwrap_or_else(|| "ALPHANUMERIC".into()).to_ascii_uppercase();
    let len = a.hf("length").unwrap_or(36.0).clamp(0.0, 100_000.0) as usize;
    let pool: &str = match kind.as_str() {
        "UUID" => return Ok(Value::String(uuid::Uuid::new_v4().to_string())),
        "ALPHANUMERIC" => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
        "ALPHABETIC" => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        "NUMERIC" => "0123456789",
        "HEXADECIMAL" => "0123456789abcdef",
        "ALPHANUMERIC_AND_SYMBOLS" => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!#$%&()*+,-./:;<=>?@[]^_{|}~",
        other => return Err(format!("unknown randomValue type {other}")),
    };
    let chars: Vec<char> = pool.chars().collect();
    let s: String = (0..len).map(|_| chars[fastrand::usize(..chars.len())]).collect();
    Ok(Value::String(if a.hb("uppercase") { s.to_uppercase() } else { s }))
}
fn n_random(a: &Args) -> Res {
    let key = a.str(0).ok_or("random needs a faker key, e.g. 'Name.firstName'")?;
    super::faker::fake_value(&key).map(Value::String).ok_or_else(|| format!("unknown faker key {key}"))
}

// ---------------------------------------------------------------- comparison

fn cmp_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (num_strict(a), num_strict(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y),
        _ => Some(json_text(a).cmp(&json_text(b))),
    }
}
fn num_strict(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if !s.trim().is_empty() => s.trim().parse().ok(),
        _ => None,
    }
}
fn eq_values(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        _ => cmp_values(a, b) == Some(std::cmp::Ordering::Equal),
    }
}
fn arg(a: &Args, i: usize) -> Value {
    a.params.get(i).cloned().unwrap_or(Value::Null)
}
fn c_eq(a: &Args) -> Res {
    Ok(Value::Bool(eq_values(&arg(a, 0), &arg(a, 1))))
}
fn c_neq(a: &Args) -> Res {
    Ok(Value::Bool(!eq_values(&arg(a, 0), &arg(a, 1))))
}
fn c_gt(a: &Args) -> Res {
    Ok(Value::Bool(cmp_values(&arg(a, 0), &arg(a, 1)) == Some(std::cmp::Ordering::Greater)))
}
fn c_gte(a: &Args) -> Res {
    Ok(Value::Bool(matches!(cmp_values(&arg(a, 0), &arg(a, 1)), Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal))))
}
fn c_lt(a: &Args) -> Res {
    Ok(Value::Bool(cmp_values(&arg(a, 0), &arg(a, 1)) == Some(std::cmp::Ordering::Less)))
}
fn c_lte(a: &Args) -> Res {
    Ok(Value::Bool(matches!(cmp_values(&arg(a, 0), &arg(a, 1)), Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal))))
}
fn c_and(a: &Args) -> Res {
    Ok(Value::Bool(!a.params.is_empty() && a.params.iter().all(truthy)))
}
fn c_or(a: &Args) -> Res {
    Ok(Value::Bool(a.params.iter().any(truthy)))
}
fn c_not(a: &Args) -> Res {
    Ok(Value::Bool(!truthy(&arg(a, 0))))
}
fn c_contains(a: &Args) -> Res {
    let needle = arg(a, 1);
    Ok(Value::Bool(match arg(a, 0) {
        Value::Array(items) => items.iter().any(|i| eq_values(i, &needle)),
        Value::Object(o) => o.contains_key(&json_text(&needle)),
        Value::Null => false,
        other => json_text(&other).contains(&json_text(&needle)),
    }))
}
fn c_matches(a: &Args) -> Res {
    let text = a.str(0).unwrap_or_default();
    let re = java_regex(&a.str(1).unwrap_or_default())?;
    Ok(Value::Bool(regex_matches(&re, &text)))
}
fn c_is_odd(a: &Args) -> Res {
    Ok(Value::Bool(a.get(0).and_then(num).is_some_and(|n| (n as i64) % 2 != 0)))
}
fn c_is_even(a: &Args) -> Res {
    Ok(Value::Bool(a.get(0).and_then(num).is_some_and(|n| (n as i64) % 2 == 0)))
}

// ---------------------------------------------------------------- JSON & XML

fn as_json(v: Option<&Value>) -> Result<Value, String> {
    match v {
        Some(Value::String(s)) => serde_json::from_str(s).map_err(|e| format!("invalid JSON: {e}")),
        Some(other) => Ok(other.clone()),
        None => Err("missing JSON input".into()),
    }
}
fn j_json_path(a: &Args) -> Res {
    let (doc, expr) = match &a.block {
        Some(b) => (as_json(Some(&Value::String(b.clone()))), a.str(0)),
        None => (as_json(a.get(0)), a.str(1)),
    };
    let expr = expr.ok_or("jsonPath needs an expression")?;
    let default = a.h("default").cloned();
    let doc = match doc {
        Ok(d) => d,
        Err(e) => return default.ok_or(e),
    };
    let path = JsonPathExpr::compile(&expr)?;
    match path.eval_value(&doc) {
        Some(Value::Null) | None => default.map_or_else(|| Ok(Value::String(String::new())), Ok),
        Some(v) => Ok(v),
    }
}
fn j_to_json(a: &Args) -> Res {
    Ok(Value::String(a.get(0).cloned().unwrap_or(Value::Null).to_string()))
}
fn j_format_json(a: &Args) -> Res {
    let input = match &a.block {
        Some(b) => as_json(Some(&Value::String(b.clone())))?,
        None => as_json(a.get(0))?,
    };
    let compact = a.hs("format").is_some_and(|f| f.eq_ignore_ascii_case("compact"));
    Ok(Value::String(if compact { input.to_string() } else { serde_json::to_string_pretty(&input).unwrap_or_default() }))
}
fn merge(base: &mut Value, other: &Value) {
    match (base, other) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => merge(existing, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}
fn j_merge(a: &Args) -> Res {
    let mut base = as_json(a.get(0))?;
    let other = match &a.block {
        Some(b) => as_json(Some(&Value::String(b.clone())))?,
        None => as_json(a.get(1))?,
    };
    merge(&mut base, &other);
    Ok(base)
}
fn j_array_add(a: &Args) -> Res {
    let mut doc = as_json(a.get(0))?;
    let item = match &a.block {
        Some(b) => as_json(Some(&Value::String(b.clone())))?,
        None => as_json(a.get(1)).or_else(|_| a.get(1).cloned().ok_or("missing item"))?,
    };
    let target: &mut Value = match a.hs("jsonPath") {
        Some(p) => {
            let compiled = serde_json_path::JsonPath::parse(&p).map_err(|e| e.to_string())?;
            let pointer = compiled.query_located(&doc).all().first().map(|n| n.location().to_json_pointer()).ok_or("jsonPath matched nothing")?;
            doc.pointer_mut(&pointer).ok_or("jsonPath target vanished")?
        }
        None => &mut doc,
    };
    let arr = target.as_array_mut().ok_or("target is not an array")?;
    match (item, a.hb("flatten")) {
        (Value::Array(items), true) => arr.extend(items),
        (item, _) => arr.push(item),
    }
    if let Some(max) = a.hf("maxItems") {
        let max = max.max(0.0) as usize;
        if arr.len() > max {
            let excess = arr.len() - max;
            arr.drain(..excess);
        }
    }
    Ok(doc)
}
fn j_remove(a: &Args) -> Res {
    let mut doc = as_json(a.get(0))?;
    let expr = a.str(1).ok_or("jsonRemove needs a JSONPath")?;
    let compiled = serde_json_path::JsonPath::parse(&expr).map_err(|e| e.to_string())?;
    let mut pointers: Vec<String> = compiled.query_located(&doc).all().iter().map(|n| n.location().to_json_pointer()).collect();
    pointers.sort();
    for p in pointers.iter().rev() {
        let (parent, last) = p.rsplit_once('/').unwrap_or(("", p));
        let last = last.replace("~1", "/").replace("~0", "~");
        match doc.pointer_mut(parent) {
            Some(Value::Object(o)) => {
                o.remove(&last);
            }
            Some(Value::Array(arr)) => {
                if let Ok(i) = last.parse::<usize>()
                    && i < arr.len()
                {
                    arr.remove(i);
                }
            }
            _ => {}
        }
    }
    Ok(doc)
}
fn x_xpath(a: &Args) -> Res {
    let xml_text = a.str(0).unwrap_or_default();
    let expr = a.str(1).ok_or("xPath needs an expression")?;
    let res = xml::eval_xpath(&xml_text, &expr, &[])?;
    let values: Vec<Value> = res.values().into_iter().map(|i| Value::String(i.xml.unwrap_or(i.text))).collect();
    Ok(match values.len() {
        0 => Value::String(String::new()),
        1 => values.into_iter().next().unwrap_or(Value::Null),
        _ => Value::Array(values),
    })
}
fn x_format_xml(a: &Args) -> Res {
    Ok(Value::String(xml::pretty(&a.input().unwrap_or_default())?))
}

// ---------------------------------------------------------------- dates

fn date_input(v: Option<&Value>) -> Option<chrono::DateTime<Utc>> {
    match v? {
        Value::Number(n) => n.as_i64().and_then(dates::from_epoch_guess),
        other => dates::parse_flexible(&json_text(other)),
    }
}
fn finish_date(a: &Args, mut dt: chrono::DateTime<Utc>, format: Option<String>) -> Res {
    if let Some(off) = a.hs("offset") {
        let off = dates::parse_offset(&off).ok_or_else(|| format!("invalid offset {off:?}"))?;
        dt = dates::apply_offset(dt, off);
    }
    let zone = match a.hs("timezone") {
        Some(z) => Some(dates::parse_zone(&z).ok_or_else(|| format!("unknown timezone {z:?}"))?),
        None => None,
    };
    let format = format.or_else(|| a.hs("format"));
    let s = dates::format_date(dt, format.as_deref(), zone);
    if matches!(format.as_deref(), Some("epoch" | "unix")) {
        return Ok(s.parse::<i64>().map_or(Value::String(s), Value::from));
    }
    Ok(Value::String(s))
}
fn d_now(a: &Args) -> Res {
    finish_date(a, Utc::now(), None)
}
fn d_date(a: &Args) -> Res {
    let dt = date_input(a.get(0)).ok_or("date: cannot parse input date")?;
    finish_date(a, dt, a.str(1))
}
fn d_parse_date(a: &Args) -> Res {
    let text = a.str(0).ok_or("parseDate needs a date")?;
    let dt = match a.hs("format") {
        Some(f) => dates::parse_with_format(&text, &f),
        None => dates::parse_flexible(&text),
    }
    .ok_or_else(|| format!("cannot parse date {text:?}"))?;
    Ok(Value::String(dates::format_date(dt, None, None)))
}
fn d_truncate(a: &Args) -> Res {
    let dt = date_input(a.get(0)).ok_or("truncateDate: cannot parse input date")?;
    let spec = a.str(1).ok_or("truncateDate needs a truncation")?;
    let t = dates::truncate(dt, &spec).ok_or_else(|| format!("unknown truncation {spec:?}"))?;
    Ok(Value::String(dates::format_date(t, None, None)))
}

fn misc_hostname(_: &Args) -> Res {
    Ok(Value::String(std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "localhost".into())))
}
fn misc_system_value(_: &Args) -> Res {
    Err("systemValue is disabled in /dev/all".into())
}

type HelperEntry = (&'static str, fn(&Args) -> Res, Mode);

pub(super) fn register(hb: &mut Handlebars<'static>) {
    let fx: &[HelperEntry] = &[
        ("upper", s_upper, Mode::Value),
        ("lower", s_lower, Mode::Value),
        ("capitalize", s_capitalize, Mode::Value),
        ("capitalizeFirst", s_capitalize_first, Mode::Value),
        ("trim", s_trim, Mode::Value),
        ("abbreviate", s_abbreviate, Mode::Value),
        ("center", s_center, Mode::Value),
        ("ljust", s_ljust, Mode::Value),
        ("rjust", s_rjust, Mode::Value),
        ("cut", s_cut, Mode::Value),
        ("defaultIfEmpty", s_default_if_empty, Mode::Value),
        ("replace", s_replace, Mode::Value),
        ("substring", s_substring, Mode::Value),
        ("stripTags", s_strip_tags, Mode::Value),
        ("slugify", s_slugify, Mode::Value),
        ("wordWrap", s_word_wrap, Mode::Value),
        ("yesno", s_yesno, Mode::Value),
        ("split", s_split, Mode::Value),
        ("stringFormat", s_string_format, Mode::Value),
        ("base64", e_base64, Mode::Value),
        ("urlEncode", e_url_encode, Mode::Value),
        ("size", c_size, Mode::Value),
        ("join", c_join, Mode::Value),
        ("arrayJoin", c_join, Mode::Value),
        ("array", c_array, Mode::Value),
        ("range", c_range, Mode::Value),
        ("pickRandom", c_pick_random, Mode::Value),
        ("arrayAdd", c_array_add, Mode::Value),
        ("arrayRemove", c_array_remove, Mode::Value),
        ("math", n_math, Mode::Value),
        ("randomInt", n_random_int, Mode::Value),
        ("randomDecimal", n_random_decimal, Mode::Value),
        ("randomValue", n_random_value, Mode::Value),
        ("random", n_random, Mode::Value),
        ("eq", c_eq, Mode::Cond),
        ("neq", c_neq, Mode::Cond),
        ("ne", c_neq, Mode::Cond),
        ("gt", c_gt, Mode::Cond),
        ("gte", c_gte, Mode::Cond),
        ("lt", c_lt, Mode::Cond),
        ("lte", c_lte, Mode::Cond),
        ("and", c_and, Mode::Cond),
        ("or", c_or, Mode::Cond),
        ("not", c_not, Mode::Cond),
        ("contains", c_contains, Mode::Cond),
        ("matches", c_matches, Mode::Cond),
        ("isOdd", c_is_odd, Mode::Cond),
        ("isEven", c_is_even, Mode::Cond),
        ("jsonPath", j_json_path, Mode::Json),
        ("toJson", j_to_json, Mode::Value),
        ("formatJson", j_format_json, Mode::Value),
        ("jsonMerge", j_merge, Mode::Json),
        ("jsonArrayAdd", j_array_add, Mode::Json),
        ("jsonRemove", j_remove, Mode::Json),
        ("xPath", x_xpath, Mode::Value),
        ("formatXml", x_format_xml, Mode::Value),
        ("now", d_now, Mode::Value),
        ("date", d_date, Mode::Value),
        ("dateFormat", d_date, Mode::Value),
        ("parseDate", d_parse_date, Mode::Value),
        ("truncateDate", d_truncate, Mode::Value),
        ("hostname", misc_hostname, Mode::Value),
        ("systemValue", misc_system_value, Mode::Value),
    ];
    for (name, f, mode) in fx {
        hb.register_helper(name, Box::new(Fx { f: *f, mode: *mode }));
    }
    for (name, kind) in [
        ("assign", VarKind::Assign),
        ("regexExtract", VarKind::RegexExtract),
        ("formData", VarKind::FormData),
        ("parseJson", VarKind::ParseJson),
        ("val", VarKind::Val),
    ] {
        hb.register_helper(name, Box::new(VarHelper(kind)));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{TemplateEngine, model};
    use crate::request::MockRequest;
    use serde_json::json;

    fn r(tpl: &str) -> String {
        let req = MockRequest::new(
            "POST",
            "/things/7?q=Hello%20World",
            vec![("Content-Type".into(), "application/json".into()), ("X-Date".into(), "2024-03-05T07:08:09Z".into())],
            r#"{"name":"bob smith","items":[{"id":1,"price":2.5},{"id":2,"price":4}],"tags":["a","b","c"],"nested":{"x":1}}"#,
        );
        TemplateEngine::new().render(tpl, &model(&req, &[], &json!({}))).unwrap()
    }

    #[test]
    fn strings() {
        assert_eq!(r("{{upper 'abc'}} {{lower 'ABC'}}"), "ABC abc");
        assert_eq!(r("{{capitalize 'hello big world'}}"), "Hello Big World");
        assert_eq!(r("{{capitalizeFirst 'hello world'}}"), "Hello world");
        assert_eq!(r("[{{trim '  x  '}}]"), "[x]");
        assert_eq!(r("[{{#trim}}   block   {{/trim}}]"), "[block]");
        assert_eq!(r("{{abbreviate 'Truncate this text' 10}}"), "Truncat...");
        assert_eq!(r("{{replace 'a-b-c' '-' '+'}}"), "a+b+c");
        assert_eq!(r("{{substring 'abcdef' 1 3}}"), "bc");
        assert_eq!(r("{{defaultIfEmpty request.query.nope 'fallback'}}"), "fallback");
        assert_eq!(r("{{stringFormat '%s is %d years (%.1f)' 'Ann' 30 1.25}}"), "Ann is 30 years (1.2)");
        assert_eq!(r("{{rjust '7' size=3 pad='0'}}"), "007");
        assert_eq!(r("{{yesno true}}/{{yesno false yes='Y' no='N'}}"), "yes/N");
        assert_eq!(r("{{join ',' (split 'a;b' ';')}}"), "a,b");
        assert_eq!(r("{{stripTags '<b>bold</b>'}}"), "bold");
    }

    #[test]
    fn encoding() {
        assert_eq!(r("{{base64 'hello'}}"), "aGVsbG8=");
        assert_eq!(r("{{base64 'hello' padding=false}}"), "aGVsbG8");
        assert_eq!(r("{{base64 'aGVsbG8=' decode=true}}"), "hello");
        assert_eq!(r("{{#base64}}hi{{/base64}}"), "aGk=");
        assert_eq!(r("{{urlEncode 'a b&c'}}"), "a+b%26c");
        assert_eq!(r("{{urlEncode 'a+b%26c' decode=true}}"), "a b&c");
        assert_eq!(r("{{request.query.q}}"), "Hello World");
    }

    #[test]
    fn json_and_collections() {
        assert_eq!(r("{{jsonPath request.body '$.name'}}"), "bob smith");
        assert_eq!(r("{{jsonPath request.body '$.nested'}}"), r#"{"x":1}"#);
        assert_eq!(r("{{jsonPath request.body '$.missing' default='none'}}"), "none");
        assert_eq!(r("{{#each (jsonPath request.body '$.items') as |item|}}{{item.id}};{{/each}}"), "1;2;");
        assert_eq!(r("{{size (jsonPath request.body '$.tags')}}"), "3");
        assert_eq!(r("{{join '-' (jsonPath request.body '$.tags')}}"), "a-b-c");
        assert_eq!(r("{{arrayJoin ',' (array 'x' 'y') prefix='[' suffix=']'}}"), "[x,y]");
        assert_eq!(r("{{array 1 'two' true}}"), "[1, two, true]");
        assert_eq!(r("{{range 1 4}}"), "[1, 2, 3, 4]");
        assert_eq!(r("{{math 1 '+' 2}} {{math 10 '/' 4}} {{math 7 '%' 3}}"), "3 2.5 1");
        assert_eq!(r("{{toJson (array 1 2)}}"), "[1,2]");
        assert_eq!(r("{{#parseJson 'obj'}}{\"k\":\"v\"}{{/parseJson}}{{obj.k}}"), "v");
        assert_eq!(r("{{parseJson '{\"a\":[1,2]}' 'p'}}{{p.a.[1]}}"), "2");
        assert_eq!(r("{{formatJson '{\"a\":1}' format='compact'}}"), r#"{"a":1}"#);
        assert_eq!(r("{{jsonMerge '{\"a\":1}' '{\"b\":2}'}}"), r#"{"a":1,"b":2}"#);
        assert_eq!(r("{{jsonArrayAdd '[1,2]' '3'}}"), "[1,2,3]");
        assert_eq!(r("{{jsonRemove '{\"a\":1,\"b\":2}' '$.b'}}"), r#"{"a":1}"#);
        assert_eq!(r("{{arrayAdd (array 1 2) 0 position='start'}}"), "[0, 1, 2]");
        assert_eq!(r("{{arrayRemove (array 1 2 3) position=1}}"), "[1, 3]");
        let pick = r("{{pickRandom 'a' 'b' 'c'}}");
        assert!(["a", "b", "c"].contains(&pick.as_str()));
        assert_eq!(r("{{size (pickRandom (array 1 2 3 4) count=2)}}"), "2");
        assert_eq!(r("{{val request.query.nope default='d'}}"), "d");
        assert_eq!(r("{{val 'v' assign='x'}}{{x}}"), "v");
        assert_eq!(r("{{#assign 'greeting'}}Hi {{upper 'there'}}{{/assign}}{{greeting}}!"), "Hi THERE!");
    }

    #[test]
    fn regex_and_forms() {
        assert_eq!(r("{{regexExtract 'abc-123-xyz' '[0-9]+'}}"), "123");
        assert_eq!(r("{{regexExtract 'abc-123-XYZ' '([a-z]+)-([0-9]+)-([A-Z]+)' 'parts'}}{{parts.0}}|{{parts.2}}"), "abc|XYZ");
        assert_eq!(r("{{regexExtract 'abc' '[0-9]+' default='none'}}"), "none");
        assert!(r("{{regexExtract 'abc' '[0-9]+'}}").starts_with("[ERROR:"));
        assert_eq!(r("{{formData 'a=1&b=hello%20there&a=2' 'form' urlDecode=true}}{{form.b}} {{form.a.[1]}}"), "hello there 2");
        assert_eq!(r("{{#if (matches 'abc123' '[a-z]+[0-9]+')}}yes{{/if}}"), "yes");
        assert_eq!(r("{{#matches 'abc' '\\d+'}}yes{{else}}no{{/matches}}"), "no");
        assert_eq!(r("{{#contains (array 'a' 'b') 'b'}}has{{/contains}}"), "has");
        assert_eq!(r("{{contains 'hello' 'ell'}}"), "true");
    }

    #[test]
    fn conditionals() {
        assert_eq!(r("{{#eq request.method 'POST'}}post{{else}}other{{/eq}}"), "post");
        assert_eq!(r("{{#neq request.method 'POST'}}x{{else}}y{{/neq}}"), "y");
        assert_eq!(r("{{#if (eq request.path.[1] '7')}}seven{{/if}}"), "seven");
        assert_eq!(r("{{#gt 10 9}}big{{/gt}}{{#lt 1 2}} small{{/lt}}"), "big small");
        assert_eq!(r("{{#if (gte 2 '2')}}ok{{/if}}"), "ok");
        assert_eq!(r("{{#and true 1}}A{{/and}}{{#or false 0}}B{{else}}C{{/or}}{{#not false}}D{{/not}}"), "ACD");
        assert_eq!(r("{{#each (range 1 3)}}{{#if (isOdd this)}}o{{else}}e{{/if}}{{/each}}"), "oeo");
    }

    #[test]
    fn randomness() {
        let v = r("{{randomValue length=12 type='ALPHANUMERIC'}}");
        assert_eq!(v.len(), 12);
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric()));
        let u = r("{{randomValue type='UUID'}}");
        assert!(uuid::Uuid::parse_str(&u).is_ok());
        let h = r("{{randomValue length=8 type='HEXADECIMAL' uppercase=true}}");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
        let n: String = r("{{randomValue length=5 type='NUMERIC'}}");
        assert!(n.chars().all(|c| c.is_ascii_digit()));
        let i: i64 = r("{{randomInt lower=5 upper=9}}").parse().unwrap();
        assert!((5..=9).contains(&i));
        let d: f64 = r("{{randomDecimal lower=1.5 upper=2.5}}").parse().unwrap();
        assert!((1.5..=2.5).contains(&d));
        assert!(!r("{{random 'Name.firstName'}}").is_empty());
        assert!(r("{{random 'Internet.emailAddress'}}").contains('@'));
        assert!(r("{{random 'Nope.nothing'}}").starts_with("[ERROR:"));
    }

    #[test]
    fn dates() {
        let now = chrono::Utc::now();
        let year = r("{{now format='yyyy'}}");
        assert_eq!(year, now.format("%Y").to_string());
        let epoch: i64 = r("{{now format='epoch'}}").parse().unwrap();
        assert!((epoch - now.timestamp_millis()).abs() < 60_000);
        let later = r("{{now offset='3 days' format='yyyy-MM-dd'}}");
        assert_eq!(later, (now + chrono::Duration::days(3)).format("%Y-%m-%d").to_string());
        assert_eq!(r("{{date (parseDate request.headers.X-Date) offset='1 days' format='yyyy-MM-dd'}}"), "2024-03-06");
        assert_eq!(r("{{parseDate '05/03/2024' format='dd/MM/yyyy'}}"), "2024-03-05T00:00:00Z");
        assert_eq!(r("{{date '2024-03-05T07:08:09Z' timezone='Australia/Sydney' format='HH:mm'}}"), "18:08");
        assert_eq!(r("{{truncateDate '2024-03-05T07:08:09Z' 'first day of month'}}"), "2024-03-01T00:00:00Z");
        assert!(r("{{now offset='banana'}}").starts_with("[ERROR:"));
    }

    #[test]
    fn xml_helpers() {
        assert_eq!(r("{{xPath '<a><b>hi</b></a>' '/a/b/text()'}}"), "hi");
        assert_eq!(r("{{xPath '<a><b>hi</b></a>' '/a/b'}}"), "<b>hi</b>");
        assert!(r("{{formatXml '<a><b>1</b></a>'}}").contains("\n  <b>1</b>"));
    }
}
