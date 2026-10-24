//! JSON sample → type definitions (quicktype-style). Shapes from every array element and every
//! occurrence of an object are merged: keys missing somewhere become optional, `null` makes a
//! field nullable, int + float widens to float, anything else that conflicts becomes "any".
//! Identical object shapes share one type.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use crate::{Result, case, invalid, parse_json, pretty};

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// Nothing seen yet (empty array) or only `null`.
    Unknown,
    Bool,
    Int,
    Float,
    Str(Option<&'static str>),
    Array(Box<Shape>),
    Object(Vec<FieldShape>, usize),
    Any,
}

#[derive(Debug, Clone, PartialEq)]
struct Shape {
    kind: Kind,
    nullable: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct FieldShape {
    name: String,
    shape: Shape,
    seen: usize,
}

fn string_format(s: &str) -> Option<&'static str> {
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        Some("date-time")
    } else if chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() && s.len() == 10 {
        Some("date")
    } else if uuid::Uuid::try_parse(s).is_ok() && s.len() == 36 {
        Some("uuid")
    } else if (s.starts_with("http://") || s.starts_with("https://")) && url::Url::parse(s).is_ok() {
        Some("uri")
    } else if s.contains('@') && !s.contains(' ') && s.split('@').count() == 2 && s.rsplit('@').next().is_some_and(|d| d.contains('.')) {
        Some("email")
    } else {
        None
    }
}

fn infer(v: &Value) -> Shape {
    let kind = match v {
        Value::Null => return Shape { kind: Kind::Unknown, nullable: true },
        Value::Bool(_) => Kind::Bool,
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                Kind::Int
            } else {
                Kind::Float
            }
        }
        Value::String(s) => Kind::Str(string_format(s)),
        Value::Array(items) => Kind::Array(Box::new(items.iter().map(infer).reduce(merge).unwrap_or(Shape { kind: Kind::Unknown, nullable: false }))),
        Value::Object(m) => Kind::Object(m.iter().map(|(k, v)| FieldShape { name: k.clone(), shape: infer(v), seen: 1 }).collect(), 1),
    };
    Shape { kind, nullable: false }
}

fn merge(a: Shape, b: Shape) -> Shape {
    let nullable = a.nullable || b.nullable;
    let kind = match (a.kind, b.kind) {
        (Kind::Unknown, k) | (k, Kind::Unknown) => k,
        (Kind::Int, Kind::Int) => Kind::Int,
        (Kind::Int | Kind::Float, Kind::Int | Kind::Float) => Kind::Float,
        (Kind::Bool, Kind::Bool) => Kind::Bool,
        (Kind::Str(x), Kind::Str(y)) => Kind::Str(if x == y { x } else { None }),
        (Kind::Array(x), Kind::Array(y)) => Kind::Array(Box::new(merge(*x, *y))),
        (Kind::Object(mut fa, na), Kind::Object(fb, nb)) => {
            for f in fb {
                match fa.iter_mut().find(|g| g.name == f.name) {
                    Some(g) => {
                        let shape = std::mem::replace(&mut g.shape, Shape { kind: Kind::Unknown, nullable: false });
                        g.shape = merge(shape, f.shape);
                        g.seen += f.seen;
                    }
                    None => fa.push(f),
                }
            }
            Kind::Object(fa, na + nb)
        }
        _ => Kind::Any,
    };
    Shape { kind, nullable }
}

// ── Named type model ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Ty {
    Any,
    /// Only `null` was ever seen.
    Null,
    Bool,
    Int,
    Float,
    Str(Option<&'static str>),
    Array(Box<Ty>),
    Struct(usize),
}

#[derive(Debug, Clone)]
struct FieldDef {
    json: String,
    ty: Ty,
    optional: bool,
    nullable: bool,
}

#[derive(Debug, Clone)]
struct StructDef {
    name: String,
    fields: Vec<FieldDef>,
}

