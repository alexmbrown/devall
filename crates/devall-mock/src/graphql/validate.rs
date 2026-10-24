//! Query validation against a schema — the spec rules a mock needs to reject what a real server
//! would (messages follow graphql-js): executable definitions, operation/fragment/variable
//! uniqueness, lone anonymous operation, fields on correct type, leaf selections, known and
//! required arguments, literal value types, known types/fragments/directives, fragments on
//! composite types, possible spreads, fragment cycles, undefined/unused variables and fragments,
//! variables of input types used in compatible positions, and variable value coercion.
//! Not checked: overlapping field merging and stream/defer rules.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value as Json};

use super::GqlError;
use super::ast::*;
use super::schema::Schema;

struct V<'a> {
    schema: &'a Schema,
    fragments: HashMap<&'a str, &'a Fragment>,
    errors: Vec<GqlError>,
    /// Variable definitions of the operation being walked (`None` inside a lone fragment walk).
    var_defs: Option<&'a [VarDef]>,
    used_vars: Vec<(String, Pos)>,
    used_fragments: HashSet<String>,
    /// Fragments already part of a reported cycle.
    in_cycle: HashSet<String>,
}

pub fn validate(schema: &Schema, doc: &Document) -> Vec<GqlError> {
    let mut v = V { schema, fragments: HashMap::new(), errors: Vec::new(), var_defs: None, used_vars: Vec::new(), used_fragments: HashSet::new(), in_cycle: HashSet::new() };
    for d in &doc.definitions {
        match d {
            Definition::Type { def, .. } => v.errors.push(GqlError::new(format!("The \"{}\" definition is not executable.", def.name))),
            Definition::Schema(_) => v.errors.push(GqlError::new("The schema definition is not executable.")),
            Definition::Directive(d) => v.errors.push(GqlError::new(format!("The \"@{}\" definition is not executable.", d.name))),
            _ => {}
        }
    }
    for f in doc.fragments() {
        if v.fragments.insert(f.name.as_str(), f).is_some() {
            v.errors.push(GqlError::at(format!("There can be only one fragment named \"{}\".", f.name), f.pos));
        }
    }
    let ops: Vec<&Operation> = doc.operations().collect();
    let mut names = HashSet::new();
    for op in &ops {
        match &op.name {
            Some(n) => {
                if !names.insert(n.as_str()) {
                    v.errors.push(GqlError::at(format!("There can be only one operation named \"{n}\"."), op.pos));
                }
            }
            None if ops.len() > 1 => v.errors.push(GqlError::at("This anonymous operation must be the only defined operation.", op.pos)),
            None => {}
        }
    }
    if ops.is_empty() && v.errors.is_empty() {
        v.errors.push(GqlError::new("The document does not contain an operation."));
    }
    let mut all_used = HashSet::new();
    for op in &ops {
        v.operation(op);
        all_used.extend(v.used_fragments.drain());
    }
    // Fragments: type conditions, cycles, unused.
    let frags: Vec<&Fragment> = doc.fragments().collect();
    for f in &frags {
        v.var_defs = None;
        if !v.schema.types.contains_key(&f.type_cond) {
            v.errors.push(GqlError::at(format!("Unknown type \"{}\".", f.type_cond), f.pos));
        } else if !v.schema.is_composite(&f.type_cond) {
            v.errors.push(GqlError::at(format!("Fragment \"{}\" cannot condition on non composite type \"{}\".", f.name, f.type_cond), f.pos));
        }
        if !all_used.contains(&f.name) {
            v.errors.push(GqlError::at(format!("Fragment \"{}\" is never used.", f.name), f.pos));
        }
        if !v.in_cycle.contains(&f.name) {
            let mut path = vec![f.name.as_str()];
            v.cycles(&f.selection, &mut path);
        }
    }
    // Fragments spread in several places report their problems once.
    let mut seen = HashSet::new();
    v.errors.retain(|e| seen.insert((e.message.clone(), e.locations.clone())));
    v.errors
}

