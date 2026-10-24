//! Text diff (line / word / char, via `similar`) and semantic JSON diff with RFC 6902 patch output.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Map, Value, json};
use similar::{Algorithm, ChangeTag, DiffTag, TextDiff};

use crate::{Result, invalid, parse_json};

/// Diffs give up refining (and approximate) after this long on pathological inputs.
const DIFF_TIMEOUT: Duration = Duration::from_secs(2);

fn tag_name(t: ChangeTag) -> &'static str {
    match t {
        ChangeTag::Equal => "equal",
        ChangeTag::Delete => "delete",
        ChangeTag::Insert => "insert",
    }
}

pub fn text(old: &str, new: &str, mode: &str, context: usize) -> Result<Value> {
    let mut cfg = TextDiff::configure();
    cfg.algorithm(Algorithm::Myers).timeout(DIFF_TIMEOUT);
    match mode {
        "word" | "char" => {
            let diff = if mode == "word" { cfg.diff_words(old, new) } else { cfg.diff_chars(old, new) };
            let mut segments: Vec<(ChangeTag, String)> = Vec::new();
            let (mut ins, mut del) = (0usize, 0usize);
            for c in diff.iter_all_changes() {
                match c.tag() {
                    ChangeTag::Insert => ins += 1,
                    ChangeTag::Delete => del += 1,
                    ChangeTag::Equal => {}
                }
                match segments.last_mut() {
                    Some((t, s)) if *t == c.tag() => s.push_str(c.value()),
                    _ => segments.push((c.tag(), c.value().to_string())),
                }
            }
            // git --word-diff style rendering.
            let rendered: String = segments
                .iter()
                .map(|(t, s)| match t {
                    ChangeTag::Equal => s.clone(),
                    ChangeTag::Delete => format!("[-{s}-]"),
                    ChangeTag::Insert => format!("{{+{s}+}}"),
                })
                .collect();
            Ok(json!({
                "mode": mode,
                "identical": ins == 0 && del == 0,
                "similarity": diff.ratio(),
                "stats": { "inserted": ins, "deleted": del, "unit": if mode == "word" { "words" } else { "characters" } },
                "unified": rendered,
                "segments": segments.iter().map(|(t, s)| json!({ "tag": tag_name(*t), "value": s })).collect::<Vec<_>>(),
            }))
        }
        _ => {
            let diff = cfg.diff_lines(old, new);
            let unified = diff.unified_diff().context_radius(context).header("a", "b").to_string();
            let (mut ins, mut del) = (0usize, 0usize);
            let mut hunks = Vec::new();
            for group in diff.grouped_ops(context) {
                if group.is_empty() {
                    continue;
                }
                let (first, last) = (&group[0], &group[group.len() - 1]);
                let (os, oe) = (first.old_range().start, last.old_range().end);
                let (ns, ne) = (first.new_range().start, last.new_range().end);
                let mut changes = Vec::new();
                for op in &group {
                    for c in diff.iter_inline_changes(op) {
                        match c.tag() {
                            ChangeTag::Insert => ins += 1,
                            ChangeTag::Delete => del += 1,
                            ChangeTag::Equal => {}
                        }
                        let value: String = c.iter_strings_lossy().map(|(_, s)| s.into_owned()).collect();
                        let mut ch = json!({
                            "tag": tag_name(c.tag()),
                            "oldLine": c.old_index().map(|i| i + 1),
                            "newLine": c.new_index().map(|i| i + 1),
                            "value": value.strip_suffix('\n').map(|s| s.strip_suffix('\r').unwrap_or(s)).unwrap_or(&value),
                        });
                        if c.tag() != ChangeTag::Equal && c.iter_strings_lossy().any(|(emph, _)| emph) {
                            ch["segments"] = c.iter_strings_lossy().map(|(e, s)| json!({ "emphasized": e, "value": s.trim_end_matches(['\n', '\r']) })).collect();
                        }
                        changes.push(ch);
                    }
                }
                hunks.push(json!({
                    "header": format!("@@ -{},{} +{},{} @@", if oe > os { os + 1 } else { os }, oe - os, if ne > ns { ns + 1 } else { ns }, ne - ns),
                    "oldStart": os + 1,
                    "oldLines": oe - os,
                    "newStart": ns + 1,
                    "newLines": ne - ns,
                    "changes": changes,
                }));
            }
            Ok(json!({
                "mode": "line",
                "identical": ins == 0 && del == 0,
                "similarity": diff.ratio(),
                "stats": { "inserted": ins, "deleted": del, "unit": "lines" },
                "unified": unified,
                "hunks": hunks,
            }))
        }
    }
}

