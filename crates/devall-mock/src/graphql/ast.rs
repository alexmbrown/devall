//! GraphQL syntax tree (executable documents and SDL), owned strings, with source positions.

use serde_json::{Map, Value as Json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Pos {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Variable(String),
    Int(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    Null,
    Enum(String),
    List(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// JSON for this literal with variables substituted.
    pub fn to_json(&self, vars: &Map<String, Json>) -> Json {
        match self {
            Value::Variable(v) => vars.get(v).cloned().unwrap_or(Json::Null),
            Value::Int(i) => Json::from(*i),
            Value::Float(f) => serde_json::Number::from_f64(*f).map(Json::Number).unwrap_or(Json::Null),
            Value::String(s) | Value::Enum(s) => Json::String(s.clone()),
            Value::Boolean(b) => Json::Bool(*b),
            Value::Null => Json::Null,
            Value::List(l) => Json::Array(l.iter().map(|v| v.to_json(vars)).collect()),
            Value::Object(o) => Json::Object(o.iter().map(|(k, v)| (k.clone(), v.to_json(vars))).collect()),
        }
    }

    /// Canonical GraphQL text (for normalised query comparison and introspection defaults).
    pub fn print(&self) -> String {
        match self {
            Value::Variable(v) => format!("${v}"),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
            Value::Boolean(b) => b.to_string(),
            Value::Null => "null".into(),
            Value::Enum(e) => e.clone(),
            Value::List(l) => format!("[{}]", l.iter().map(Value::print).collect::<Vec<_>>().join(", ")),
            Value::Object(o) => format!("{{{}}}", o.iter().map(|(k, v)| format!("{k}: {}", v.print())).collect::<Vec<_>>().join(", ")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

impl TypeRef {
    pub fn named(&self) -> &str {
        match self {
            TypeRef::Named(n) => n,
            TypeRef::List(t) | TypeRef::NonNull(t) => t.named(),
        }
    }

    pub fn is_non_null(&self) -> bool {
        matches!(self, TypeRef::NonNull(_))
    }
}

impl std::fmt::Display for TypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeRef::Named(n) => f.write_str(n),
            TypeRef::List(t) => write!(f, "[{t}]"),
            TypeRef::NonNull(t) => write!(f, "{t}!"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Directive {
    pub name: String,
    pub args: Vec<(String, Value)>,
    pub pos: Pos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Query,
    Mutation,
    Subscription,
}

impl OpKind {
    pub fn keyword(self) -> &'static str {
        match self {
            OpKind::Query => "query",
            OpKind::Mutation => "mutation",
            OpKind::Subscription => "subscription",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDef {
    pub name: String,
    pub ty: TypeRef,
    pub default: Option<Value>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub alias: Option<String>,
    pub name: String,
    pub args: Vec<(String, Value)>,
    pub directives: Vec<Directive>,
    pub selection: Vec<Selection>,
    pub pos: Pos,
}

impl Field {
    pub fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Field(Field),
    Spread { name: String, directives: Vec<Directive>, pos: Pos },
    Inline { type_cond: Option<String>, directives: Vec<Directive>, selection: Vec<Selection>, pos: Pos },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub kind: OpKind,
    pub name: Option<String>,
    pub variables: Vec<VarDef>,
    pub directives: Vec<Directive>,
    pub selection: Vec<Selection>,
    pub pos: Pos,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub name: String,
    pub type_cond: String,
    pub directives: Vec<Directive>,
    pub selection: Vec<Selection>,
    pub pos: Pos,
}

// ---------------------------------------------------------------- type system (SDL)

#[derive(Debug, Clone, PartialEq)]
pub struct InputValueDef {
    pub name: String,
    pub description: Option<String>,
    pub ty: TypeRef,
    pub default: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub description: Option<String>,
    pub args: Vec<InputValueDef>,
    pub ty: TypeRef,
    /// `Some(reason)` when `@deprecated`.
    pub deprecated: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumValueDef {
    pub name: String,
    pub description: Option<String>,
    pub deprecated: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeKind {
    Scalar,
    Object,
    Interface,
    Union,
    Enum,
    InputObject,
}

impl TypeKind {
    pub fn introspection_name(self) -> &'static str {
        match self {
            TypeKind::Scalar => "SCALAR",
            TypeKind::Object => "OBJECT",
            TypeKind::Interface => "INTERFACE",
            TypeKind::Union => "UNION",
            TypeKind::Enum => "ENUM",
            TypeKind::InputObject => "INPUT_OBJECT",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeKind,
    pub description: Option<String>,
    /// Object / interface fields.
    pub fields: Vec<FieldDef>,
    /// Implemented interfaces (objects, interfaces).
    pub interfaces: Vec<String>,
    /// Union members.
    pub members: Vec<String>,
    pub enum_values: Vec<EnumValueDef>,
    pub input_fields: Vec<InputValueDef>,
    /// `@specifiedBy(url:)` on scalars.
    pub specified_by: Option<String>,
}

impl TypeDef {
    pub fn new(name: impl Into<String>, kind: TypeKind) -> Self {
        Self {
            name: name.into(),
            kind,
            description: None,
            fields: Vec::new(),
            interfaces: Vec::new(),
            members: Vec::new(),
            enum_values: Vec::new(),
            input_fields: Vec::new(),
            specified_by: None,
        }
    }

    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectiveDef {
    pub name: String,
    pub description: Option<String>,
    pub args: Vec<InputValueDef>,
    pub locations: Vec<String>,
    pub repeatable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Definition {
    Operation(Operation),
    Fragment(Fragment),
    /// `schema { query: … }` (or `extend schema`).
    Schema(Vec<(OpKind, String)>),
    /// A type definition (`extend` = merge into an existing one).
    Type { def: TypeDef, extend: bool },
    Directive(DirectiveDef),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub definitions: Vec<Definition>,
}

impl Document {
    pub fn operations(&self) -> impl Iterator<Item = &Operation> {
        self.definitions.iter().filter_map(|d| if let Definition::Operation(o) = d { Some(o) } else { None })
    }

    pub fn fragments(&self) -> impl Iterator<Item = &Fragment> {
        self.definitions.iter().filter_map(|d| if let Definition::Fragment(f) = d { Some(f) } else { None })
    }
}