impl<'a> V<'a> {
    fn operation(&mut self, op: &'a Operation) {
        self.var_defs = Some(&op.variables);
        self.used_vars.clear();
        let mut seen = HashSet::new();
        for d in &op.variables {
            if !seen.insert(d.name.as_str()) {
                self.errors.push(GqlError::at(format!("There can be only one variable named \"${}\".", d.name), d.pos));
            }
            let base = d.ty.named();
            if !self.schema.types.contains_key(base) {
                self.errors.push(GqlError::at(format!("Unknown type \"{base}\"."), d.pos));
            } else if !self.schema.is_input(base) {
                self.errors.push(GqlError::at(format!("Variable \"${}\" cannot be non-input type \"{}\".", d.name, d.ty), d.pos));
            } else if let Some(def) = &d.default {
                self.value(def, &d.ty, d.pos);
            }
        }
        self.directives(&op.directives, op.kind.keyword().to_ascii_uppercase().as_str());
        let Some(root) = self.schema.root(op.kind) else {
            let what = match op.kind {
                OpKind::Mutation => "mutations",
                OpKind::Subscription => "subscriptions",
                OpKind::Query => "queries",
            };
            self.errors.push(GqlError::at(format!("Schema is not configured for {what}."), op.pos));
            return;
        };
        let root_name = root.name.clone();
        let mut visited = HashSet::new();
        self.selection(&op.selection, &root_name, &mut visited);
        let defined: HashSet<&str> = op.variables.iter().map(|d| d.name.as_str()).collect();
        let used: Vec<(String, Pos)> = std::mem::take(&mut self.used_vars);
        let mut reported = HashSet::new();
        for (u, pos) in &used {
            if !defined.contains(u.as_str()) && reported.insert(u.clone()) {
                self.errors.push(GqlError::at(
                    match &op.name {
                        Some(n) => format!("Variable \"${u}\" is not defined by operation \"{n}\"."),
                        None => format!("Variable \"${u}\" is not defined."),
                    },
                    *pos,
                ));
            }
        }
        for d in &op.variables {
            if !used.iter().any(|(u, _)| u == &d.name) {
                self.errors.push(GqlError::at(
                    match &op.name {
                        Some(n) => format!("Variable \"${}\" is never used in operation \"{n}\".", d.name),
                        None => format!("Variable \"${}\" is never used.", d.name),
                    },
                    d.pos,
                ));
            }
        }
    }

    fn cycles(&mut self, sel: &'a [Selection], path: &mut Vec<&'a str>) {
        for s in sel {
            match s {
                Selection::Field(f) => self.cycles(&f.selection, path),
                Selection::Inline { selection, .. } => self.cycles(selection, path),
                Selection::Spread { name, pos, .. } => {
                    if path.first() == Some(&name.as_str()) {
                        let via: Vec<String> = path[1..].iter().map(|p| format!("\"{p}\"")).collect();
                        let msg = if via.is_empty() {
                            format!("Cannot spread fragment \"{name}\" within itself.")
                        } else {
                            format!("Cannot spread fragment \"{name}\" within itself via {}.", via.join(", "))
                        };
                        self.in_cycle.extend(path.iter().map(|p| p.to_string()));
                        self.errors.push(GqlError::at(msg, *pos));
                    } else if !path.contains(&name.as_str())
                        && let Some(f) = self.fragments.get(name.as_str()).copied()
                    {
                        path.push(f.name.as_str());
                        self.cycles(&f.selection, path);
                        path.pop();
                    }
                }
            }
        }
    }

