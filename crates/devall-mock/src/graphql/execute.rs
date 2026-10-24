//! Executes a validated operation against a schema with generated data: every field gets a value
//! shaped by its type and selection set ([`super::gen`]), abstract types resolve to a concrete
//! object type, and introspection (`__schema`, `__type`, `__typename`) is answered from the
//! schema itself. Stub data (`overrides`, shaped like the response, keyed by alias) replaces the
//! generated value wherever it is present — lists take the override's length, objects are merged
//! field by field, and `__typename` in an override picks the concrete type.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::{Map, Value as Json, json};

use super::ast::*;
use super::fake;
use super::schema::Schema;

#[derive(Debug, Clone, Copy)]
pub struct ExecOptions {
    pub seed: u64,
    /// Items in generated lists (a `first`/`last`/`limit`/… argument wins).
    pub list_length: usize,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self { seed: 42, list_length: 2 }
    }
}

enum Src<'v> {
    /// Generated data, with the stub's override for this object (if any).
    Mock(Option<&'v Json>),
    /// An introspection object.
    Json(&'v Json),
}

pub struct Executor<'a> {
    schema: &'a Schema,
    fragments: HashMap<&'a str, &'a Fragment>,
    vars: Map<String, Json>,
    opts: ExecOptions,
    intro: Option<Json>,
}

const LIST_ARGS: [&str; 7] = ["first", "last", "limit", "take", "count", "pageSize", "size"];

impl<'a> Executor<'a> {
    pub fn new(schema: &'a Schema, doc: &'a Document, vars: Map<String, Json>, opts: ExecOptions) -> Self {
        Self { schema, fragments: doc.fragments().map(|f| (f.name.as_str(), f)).collect(), vars, opts, intro: None }
    }

    /// `data` for `op`.
    pub fn run(&mut self, op: &Operation, overrides: Option<&Json>) -> Json {
        let root = self.schema.root(op.kind).map(|t| t.name.clone()).unwrap_or_else(|| self.schema.query.clone());
        self.object(&root, &op.selection, Src::Mock(overrides), "", true, None)
    }

    fn arg(&self, f: &Field, name: &str) -> Option<Json> {
        f.args.iter().find(|(k, _)| k == name).map(|(_, v)| v.to_json(&self.vars))
    }

    fn directive_skips(&self, dirs: &[Directive]) -> bool {
        let cond = |d: &Directive| d.args.iter().find(|(k, _)| k == "if").map(|(_, v)| v.to_json(&self.vars)).and_then(|v| v.as_bool()).unwrap_or(false);
        dirs.iter().any(|d| (d.name == "skip" && cond(d)) || (d.name == "include" && !cond(d)))
    }