// ── Semantic JSON diff ───────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Seg {
    Key(String),
    Index(usize),
}

fn pointer(path: &[Seg]) -> String {
    path.iter()
        .map(|s| match s {
            Seg::Key(k) => format!("/{}", k.replace('~', "~0").replace('/', "~1")),
            Seg::Index(i) => format!("/{i}"),
        })
        .collect()
}

fn display_path(path: &[Seg]) -> String {
    let mut s = String::from("$");
    for seg in path {
        match seg {
            Seg::Index(i) => s.push_str(&format!("[{i}]")),
            Seg::Key(k) if !k.is_empty() && k.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') && k.chars().all(|c| c.is_alphanumeric() || c == '_') => {
                s.push('.');
                s.push_str(k);
            }
            Seg::Key(k) => s.push_str(&format!("['{}']", k.replace('\\', "\\\\").replace('\'', "\\'"))),
        }
    }
    s
}

fn push(path: &[Seg], seg: Seg) -> Vec<Seg> {
    let mut p = path.to_vec();
    p.push(seg);
    p
}

/// Canonical text form: sorted keys, numbers normalized, arrays sorted when order is ignored.
fn canon(v: &Value, unordered: bool) -> String {
    match v {
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => format!("{}", f as i64),
            Some(f) => format!("{f}"),
            None => n.to_string(),
        },
        Value::Array(a) => {
            let mut items: Vec<String> = a.iter().map(|x| canon(x, unordered)).collect();
            if unordered {
                items.sort();
            }
            format!("[{}]", items.join(","))
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys.iter().map(|k| format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), canon(&m[*k], unordered))).collect();
            format!("{{{}}}", parts.join(","))
        }
        other => other.to_string(),
    }
}

fn same(a: &Value, b: &Value, unordered: bool) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w, unordered))),
        (Value::Array(x), Value::Array(y)) if !unordered => x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w, false)),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && canon(a, true) == canon(b, true),
        _ => a == b,
    }
}

/// Ordered alignment of two arrays (LCS on canonical forms).
fn align(a: &[Value], b: &[Value]) -> Vec<(DiffTag, std::ops::Range<usize>, std::ops::Range<usize>)> {
    let ka: Vec<String> = a.iter().map(|v| canon(v, false)).collect();
    let kb: Vec<String> = b.iter().map(|v| canon(v, false)).collect();
    let deadline = std::time::Instant::now() + DIFF_TIMEOUT;
    similar::capture_diff_slices_deadline(Algorithm::Myers, &ka, &kb, Some(deadline)).iter().map(|op| op.as_tag_tuple()).collect()
}

/// Unordered (multiset) matching: exact matches first, then elements with the same identity key
/// (`id`, `_id`, `key`, `name`, `uuid`) are paired as "changed".
struct SetMatch {
    pairs: Vec<(usize, usize)>,
    removed: Vec<usize>,
    added: Vec<usize>,
}

const IDENTITY_KEYS: [&str; 5] = ["id", "_id", "key", "name", "uuid"];

fn match_unordered(a: &[Value], b: &[Value]) -> SetMatch {
    let mut by_canon: HashMap<String, Vec<usize>> = HashMap::new();
    for (j, v) in b.iter().enumerate().rev() {
        by_canon.entry(canon(v, true)).or_default().push(j);
    }
    let mut used_b = vec![false; b.len()];
    let mut left: Vec<usize> = Vec::new();
    for (i, v) in a.iter().enumerate() {
        match by_canon.get_mut(&canon(v, true)).and_then(Vec::pop) {
            Some(j) => used_b[j] = true,
            None => left.push(i),
        }
    }
    let mut pairs = Vec::new();
    let mut removed = Vec::new();
    for i in left {
        let partner = a[i].as_object().and_then(|ao| {
            IDENTITY_KEYS.iter().find_map(|k| {
                let id = ao.get(*k)?;
                (0..b.len()).find(|&j| !used_b[j] && b[j].as_object().and_then(|bo| bo.get(*k)).is_some_and(|x| same(x, id, true)))
            })
        });
        match partner {
            Some(j) => {
                used_b[j] = true;
                pairs.push((i, j));
            }
            None => removed.push(i),
        }
    }
    let added = (0..b.len()).filter(|&j| !used_b[j]).collect();
    SetMatch { pairs, removed, added }
}