    fn directives(&mut self, dirs: &[Directive], location: &str) {
        for d in dirs {
            let Some(def) = self.schema.directives.get(&d.name) else {
                self.errors.push(GqlError::at(format!("Unknown directive \"@{}\".", d.name), d.pos));
                continue;
            };
            if !def.locations.iter().any(|l| l == location) {
                self.errors.push(GqlError::at(format!("Directive \"@{}\" may not be used on {location}.", d.name), d.pos));
            }
            let def = def.clone();
            self.arguments(&d.args, &def.args, None, &d.name, d.pos);
        }
    }

    /// `field` = `Some((parent, name))` for field arguments, `None` + `directive` for directives.
    fn arguments(&mut self, args: &[(String, Value)], defs: &[InputValueDef], field: Option<(&str, &str)>, directive: &str, pos: Pos) {
        for (name, value) in args {
            match defs.iter().find(|d| &d.name == name) {
                Some(def) => self.value(value, &def.ty, pos),
                None => {
                    let msg = match field {
                        Some((parent, f)) => format!("Unknown argument \"{name}\" on field \"{parent}.{f}\"."),
                        None => format!("Unknown argument \"{name}\" on directive \"@{directive}\"."),
                    };
                    self.errors.push(GqlError::at(msg, pos));
                    self.collect_vars(value, pos);
                }
            }
        }
        for def in defs {
            if def.ty.is_non_null() && def.default.is_none() && !args.iter().any(|(n, _)| n == &def.name) {
                let owner = match field {
                    Some((_, f)) => format!("Field \"{f}\""),
                    None => format!("Directive \"@{directive}\""),
                };
                self.errors.push(GqlError::at(format!("{owner} argument \"{}\" of type \"{}\" is required, but it was not provided.", def.name, def.ty), pos));
            }
        }
    }

    fn selection(&mut self, sel: &'a [Selection], parent: &str, visited: &mut HashSet<&'a str>) {
        for s in sel {
            match s {
                Selection::Field(f) => self.field(f, parent, visited),
                Selection::Inline { type_cond, directives, selection, pos } => {
                    self.directives(directives, "INLINE_FRAGMENT");
                    let ty = match type_cond {
                        Some(t) => {
                            if !self.schema.types.contains_key(t) {
                                self.errors.push(GqlError::at(format!("Unknown type \"{t}\"."), *pos));
                                continue;
                            }
                            if !self.schema.is_composite(t) {
                                self.errors.push(GqlError::at(format!("Fragment cannot condition on non composite type \"{t}\"."), *pos));
                                continue;
                            }
                            if !self.schema.types_overlap(t, parent) {
                                self.errors.push(GqlError::at(format!("Fragment cannot be spread here as objects of type \"{parent}\" can never be of type \"{t}\"."), *pos));
                            }
                            t.clone()
                        }
                        None => parent.to_string(),
                    };
                    self.selection(selection, &ty, visited);
                }
                Selection::Spread { name, directives, pos } => {
                    self.directives(directives, "FRAGMENT_SPREAD");
                    self.used_fragments.insert(name.clone());
                    let Some(frag) = self.fragments.get(name.as_str()).copied() else {
                        self.errors.push(GqlError::at(format!("Unknown fragment \"{name}\"."), *pos));
                        continue;
                    };
                    if self.schema.is_composite(&frag.type_cond) && !self.schema.types_overlap(&frag.type_cond, parent) {
                        self.errors.push(GqlError::at(
                            format!("Fragment \"{name}\" cannot be spread here as objects of type \"{parent}\" can never be of type \"{}\".", frag.type_cond),
                            *pos,
                        ));
                    }
                    if visited.insert(frag.name.as_str()) && self.schema.is_composite(&frag.type_cond) {
                        self.directives(&frag.directives, "FRAGMENT_DEFINITION");
                        let cond = frag.type_cond.clone();
                        self.selection(&frag.selection, &cond, visited);
                        visited.remove(frag.name.as_str());
                    }
                }
            }
        }
    }