#[derive(Default)]
struct Model {
    /// Post-order: dependencies before dependents.
    structs: Vec<StructDef>,
    names: HashSet<String>,
    sigs: HashMap<String, usize>,
}

fn singular(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.ends_with("ies") && name.len() > 3 {
        format!("{}y", &name[..name.len() - 3])
    } else if lower.ends_with("sses") || lower.ends_with("xes") || lower.ends_with("ches") || lower.ends_with("shes") || lower.ends_with("uses") {
        name[..name.len() - 2].to_string()
    } else if lower.ends_with('s') && !lower.ends_with("ss") && name.len() > 1 {
        name[..name.len() - 1].to_string()
    } else {
        format!("{name}Item")
    }
}

fn type_name(hint: &str) -> String {
    let p = case::pascal(hint);
    match p.chars().next() {
        None => "Type".into(),
        Some(c) if c.is_ascii_digit() => format!("T{p}"),
        _ => p,
    }
}

fn sig(t: &Ty) -> String {
    match t {
        Ty::Array(i) => format!("[{}]", sig(i)),
        Ty::Struct(i) => format!("#{i}"),
        other => format!("{other:?}"),
    }
}

impl Model {
    fn unique(&mut self, base: String) -> String {
        let mut name = base.clone();
        let mut n = 2;
        while self.names.contains(&name) {
            name = format!("{base}{n}");
            n += 1;
        }
        self.names.insert(name.clone());
        name
    }

    fn build(&mut self, shape: &Shape, hint: &str) -> Ty {
        match &shape.kind {
            Kind::Unknown => {
                if shape.nullable {
                    Ty::Null
                } else {
                    Ty::Any
                }
            }
            Kind::Bool => Ty::Bool,
            Kind::Int => Ty::Int,
            Kind::Float => Ty::Float,
            Kind::Str(f) => Ty::Str(*f),
            Kind::Any => Ty::Any,
            Kind::Array(inner) => Ty::Array(Box::new(self.build(inner, &singular(hint)))),
            Kind::Object(fields, samples) => {
                let defs: Vec<FieldDef> = fields
                    .iter()
                    .map(|f| FieldDef { json: f.name.clone(), ty: self.build(&f.shape, &f.name), optional: f.seen < *samples, nullable: f.shape.nullable })
                    .collect();
                let key = defs.iter().map(|d| format!("{}:{}:{}:{}", d.json, sig(&d.ty), d.optional, d.nullable)).collect::<Vec<_>>().join(",");
                if let Some(&i) = self.sigs.get(&key) {
                    return Ty::Struct(i);
                }
                let name = self.unique(type_name(hint));
                self.structs.push(StructDef { name, fields: defs });
                let idx = self.structs.len() - 1;
                self.sigs.insert(key, idx);
                Ty::Struct(idx)
            }
        }
    }
}

pub fn generate(input: &str, language: &str, root: &str) -> Result<String> {
    let v = parse_json(input, "the sample")?;
    let root = if root.trim().is_empty() { "Root".to_string() } else { type_name(root.trim()) };
    let shape = infer(&v);
    let mut model = Model::default();
    // A root object takes the root name; a root array names its element `<Root>Item`.
    let root_ty = match &shape.kind {
        Kind::Object(..) => model.build(&shape, &root),
        Kind::Array(inner) => {
            model.names.insert(root.clone());
            Ty::Array(Box::new(model.build(inner, &format!("{root}Item"))))
        }
        _ => {
            model.names.insert(root.clone());
            model.build(&shape, &root)
        }
    };
    let alias = !matches!(root_ty, Ty::Struct(_));
    let r = Render { m: &model, root: &root, root_ty: &root_ty, alias, root_nullable: shape.nullable };
    Ok(match language {
        "Rust (serde)" | "Rust" | "rust" => r.rust(),
        "Go" | "go" => r.go(),
        "Python (dataclasses)" | "python" => r.python_dataclasses(),
        "Python (pydantic)" | "pydantic" => r.pydantic(),
        "Java records" | "Java" | "java" => r.java(),
        "C#" | "csharp" => r.csharp(),
        "Kotlin" | "kotlin" => r.kotlin(),
        "Swift" | "swift" => r.swift(),
        "JSON Schema" | "jsonschema" => r.json_schema(),
        "TypeScript" | "typescript" | "" => r.typescript(),
        other => return Err(invalid(format!("unsupported language {other}"))),
    })
}

