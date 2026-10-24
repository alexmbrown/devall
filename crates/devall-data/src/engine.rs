//! Schema compilation (validation, dependency ordering) and row generation.
//!
//! Determinism: every field has its own RNG stream seeded from `(schema seed, entity, field
//! path)`. Adding, removing or reordering *other* fields, or changing an entity's count, never
//! changes the values of a field; and the first N rows are the same whatever the count. Values
//! may change across versions of this crate (or of `fake`/`rand`), like with any faker library.
//!
//! Order: entities are generated parents-first (topological order over `ref` fields); fields in
//! a row are generated in dependency order (`template` references, e-mails/usernames/full names
//! derived from the row's name fields) and emitted in declared order.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use indexmap::IndexMap;
use rand::{RngExt, SeedableRng};
use serde::Serialize;
use serde_json::{Map, Number, Value};

use crate::error::{DataError, Result, invalid};
use crate::fakers::{self, DRng, Locale, Text};
use crate::model::{Entity, Field, FieldKind, Schema, TimestampUnit};
use crate::regex_gen::RegexGen;

/// Draws per value before a `unique` field gives up.
const UNIQUE_ATTEMPTS: u32 = 1000;
pub(crate) const DEFAULT_FROM: &str = "2020-01-01";
pub(crate) const DEFAULT_TO: &str = "2025-12-31";
pub(crate) const DATE_FORMAT: &str = "%Y-%m-%d";
pub(crate) const DATETIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";
/// ULID / UUIDv7 timestamps count from 2024-01-01T00:00:00Z (one second per id) so they stay
/// deterministic and sorted.
const ID_EPOCH_MS: u64 = 1_704_067_200_000;
const MAX_NESTING: usize = 32;

// ---------- public surface ----------