    fn field(&mut self, f: &'a Field, parent: &str, visited: &mut HashSet<&'a str>) {
        self.directives(&f.directives, "FIELD");
        if f.name == "__typename" {
            if !f.selection.is_empty() {
                self.errors.push(GqlError::at("Field \"__typename\" must not have a selection since type \"String!\" has no subfields.", f.pos));
            }
            return;
        }
        let is_query_root = self.schema.query == parent;
        let def = match (f.name.as_str(), is_query_root) {
            ("__schema", true) => Some(FieldDef { name: "__schema".into(), description: None, args: Vec::new(), ty: TypeRef::NonNull(Box::new(TypeRef::Named("__Schema".into()))), deprecated: None }),
            ("__type", true) => Some(FieldDef {
                name: "__type".into(),
                description: None,
                args: vec![InputValueDef { name: "name".into(), description: None, ty: TypeRef::NonNull(Box::new(TypeRef::Named("String".into()))), default: None }],
                ty: TypeRef::Named("__Type".into()),
                deprecated: None,
            }),
            _ => self.schema.field_of(parent, &f.name).cloned(),
        };
        let Some(def) = def else {
            self.errors.push(GqlError::at(format!("Cannot query field \"{}\" on type \"{parent}\".", f.name), f.pos));
            // Still look for variable usages so they aren't reported as unused.
            for (_, v) in &f.args {
                self.collect_vars(v, f.pos);
            }
            return;
        };
        self.arguments(&f.args, &def.args, Some((parent, &f.name)), "", f.pos);
        let ty = def.ty.named();
        if self.schema.is_leaf(ty) {
            if !f.selection.is_empty() {
                self.errors.push(GqlError::at(format!("Field \"{}\" must not have a selection since type \"{}\" has no subfields.", f.name, def.ty), f.pos));
            }
        } else if f.selection.is_empty() {
            self.errors.push(GqlError::at(format!("Field \"{}\" of type \"{}\" must have a selection of subfields. Did you mean \"{} {{ ... }}\"?", f.name, def.ty, f.name), f.pos));
        } else {
            let ty = ty.to_string();
            self.selection(&f.selection, &ty, visited);
        }
    }

    fn collect_vars(&mut self, v: &Value, pos: Pos) {
        match v {
            Value::Variable(n) => self.used_vars.push((n.clone(), pos)),
            Value::List(l) => l.iter().for_each(|x| self.collect_vars(x, pos)),
            Value::Object(o) => o.iter().for_each(|(_, x)| self.collect_vars(x, pos)),
            _ => {}
        }
    }

    /// Checks a literal against an input type (and records variable usages).
    fn value(&mut self, v: &Value, ty: &TypeRef, pos: Pos) {
        if let Value::Variable(name) = v {
            self.used_vars.push((name.clone(), pos));
            if let Some(def) = self.var_defs.and_then(|defs| defs.iter().find(|d| &d.name == name)) {
                let var_ty = if def.default.is_some() && !def.ty.is_non_null() && ty.is_non_null() { TypeRef::NonNull(Box::new(def.ty.clone())) } else { def.ty.clone() };
                if !sub_type(&var_ty, ty) {
                    self.errors.push(GqlError::at(format!("Variable \"${name}\" of type \"{}\" used in position expecting type \"{ty}\".", def.ty), def.pos));
                }
            }
            return;
        }
        if let Err(msg) = self.literal(v, ty) {
            self.errors.push(GqlError::at(msg, pos));
        }
        // Variables nested in lists / objects.
        if let Value::List(_) | Value::Object(_) = v {
            self.collect_nested(v);
        }
    }

    fn collect_nested(&mut self, v: &Value) {
        match v {
            Value::Variable(n) => self.used_vars.push((n.clone(), Pos::default())),
            Value::List(l) => l.iter().for_each(|x| self.collect_nested(x)),
            Value::Object(o) => o.iter().for_each(|(_, x)| self.collect_nested(x)),
            _ => {}
        }
    }