struct Render<'a> {
    m: &'a Model,
    root: &'a str,
    root_ty: &'a Ty,
    alias: bool,
    root_nullable: bool,
}

fn uses_any(m: &Model, root: &Ty) -> bool {
    fn has(t: &Ty) -> bool {
        match t {
            Ty::Any | Ty::Null => true,
            Ty::Array(i) => has(i),
            _ => false,
        }
    }
    has(root) || m.structs.iter().any(|s| s.fields.iter().any(|f| has(&f.ty)))
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_' || f == '$') && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
}

fn fix_ident(name: String, keywords: &[&str], fallback: &str) -> String {
    let name = if name.is_empty() { fallback.to_string() } else { name };
    let name = if name.chars().next().is_some_and(|c| c.is_ascii_digit()) { format!("_{name}") } else { name };
    if keywords.contains(&name.as_str()) { format!("{name}_") } else { name }
}

const RUST_KW: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "unsafe", "use", "where", "while", "yield", "abstract", "become", "do", "final", "macro", "override", "priv", "typeof", "unsized",
    "virtual",
];
const PY_KW: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "match", "case", "type",
];
const JAVA_KW: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const", "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally", "float", "for", "goto", "if",
    "implements", "import", "instanceof", "int", "interface", "long", "native", "new", "package", "private", "protected", "public", "return", "short", "static", "strictfp", "super", "switch", "synchronized", "this",
    "throw", "throws", "transient", "try", "void", "volatile", "while", "record", "var", "yield", "true", "false", "null",
];
const KOTLIN_KW: &[&str] = &[
    "as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in", "interface", "is", "null", "object", "package", "return", "super", "this", "throw", "true", "try", "typealias", "typeof", "val",
    "var", "when", "while",
];
const SWIFT_KW: &[&str] = &[
    "associatedtype", "class", "deinit", "enum", "extension", "fileprivate", "func", "import", "init", "inout", "internal", "let", "open", "operator", "private", "protocol", "public", "rethrows", "static", "struct",
    "subscript", "typealias", "var", "break", "case", "continue", "default", "defer", "do", "else", "fallthrough", "for", "guard", "if", "in", "repeat", "return", "switch", "where", "while", "as", "catch", "false",
    "is", "nil", "self", "Self", "super", "throw", "throws", "true", "try", "Type", "Protocol",
];
const GO_INITIALISMS: &[&str] = &["Id", "Url", "Uri", "Http", "Https", "Api", "Json", "Uuid", "Ip", "Html", "Sql", "Xml", "Css", "Tls", "Ttl", "Dns", "Cpu", "Os", "Ui", "Utc"];

fn go_name(json: &str) -> String {
    let words = case::words(json);
    let mut out = String::new();
    for w in words {
        let mut c = w.chars();
        let cap: String = match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
            None => continue,
        };
        if GO_INITIALISMS.contains(&cap.as_str()) { out.push_str(&cap.to_uppercase()) } else { out.push_str(&cap) }
    }
    fix_ident(out, &[], "Field")
}