/// What to generate.
#[derive(Debug, Clone, Default)]
pub struct GenerateOptions {
    /// Only emit this entity (its `ref` parents are still generated, but not emitted).
    pub entity: Option<String>,
    /// Replaces the count of the emitted entities.
    pub count_override: Option<u64>,
    /// Emit at most this many rows per entity (previews). Parents of emitted entities are still
    /// generated in full, so a preview shows exactly the first rows of the real export.
    pub limit: Option<u64>,
    /// Seed when the schema has none; when both are missing a random seed is drawn.
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateStats {
    /// The seed actually used (pin it in the schema to reproduce a random run).
    pub seed: u64,
    /// Rows emitted per entity, in emission order.
    pub rows: IndexMap<String, u64>,
}

/// Receives generated rows, entity by entity, parents first.
pub trait RowSink {
    /// Called once with the entities that will be emitted, in emission order.
    fn begin(&mut self, entities: &[&Entity]) -> Result<()>;
    fn begin_entity(&mut self, entity: &Entity, rows: u64) -> Result<()>;
    fn row(&mut self, row: &Map<String, Value>) -> Result<()>;
    fn end_entity(&mut self) -> Result<()>;
    fn finish(&mut self) -> Result<()>;
}

/// Collects rows in memory, keyed by entity name.
#[derive(Debug, Default)]
pub struct CollectSink {
    pub data: IndexMap<String, Vec<Value>>,
    current: Option<String>,
}

impl RowSink for CollectSink {
    fn begin(&mut self, _: &[&Entity]) -> Result<()> {
        Ok(())
    }
    fn begin_entity(&mut self, entity: &Entity, rows: u64) -> Result<()> {
        self.data.insert(entity.name.clone(), Vec::with_capacity(rows.min(1 << 16) as usize));
        self.current = Some(entity.name.clone());
        Ok(())
    }
    fn row(&mut self, row: &Map<String, Value>) -> Result<()> {
        if let Some(rows) = self.current.as_ref().and_then(|c| self.data.get_mut(c)) {
            rows.push(Value::Object(row.clone()));
        }
        Ok(())
    }
    fn end_entity(&mut self) -> Result<()> {
        self.current = None;
        Ok(())
    }
    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Checks a schema without generating anything (unknown refs, cycles, bad parameters, …).
pub fn validate(schema: &Schema) -> Result<()> {
    compile(schema, 0).map(|_| ())
}

/// Generates rows into `sink`.
pub fn generate(schema: &Schema, opts: &GenerateOptions, sink: &mut dyn RowSink) -> Result<GenerateStats> {
    let seed = schema.seed.or(opts.seed).unwrap_or_else(|| rand::random::<u64>() >> 11);
    let mut plan = compile(schema, seed)?;
    let n = plan.entities.len();

    let emit: Vec<bool> = match &opts.entity {
        Some(name) => {
            let i = plan.entities.iter().position(|e| &e.name == name).ok_or_else(|| DataError::UnknownEntity(name.clone()))?;
            (0..n).map(|j| j == i).collect()
        }
        None => vec![true; n],
    };
    if let Some(c) = opts.count_override {
        for (e, _) in plan.entities.iter_mut().zip(&emit).filter(|(_, on)| **on) {
            e.count = c;
        }
    }

    // Entities to generate: the emitted ones plus everything they reference, transitively.
    let mut needed = emit.clone();
    let mut stack: Vec<usize> = (0..n).filter(|&i| emit[i]).collect();
    while let Some(i) = stack.pop() {
        for (t, _) in &plan.entities[i].refs {
            if !needed[*t] {
                needed[*t] = true;
                stack.push(*t);
            }
        }
    }
    // Columns other entities (or the entity itself) pick foreign keys from.
    let mut pool_cols: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
    for i in (0..n).filter(|&i| needed[i]) {
        for (t, f) in &plan.entities[i].refs {
            pool_cols[*t].insert(f.clone());
        }
    }

    let emitted: Vec<&Entity> = plan.order.iter().filter(|&&i| emit[i]).map(|&i| &schema.entities[i]).collect();
    sink.begin(&emitted)?;

    let limit = opts.limit.unwrap_or(u64::MAX);
    let mut pools: Vec<HashMap<String, Vec<Value>>> = vec![HashMap::new(); n];
    let mut stats = GenerateStats { seed, rows: IndexMap::new() };
    let locale = plan.locale;
    for ei in plan.order.clone() {
        if !needed[ei] {
            continue;
        }
        let entity = &mut plan.entities[ei];
        let cols: Vec<(usize, String)> =
            pool_cols[ei].iter().filter_map(|name| entity.scope.index.get(name).map(|&i| (i, name.clone()))).collect();
        // Fields needed beyond the emitted rows: pool columns and what they depend on.
        let mut mask = vec![false; entity.scope.fields.len()];
        let mut todo: Vec<usize> = cols.iter().map(|(i, _)| *i).collect();
        while let Some(i) = todo.pop() {
            if !mask[i] {
                mask[i] = true;
                todo.extend(entity.scope.deps[i].iter().copied());
            }
        }
        let out_rows = if emit[ei] { entity.count.min(limit) } else { 0 };
        let total = if cols.is_empty() { out_rows } else { entity.count };
        for (_, name) in &cols {
            pools[ei].insert(name.clone(), Vec::with_capacity(total.min(1 << 20) as usize));
        }
        if emit[ei] {
            sink.begin_entity(&schema.entities[ei], out_rows)?;
        }
        for row in 0..total {
            let m = if row < out_rows { None } else { Some(&mask[..]) };
            let values = {
                let env = Env { locale, pools: &pools, entity: &entity.name, row };
                gen_scope(&mut entity.scope, &env, None, m)?
            };
            for (i, name) in &cols {
                if !values[*i].is_null()
                    && let Some(p) = pools[ei].get_mut(name)
                {
                    p.push(values[*i].clone());
                }
            }
            if row < out_rows {
                sink.row(&to_map(&entity.scope.fields, values))?;
            }
        }
        if emit[ei] {
            sink.end_entity()?;
            stats.rows.insert(entity.name.clone(), out_rows);
        }
        // Free pools nobody needs any more.
        let later: Vec<usize> = plan.order.iter().copied().skip_while(|&k| k != ei).skip(1).collect();
        for (j, pool) in pools.iter_mut().enumerate() {
            if !pool.is_empty() && later.iter().all(|&k| !plan.entities[k].refs.iter().any(|(t, _)| *t == j)) {
                pool.clear();
            }
        }
    }
    sink.finish()?;
    Ok(stats)
}

/// Generates rows into memory: `{ entity: [row, …] }`.
pub fn generate_values(schema: &Schema, opts: &GenerateOptions) -> Result<IndexMap<String, Vec<Value>>> {
    let mut sink = CollectSink::default();
    generate(schema, opts, &mut sink)?;
    Ok(sink.data)
}

/// One value of a single generator (handy for other modules: randomized mock values, load-test
/// bodies). Row-derived kinds (`template`, `ref`) have nothing to derive from here.
pub fn generate_value(kind: &FieldKind, seed: u64, locale: Option<&str>) -> Result<Value> {
    let mut schema = Schema::new("value");
    schema.seed = Some(seed);
    schema.locale = locale.map(str::to_string);
    schema.entities.push(Entity::new("value", 1, vec![Field::new("value", kind.clone())]));
    let mut data = generate_values(&schema, &GenerateOptions::default())?;
    Ok(data.swap_remove("value").and_then(|mut rows| rows.pop()).and_then(|row| row.get("value").cloned()).unwrap_or(Value::Null))
}

// ---------- compiled plan ----------

struct Plan {
    locale: Locale,
    entities: Vec<CEntity>,
    order: Vec<usize>,
}

struct CEntity {
    name: String,
    count: u64,
    scope: CScope,
    /// `(entity, top-level field)` of every `ref` in this entity, at any depth.
    refs: Vec<(usize, String)>,
}

struct CScope {
    fields: Vec<CField>,
    /// Generation order (dependencies first).
    order: Vec<usize>,
    index: HashMap<String, usize>,
    /// Sibling indices each field depends on.
    deps: Vec<Vec<usize>>,
}

struct CField {
    name: String,
    path: String,
    null_ratio: f64,
    unique: bool,
    kind: CKind,
    rng: DRng,
    seen: HashSet<u64>,
    shuffle: Option<SparseShuffle>,
    counter: u64,
}

#[derive(Clone, Copy, Default)]
struct NameSrc {
    first: Option<usize>,
    last: Option<usize>,
    full: Option<usize>,
}

enum CKind {
    Uuid { v7: bool },
    Ulid,
    Sequence { start: i64, step: i64 },
    Text(Text),
    FullName(NameSrc),
    Username(NameSrc),
    Email { src: NameSrc, domain: Option<String> },
    Url,
    Domain,
    Street,
    Coord { max: f64 },
    ProductName,
    Paragraph(u32),
    Lorem(u32),
    Number { min: f64, max: f64, decimals: u32 },
    Int { min: i64, max: i64 },
    Bool { p: f64 },
    Date { from: i64, to: i64, format: String },
    Timestamp { from: i64, to: i64, ms: bool },
    Enum { values: Vec<Value>, cumulative: Vec<f64> },
    Regex(RegexGen),
    Template(Vec<Part>),
    Ref { entity: usize, field: String },
    Object(CScope),
    Array { item: Box<CField>, min: u32, max: u32 },
    Const(Value),
}

impl CKind {
    /// Kinds whose `unique` needs a seen-set (the rest are unique by construction or sample
    /// without replacement).
    fn needs_seen_set(&self) -> bool {
        !matches!(self, CKind::Sequence { .. } | CKind::Uuid { .. } | CKind::Ulid | CKind::Int { .. } | CKind::Bool { .. } | CKind::Enum { .. } | CKind::Ref { .. })
    }
}

enum Part {
    Lit(String),
    Ref { path: Vec<String>, filters: Vec<Filter> },
}

#[derive(Clone, Copy)]
enum Filter {
    Lower,
    Upper,
    Slug,
    Ascii,
    Alnum,
    Trim,
}

// ---------- compile ----------

struct Cx<'a> {
    seed: u64,
    entity: usize,
    entity_name: &'a str,
    entity_index: &'a HashMap<&'a str, usize>,
    schema: &'a Schema,
    refs: Vec<(usize, String)>,
}

fn compile(schema: &Schema, seed: u64) -> Result<Plan> {
    let locale = match schema.locale.as_deref() {
        None => Locale::En,
        Some(code) => Locale::parse(code).ok_or_else(|| {
            invalid(format!("unknown locale '{code}' (supported: {})", fakers::LOCALES.iter().map(|l| l.0).collect::<Vec<_>>().join(", ")))
        })?,
    };
    let mut entity_index: HashMap<&str, usize> = HashMap::new();
    for (i, e) in schema.entities.iter().enumerate() {
        if e.name.trim().is_empty() {
            return Err(invalid(format!("entity #{} has no name", i + 1)));
        }
        if entity_index.insert(e.name.as_str(), i).is_some() {
            return Err(invalid(format!("duplicate entity name '{}'", e.name)));
        }
    }
    let mut entities = Vec::with_capacity(schema.entities.len());
    let mut edeps: Vec<Vec<usize>> = Vec::with_capacity(schema.entities.len());
    for (i, e) in schema.entities.iter().enumerate() {
        let mut cx = Cx { seed, entity: i, entity_name: &e.name, entity_index: &entity_index, schema, refs: Vec::new() };
        let (scope, free) = compile_scope(&e.fields, &e.name, &mut cx, 0)?;
        if let Some(name) = free.iter().next() {
            return Err(invalid(format!("{}: a template references unknown field '{name}'", e.name)));
        }
        let mut deps: Vec<usize> = cx.refs.iter().map(|(t, _)| *t).filter(|&t| t != i).collect();
        deps.sort_unstable();
        deps.dedup();
        edeps.push(deps);
        entities.push(CEntity { name: e.name.clone(), count: e.count, scope, refs: cx.refs });
    }
    let order = topo(&edeps).map_err(|cycle| {
        DataError::Cycle(format!("entities reference each other in a loop: {}", cycle.iter().map(|&i| schema.entities[i].name.as_str()).collect::<Vec<_>>().join(" → ")))
    })?;
    Ok(Plan { locale, entities, order })
}

/// Compiles one object level. Returns the scope and the names its templates reference that are
/// not fields of this level (resolved by an enclosing level).
fn compile_scope(fields: &[Field], prefix: &str, cx: &mut Cx, depth: usize) -> Result<(CScope, BTreeSet<String>)> {
    if depth > MAX_NESTING {
        return Err(invalid(format!("{prefix}: objects nested too deeply")));
    }
    let mut index = HashMap::new();
    for (i, f) in fields.iter().enumerate() {
        if f.name.is_empty() {
            return Err(invalid(format!("{prefix}: field #{} has no name", i + 1)));
        }
        if index.insert(f.name.clone(), i).is_some() {
            return Err(invalid(format!("{prefix}: duplicate field name '{}'", f.name)));
        }
    }
    let first_of = |pred: fn(&FieldKind) -> bool| fields.iter().position(|f| pred(&f.kind));
    let first = first_of(|k| matches!(k, FieldKind::FirstName));
    let last = first_of(|k| matches!(k, FieldKind::LastName));
    let full = first_of(|k| matches!(k, FieldKind::FullName));

    let mut cfields = Vec::with_capacity(fields.len());
    let mut deps = Vec::with_capacity(fields.len());
    let mut free = BTreeSet::new();
    for (i, f) in fields.iter().enumerate() {
        let path = format!("{prefix}.{}", f.name);
        if !(0.0..=100.0).contains(&f.null_percent) {
            return Err(invalid(format!("{path}: nullPercent must be between 0 and 100")));
        }
        let not_self = |x: Option<usize>| x.filter(|&j| j != i);
        let src = if first.is_some() || last.is_some() {
            NameSrc { first: not_self(first), last: not_self(last), full: None }
        } else {
            NameSrc { first: None, last: None, full: not_self(full) }
        };
        let mut wants = BTreeSet::new();
        let mut d = Vec::new();
        if let FieldKind::Ref { entity, field } = &f.kind
            && entity == cx.entity_name
        {
            if depth > 0 {
                return Err(invalid(format!("{path}: references to the same entity are only supported on top-level fields")));
            }
            if f.unique {
                return Err(invalid(format!("{path}: a unique reference to the same entity is not supported")));
            }
            match index.get(field) {
                Some(&j) if j != i => d.push(j),
                Some(_) => return Err(invalid(format!("{path}: a field cannot reference itself"))),
                None => {}
            }
        }
        let kind = compile_kind(&f.kind, &path, src, cx, depth, &mut wants)?;
        match &kind {
            CKind::FullName(s) | CKind::Username(s) | CKind::Email { src: s, .. } => d.extend([s.first, s.last, s.full].into_iter().flatten()),
            _ => {}
        }
        for name in wants {
            match index.get(&name) {
                Some(&j) if j == i => return Err(DataError::Cycle(format!("{path} references itself"))),
                Some(&j) => d.push(j),
                None => {
                    free.insert(name);
                }
            }
        }
        d.sort_unstable();
        d.dedup();
        deps.push(d);
        if f.unique && matches!(kind, CKind::Sequence { step: 0, .. }) {
            return Err(DataError::Unique(format!("{path}: a sequence with step 0 cannot be unique")));
        }
        cfields.push(CField {
            name: f.name.clone(),
            rng: DRng::seed_from_u64(stream_seed(cx.seed, cx.entity_name, &path)),
            path,
            null_ratio: f.null_percent / 100.0,
            unique: f.unique,
            kind,
            seen: HashSet::new(),
            shuffle: None,
            counter: 0,
        });
    }
    let order = topo(&deps).map_err(|cycle| {
        DataError::Cycle(format!("{prefix}: fields depend on each other in a loop: {}", cycle.iter().map(|&i| fields[i].name.as_str()).collect::<Vec<_>>().join(" → ")))
    })?;
    Ok((CScope { fields: cfields, order, index, deps }, free))
}

fn compile_kind(kind: &FieldKind, path: &str, src: NameSrc, cx: &mut Cx, depth: usize, wants: &mut BTreeSet<String>) -> Result<CKind> {
    let err = |msg: &str| invalid(format!("{path}: {msg}"));
    Ok(match kind {
        FieldKind::Uuid { version } => match version.unwrap_or(4) {
            4 => CKind::Uuid { v7: false },
            7 => CKind::Uuid { v7: true },
            v => return Err(err(&format!("unsupported UUID version {v} (use 4 or 7)"))),
        },
        FieldKind::Ulid => CKind::Ulid,
        FieldKind::Sequence { start, step } => CKind::Sequence { start: start.unwrap_or(1), step: step.unwrap_or(1) },
        FieldKind::FirstName => CKind::Text(Text::FirstName),
        FieldKind::LastName => CKind::Text(Text::LastName),
        FieldKind::FullName => CKind::FullName(NameSrc { full: None, ..src }),
        FieldKind::Username => CKind::Username(src),
        FieldKind::Email { domain } => {
            let domain = domain.as_deref().map(str::trim).filter(|d| !d.is_empty()).map(|d| d.trim_start_matches('@').to_string());
            CKind::Email { src, domain }
        }
        FieldKind::Phone => CKind::Text(Text::Phone),
        FieldKind::Url => CKind::Url,
        FieldKind::Domain => CKind::Domain,
        FieldKind::Ipv4 => CKind::Text(Text::Ipv4),
        FieldKind::Ipv6 => CKind::Text(Text::Ipv6),
        FieldKind::UserAgent => CKind::Text(Text::UserAgent),
        FieldKind::Password { min_length, max_length } => {
            let min = min_length.unwrap_or(10);
            let max = max_length.unwrap_or(min.max(16));
            if min == 0 || min > max || max > 4096 {
                return Err(err("password length must satisfy 1 ≤ minLength ≤ maxLength ≤ 4096"));
            }
            CKind::Text(Text::Password(min, max))
        }
        FieldKind::Street => CKind::Street,
        FieldKind::City => CKind::Text(Text::City),
        FieldKind::State => CKind::Text(Text::State),
        FieldKind::Zip => CKind::Text(Text::Zip),
        FieldKind::Country => CKind::Text(Text::Country),
        FieldKind::CountryCode => CKind::Text(Text::CountryCode),
        FieldKind::Latitude => CKind::Coord { max: 90.0 },
        FieldKind::Longitude => CKind::Coord { max: 180.0 },
        FieldKind::Company => CKind::Text(Text::Company),
        FieldKind::JobTitle => CKind::Text(Text::JobTitle),
        FieldKind::CatchPhrase => CKind::Text(Text::CatchPhrase),
        FieldKind::ProductName => CKind::ProductName,
        FieldKind::Price { min, max, decimals } => number(path, min.unwrap_or(1.0), max.unwrap_or(1000.0), decimals.unwrap_or(2))?,
        FieldKind::Currency => CKind::Text(Text::Currency),
        FieldKind::Word => CKind::Text(Text::Word),
        FieldKind::Sentence { min_words, max_words } => {
            let min = min_words.unwrap_or(4).clamp(1, 500);
            let max = max_words.unwrap_or(min.max(10)).clamp(1, 500);
            if min > max {
                return Err(err("minWords must not exceed maxWords"));
            }
            CKind::Text(Text::Sentence(min, max))
        }
        FieldKind::Paragraph { sentences } => CKind::Paragraph(sentences.unwrap_or(3).clamp(1, 200)),
        FieldKind::Lorem { words } => CKind::Lorem(words.unwrap_or(5).clamp(1, 10_000)),
        FieldKind::Int { min, max } => {
            let (min, max) = (min.unwrap_or(0), max.unwrap_or(1000));
            if min > max {
                return Err(err("min must not exceed max"));
            }
            CKind::Int { min, max }
        }
        FieldKind::Float { min, max, decimals } => number(path, min.unwrap_or(0.0), max.unwrap_or(1.0), decimals.unwrap_or(2))?,
        FieldKind::Bool { true_percent } => {
            let p = true_percent.unwrap_or(50.0);
            if !(0.0..=100.0).contains(&p) {
                return Err(err("truePercent must be between 0 and 100"));
            }
            CKind::Bool { p: p / 100.0 }
        }
        FieldKind::Date { from, to, format } => {
            let (from, to) = bounds(path, from.as_deref(), to.as_deref())?;
            CKind::Date { from, to, format: check_format(path, format.as_deref().unwrap_or(DATE_FORMAT))? }
        }
        FieldKind::Datetime { from, to, format } => {
            let (from, to) = bounds(path, from.as_deref(), to.as_deref())?;
            CKind::Date { from, to, format: check_format(path, format.as_deref().unwrap_or(DATETIME_FORMAT))? }
        }
        FieldKind::Timestamp { from, to, unit } => {
            let (from, to) = bounds(path, from.as_deref(), to.as_deref())?;
            CKind::Timestamp { from, to, ms: *unit == Some(TimestampUnit::Ms) }
        }
        FieldKind::Enum { values, weights } => {
            if values.is_empty() {
                return Err(err("enum needs at least one value"));
            }
            let weights = match weights {
                Some(w) if !w.is_empty() => {
                    if w.len() != values.len() {
                        return Err(err(&format!("{} weights for {} values", w.len(), values.len())));
                    }
                    if w.iter().any(|x| !x.is_finite() || *x < 0.0) || w.iter().sum::<f64>() <= 0.0 {
                        return Err(err("weights must be non-negative numbers with a positive sum"));
                    }
                    w.clone()
                }
                _ => vec![1.0; values.len()],
            };
            let mut acc = 0.0;
            let cumulative = weights.iter().map(|w| {
                acc += w;
                acc
            });
            CKind::Enum { values: values.clone(), cumulative: cumulative.collect() }
        }
        FieldKind::Regex { pattern } => CKind::Regex(RegexGen::compile(pattern).map_err(|e| err(&e.to_string()))?),
        FieldKind::Template { value } => {
            let parts = parse_template(value).map_err(|e| err(&e))?;
            for p in &parts {
                if let Part::Ref { path, .. } = p {
                    wants.insert(path[0].clone());
                }
            }
            CKind::Template(parts)
        }
        FieldKind::Ref { entity, field } => {
            let &t = cx.entity_index.get(entity.as_str()).ok_or_else(|| err(&format!("references unknown entity '{entity}'")))?;
            if !cx.schema.entities[t].fields.iter().any(|f| &f.name == field) {
                return Err(err(&format!("references unknown field '{entity}.{field}' (only top-level fields can be referenced)")));
            }
            if t == cx.entity && depth > 0 {
                return Err(err("references to the same entity are only supported on top-level fields"));
            }
            cx.refs.push((t, field.clone()));
            CKind::Ref { entity: t, field: field.clone() }
        }
        FieldKind::Object { fields } => {
            let (scope, inner_free) = compile_scope(fields, path, cx, depth + 1)?;
            wants.extend(inner_free);
            CKind::Object(scope)
        }
        FieldKind::Array { of, min, max } => {
            let (min, max) = (min.unwrap_or(1), max.unwrap_or_else(|| min.unwrap_or(1).max(3)));
            if min > max {
                return Err(err("min must not exceed max"));
            }
            if max > 100_000 {
                return Err(err("arrays are limited to 100000 items"));
            }
            let item_path = format!("{path}[]");
            let kind = compile_kind(of, &item_path, NameSrc::default(), cx, depth + 1, wants)?;
            let item = CField {
                name: String::new(),
                rng: DRng::seed_from_u64(stream_seed(cx.seed, cx.entity_name, &item_path)),
                path: item_path,
                null_ratio: 0.0,
                unique: false,
                kind,
                seen: HashSet::new(),
                shuffle: None,
                counter: 0,
            };
            CKind::Array { item: Box::new(item), min, max }
        }
        FieldKind::Json { value } => CKind::Const(value.clone()),
    })
}

fn number(path: &str, min: f64, max: f64, decimals: u32) -> Result<CKind> {
    if !min.is_finite() || !max.is_finite() || min > max {
        return Err(invalid(format!("{path}: min must be a number not exceeding max")));
    }
    Ok(CKind::Number { min, max, decimals: decimals.min(12) })
}

fn bounds(path: &str, from: Option<&str>, to: Option<&str>) -> Result<(i64, i64)> {
    let from_s = from.filter(|s| !s.trim().is_empty()).unwrap_or(DEFAULT_FROM);
    let to_s = to.filter(|s| !s.trim().is_empty()).unwrap_or(DEFAULT_TO);
    let from = parse_instant(from_s, false).ok_or_else(|| invalid(format!("{path}: cannot parse date '{from_s}' (use YYYY-MM-DD or RFC 3339)")))?;
    let to = parse_instant(to_s, true).ok_or_else(|| invalid(format!("{path}: cannot parse date '{to_s}' (use YYYY-MM-DD or RFC 3339)")))?;
    if from > to {
        return Err(invalid(format!("{path}: 'from' is after 'to'")));
    }
    Ok((from, to))
}

/// Seconds since the epoch. A bare date as an upper bound means the end of that day.
pub(crate) fn parse_instant(s: &str, end_of_day: bool) -> Option<i64> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    for f in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, f) {
            return Some(dt.and_utc().timestamp());
        }
    }
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    let t = if end_of_day { d.and_hms_opt(23, 59, 59)? } else { d.and_hms_opt(0, 0, 0)? };
    Some(t.and_utc().timestamp())
}