    fn literal(&self, v: &Value, ty: &TypeRef) -> Result<(), String> {
        match (ty, v) {
            (_, Value::Variable(_)) => Ok(()),
            (TypeRef::NonNull(_), Value::Null) => Err(format!("Expected value of type \"{ty}\", found null.")),
            (TypeRef::NonNull(inner), _) => self.literal(v, inner),
            (_, Value::Null) => Ok(()),
            (TypeRef::List(inner), Value::List(items)) => items.iter().try_for_each(|i| self.literal(i, inner)),
            (TypeRef::List(inner), _) => self.literal(v, inner),
            (TypeRef::Named(n), _) => {
                let Some(t) = self.schema.get(n) else { return Ok(()) };
                match t.kind {
                    TypeKind::Enum => match v {
                        Value::Enum(e) if t.enum_values.iter().any(|x| &x.name == e) => Ok(()),
                        Value::Enum(e) => Err(format!("Value \"{e}\" does not exist in \"{n}\" enum.")),
                        other => Err(format!("Enum \"{n}\" cannot represent non-enum value: {}.", other.print())),
                    },
                    TypeKind::InputObject => {
                        let Value::Object(fields) = v else { return Err(format!("Expected value of type \"{n}\", found {}.", v.print())) };
                        for (k, fv) in fields {
                            match t.input_fields.iter().find(|f| &f.name == k) {
                                Some(def) => self.literal(fv, &def.ty)?,
                                None => return Err(format!("Field \"{k}\" is not defined by type \"{n}\".")),
                            }
                        }
                        for def in &t.input_fields {
                            if def.ty.is_non_null() && def.default.is_none() && !fields.iter().any(|(k, _)| k == &def.name) {
                                return Err(format!("Field \"{n}.{}\" of required type \"{}\" was not provided.", def.name, def.ty));
                            }
                        }
                        Ok(())
                    }
                    TypeKind::Scalar => scalar_literal(n, v),
                    _ => Ok(()),
                }
            }
        }
    }
}

fn scalar_literal(n: &str, v: &Value) -> Result<(), String> {
    match (n, v) {
        ("Int", Value::Int(i)) if i32::try_from(*i).is_ok() => Ok(()),
        ("Int", Value::Int(_)) => Err(format!("Int cannot represent non 32-bit signed integer value: {}", v.print())),
        ("Int", _) => Err(format!("Int cannot represent non-integer value: {}", v.print())),
        ("Float", Value::Int(_) | Value::Float(_)) => Ok(()),
        ("Float", _) => Err(format!("Float cannot represent non numeric value: {}", v.print())),
        ("String", Value::String(_)) => Ok(()),
        ("String", _) => Err(format!("String cannot represent a non string value: {}", v.print())),
        ("Boolean", Value::Boolean(_)) => Ok(()),
        ("Boolean", _) => Err(format!("Boolean cannot represent a non boolean value: {}", v.print())),
        ("ID", Value::String(_) | Value::Int(_)) => Ok(()),
        ("ID", _) => Err(format!("ID cannot represent a non-string and non-integer value: {}", v.print())),
        _ => Ok(()),
    }
}

/// Whether a variable of type `var` may be used where `loc` is expected.
fn sub_type(var: &TypeRef, loc: &TypeRef) -> bool {
    match (var, loc) {
        (TypeRef::NonNull(a), TypeRef::NonNull(b)) => sub_type(a, b),
        (TypeRef::NonNull(a), b) => sub_type(a, b),
        (_, TypeRef::NonNull(_)) => false,
        (TypeRef::List(a), TypeRef::List(b)) => sub_type(a, b),
        (TypeRef::List(_), _) | (_, TypeRef::List(_)) => false,
        (TypeRef::Named(a), TypeRef::Named(b)) => a == b,
    }
}

