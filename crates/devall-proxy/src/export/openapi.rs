//! Captured exchanges → an OpenAPI 3.1 draft. Deterministic (same input, same YAML), no AI:
//!
//! 1. **Paths.** Id-like segments (integers, UUIDs, dates, ULIDs, long hex, opaque tokens) become
//!    `{param}`s named after the preceding segment (`/users/42` → `/users/{userId}`). Then, at any
//!    position where ≥ `minSiblings` (default 3) different literal values share the same
//!    surrounding path *and* a literal segment follows (`/repos/alice/issues`,
//!    `/repos/bob/issues`, …), the position becomes a parameter (`/repos/{repo}/issues`).
//!    Collections like `/api/users` vs `/api/orders` stay separate because nothing follows them.
//! 2. **Operations** per template and method: path/query parameters (required when present in
//!    every sample; integer/number/boolean/uuid/date detected), request bodies, and one response
//!    per observed status code.
//! 3. **Schemas** merge every observed JSON body: properties in first-seen order, `required` =
//!    keys present in every sample, `type` arrays for mixed/nullable values (3.1 style), string
//!    formats (`date-time`, `date`, `uuid`, `email`, `uri`) only when every value matches.
//! 4. **Security**: Bearer / Basic / API-key headers become security schemes.
//!
//! Examples are off by default (captured bodies can contain personal data).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{canonical_header, decoded_body, query_pairs};
use crate::model::Exchange;
use crate::pathgen::{SegKind, camel, classify, singular};
use crate::util::{header_value, is_json_type, mime_essence};

const MAX_DEPTH: usize = 24;
const MAX_ARRAY_ITEMS: usize = 100;
const METHOD_ORDER: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenApiOptions {
    pub title: Option<String>,
    pub version: String,
    /// Add the first observed body as `example` (may contain personal data).
    pub include_examples: bool,
    /// Distinct literal values needed before a position with a shared literal suffix becomes a
    /// parameter.
    pub min_siblings: usize,
}

impl Default for OpenApiOptions {
    fn default() -> Self {
        Self { title: None, version: "0.1.0".into(), include_examples: false, min_siblings: 3 }
    }
}

// ---------- JSON schema inference ----------

const F_DATETIME: u8 = 1;
const F_DATE: u8 = 2;
const F_UUID: u8 = 4;
const F_EMAIL: u8 = 8;
const F_URI: u8 = 16;

fn formats_of(s: &str) -> u8 {
    let mut f = 0;
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        f |= F_DATETIME;
    }
    if s.len() == 10 && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        f |= F_DATE;
    }
    if classify(s) == SegKind::Uuid {
        f |= F_UUID;
    }
    if let Some((user, domain)) = s.split_once('@')
        && !user.is_empty()
        && domain.contains('.')
        && !s.contains(char::is_whitespace)
    {
        f |= F_EMAIL;
    }
    if (s.starts_with("http://") || s.starts_with("https://")) && url::Url::parse(s).is_ok() {
        f |= F_URI;
    }
    f
}

#[derive(Debug, Clone, Default)]
struct ObjAcc {
    count: usize,
    props: Vec<(String, Acc)>,
}

/// Accumulates observed JSON values into a schema.
#[derive(Debug, Clone, Default)]
pub(crate) struct Acc {
    seen: usize,
    null: bool,
    boolean: bool,
    integer: bool,
    number: bool,
    string: Option<u8>,
    array: Option<Box<Acc>>,
    object: Option<ObjAcc>,
}

impl Acc {
    pub fn add(&mut self, v: &Value) {
        self.add_depth(v, 0);
    }