fn check_format(path: &str, format: &str) -> Result<String> {
    chrono::format::StrftimeItems::new(format).parse().map_err(|_| invalid(format!("{path}: invalid date format '{format}' (strftime, e.g. %Y-%m-%d)")))?;
    Ok(format.to_string())
}

fn parse_template(s: &str) -> std::result::Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start + 2..].find("}}") else { break };
        if start > 0 {
            parts.push(Part::Lit(rest[..start].to_string()));
        }
        let inner = &rest[start + 2..start + 2 + len];
        let mut pieces = inner.split('|').map(str::trim);
        let name = pieces.next().unwrap_or_default();
        if name.is_empty() {
            return Err("empty {{}} in template".into());
        }
        let path: Vec<String> = name.split('.').map(|p| p.trim().to_string()).collect();
        if path.iter().any(String::is_empty) {
            return Err(format!("invalid reference '{{{{{name}}}}}' in template"));
        }
        let filters = pieces
            .map(|f| match f {
                "lower" => Ok(Filter::Lower),
                "upper" => Ok(Filter::Upper),
                "slug" => Ok(Filter::Slug),
                "ascii" => Ok(Filter::Ascii),
                "alnum" => Ok(Filter::Alnum),
                "trim" => Ok(Filter::Trim),
                other => Err(format!("unknown template filter '{other}' (lower, upper, slug, ascii, alnum, trim)")),
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        parts.push(Part::Ref { path, filters });
        rest = &rest[start + 2 + len + 2..];
    }
    if !rest.is_empty() {
        parts.push(Part::Lit(rest.to_string()));
    }
    Ok(parts)
}