    /// Spec `CollectFields`: response key → fields (in order), honouring skip/include and fragments.
    fn collect<'s>(&self, type_name: &str, sel: &'s [Selection], out: &mut IndexMap<String, Vec<&'s Field>>, seen: &mut Vec<&'s str>)
    where
        'a: 's,
    {
        for s in sel {
            match s {
                Selection::Field(f) => {
                    if !self.directive_skips(&f.directives) {
                        out.entry(f.key().to_string()).or_default().push(f);
                    }
                }
                Selection::Inline { type_cond, directives, selection, .. } => {
                    if self.directive_skips(directives) || type_cond.as_deref().is_some_and(|t| !self.schema.type_applies(t, type_name)) {
                        continue;
                    }
                    self.collect(type_name, selection, out, seen);
                }
                Selection::Spread { name, directives, .. } => {
                    if self.directive_skips(directives) || seen.contains(&name.as_str()) {
                        continue;
                    }
                    let Some(frag) = self.fragments.get(name.as_str()).copied() else { continue };
                    if !self.schema.type_applies(&frag.type_cond, type_name) {
                        continue;
                    }
                    seen.push(frag.name.as_str());
                    self.collect(type_name, &frag.selection, out, seen);
                }
            }
        }
    }

    fn introspection(&mut self) -> Json {
        self.intro.get_or_insert_with(|| self.schema.introspection()).clone()
    }

    #[allow(clippy::too_many_arguments)]
    fn object(&mut self, type_name: &str, sel: &[Selection], src: Src<'_>, path: &str, root: bool, id_hint: Option<&Json>) -> Json {
        let mut groups = IndexMap::new();
        self.collect(type_name, sel, &mut groups, &mut Vec::new());
        let mut out = Map::new();
        // `$ref` introspection types expand to the full type when anything but kind/name is asked.
        let expanded;
        let src = match src {
            Src::Json(node) if node.get("$ref").is_some() && groups.values().any(|fs| !matches!(fs[0].name.as_str(), "kind" | "name" | "ofType" | "__typename")) => {
                let name = node.get("$ref").and_then(Json::as_str).unwrap_or_default();
                expanded = self.schema.get(name).map(|t| self.schema.type_json(t)).unwrap_or(Json::Null);
                Src::Json(&expanded)
            }
            other => other,
        };
        for (key, fields) in groups {
            let f = fields[0];
            if f.name == "__typename" {
                out.insert(key, json!(type_name));
                continue;
            }
            let merged: Vec<Selection> = fields.iter().flat_map(|x| x.selection.iter().cloned()).collect();
            let child_path = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
            let value = match &src {
                Src::Mock(over) if root && f.name == "__schema" => {
                    let intro = self.introspection();
                    let _ = over;
                    self.object("__Schema", &merged, Src::Json(&intro), &child_path, false, None)
                }
                Src::Mock(_) if root && f.name == "__type" => {
                    let name = self.arg(f, "name").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                    match self.schema.get(&name).map(|t| self.schema.type_json(t)) {
                        Some(t) => self.object("__Type", &merged, Src::Json(&t), &child_path, false, None),
                        None => Json::Null,
                    }
                }
                Src::Mock(over) => {
                    let Some(def) = self.schema.field_of(type_name, &f.name).cloned() else {
                        out.insert(key, Json::Null);
                        continue;
                    };
                    let ov = over.and_then(|o| o.get(&key));
                    if f.name == "id" && ov.is_none()
                        && let Some(h) = id_hint
                    {
                        out.insert(key, h.clone());
                        continue;
                    }
                    let list_len = LIST_ARGS.iter().find_map(|a| self.arg(f, a)).and_then(|v| v.as_u64()).map(|n| (n as usize).min(100));
                    // IDs serialise as strings.
                    let hint = self.arg(f, "id").and_then(|v| match v {
                        Json::String(_) => Some(v),
                        Json::Number(n) => Some(Json::String(n.to_string())),
                        _ => None,
                    });
                    self.mock(&def.ty, &merged, ov, &child_path, &f.name, list_len, hint.as_ref())
                }
                Src::Json(node) => {
                    let raw = node.get(&f.name).cloned().unwrap_or(Json::Null);
                    let include_deprecated = self.arg(f, "includeDeprecated").and_then(|v| v.as_bool()).unwrap_or(false);
                    let raw = match raw {
                        Json::Array(items) if !include_deprecated && matches!(f.name.as_str(), "fields" | "enumValues" | "inputFields" | "args") => {
                            Json::Array(items.into_iter().filter(|i| !i.get("isDeprecated").and_then(Json::as_bool).unwrap_or(false)).collect())
                        }
                        other => other,
                    };
                    let ty = self.schema.field_of(type_name, &f.name).map(|d| d.ty.clone());
                    match ty {
                        Some(ty) => self.json(&ty, &merged, &raw, &child_path),
                        None => raw,
                    }
                }
            };
            out.insert(key, value);
        }
        Json::Object(out)
    }

    /// Projects an introspection value through a selection set.
    fn json(&mut self, ty: &TypeRef, sel: &[Selection], raw: &Json, path: &str) -> Json {
        match ty {
            TypeRef::NonNull(inner) => self.json(inner, sel, raw, path),
            TypeRef::List(inner) => match raw {
                Json::Array(items) => Json::Array(items.iter().enumerate().map(|(i, x)| self.json(inner, sel, x, &format!("{path}.{i}"))).collect()),
                _ => Json::Null,
            },
            TypeRef::Named(n) => {
                if raw.is_null() || self.schema.is_leaf(n) {
                    raw.clone()
                } else {
                    self.object(n, sel, Src::Json(raw), path, false, None)
                }
            }
        }
    }

    /// The concrete type for an abstract one: the override's `__typename`, else the first
    /// possible type the selection asks about, else a seeded pick.
    fn concrete(&self, abstract_name: &str, sel: &[Selection], ov: Option<&Json>, path: &str) -> Option<String> {
        let possible = self.schema.possible_types(abstract_name);
        if let Some(t) = ov.and_then(|o| o.get("__typename")).and_then(Json::as_str).filter(|t| possible.contains(t)) {
            return Some(t.to_string());
        }
        let mut conds = Vec::new();
        self.conditions(sel, &mut conds, 0);
        if let Some(t) = conds.iter().find(|c| possible.contains(&c.as_str())) {
            return Some(t.clone());
        }
        if possible.is_empty() {
            return None;
        }
        let mut rng = fake::rng_for(self.opts.seed, &format!("{path}#type"));
        Some(possible[rng.usize(..possible.len())].to_string())
    }

    fn conditions(&self, sel: &[Selection], out: &mut Vec<String>, depth: usize) {
        if depth > 8 {
            return;
        }
        for s in sel {
            match s {
                Selection::Inline { type_cond: Some(t), .. } => out.push(t.clone()),
                Selection::Spread { name, .. } => {
                    if let Some(f) = self.fragments.get(name.as_str()) {
                        out.push(f.type_cond.clone());
                        self.conditions(&f.selection, out, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn mock(&mut self, ty: &TypeRef, sel: &[Selection], ov: Option<&Json>, path: &str, field: &str, list_len: Option<usize>, id_hint: Option<&Json>) -> Json {
        match ty {
            TypeRef::NonNull(inner) => self.mock(inner, sel, ov, path, field, list_len, id_hint),
            TypeRef::List(inner) => match ov {
                Some(Json::Array(items)) => Json::Array(items.iter().enumerate().map(|(i, x)| self.mock(inner, sel, Some(x), &format!("{path}.{i}"), field, None, None)).collect()),
                Some(Json::Null) => Json::Null,
                Some(single) => Json::Array(vec![self.mock(inner, sel, Some(single), &format!("{path}.0"), field, None, None)]),
                None => {
                    let n = list_len.unwrap_or(self.opts.list_length);
                    Json::Array((0..n).map(|i| self.mock(inner, sel, None, &format!("{path}.{i}"), field, None, None)).collect())
                }
            },
            TypeRef::Named(n) => {
                let Some(t) = self.schema.get(n) else { return Json::Null };
                match t.kind {
                    TypeKind::Scalar => match ov {
                        Some(v) => v.clone(),
                        None => fake::scalar(n, field, &mut fake::rng_for(self.opts.seed, path)),
                    },
                    TypeKind::Enum => match ov {
                        Some(v) => v.clone(),
                        None => {
                            let values: Vec<&EnumValueDef> = t.enum_values.iter().filter(|e| e.deprecated.is_none()).collect();
                            if values.is_empty() {
                                Json::Null
                            } else {
                                json!(values[fake::rng_for(self.opts.seed, path).usize(..values.len())].name)
                            }
                        }
                    },
                    TypeKind::Object => match ov {
                        Some(Json::Null) => Json::Null,
                        Some(o @ Json::Object(_)) => self.object(n, sel, Src::Mock(Some(o)), path, false, id_hint),
                        _ => self.object(n, sel, Src::Mock(None), path, false, id_hint),
                    },
                    TypeKind::Interface | TypeKind::Union => {
                        if matches!(ov, Some(Json::Null)) {
                            return Json::Null;
                        }
                        let Some(concrete) = self.concrete(n, sel, ov, path) else { return Json::Null };
                        let ov = ov.filter(|o| o.is_object());
                        self.object(&concrete, sel, Src::Mock(ov), path, false, id_hint)
                    }
                    TypeKind::InputObject => Json::Null,
                }
            }
        }
    }
}