/// Coerces request variables: defaults applied, required ones present and of the right shape.
pub fn coerce_variables(schema: &Schema, op: &Operation, provided: &Map<String, Json>) -> Result<Map<String, Json>, Vec<GqlError>> {
    let mut out = Map::new();
    let mut errors = Vec::new();
    for d in &op.variables {
        match provided.get(&d.name) {
            None if d.default.is_some() => {
                out.insert(d.name.clone(), d.default.as_ref().map(|v| v.to_json(&Map::new())).unwrap_or(Json::Null));
            }
            None if d.ty.is_non_null() => errors.push(GqlError::at(format!("Variable \"${}\" of required type \"{}\" was not provided.", d.name, d.ty), d.pos)),
            None => {}
            Some(Json::Null) if d.ty.is_non_null() => errors.push(GqlError::at(format!("Variable \"${}\" of non-null type \"{}\" must not be null.", d.name, d.ty), d.pos)),
            Some(v) => match json_fits(schema, v, &d.ty) {
                Ok(()) => {
                    out.insert(d.name.clone(), v.clone());
                }
                Err(e) => errors.push(GqlError::at(format!("Variable \"${}\" got invalid value {}; {e}", d.name, short_json(v)), d.pos)),
            },
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(errors) }
}

fn short_json(v: &Json) -> String {
    let s = v.to_string();
    if s.len() > 60 { format!("{}…", &s[..s.floor_char_boundary(60)]) } else { s }
}

fn json_fits(schema: &Schema, v: &Json, ty: &TypeRef) -> Result<(), String> {
    match (ty, v) {
        (TypeRef::NonNull(_), Json::Null) => Err(format!("Expected non-nullable type \"{ty}\" not to be null.")),
        (TypeRef::NonNull(inner), _) => json_fits(schema, v, inner),
        (_, Json::Null) => Ok(()),
        (TypeRef::List(inner), Json::Array(items)) => items.iter().try_for_each(|i| json_fits(schema, i, inner)),
        (TypeRef::List(inner), _) => json_fits(schema, v, inner),
        (TypeRef::Named(n), _) => {
            let Some(t) = schema.get(n) else { return Ok(()) };
            match t.kind {
                TypeKind::Enum => match v.as_str() {
                    Some(s) if t.enum_values.iter().any(|e| e.name == s) => Ok(()),
                    _ => Err(format!("Value {} does not exist in \"{n}\" enum.", short_json(v))),
                },
                TypeKind::InputObject => {
                    let Some(o) = v.as_object() else { return Err(format!("Expected type \"{n}\" to be an object.")) };
                    for (k, fv) in o {
                        let def = t.input_fields.iter().find(|f| &f.name == k).ok_or_else(|| format!("Field \"{k}\" is not defined by type \"{n}\"."))?;
                        json_fits(schema, fv, &def.ty)?;
                    }
                    for def in &t.input_fields {
                        if def.ty.is_non_null() && def.default.is_none() && !o.contains_key(&def.name) {
                            return Err(format!("Field \"{}\" of required type \"{}\" was not provided.", def.name, def.ty));
                        }
                    }
                    Ok(())
                }
                TypeKind::Scalar => match n.as_str() {
                    "Int" if v.as_i64().is_some_and(|i| i32::try_from(i).is_ok()) => Ok(()),
                    "Int" => Err(format!("Int cannot represent non-integer value: {}", short_json(v))),
                    "Float" if v.is_number() => Ok(()),
                    "Float" => Err(format!("Float cannot represent non numeric value: {}", short_json(v))),
                    "String" if v.is_string() => Ok(()),
                    "String" => Err(format!("String cannot represent a non string value: {}", short_json(v))),
                    "Boolean" if v.is_boolean() => Ok(()),
                    "Boolean" => Err(format!("Boolean cannot represent a non boolean value: {}", short_json(v))),
                    "ID" if v.is_string() || v.is_i64() || v.is_u64() => Ok(()),
                    "ID" => Err(format!("ID cannot represent value: {}", short_json(v))),
                    _ => Ok(()),
                },
                _ => Ok(()),
            }
        }
    }
}