/// Stable topological order (lowest index first among ready nodes). `deps[i]` must come before
/// `i`. On a cycle, returns the nodes of one cycle (first node repeated at the end).
fn topo(deps: &[Vec<usize>]) -> std::result::Result<Vec<usize>, Vec<usize>> {
    let n = deps.len();
    let mut indeg = vec![0usize; n];
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, ds) in deps.iter().enumerate() {
        for &d in ds {
            indeg[i] += 1;
            users[d].push(i);
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut out = Vec::with_capacity(n);
    while let Some(i) = ready.pop_first() {
        out.push(i);
        for &u in &users[i] {
            indeg[u] -= 1;
            if indeg[u] == 0 {
                ready.insert(u);
            }
        }
    }
    if out.len() == n {
        return Ok(out);
    }
    // Walk dependencies among the remaining nodes until one repeats.
    let remaining: HashSet<usize> = (0..n).filter(|i| !out.contains(i)).collect();
    let start = *remaining.iter().min().unwrap_or(&0);
    let mut path = vec![start];
    let mut cur = start;
    loop {
        let next = deps[cur].iter().copied().find(|d| remaining.contains(d)).unwrap_or(cur);
        if let Some(pos) = path.iter().position(|&p| p == next) {
            let mut cycle: Vec<usize> = path[pos..].to_vec();
            cycle.reverse();
            cycle.push(cycle[0]);
            return Err(cycle);
        }
        path.push(next);
        cur = next;
    }
}

// ---------- generate ----------

struct Env<'a> {
    locale: Locale,
    pools: &'a [HashMap<String, Vec<Value>>],
    entity: &'a str,
    row: u64,
}