    fn add_depth(&mut self, v: &Value, depth: usize) {
        self.seen += 1;
        match v {
            Value::Null => self.null = true,
            Value::Bool(_) => self.boolean = true,
            Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    self.integer = true;
                } else {
                    self.number = true;
                }
            }
            Value::String(s) => {
                let f = formats_of(s);
                self.string = Some(self.string.map_or(f, |prev| prev & f));
            }
            Value::Array(items) => {
                let acc = self.array.get_or_insert_with(Box::default);
                if depth < MAX_DEPTH {
                    for x in items.iter().take(MAX_ARRAY_ITEMS) {
                        acc.add_depth(x, depth + 1);
                    }
                }
            }
            Value::Object(map) => {
                let obj = self.object.get_or_insert_with(ObjAcc::default);
                obj.count += 1;
                if depth < MAX_DEPTH {
                    for (k, x) in map {
                        match obj.props.iter_mut().find(|(n, _)| n == k) {
                            Some((_, acc)) => acc.add_depth(x, depth + 1),
                            None => {
                                let mut acc = Acc::default();
                                acc.add_depth(x, depth + 1);
                                obj.props.push((k.clone(), acc));
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn schema(&self) -> Value {
        if self.seen == 0 {
            return json!({});
        }
        let mut types: Vec<&str> = Vec::new();
        let mut out = Map::new();
        if let Some(obj) = &self.object {
            types.push("object");
            if !obj.props.is_empty() {
                let mut props = Map::new();
                let mut required = Vec::new();
                for (k, acc) in &obj.props {
                    props.insert(k.clone(), acc.schema());
                    if acc.seen >= obj.count {
                        required.push(json!(k));
                    }
                }
                out.insert("properties".into(), Value::Object(props));
                if !required.is_empty() {
                    out.insert("required".into(), Value::Array(required));
                }
            }
        }
        if let Some(items) = &self.array {
            types.push("array");
            out.insert("items".into(), items.schema());
        }
        if let Some(f) = self.string {
            types.push("string");
            let format = [(F_DATETIME, "date-time"), (F_DATE, "date"), (F_UUID, "uuid"), (F_EMAIL, "email"), (F_URI, "uri")]
                .iter()
                .find(|(bit, _)| f & bit != 0)
                .map(|(_, n)| *n);
            if let Some(fm) = format {
                out.insert("format".into(), json!(fm));
            }
        }
        if self.number {
            types.push("number");
        } else if self.integer {
            types.push("integer");
        }
        if self.boolean {
            types.push("boolean");
        }
        if self.null {
            types.push("null");
        }
        let mut schema = Map::new();
        schema.insert("type".into(), if types.len() == 1 { json!(types[0]) } else { json!(types) });
        for (k, v) in out {
            schema.insert(k, v);
        }
        Value::Object(schema)
    }
}

// ---------- samples & path templating ----------

#[derive(Debug, Clone, PartialEq)]
enum Seg {
    Lit(String),
    Param(SegKind, String),
}

struct Sample<'a> {
    ex: &'a Exchange,
    origin: String,
    method: String,
    segs: Vec<Seg>,
    trailing_slash: bool,
    query: Vec<(String, String)>,
}

fn signature(segs: &[Seg], skip: usize) -> String {
    segs.iter()
        .enumerate()
        .map(|(i, s)| match s {
            _ if i == skip => "\u{1}".to_string(),
            Seg::Lit(l) => l.clone(),
            Seg::Param(..) => "\u{2}".to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Turns positions with many sibling values and a shared literal suffix into parameters.
fn merge_structural(samples: &mut [Sample<'_>], min_siblings: usize) {
    for _round in 0..8 {
        let mut changes: Vec<(usize, usize)> = Vec::new();
        let max_len = samples.iter().map(|s| s.segs.len()).max().unwrap_or(0);
        for pos in 0..max_len {
            let mut groups: BTreeMap<(usize, String), Vec<usize>> = BTreeMap::new();
            for (i, s) in samples.iter().enumerate() {
                if pos + 1 >= s.segs.len() || !matches!(s.segs[pos], Seg::Lit(_)) {
                    continue;
                }
                // A literal must follow somewhere after this position.
                if !s.segs[pos + 1..].iter().any(|x| matches!(x, Seg::Lit(l) if !l.is_empty())) {
                    continue;
                }
                groups.entry((s.segs.len(), signature(&s.segs, pos))).or_default().push(i);
            }
            for idxs in groups.values() {
                let mut distinct: Vec<&str> =
                    idxs.iter().filter_map(|i| if let Seg::Lit(l) = &samples[*i].segs[pos] { Some(l.as_str()) } else { None }).collect();
                distinct.sort_unstable();
                distinct.dedup();
                if distinct.len() >= min_siblings.max(2) {
                    changes.extend(idxs.iter().map(|i| (*i, pos)));
                }
            }
        }
        if changes.is_empty() {
            return;
        }
        for (i, pos) in changes {
            if let Seg::Lit(l) = &samples[i].segs[pos] {
                let v = l.clone();
                samples[i].segs[pos] = Seg::Param(SegKind::Literal, v);
            }
        }
    }
}

fn param_names(segs: &[Seg]) -> Vec<Option<String>> {
    let mut used: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        match s {
            Seg::Lit(_) => out.push(None),
            Seg::Param(kind, _) => {
                let prev = i.checked_sub(1).and_then(|p| if let Seg::Lit(l) = &segs[p] { Some(l.as_str()) } else { None }).filter(|l| !l.is_empty());
                let base = match (prev, kind) {
                    (Some(p), SegKind::Literal) => camel(&singular(p)),
                    (Some(p), _) => format!("{}Id", camel(&singular(p))),
                    (None, SegKind::Literal) => "param".to_string(),
                    (None, _) => "id".to_string(),
                };
                let mut name = base.clone();
                let mut n = 2;
                while used.contains(&name) {
                    name = format!("{base}{n}");
                    n += 1;
                }
                used.push(name.clone());
                out.push(Some(name));
            }
        }
    }
    out
}

fn template_of(segs: &[Seg], trailing_slash: bool) -> String {
    let names = param_names(segs);
    let body = segs
        .iter()
        .zip(names)
        .map(|(s, n)| match (s, n) {
            (Seg::Lit(l), _) => l.clone(),
            (Seg::Param(..), Some(n)) => format!("{{{n}}}"),
            (Seg::Param(..), None) => "{param}".into(),
        })
        .collect::<Vec<_>>()
        .join("/");
    let mut t = format!("/{body}");
    if trailing_slash && !t.ends_with('/') {
        t.push('/');
    }
    t
}

fn scalar_schema(values: &[&str]) -> Value {
    if !values.is_empty() && values.iter().all(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()) && v.len() < 16) {
        json!({ "type": "integer" })
    } else if !values.is_empty() && values.iter().all(|v| v.parse::<f64>().is_ok_and(f64::is_finite)) {
        json!({ "type": "number" })
    } else if !values.is_empty() && values.iter().all(|v| *v == "true" || *v == "false") {
        json!({ "type": "boolean" })
    } else if !values.is_empty() && values.iter().all(|v| classify(v) == SegKind::Uuid) {
        json!({ "type": "string", "format": "uuid" })
    } else if !values.is_empty() && values.iter().all(|v| classify(v) == SegKind::Date) {
        json!({ "type": "string", "format": "date" })
    } else {
        json!({ "type": "string" })
    }
}

fn pascal(s: &str) -> String {
    let c = camel(s);
    let mut chars = c.chars();
    match chars.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn operation_id(method: &str, template: &str) -> String {
    let mut lits = Vec::new();
    let mut params = Vec::new();
    for seg in template.split('/').filter(|s| !s.is_empty()) {
        if let Some(p) = seg.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
            params.push(pascal(p));
        } else {
            lits.push(pascal(seg));
        }
    }
    let mut id = method.to_ascii_lowercase();
    id.push_str(&lits.concat());
    if !params.is_empty() {
        id.push_str("By");
        id.push_str(&params.join("And"));
    }
    if id == method.to_ascii_lowercase() {
        id.push_str("Root");
    }
    id
}

#[derive(Default)]
struct MediaAcc {
    acc: Acc,
    json: bool,
    form: bool,
    binary: bool,
    example: Option<Value>,
}

fn media_schema(m: &MediaAcc) -> Value {
    if m.json || m.form {
        m.acc.schema()
    } else if m.binary {
        json!({ "type": "string", "format": "binary" })
    } else {
        json!({ "type": "string" })
    }
}

fn add_body(media: &mut BTreeMap<String, MediaAcc>, ct_header: Option<&str>, bytes: &[u8], include_example: bool) {
    let ct = ct_header.map(mime_essence).filter(|c| !c.is_empty()).unwrap_or_else(|| "application/octet-stream".into());
    let entry = media.entry(ct.clone()).or_default();
    if (is_json_type(&ct) || ct == "application/octet-stream")
        && let Ok(v) = serde_json::from_slice::<Value>(bytes)
    {
        entry.json = true;
        entry.acc.add(&v);
        if include_example && entry.example.is_none() {
            entry.example = Some(v);
        }
        return;
    }
    if ct == "application/x-www-form-urlencoded" {
        let map: Map<String, Value> = url::form_urlencoded::parse(bytes).map(|(k, v)| (k.into_owned(), Value::String(v.into_owned()))).collect();
        entry.form = true;
        entry.acc.add(&Value::Object(map));
        return;
    }
    if std::str::from_utf8(bytes).is_err()
        || ct.starts_with("image/")
        || ct.starts_with("audio/")
        || ct.starts_with("video/")
        || ct == "application/octet-stream"
    {
        entry.binary = true;
    }
    entry.acc.seen += 1;
    if include_example && entry.example.is_none() && !entry.binary {
        entry.example = std::str::from_utf8(bytes).ok().map(|t| Value::String(t.chars().take(2000).collect()));
    }
}

fn content_json(media: &BTreeMap<String, MediaAcc>) -> Value {
    let mut c = Map::new();
    for (ct, m) in media {
        let mut mt = Map::new();
        mt.insert("schema".into(), media_schema(m));
        if let Some(ex) = &m.example {
            mt.insert("example".into(), ex.clone());
        }
        c.insert(ct.clone(), Value::Object(mt));
    }
    Value::Object(c)
}

#[derive(Default)]
struct SecurityAcc {
    bearer: usize,
    basic: usize,
    api_key: usize,
}

/// Infers an OpenAPI 3.1 document from exchanges (requests without a response, tunnels and
/// WebSocket upgrades are ignored).
pub fn infer(exchanges: &[Exchange], opts: &OpenApiOptions) -> Value {
    let mut samples: Vec<Sample<'_>> = Vec::new();
    for ex in exchanges {
        let Some(resp) = &ex.response else { continue };
        if ex.request.method == "CONNECT" || resp.status < 100 || resp.status == 101 {
            continue;
        }
        let Ok(url) = url::Url::parse(&ex.request.url) else {
            continue;
        };
        let origin = url.origin().ascii_serialization();
        let path = url.path();
        let trimmed = path.trim_start_matches('/');
        let trailing_slash = path.len() > 1 && path.ends_with('/');
        let body = trimmed.trim_end_matches('/');
        let segs: Vec<Seg> = if body.is_empty() {
            Vec::new()
        } else {
            body.split('/')
                .map(|s| {
                    let decoded = percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
                    match classify(&decoded) {
                        SegKind::Literal => Seg::Lit(s.to_string()),
                        k => Seg::Param(k, decoded),
                    }
                })
                .collect()
        };
        samples.push(Sample { ex, origin, method: ex.request.method.to_ascii_lowercase(), segs, trailing_slash, query: query_pairs(&ex.request.url) });
    }
    merge_structural(&mut samples, opts.min_siblings);

    // template → method → sample indexes
    let mut ops: BTreeMap<String, BTreeMap<String, Vec<usize>>> = BTreeMap::new();
    let mut origins: Vec<String> = Vec::new();
    for (i, s) in samples.iter().enumerate() {
        ops.entry(template_of(&s.segs, s.trailing_slash)).or_default().entry(s.method.clone()).or_default().push(i);
        if !origins.contains(&s.origin) {
            origins.push(s.origin.clone());
        }
    }
    origins.sort();

    let mut paths = Map::new();
    let mut used_ids: Vec<String> = Vec::new();
    let mut schemes_used = SecurityAcc::default();
    let mut api_key_name: Option<String> = None;
    for (template, methods) in &ops {
        let mut item = Map::new();
        let mut methods_sorted: Vec<(&String, &Vec<usize>)> = methods.iter().collect();
        methods_sorted.sort_by_key(|(m, _)| METHOD_ORDER.iter().position(|x| x == m).unwrap_or(99));
        for (method, idxs) in methods_sorted {
            let group: Vec<&Sample<'_>> = idxs.iter().map(|i| &samples[*i]).collect();
            let mut op = Map::new();
            let mut id = operation_id(method, template);
            let base = id.clone();
            let mut n = 2;
            while used_ids.contains(&id) {
                id = format!("{base}{n}");
                n += 1;
            }
            used_ids.push(id.clone());
            op.insert("operationId".into(), json!(id));
            op.insert("summary".into(), json!(format!("{} {}", method.to_ascii_uppercase(), template)));

            let mut params: Vec<Value> = Vec::new();
            let names = param_names(&group[0].segs);
            for (pos, name) in names.iter().enumerate() {
                let Some(name) = name else { continue };
                let values: Vec<&str> =
                    group.iter().filter_map(|s| if let Some(Seg::Param(_, v)) = s.segs.get(pos) { Some(v.as_str()) } else { None }).collect();
                params.push(json!({ "name": name, "in": "path", "required": true, "schema": scalar_schema(&values) }));
            }
            let mut qnames: Vec<String> = Vec::new();
            for s in &group {
                for (k, _) in &s.query {
                    if !qnames.contains(k) {
                        qnames.push(k.clone());
                    }
                }
            }
            for q in &qnames {
                let present = group.iter().filter(|s| s.query.iter().any(|(k, _)| k == q)).count();
                let repeated = group.iter().any(|s| s.query.iter().filter(|(k, _)| k == q).count() > 1);
                let values: Vec<&str> = group.iter().flat_map(|s| s.query.iter().filter(|(k, _)| k == q).map(|(_, v)| v.as_str())).collect();
                let scalar = scalar_schema(&values);
                let schema = if repeated { json!({ "type": "array", "items": scalar }) } else { scalar };
                let mut p = Map::new();
                p.insert("name".into(), json!(q));
                p.insert("in".into(), json!("query"));
                if present == group.len() {
                    p.insert("required".into(), json!(true));
                }
                p.insert("schema".into(), schema);
                if repeated {
                    p.insert("explode".into(), json!(true));
                }
                params.push(Value::Object(p));
            }
            if !params.is_empty() {
                op.insert("parameters".into(), Value::Array(params));
            }

            let mut req_media: BTreeMap<String, MediaAcc> = BTreeMap::new();
            let mut with_body = 0;
            for s in &group {
                if s.ex.request.body.size == 0 {
                    continue;
                }
                if let Some(b) = decoded_body(&s.ex.request.body, &s.ex.request.headers).filter(|b| !b.is_empty()) {
                    with_body += 1;
                    add_body(&mut req_media, header_value(&s.ex.request.headers, "content-type"), &b, opts.include_examples);
                }
            }
            if !req_media.is_empty() {
                let mut rb = Map::new();
                if with_body == group.len() {
                    rb.insert("required".into(), json!(true));
                }
                rb.insert("content".into(), content_json(&req_media));
                op.insert("requestBody".into(), Value::Object(rb));
            }

            let mut by_status: BTreeMap<u16, (BTreeMap<String, MediaAcc>, Vec<String>)> = BTreeMap::new();
            for s in &group {
                let Some(r) = &s.ex.response else { continue };
                let entry = by_status.entry(r.status).or_default();
                if let Some(b) = decoded_body(&r.body, &r.headers).filter(|b| !b.is_empty()) {
                    add_body(&mut entry.0, header_value(&r.headers, "content-type"), &b, opts.include_examples);
                }
                if let Some(loc) = header_value(&r.headers, "location")
                    && !loc.is_empty()
                    && !entry.1.iter().any(|h| h == "Location")
                {
                    entry.1.push("Location".into());
                }
            }
            let mut responses = Map::new();
            for (status, (media, headers)) in &by_status {
                let mut r = Map::new();
                let reason = http::StatusCode::from_u16(*status).ok().and_then(|s| s.canonical_reason()).unwrap_or("Response");
                r.insert("description".into(), json!(reason));
                if !headers.is_empty() {
                    let mut h = Map::new();
                    for name in headers {
                        h.insert(canonical_header(name), json!({ "schema": { "type": "string" } }));
                    }
                    r.insert("headers".into(), Value::Object(h));
                }
                if !media.is_empty() {
                    r.insert("content".into(), content_json(media));
                }
                responses.insert(status.to_string(), Value::Object(r));
            }
            op.insert("responses".into(), Value::Object(responses));

            let mut sec = SecurityAcc::default();
            for s in &group {
                let h = &s.ex.request.headers;
                match header_value(h, "authorization").map(|v| v.split(' ').next().unwrap_or("").to_ascii_lowercase()) {
                    Some(sch) if sch == "bearer" => sec.bearer += 1,
                    Some(sch) if sch == "basic" => sec.basic += 1,
                    _ => {}
                }
                if let Some((k, _)) = h.iter().find(|(k, _)| matches!(k.to_ascii_lowercase().as_str(), "x-api-key" | "api-key" | "apikey")) {
                    sec.api_key += 1;
                    api_key_name.get_or_insert_with(|| canonical_header(k));
                }
            }
            let mut reqs: Vec<Value> = Vec::new();
            let mut all_covered = true;
            for (count, name) in [(sec.bearer, "bearerAuth"), (sec.basic, "basicAuth"), (sec.api_key, "apiKeyAuth")] {
                if count > 0 {
                    reqs.push(json!({ name: [] }));
                    match name {
                        "bearerAuth" => schemes_used.bearer += 1,
                        "basicAuth" => schemes_used.basic += 1,
                        _ => schemes_used.api_key += 1,
                    }
                }
            }
            if sec.bearer.max(sec.basic).max(sec.api_key) < group.len() {
                all_covered = false;
            }
            if !reqs.is_empty() {
                if !all_covered {
                    reqs.push(json!({}));
                }
                op.insert("security".into(), Value::Array(reqs));
            }
            item.insert(method.clone(), Value::Object(op));
        }
        paths.insert(template.clone(), Value::Object(item));
    }

    let title = opts.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| match origins.as_slice() {
        [one] => format!("{} API", url::Url::parse(one).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| one.clone())),
        _ => "Captured API".into(),
    });
    let mut doc = Map::new();
    doc.insert("openapi".into(), json!("3.1.0"));
    doc.insert(
        "info".into(),
        json!({
            "title": title,
            "version": if opts.version.trim().is_empty() { "0.1.0" } else { opts.version.trim() },
            "description": format!("Draft inferred by /dev/all from {} captured exchange(s). Review names, types and required fields before publishing.", samples.len()),
        }),
    );
    if !origins.is_empty() {
        doc.insert("servers".into(), Value::Array(origins.iter().map(|o| json!({ "url": o })).collect()));
    }
    doc.insert("paths".into(), Value::Object(paths));
    let mut schemes = Map::new();
    if schemes_used.bearer > 0 {
        schemes.insert("bearerAuth".into(), json!({ "type": "http", "scheme": "bearer" }));
    }
    if schemes_used.basic > 0 {
        schemes.insert("basicAuth".into(), json!({ "type": "http", "scheme": "basic" }));
    }
    if schemes_used.api_key > 0 {
        schemes.insert("apiKeyAuth".into(), json!({ "type": "apiKey", "in": "header", "name": api_key_name.unwrap_or_else(|| "X-API-Key".into()) }));
    }
    if !schemes.is_empty() {
        doc.insert("components".into(), json!({ "securitySchemes": schemes }));
    }
    Value::Object(doc)
}

/// [`infer`] rendered as YAML.
pub fn infer_yaml(exchanges: &[Exchange], opts: &OpenApiOptions) -> String {
    serde_yaml_ng::to_string(&infer(exchanges, opts)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_merging() {
        let mut acc = Acc::default();
        acc.add(&json!({"id": 1, "name": "a", "email": "a@x.io", "tags": ["x"], "meta": null, "at": "2024-01-01T10:00:00Z"}));
        acc.add(&json!({"id": 2, "name": "b", "email": "b@x.io", "tags": [], "meta": {"k": 1.5}, "at": "2024-01-02T10:00:00Z", "extra": true}));
        let s = acc.schema();
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"], json!(["id", "name", "email", "tags", "meta", "at"]));
        assert_eq!(s["properties"]["id"]["type"], "integer");
        assert_eq!(s["properties"]["email"]["format"], "email");
        assert_eq!(s["properties"]["at"]["format"], "date-time");
        assert_eq!(s["properties"]["meta"]["type"], json!(["object", "null"]));
        assert_eq!(s["properties"]["meta"]["properties"]["k"]["type"], "number");
        assert_eq!(s["properties"]["tags"]["items"]["type"], "string");
        assert_eq!(s["properties"]["extra"]["type"], "boolean");
        let mut mixed = Acc::default();
        mixed.add(&json!("2024-01-01"));
        mixed.add(&json!("not a date"));
        assert_eq!(mixed.schema(), json!({"type": "string"}));
    }

    #[test]
    fn operation_ids() {
        assert_eq!(operation_id("get", "/users/{userId}/orders"), "getUsersOrdersByUserId");
        assert_eq!(operation_id("post", "/"), "postRoot");
    }
}