impl Render<'_> {
    fn name(&self, i: usize) -> &str {
        &self.m.structs[i].name
    }

    /// Root first, then the rest in definition order (reads top-down).
    fn top_down(&self) -> Vec<usize> {
        let n = self.m.structs.len();
        let mut order: Vec<usize> = (0..n).rev().collect();
        if let Ty::Struct(r) = self.root_ty {
            order.retain(|i| i != r);
            order.insert(0, *r);
        }
        order
    }

    // ── TypeScript ──
    fn ts(&self, t: &Ty) -> String {
        match t {
            Ty::Any => "unknown".into(),
            Ty::Null => "null".into(),
            Ty::Bool => "boolean".into(),
            Ty::Int | Ty::Float => "number".into(),
            Ty::Str(_) => "string".into(),
            Ty::Array(i) => {
                let inner = self.ts(i);
                if inner.contains(' ') { format!("Array<{inner}>") } else { format!("{inner}[]") }
            }
            Ty::Struct(i) => self.name(*i).into(),
        }
    }

    fn typescript(&self) -> String {
        let mut s = String::new();
        if self.alias {
            let _ = writeln!(s, "export type {} = {}{};\n", self.root, self.ts(self.root_ty), if self.root_nullable { " | null" } else { "" });
        }
        for i in self.top_down() {
            let st = &self.m.structs[i];
            let _ = writeln!(s, "export interface {} {{", st.name);
            for f in &st.fields {
                let key = if is_ident(&f.json) { f.json.clone() } else { serde_json::to_string(&f.json).unwrap_or_default() };
                let null = if f.nullable && f.ty != Ty::Null { " | null" } else { "" };
                let _ = writeln!(s, "  {key}{}: {}{null};", if f.optional { "?" } else { "" }, self.ts(&f.ty));
            }
            s.push_str("}\n\n");
        }
        s.trim_end().to_string() + "\n"
    }

    // ── Rust ──
    fn rs(&self, t: &Ty) -> String {
        match t {
            Ty::Any => "Value".into(),
            Ty::Null => "Option<Value>".into(),
            Ty::Bool => "bool".into(),
            Ty::Int => "i64".into(),
            Ty::Float => "f64".into(),
            Ty::Str(_) => "String".into(),
            Ty::Array(i) => format!("Vec<{}>", self.rs(i)),
            Ty::Struct(i) => self.name(*i).into(),
        }
    }

    fn rust(&self) -> String {
        let mut s = String::from("use serde::{Deserialize, Serialize};\n");
        if uses_any(self.m, self.root_ty) {
            s.push_str("use serde_json::Value;\n");
        }
        s.push('\n');
        if self.alias {
            let inner = self.rs(self.root_ty);
            let _ = writeln!(s, "pub type {} = {};\n", self.root, if self.root_nullable && !inner.starts_with("Option<") { format!("Option<{inner}>") } else { inner });
        }
        for i in self.top_down() {
            let st = &self.m.structs[i];
            let _ = writeln!(s, "#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub struct {} {{", st.name);
            for f in &st.fields {
                let field = fix_ident(case::joined(&f.json, "_", false), RUST_KW, "field");
                let mut attrs: Vec<String> = Vec::new();
                if field != f.json {
                    attrs.push(format!("rename = {}", serde_json::to_string(&f.json).unwrap_or_default()));
                }
                let base = self.rs(&f.ty);
                let wrap = (f.optional || f.nullable) && !base.starts_with("Option<");
                if f.optional {
                    attrs.push("default".into());
                    attrs.push("skip_serializing_if = \"Option::is_none\"".into());
                }
                if !attrs.is_empty() {
                    let _ = writeln!(s, "    #[serde({})]", attrs.join(", "));
                }
                let _ = writeln!(s, "    pub {field}: {},", if wrap { format!("Option<{base}>") } else { base });
            }
            s.push_str("}\n\n");
        }
        s.trim_end().to_string() + "\n"
    }

    // ── Go ──
    fn go_ty(&self, t: &Ty, pointer: bool) -> String {
        let base = match t {
            Ty::Any | Ty::Null => return "any".into(),
            Ty::Bool => "bool".into(),
            Ty::Int => "int64".into(),
            Ty::Float => "float64".into(),
            Ty::Str(_) => "string".into(),
            Ty::Array(i) => return format!("[]{}", self.go_ty(i, false)),
            Ty::Struct(i) => self.name(*i).to_string(),
        };
        if pointer { format!("*{base}") } else { base }
    }

    fn go(&self) -> String {
        let mut s = String::from("package model\n\n");
        if self.alias {
            let _ = writeln!(s, "type {} {}\n", self.root, self.go_ty(self.root_ty, self.root_nullable));
        }
        for i in self.top_down() {
            let st = &self.m.structs[i];
            let names: Vec<String> = st.fields.iter().map(|f| go_name(&f.json)).collect();
            let w_name = names.iter().map(String::len).max().unwrap_or(0);
            let types: Vec<String> = st.fields.iter().map(|f| self.go_ty(&f.ty, f.optional || f.nullable)).collect();
            let w_ty = types.iter().map(String::len).max().unwrap_or(0);
            let _ = writeln!(s, "type {} struct {{", st.name);
            for ((f, n), t) in st.fields.iter().zip(&names).zip(&types) {
                let tag = format!("`json:\"{}{}\"`", f.json, if f.optional { ",omitempty" } else { "" });
                let _ = writeln!(s, "\t{n:<w_name$} {t:<w_ty$} {tag}");
            }
            s.push_str("}\n\n");
        }
        s.trim_end().to_string() + "\n"
    }

    // ── Python ──
    fn py(&self, t: &Ty) -> String {
        match t {
            Ty::Any | Ty::Null => "Any".into(),
            Ty::Bool => "bool".into(),
            Ty::Int => "int".into(),
            Ty::Float => "float".into(),
            Ty::Str(_) => "str".into(),
            Ty::Array(i) => format!("list[{}]", self.py(i)),
            Ty::Struct(i) => self.name(*i).into(),
        }
    }

    fn py_field_ty(&self, f: &FieldDef) -> String {
        let base = self.py(&f.ty);
        if (f.optional || f.nullable) && base != "Any" { format!("Optional[{base}]") } else { base }
    }

    fn python_dataclasses(&self) -> String {
        let mut s = String::from("from __future__ import annotations\n\nfrom dataclasses import dataclass\nfrom typing import Any, Optional\n\n\n");
        for st in &self.m.structs {
            let _ = writeln!(s, "@dataclass\nclass {}:", st.name);
            if st.fields.is_empty() {
                s.push_str("    pass\n");
            }
            // Fields with defaults must follow those without.
            let mut fields: Vec<&FieldDef> = st.fields.iter().collect();
            fields.sort_by_key(|f| f.optional);
            for f in fields {
                let name = fix_ident(case::joined(&f.json, "_", false), PY_KW, "field");
                let default = if f.optional { " = None" } else { "" };
                let note = if name != f.json { format!("  # JSON: {}", serde_json::to_string(&f.json).unwrap_or_default()) } else { String::new() };
                let _ = writeln!(s, "    {name}: {}{default}{note}", self.py_field_ty(f));
            }
            s.push_str("\n\n");
        }
        if self.alias {
            let _ = writeln!(s, "{} = {}", self.root, self.py(self.root_ty));
        }
        s.trim_end().to_string() + "\n"
    }

    fn pydantic(&self) -> String {
        let mut s = String::from("from __future__ import annotations\n\nfrom typing import Any, Optional\n\nfrom pydantic import BaseModel, ConfigDict, Field\n\n\n");
        for st in &self.m.structs {
            let _ = writeln!(s, "class {}(BaseModel):", st.name);
            let mut body = String::new();
            let mut aliased = false;
            for f in &st.fields {
                let name = fix_ident(case::joined(&f.json, "_", false), PY_KW, "field");
                let alias = (name != f.json).then(|| serde_json::to_string(&f.json).unwrap_or_default());
                aliased |= alias.is_some();
                let default = match (f.optional || f.nullable, &alias) {
                    (true, Some(a)) => format!(" = Field(default=None, alias={a})"),
                    (true, None) => " = None".into(),
                    (false, Some(a)) => format!(" = Field(alias={a})"),
                    (false, None) => String::new(),
                };
                let _ = writeln!(body, "    {name}: {}{default}", self.py_field_ty(f));
            }
            if aliased {
                s.push_str("    model_config = ConfigDict(populate_by_name=True)\n\n");
            }
            if body.is_empty() {
                body.push_str("    pass\n");
            }
            s.push_str(&body);
            s.push_str("\n\n");
        }
        if self.alias {
            let _ = writeln!(s, "{} = {}", self.root, self.py(self.root_ty));
        }
        s.trim_end().to_string() + "\n"
    }

    // ── Java ──
    fn java_ty(&self, t: &Ty, boxed: bool) -> String {
        match t {
            Ty::Any | Ty::Null => "Object".into(),
            Ty::Bool => (if boxed { "Boolean" } else { "boolean" }).into(),
            Ty::Int => (if boxed { "Long" } else { "long" }).into(),
            Ty::Float => (if boxed { "Double" } else { "double" }).into(),
            Ty::Str(_) => "String".into(),
            Ty::Array(i) => format!("List<{}>", self.java_ty(i, true)),
            Ty::Struct(i) => self.name(*i).into(),
        }
    }

    fn java(&self) -> String {
        let mut body = String::new();
        let mut annotated = false;
        let mut list = matches!(self.root_ty, Ty::Array(_));
        for i in self.top_down() {
            let st = &self.m.structs[i];
            let params: Vec<String> = st
                .fields
                .iter()
                .map(|f| {
                    let name = fix_ident(case::camel(&f.json), JAVA_KW, "field");
                    let ty = self.java_ty(&f.ty, f.optional || f.nullable);
                    list |= ty.contains("List<");
                    if name != f.json {
                        annotated = true;
                        format!("        @JsonProperty({}) {ty} {name}", serde_json::to_string(&f.json).unwrap_or_default())
                    } else {
                        format!("        {ty} {name}")
                    }
                })
                .collect();
            if params.is_empty() {
                let _ = writeln!(body, "public record {}() {{}}\n", st.name);
            } else {
                let _ = writeln!(body, "public record {}(\n{}\n) {{}}\n", st.name, params.join(",\n"));
            }
        }
        let mut s = String::new();
        if list {
            s.push_str("import java.util.List;\n");
        }
        if annotated {
            s.push_str("import com.fasterxml.jackson.annotation.JsonProperty;\n");
        }
        if !s.is_empty() {
            s.push('\n');
        }
        if self.alias {
            let _ = writeln!(s, "// {} is {}\n", self.root, self.java_ty(self.root_ty, true));
        }
        s.push_str("// One record per file, or nest them as `public static` members of a holder class.\n\n");
        s.push_str(&body);
        s.trim_end().to_string() + "\n"
    }

    // ── C# ──
    fn cs_ty(&self, t: &Ty) -> String {
        match t {
            Ty::Any | Ty::Null => "JsonElement".into(),
            Ty::Bool => "bool".into(),
            Ty::Int => "long".into(),
            Ty::Float => "double".into(),
            Ty::Str(_) => "string".into(),
            Ty::Array(i) => format!("List<{}>", self.cs_ty(i)),
            Ty::Struct(i) => self.name(*i).into(),
        }
    }

    fn csharp(&self) -> String {
        let mut s = String::from("using System.Collections.Generic;\nusing System.Text.Json;\nusing System.Text.Json.Serialization;\n\n");
        if self.alias {
            let _ = writeln!(s, "// {} is {}\n", self.root, self.cs_ty(self.root_ty));
        }
        for i in self.top_down() {
            let st = &self.m.structs[i];
            let _ = writeln!(s, "public class {}\n{{", st.name);
            for (n, f) in st.fields.iter().enumerate() {
                let mut prop = fix_ident(case::pascal(&f.json), &[], "Field");
                if prop == st.name {
                    prop.push_str("Value");
                }
                let ty = self.cs_ty(&f.ty);
                let maybe = f.optional || f.nullable;
                let decl = if maybe { format!("public {ty}? {prop} {{ get; set; }}") } else { format!("public required {ty} {prop} {{ get; set; }}") };
                if n > 0 {
                    s.push('\n');
                }
                let _ = writeln!(s, "    [JsonPropertyName({})]\n    {decl}", serde_json::to_string(&f.json).unwrap_or_default());
            }
            s.push_str("}\n\n");
        }
        s.trim_end().to_string() + "\n"
    }

    // ── Kotlin ──
    fn kt_ty(&self, t: &Ty) -> String {
        match t {
            Ty::Any | Ty::Null => "JsonElement".into(),
            Ty::Bool => "Boolean".into(),
            Ty::Int => "Long".into(),
            Ty::Float => "Double".into(),
            Ty::Str(_) => "String".into(),
            Ty::Array(i) => format!("List<{}>", self.kt_ty(i)),
            Ty::Struct(i) => self.name(*i).into(),
        }
    }

    fn kotlin(&self) -> String {
        let mut body = String::new();
        let mut serial_name = false;
        for i in self.top_down() {
            let st = &self.m.structs[i];
            let params: Vec<String> = st
                .fields
                .iter()
                .map(|f| {
                    let mut name = case::camel(&f.json);
                    if name.is_empty() {
                        name = "field".into();
                    }
                    if KOTLIN_KW.contains(&name.as_str()) || name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        name = format!("`{name}`");
                    }
                    let ann = if name.trim_matches('`') != f.json {
                        serial_name = true;
                        format!("@SerialName({}) ", serde_json::to_string(&f.json).unwrap_or_default())
                    } else {
                        String::new()
                    };
                    let ty = self.kt_ty(&f.ty);
                    let ty = if f.optional || f.nullable { format!("{ty}? = null") } else { ty };
                    format!("    {ann}val {name}: {ty},")
                })
                .collect();
            if params.is_empty() {
                let _ = writeln!(body, "@Serializable\nclass {}\n", st.name);
            } else {
                let _ = writeln!(body, "@Serializable\ndata class {}(\n{}\n)\n", st.name, params.join("\n"));
            }
        }
        let mut s = String::new();
        if serial_name {
            s.push_str("import kotlinx.serialization.SerialName\n");
        }
        s.push_str("import kotlinx.serialization.Serializable\n");
        if uses_any(self.m, self.root_ty) {
            s.push_str("import kotlinx.serialization.json.JsonElement\n");
        }
        s.push('\n');
        if self.alias {
            let _ = writeln!(s, "typealias {} = {}{}\n", self.root, self.kt_ty(self.root_ty), if self.root_nullable { "?" } else { "" });
        }
        s.push_str(&body);
        s.trim_end().to_string() + "\n"
    }

    // ── Swift ──
    fn sw_ty(&self, t: &Ty) -> String {
        match t {
            Ty::Any | Ty::Null => "JSONValue".into(),
            Ty::Bool => "Bool".into(),
            Ty::Int => "Int".into(),
            Ty::Float => "Double".into(),
            Ty::Str(_) => "String".into(),
            Ty::Array(i) => format!("[{}]", self.sw_ty(i)),
            Ty::Struct(i) => self.name(*i).into(),
        }
    }

    fn swift(&self) -> String {
        let mut s = String::from("import Foundation\n\n");
        if self.alias {
            let _ = writeln!(s, "typealias {} = {}{}\n", self.root, self.sw_ty(self.root_ty), if self.root_nullable { "?" } else { "" });
        }
        for i in self.top_down() {
            let st = &self.m.structs[i];
            let _ = writeln!(s, "struct {}: Codable {{", st.name);
            let mut keys = Vec::new();
            let mut renamed = false;
            for f in &st.fields {
                let mut name = case::camel(&f.json);
                if name.is_empty() {
                    name = "field".into();
                }
                if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    name = format!("_{name}");
                }
                let decl = if SWIFT_KW.contains(&name.as_str()) { format!("`{name}`") } else { name.clone() };
                let ty = self.sw_ty(&f.ty);
                let opt = if f.optional || f.nullable { "?" } else { "" };
                let _ = writeln!(s, "    let {decl}: {ty}{opt}");
                if name != f.json {
                    renamed = true;
                    keys.push(format!("        case {decl} = {}", serde_json::to_string(&f.json).unwrap_or_default()));
                } else {
                    keys.push(format!("        case {decl}"));
                }
            }
            if renamed {
                let _ = writeln!(s, "\n    enum CodingKeys: String, CodingKey {{\n{}\n    }}", keys.join("\n"));
            }
            s.push_str("}\n\n");
        }
        if uses_any(self.m, self.root_ty) {
            s.push_str(SWIFT_JSON_VALUE);
        }
        s.trim_end().to_string() + "\n"
    }

    // ── JSON Schema ──
    fn schema(&self, t: &Ty, nullable: bool) -> Value {
        let mut v = match t {
            Ty::Any => json!({}),
            Ty::Null => return json!({ "type": "null" }),
            Ty::Bool => json!({ "type": "boolean" }),
            Ty::Int => json!({ "type": "integer" }),
            Ty::Float => json!({ "type": "number" }),
            Ty::Str(fmt) => {
                let mut o = json!({ "type": "string" });
                if let Some(f) = fmt {
                    o["format"] = json!(f);
                }
                o
            }
            Ty::Array(i) => json!({ "type": "array", "items": self.schema(i, false) }),
            Ty::Struct(i) => json!({ "$ref": format!("#/$defs/{}", self.name(*i)) }),
        };
        if nullable && !matches!(t, Ty::Any) {
            if let Some(ty) = v.get("type").cloned() {
                v["type"] = json!([ty, "null"]);
            } else {
                v = json!({ "anyOf": [v, { "type": "null" }] });
            }
        }
        v
    }

    fn object_schema(&self, st: &StructDef) -> Value {
        let mut props = Map::new();
        let mut required = Vec::new();
        for f in &st.fields {
            props.insert(f.json.clone(), self.schema(&f.ty, f.nullable));
            if !f.optional {
                required.push(json!(f.json));
            }
        }
        json!({ "type": "object", "properties": props, "required": required })
    }

    fn json_schema(&self) -> String {
        let mut out = Map::new();
        out.insert("$schema".into(), json!("https://json-schema.org/draft/2020-12/schema"));
        out.insert("title".into(), json!(self.root));
        let root_struct = match self.root_ty {
            Ty::Struct(i) => Some(*i),
            _ => None,
        };
        match root_struct {
            Some(i) => {
                if let Value::Object(m) = self.object_schema(&self.m.structs[i]) {
                    out.extend(m);
                }
            }
            None => {
                if let Value::Object(m) = self.schema(self.root_ty, self.root_nullable) {
                    out.extend(m);
                }
            }
        }
        let defs: Map<String, Value> = (0..self.m.structs.len()).rev().filter(|i| Some(*i) != root_struct).map(|i| (self.name(i).to_string(), self.object_schema(&self.m.structs[i]))).collect();
        if !defs.is_empty() {
            out.insert("$defs".into(), Value::Object(defs));
        }
        pretty(&Value::Object(out)) + "\n"
    }
}

const SWIFT_JSON_VALUE: &str = r#"/// Any JSON value (used where the sample had mixed or unknown types).
enum JSONValue: Codable, Equatable {
    case string(String), number(Double), bool(Bool), object([String: JSONValue]), array([JSONValue]), null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([JSONValue].self) { self = .array(v) }
        else { self = .object(try c.decode([String: JSONValue].self)) }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .string(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .bool(let v): try c.encode(v)
        case .object(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .null: try c.encodeNil()
        }
    }
}
"#;