/// Values of the enclosing object levels, for template lookups.
struct Frames<'a> {
    index: &'a HashMap<String, usize>,
    values: &'a [Value],
    parent: Option<&'a Frames<'a>>,
}

impl Frames<'_> {
    fn lookup(&self, name: &str) -> Option<&Value> {
        match self.index.get(name) {
            Some(&i) => Some(&self.values[i]),
            None => self.parent.and_then(|p| p.lookup(name)),
        }
    }
}

fn to_map(fields: &[CField], values: Vec<Value>) -> Map<String, Value> {
    let mut m = Map::with_capacity(fields.len());
    for (f, v) in fields.iter().zip(values) {
        m.insert(f.name.clone(), v);
    }
    m
}

fn gen_scope(scope: &mut CScope, env: &Env, parent: Option<&Frames>, mask: Option<&[bool]>) -> Result<Vec<Value>> {
    let CScope { fields, order, index, .. } = scope;
    let mut values = vec![Value::Null; fields.len()];
    for &i in order.iter() {
        if mask.is_some_and(|m| !m[i]) {
            continue;
        }
        let v = {
            let frames = Frames { index, values: &values, parent };
            gen_field(&mut fields[i], env, &frames)?
        };
        values[i] = v;
    }
    Ok(values)
}

fn gen_field(f: &mut CField, env: &Env, frames: &Frames) -> Result<Value> {
    if f.null_ratio > 0.0 && f.rng.random::<f64>() < f.null_ratio {
        return Ok(Value::Null);
    }
    if !f.unique || !f.kind.needs_seen_set() {
        return gen_value(f, env, frames, 0);
    }
    for attempt in 0..UNIQUE_ATTEMPTS {
        let v = gen_value(f, env, frames, attempt)?;
        if v.is_null() || f.seen.insert(hash_value(&v)) {
            return Ok(v);
        }
    }
    Err(DataError::Unique(format!(
        "{}: no new unique value after {UNIQUE_ATTEMPTS} attempts at row {} ({} distinct so far) — widen the generator, add variety (e.g. a template with a sequence), or turn off unique",
        f.path,
        env.row + 1,
        f.seen.len()
    )))
}