struct Walker {
    unordered: bool,
    diffs: Vec<Value>,
}

impl Walker {
    fn record(&mut self, op: &str, path: &[Seg], old: Option<&Value>, new: Option<&Value>) {
        let mut d = json!({ "op": op, "path": display_path(path), "pointer": pointer(path) });
        if let Some(o) = old {
            d["old"] = o.clone();
        }
        if let Some(n) = new {
            d["new"] = n.clone();
        }
        self.diffs.push(d);
    }

    fn walk(&mut self, a: &Value, b: &Value, path: &[Seg]) {
        if same(a, b, self.unordered) {
            return;
        }
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                for (k, v) in x {
                    match y.get(k) {
                        Some(w) => self.walk(v, w, &push(path, Seg::Key(k.clone()))),
                        None => self.record("removed", &push(path, Seg::Key(k.clone())), Some(v), None),
                    }
                }
                for (k, w) in y {
                    if !x.contains_key(k) {
                        self.record("added", &push(path, Seg::Key(k.clone())), None, Some(w));
                    }
                }
            }
            (Value::Array(x), Value::Array(y)) if self.unordered => {
                let m = match_unordered(x, y);
                for (i, j) in &m.pairs {
                    self.walk(&x[*i], &y[*j], &push(path, Seg::Index(*i)));
                }
                for i in &m.removed {
                    self.record("removed", &push(path, Seg::Index(*i)), Some(&x[*i]), None);
                }
                for j in &m.added {
                    self.record("added", &push(path, Seg::Index(*j)), None, Some(&y[*j]));
                }
            }
            (Value::Array(x), Value::Array(y)) => {
                for (tag, or, nr) in align(x, y) {
                    match tag {
                        DiffTag::Equal => {}
                        DiffTag::Delete => or.for_each(|i| self.record("removed", &push(path, Seg::Index(i)), Some(&x[i]), None)),
                        DiffTag::Insert => nr.for_each(|j| self.record("added", &push(path, Seg::Index(j)), None, Some(&y[j]))),
                        DiffTag::Replace => {
                            let n = or.len().min(nr.len());
                            for k in 0..n {
                                self.walk(&x[or.start + k], &y[nr.start + k], &push(path, Seg::Index(or.start + k)));
                            }
                            for (i, v) in x.iter().enumerate().take(or.end).skip(or.start + n) {
                                self.record("removed", &push(path, Seg::Index(i)), Some(v), None);
                            }
                            for (j, w) in y.iter().enumerate().take(nr.end).skip(nr.start + n) {
                                self.record("added", &push(path, Seg::Index(j)), None, Some(w));
                            }
                        }
                    }
                }
            }
            _ => self.record("changed", path, Some(a), Some(b)),
        }
    }
}