fn exhausted(path: &str, n: u64, env: &Env) -> DataError {
    DataError::Unique(format!("{path}: only {n} distinct values are possible but row {} of {} needs another", env.row + 1, env.entity))
}

fn gen_value(f: &mut CField, env: &Env, frames: &Frames, attempt: u32) -> Result<Value> {
    let CField { kind, rng, shuffle, counter, unique, path, .. } = f;
    let locale = env.locale;
    Ok(match kind {
        CKind::Uuid { v7 } => {
            let bytes = rng.random::<u128>().to_le_bytes();
            let id = if *v7 {
                let ms = ID_EPOCH_MS + *counter * 1000 + rng.random_range(0..1000);
                let mut rand10 = [0u8; 10];
                rand10.copy_from_slice(&bytes[..10]);
                uuid::Builder::from_unix_timestamp_millis(ms, &rand10).into_uuid()
            } else {
                uuid::Builder::from_random_bytes(bytes).into_uuid()
            };
            *counter += 1;
            Value::String(id.to_string())
        }
        CKind::Ulid => {
            let ms = ID_EPOCH_MS + *counter * 1000 + rng.random_range(0..1000);
            *counter += 1;
            Value::String(ulid::Ulid::from_parts(ms, rng.random::<u128>()).to_string())
        }
        CKind::Sequence { start, step } => {
            let v = (*start as i128 + *step as i128 * *counter as i128).clamp(i64::MIN as i128, i64::MAX as i128) as i64;
            *counter += 1;
            Value::from(v)
        }
        CKind::Text(t) => Value::String(fakers::text(locale, *t, rng)),
        CKind::FullName(src) => {
            let (first, last) = name_parts(*src, frames.values, locale, rng);
            Value::String(if locale.family_first() { format!("{last}{first}") } else { format!("{first} {last}") })
        }
        CKind::Username(src) => {
            let (first, last) = name_parts(*src, frames.values, locale, rng);
            let (f, l) = slug_pair(&first, &last);
            let base = match rng.random_range(0..5) {
                0 => format!("{f}.{l}"),
                1 => format!("{f}_{l}"),
                2 => format!("{f}{}", rng.random_range(1..100)),
                3 => format!("{}{l}", &f[..1]),
                _ => format!("{f}{l}{}", rng.random_range(1..1000)),
            };
            Value::String(with_suffix(base, attempt, rng))
        }
        CKind::Email { src, domain } => {
            let (first, last) = name_parts(*src, frames.values, locale, rng);
            let (f, l) = slug_pair(&first, &last);
            let local = match rng.random_range(0..5) {
                0 | 1 => format!("{f}.{l}"),
                2 => format!("{f}{l}"),
                3 => format!("{}.{l}", &f[..1]),
                _ => format!("{f}_{l}"),
            };
            let local = with_suffix(local, attempt, rng);
            let domain = match domain {
                Some(d) => d.clone(),
                None => fakers::pick(fakers::SAFE_EMAIL_DOMAINS, rng).to_string(),
            };
            Value::String(format!("{local}@{domain}"))
        }
        CKind::Url => {
            let domain = make_domain(locale, rng);
            Value::String(if rng.random_bool(0.5) {
                format!("https://{domain}")
            } else {
                format!("https://www.{domain}/{}", fakers::ascii_slug(&fakers::text(locale, Text::Word, rng)))
            })
        }
        CKind::Domain => Value::String(make_domain(locale, rng)),
        CKind::Street => {
            let number = fakers::text(locale, Text::BuildingNumber, rng);
            let street = fakers::text(locale, Text::StreetName, rng);
            Value::String(match locale {
                Locale::DeDe | Locale::NlNl | Locale::ItIt | Locale::PtBr | Locale::PtPt | Locale::TrTr => format!("{street} {number}"),
                _ => format!("{number} {street}"),
            })
        }
        CKind::Coord { max } => float_value(fakers::round_to(rng.random_range(-*max..=*max), 6), 6),
        CKind::ProductName => Value::String(fakers::product_name(rng)),
        CKind::Paragraph(n) => Value::String(fakers::paragraph(locale, *n, rng)),
        CKind::Lorem(n) => Value::String(fakers::lorem_words(locale, *n, rng)),
        CKind::Number { min, max, decimals } => {
            let x = if min == max { *min } else { rng.random_range(*min..=*max) };
            float_value(fakers::round_to(x, *decimals), *decimals)
        }
        CKind::Int { min, max } => {
            if *unique {
                let size = (*max as i128 - *min as i128 + 1).min(u64::MAX as i128) as u64;
                let s = shuffle.get_or_insert_with(|| SparseShuffle::new(size));
                let off = s.next(rng).ok_or_else(|| exhausted(path, size, env))?;
                Value::from((*min as i128 + off as i128) as i64)
            } else {
                Value::from(rng.random_range(*min..=*max))
            }
        }
        CKind::Bool { p } => {
            if *unique {
                let s = shuffle.get_or_insert_with(|| SparseShuffle::new(2));
                Value::Bool(s.next(rng).ok_or_else(|| exhausted(path, 2, env))? == 1)
            } else {
                Value::Bool(rng.random_bool(*p))
            }
        }
        CKind::Date { from, to, format } => {
            let secs = rng.random_range(*from..=*to);
            let dt = DateTime::from_timestamp(secs, 0).unwrap_or_default();
            Value::String(dt.format(format).to_string())
        }
        CKind::Timestamp { from, to, ms } => {
            let secs = rng.random_range(*from..=*to);
            Value::from(if *ms { secs * 1000 + rng.random_range(0..1000) } else { secs })
        }
        CKind::Enum { values, cumulative } => {
            if *unique {
                let size = values.len() as u64;
                let s = shuffle.get_or_insert_with(|| SparseShuffle::new(size));
                values[s.next(rng).ok_or_else(|| exhausted(path, size, env))? as usize].clone()
            } else {
                let total = *cumulative.last().unwrap_or(&1.0);
                let r = rng.random::<f64>() * total;
                let i = cumulative.partition_point(|&c| c <= r).min(values.len() - 1);
                values[i].clone()
            }
        }
        CKind::Regex(g) => Value::String(g.sample(rng)),
        CKind::Template(parts) => Value::String(render_template(parts, frames)),
        CKind::Ref { entity, field } => {
            let pool = env.pools.get(*entity).and_then(|p| p.get(field.as_str()));
            match pool {
                None => Value::Null,
                Some(pool) if pool.is_empty() => {
                    if *unique {
                        return Err(exhausted(path, 0, env));
                    }
                    Value::Null
                }
                Some(pool) => {
                    if *unique {
                        let size = pool.len() as u64;
                        let s = shuffle.get_or_insert_with(|| SparseShuffle::new(size));
                        pool[s.next(rng).ok_or_else(|| exhausted(path, size, env))? as usize].clone()
                    } else {
                        pool[rng.random_range(0..pool.len())].clone()
                    }
                }
            }
        }
        CKind::Object(scope) => {
            let values = gen_scope(scope, env, Some(frames), None)?;
            Value::Object(to_map(&scope.fields, values))
        }
        CKind::Array { item, min, max } => {
            let n = rng.random_range(*min..=*max);
            let mut out = Vec::with_capacity(n as usize);
            for _ in 0..n {
                out.push(gen_field(item, env, frames)?);
            }
            Value::Array(out)
        }
        CKind::Const(v) => v.clone(),
    })
}

fn float_value(x: f64, decimals: u32) -> Value {
    if decimals == 0 && x.abs() < 9.0e15 {
        return Value::from(x as i64);
    }
    Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

fn name_parts(src: NameSrc, values: &[Value], locale: Locale, rng: &mut DRng) -> (String, String) {
    let get = |i: Option<usize>| i.and_then(|i| values.get(i)).and_then(Value::as_str).map(str::to_string);
    let (mut first, mut last) = (get(src.first), get(src.last));
    if first.is_none()
        && last.is_none()
        && let Some(full) = get(src.full)
    {
        let words: Vec<&str> = full.split_whitespace().collect();
        if words.len() >= 2 {
            first = Some(words[0].to_string());
            last = Some(words[words.len() - 1].to_string());
        } else {
            first = words.first().map(|w| w.to_string());
        }
    }
    let first = first.unwrap_or_else(|| fakers::text(locale, Text::FirstName, rng));
    let last = last.unwrap_or_else(|| fakers::text(locale, Text::LastName, rng));
    (first, last)
}

fn slug_pair(first: &str, last: &str) -> (String, String) {
    let f = fakers::ascii_slug(first);
    let l = fakers::ascii_slug(last);
    (if f.is_empty() { "user".into() } else { f }, if l.is_empty() { "dev".into() } else { l })
}

/// Retries of unique derived values get a growing numeric suffix.
fn with_suffix(base: String, attempt: u32, rng: &mut DRng) -> String {
    if attempt == 0 {
        return base;
    }
    let digits = (2 + attempt / 4).min(9);
    format!("{base}{}", rng.random_range(1..10u64.pow(digits)))
}

fn make_domain(locale: Locale, rng: &mut DRng) -> String {
    let mut name = fakers::ascii_slug(&fakers::text(locale, Text::LastName, rng));
    if name.is_empty() {
        name = fakers::ascii_slug(&fakers::text(Locale::En, Text::LastName, rng));
    }
    if rng.random_bool(0.3) {
        let other = fakers::ascii_slug(&fakers::text(Locale::En, Text::LastName, rng));
        name = format!("{name}-{other}");
    }
    let mut suffix = fakers::text(locale, Text::DomainSuffix, rng).trim_start_matches('.').to_ascii_lowercase();
    if suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        suffix = "com".into();
    }
    format!("{name}.{suffix}")
}