/// RFC 6902 operations turning `a` into `b`. Array ops run from the end backwards so earlier
/// indices stay valid while later ones are edited.
fn patch(a: &Value, b: &Value, path: &[Seg], unordered: bool, out: &mut Vec<Value>) {
    if same(a, b, unordered) {
        return;
    }
    let op = |name: &str, p: &[Seg], v: Option<&Value>| {
        let mut o = json!({ "op": name, "path": pointer(p) });
        if let Some(v) = v {
            o["value"] = v.clone();
        }
        o
    };
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => patch(v, w, &push(path, Seg::Key(k.clone())), unordered, out),
                    None => out.push(op("remove", &push(path, Seg::Key(k.clone())), None)),
                }
            }
            for (k, w) in y {
                if !x.contains_key(k) {
                    out.push(op("add", &push(path, Seg::Key(k.clone())), Some(w)));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if unordered => {
            let m = match_unordered(x, y);
            for (i, j) in &m.pairs {
                patch(&x[*i], &y[*j], &push(path, Seg::Index(*i)), unordered, out);
            }
            for i in m.removed.iter().rev() {
                out.push(op("remove", &push(path, Seg::Index(*i)), None));
            }
            for j in &m.added {
                out.push(op("add", &push(path, Seg::Key("-".into())), Some(&y[*j])));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            for (tag, or, nr) in align(x, y).into_iter().rev() {
                match tag {
                    DiffTag::Equal => {}
                    DiffTag::Delete => {
                        for i in or.rev() {
                            out.push(op("remove", &push(path, Seg::Index(i)), None));
                        }
                    }
                    DiffTag::Insert => {
                        for (k, j) in nr.enumerate() {
                            out.push(op("add", &push(path, Seg::Index(or.start + k)), Some(&y[j])));
                        }
                    }
                    DiffTag::Replace => {
                        let n = or.len().min(nr.len());
                        for i in (or.start + n..or.end).rev() {
                            out.push(op("remove", &push(path, Seg::Index(i)), None));
                        }
                        for k in 0..n {
                            patch(&x[or.start + k], &y[nr.start + k], &push(path, Seg::Index(or.start + k)), unordered, out);
                        }
                        for k in n..nr.len() {
                            out.push(op("add", &push(path, Seg::Index(or.start + k)), Some(&y[nr.start + k])));
                        }
                    }
                }
            }
        }
        _ => out.push(op("replace", path, Some(b))),
    }
}

pub fn json(left: &str, right: &str, unordered: bool) -> Result<Value> {
    if right.trim().is_empty() {
        return Err(invalid("paste the changed JSON into \"Changed JSON\""));
    }
    let a = parse_json(left, "the original")?;
    let b = parse_json(right, "the changed document")?;
    let mut w = Walker { unordered, diffs: Vec::new() };
    w.walk(&a, &b, &[]);
    let mut ops = Vec::new();
    patch(&a, &b, &[], unordered, &mut ops);
    let count = |op: &str| w.diffs.iter().filter(|d| d["op"] == op).count();
    let summary = json!({ "added": count("added"), "removed": count("removed"), "changed": count("changed") });
    let mut out = Map::new();
    out.insert("equal".into(), json!(w.diffs.is_empty()));
    out.insert("ignoreArrayOrder".into(), json!(unordered));
    out.insert("summary".into(), summary);
    out.insert("differences".into(), Value::Array(w.diffs));
    out.insert("patch".into(), Value::Array(ops));
    if unordered {
        out.insert("note".into(), json!("Arrays compared as multisets; the patch reproduces the elements, appended items go to the end (\"/-\")."));
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
pub(crate) fn apply_patch(doc: &Value, ops: &[Value]) -> Value {
    // Minimal RFC 6902 applier (add/remove/replace) used to round-trip-test the generated patches.
    fn parts(p: &str) -> Vec<String> {
        p.split('/').skip(1).map(|s| s.replace("~1", "/").replace("~0", "~")).collect()
    }
    let mut doc = doc.clone();
    for op in ops {
        let path = parts(op["path"].as_str().unwrap());
        let kind = op["op"].as_str().unwrap();
        if path.is_empty() {
            doc = op["value"].clone();
            continue;
        }
        let (last, parent_path) = path.split_last().unwrap();
        let mut parent = &mut doc;
        for seg in parent_path {
            parent = match parent {
                Value::Array(a) => &mut a[seg.parse::<usize>().unwrap()],
                Value::Object(m) => m.get_mut(seg).unwrap(),
                _ => panic!("bad path"),
            };
        }
        match (parent, kind) {
            (Value::Array(a), "add") if last == "-" => a.push(op["value"].clone()),
            (Value::Array(a), "add") => a.insert(last.parse().unwrap(), op["value"].clone()),
            (Value::Array(a), "remove") => {
                a.remove(last.parse().unwrap());
            }
            (Value::Array(a), "replace") => a[last.parse::<usize>().unwrap()] = op["value"].clone(),
            (Value::Object(m), "add" | "replace") => {
                m.insert(last.clone(), op["value"].clone());
            }
            (Value::Object(m), "remove") => {
                m.remove(last);
            }
            _ => panic!("unsupported op"),
        }
    }
    doc
}