fn render_template(parts: &[Part], frames: &Frames) -> String {
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Lit(s) => out.push_str(s),
            Part::Ref { path, filters } => {
                let mut v = frames.lookup(&path[0]);
                for seg in &path[1..] {
                    v = v.and_then(|x| match x {
                        Value::Object(m) => m.get(seg.as_str()),
                        Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
                        _ => None,
                    });
                }
                let mut s = match v {
                    None | Some(Value::Null) => String::new(),
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                };
                for f in filters {
                    s = match f {
                        Filter::Lower => s.to_lowercase(),
                        Filter::Upper => s.to_uppercase(),
                        Filter::Ascii => deunicode::deunicode(&s),
                        Filter::Alnum => fakers::ascii_slug(&s),
                        Filter::Trim => s.trim().to_string(),
                        Filter::Slug => slug(&s),
                    };
                }
                out.push_str(&s);
            }
        }
    }
    out
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in deunicode::deunicode(s).chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

// ---------- helpers ----------

/// Uniform sampling without replacement from `0..n` in O(1) per draw (lazy Fisher–Yates).
struct SparseShuffle {
    n: u64,
    taken: u64,
    swaps: HashMap<u64, u64>,
}

impl SparseShuffle {
    fn new(n: u64) -> Self {
        Self { n, taken: 0, swaps: HashMap::new() }
    }

    fn next(&mut self, rng: &mut DRng) -> Option<u64> {
        if self.taken >= self.n {
            return None;
        }
        let j = rng.random_range(self.taken..self.n);
        let at_j = self.swaps.get(&j).copied().unwrap_or(j);
        let at_t = self.swaps.remove(&self.taken).unwrap_or(self.taken);
        if j != self.taken {
            self.swaps.insert(j, at_t);
        }
        self.taken += 1;
        Some(at_j)
    }
}

pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn stream_seed(seed: u64, entity: &str, path: &str) -> u64 {
    splitmix(splitmix(seed ^ fnv1a(entity.as_bytes())) ^ fnv1a(path.as_bytes()))
}

fn hash_value(v: &Value) -> u64 {
    match v {
        Value::String(s) => fnv1a(s.as_bytes()) ^ 0x5bd1_e995,
        other => fnv1a(other.to_string().as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topo_is_stable_and_finds_cycles() {
        assert_eq!(topo(&[vec![], vec![], vec![]]).unwrap(), vec![0, 1, 2]);
        assert_eq!(topo(&[vec![2], vec![], vec![1]]).unwrap(), vec![1, 2, 0]);
        let cycle = topo(&[vec![1], vec![2], vec![0], vec![]]).unwrap_err();
        assert_eq!(cycle.first(), cycle.last());
        assert_eq!(cycle.len(), 4);
        assert_eq!(topo(&[vec![0]]).unwrap_err(), vec![0, 0]);
    }

    #[test]
    fn sparse_shuffle_is_a_permutation() {
        let mut rng = DRng::seed_from_u64(7);
        let mut s = SparseShuffle::new(1000);
        let mut seen: Vec<u64> = std::iter::from_fn(|| s.next(&mut rng)).collect();
        assert_eq!(seen.len(), 1000);
        seen.sort_unstable();
        assert_eq!(seen, (0..1000).collect::<Vec<_>>());
    }

    #[test]
    fn templates_parse() {
        let parts = parse_template("{{ first | lower }}.{{last|upper}}@x {{a.b}}").unwrap();
        assert_eq!(parts.len(), 5);
        assert!(parse_template("{{x|nope}}").is_err());
        assert!(parse_template("{{}}").is_err());
        // Unclosed braces stay literal.
        assert!(matches!(&parse_template("a {{b").unwrap()[..], [Part::Lit(s)] if s == "a {{b"));
    }

    #[test]
    fn instants() {
        assert_eq!(parse_instant("1970-01-01", false), Some(0));
        assert_eq!(parse_instant("1970-01-01", true), Some(86_399));
        assert_eq!(parse_instant("1970-01-01T00:01:00Z", false), Some(60));
        assert_eq!(parse_instant("nope", false), None);
    }
}
