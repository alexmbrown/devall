//! The schema model: built from SDL (one or more files, `extend` merged) or an introspection
//! result, always with the built-in scalars, directives and introspection types, and able to
//! produce its own introspection result.

use indexmap::IndexMap;
use serde_json::{Map, Value as Json, json};

use super::GqlError;
use super::ast::*;
use super::parser::parse;

/// The introspection types, as SDL (parsed into every schema so `__schema { … }` validates).
const INTROSPECTION_SDL: &str = r#"
type __Schema { description: String types: [__Type!]! queryType: __Type! mutationType: __Type subscriptionType: __Type directives: [__Directive!]! }
type __Type { kind: __TypeKind! name: String description: String specifiedByURL: String fields(includeDeprecated: Boolean = false): [__Field!] interfaces: [__Type!] possibleTypes: [__Type!] enumValues(includeDeprecated: Boolean = false): [__EnumValue!] inputFields(includeDeprecated: Boolean = false): [__InputValue!] ofType: __Type isOneOf: Boolean }
enum __TypeKind { SCALAR OBJECT INTERFACE UNION ENUM INPUT_OBJECT LIST NON_NULL }
type __Field { name: String! description: String args(includeDeprecated: Boolean = false): [__InputValue!]! type: __Type! isDeprecated: Boolean! deprecationReason: String }
type __InputValue { name: String! description: String type: __Type! defaultValue: String isDeprecated: Boolean! deprecationReason: String }
type __EnumValue { name: String! description: String isDeprecated: Boolean! deprecationReason: String }
type __Directive { name: String! description: String isRepeatable: Boolean! locations: [__DirectiveLocation!]! args(includeDeprecated: Boolean = false): [__InputValue!]! }
enum __DirectiveLocation { QUERY MUTATION SUBSCRIPTION FIELD FRAGMENT_DEFINITION FRAGMENT_SPREAD INLINE_FRAGMENT VARIABLE_DEFINITION SCHEMA SCALAR OBJECT FIELD_DEFINITION ARGUMENT_DEFINITION INTERFACE UNION ENUM ENUM_VALUE INPUT_OBJECT INPUT_FIELD_DEFINITION }
directive @include(if: Boolean!) on FIELD | FRAGMENT_SPREAD | INLINE_FRAGMENT
directive @skip(if: Boolean!) on FIELD | FRAGMENT_SPREAD | INLINE_FRAGMENT
directive @deprecated(reason: String = "No longer supported") on FIELD_DEFINITION | ARGUMENT_DEFINITION | INPUT_FIELD_DEFINITION | ENUM_VALUE
directive @specifiedBy(url: String!) on SCALAR
"#;

pub const BUILTIN_SCALARS: [&str; 5] = ["String", "Int", "Float", "Boolean", "ID"];

#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub types: IndexMap<String, TypeDef>,
    pub directives: IndexMap<String, DirectiveDef>,
    pub query: String,
    pub mutation: Option<String>,
    pub subscription: Option<String>,
}

impl Schema {
    fn empty() -> Self {
        let mut s = Self { types: IndexMap::new(), directives: IndexMap::new(), query: "Query".into(), mutation: None, subscription: None };
        for n in BUILTIN_SCALARS {
            s.types.insert(n.into(), TypeDef::new(n, TypeKind::Scalar));
        }
        let intro = parse(INTROSPECTION_SDL).expect("introspection SDL parses");
        for d in intro.definitions {
            match d {
                Definition::Type { def, .. } => {
                    s.types.insert(def.name.clone(), def);
                }
                Definition::Directive(d) => {
                    s.directives.insert(d.name.clone(), d);
                }
                _ => {}
            }
        }
        s
    }

    /// Builds a schema from SDL sources (merged in order; `extend type` adds to earlier types).
    pub fn from_sdl(sources: &[&str]) -> Result<Self, GqlError> {
        let mut s = Self::empty();
        let mut roots: Vec<(OpKind, String)> = Vec::new();
        let mut extensions: Vec<TypeDef> = Vec::new();
        for src in sources {
            let doc = parse(src)?;
            for d in doc.definitions {
                match d {
                    Definition::Type { def, extend: false } => {
                        if BUILTIN_SCALARS.contains(&def.name.as_str()) {
                            continue;
                        }
                        if s.types.contains_key(&def.name) && !def.name.starts_with("__") {
                            return Err(GqlError::new(format!("There can be only one type named \"{}\".", def.name)));
                        }
                        s.types.insert(def.name.clone(), def);
                    }
                    Definition::Type { def, extend: true } => extensions.push(def),
                    Definition::Schema(r) => roots.extend(r),
                    Definition::Directive(d) => {
                        s.directives.insert(d.name.clone(), d);
                    }
                    Definition::Operation(_) | Definition::Fragment(_) => {
                        return Err(GqlError::new("A schema file must only contain type system definitions (found an operation or fragment)."));
                    }
                }
            }
        }
        for ext in extensions {
            let Some(t) = s.types.get_mut(&ext.name) else { return Err(GqlError::new(format!("Cannot extend type \"{}\" because it is not defined.", ext.name))) };
            t.fields.extend(ext.fields);
            t.interfaces.extend(ext.interfaces);
            t.members.extend(ext.members);
            t.enum_values.extend(ext.enum_values);
            t.input_fields.extend(ext.input_fields);
        }
        for (kind, name) in roots {
            match kind {
                OpKind::Query => s.query = name,
                OpKind::Mutation => s.mutation = Some(name),
                OpKind::Subscription => s.subscription = Some(name),
            }
        }
        if s.mutation.is_none() && s.types.contains_key("Mutation") {
            s.mutation = Some("Mutation".into());
        }
        if s.subscription.is_none() && s.types.contains_key("Subscription") {
            s.subscription = Some("Subscription".into());
        }
        s.check()?;
        Ok(s)
    }

    /// Builds a schema from an introspection result (`{"data": {"__schema": …}}` or `{"__schema": …}`).
    pub fn from_introspection(v: &Json) -> Result<Self, GqlError> {
        let sch = v.pointer("/data/__schema").or_else(|| v.get("__schema")).ok_or_else(|| GqlError::new("not an introspection result (no __schema)"))?;
        let mut s = Self::empty();
        let name_of = |x: Option<&Json>| x.and_then(|t| t.get("name")).and_then(Json::as_str).map(str::to_string);
        s.query = name_of(sch.get("queryType")).ok_or_else(|| GqlError::new("introspection: queryType is missing"))?;
        s.mutation = name_of(sch.get("mutationType"));
        s.subscription = name_of(sch.get("subscriptionType"));
        let text = |x: &Json, k: &str| x.get(k).and_then(Json::as_str).map(str::to_string);
        let input_value = |x: &Json| -> Result<InputValueDef, GqlError> {
            Ok(InputValueDef {
                name: text(x, "name").unwrap_or_default(),
                description: text(x, "description"),
                ty: type_ref_of(x.get("type"))?,
                default: text(x, "defaultValue").and_then(|d| parse_const_value(&d).ok()),
            })
        };
        for t in sch.get("types").and_then(Json::as_array).into_iter().flatten() {
            let name = text(t, "name").unwrap_or_default();
            if name.starts_with("__") || BUILTIN_SCALARS.contains(&name.as_str()) {
                continue;
            }
            let kind = match t.get("kind").and_then(Json::as_str) {
                Some("SCALAR") => TypeKind::Scalar,
                Some("OBJECT") => TypeKind::Object,
                Some("INTERFACE") => TypeKind::Interface,
                Some("UNION") => TypeKind::Union,
                Some("ENUM") => TypeKind::Enum,
                Some("INPUT_OBJECT") => TypeKind::InputObject,
                other => return Err(GqlError::new(format!("introspection: type {name} has unknown kind {other:?}"))),
            };
            let mut def = TypeDef::new(name, kind);
            def.description = text(t, "description");
            def.specified_by = text(t, "specifiedByURL").or_else(|| text(t, "specifiedByUrl"));
            for f in t.get("fields").and_then(Json::as_array).into_iter().flatten() {
                def.fields.push(FieldDef {
                    name: text(f, "name").unwrap_or_default(),
                    description: text(f, "description"),
                    args: f.get("args").and_then(Json::as_array).into_iter().flatten().map(input_value).collect::<Result<_, _>>()?,
                    ty: type_ref_of(f.get("type"))?,
                    deprecated: f.get("isDeprecated").and_then(Json::as_bool).unwrap_or(false).then(|| text(f, "deprecationReason").unwrap_or_else(|| "No longer supported".into())),
                });
            }
            def.interfaces = t.get("interfaces").and_then(Json::as_array).into_iter().flatten().filter_map(|i| text(i, "name")).collect();
            if kind == TypeKind::Union {
                def.members = t.get("possibleTypes").and_then(Json::as_array).into_iter().flatten().filter_map(|i| text(i, "name")).collect();
            }
            for e in t.get("enumValues").and_then(Json::as_array).into_iter().flatten() {
                def.enum_values.push(EnumValueDef {
                    name: text(e, "name").unwrap_or_default(),
                    description: text(e, "description"),
                    deprecated: e.get("isDeprecated").and_then(Json::as_bool).unwrap_or(false).then(|| text(e, "deprecationReason").unwrap_or_else(|| "No longer supported".into())),
                });
            }
            def.input_fields = t.get("inputFields").and_then(Json::as_array).into_iter().flatten().map(input_value).collect::<Result<_, _>>()?;
            s.types.insert(def.name.clone(), def);
        }
        for d in sch.get("directives").and_then(Json::as_array).into_iter().flatten() {
            let name = text(d, "name").unwrap_or_default();
            if s.directives.contains_key(&name) {
                continue;
            }
            s.directives.insert(
                name.clone(),
                DirectiveDef {
                    name,
                    description: text(d, "description"),
                    args: d.get("args").and_then(Json::as_array).into_iter().flatten().map(input_value).collect::<Result<_, _>>()?,
                    locations: d.get("locations").and_then(Json::as_array).into_iter().flatten().filter_map(|l| l.as_str().map(str::to_string)).collect(),
                    repeatable: d.get("isRepeatable").and_then(Json::as_bool).unwrap_or(false),
                },
            );
        }
        s.check()?;
        Ok(s)
    }

    /// Every referenced type exists; root types are objects.
    fn check(&self) -> Result<(), GqlError> {
        let missing = |n: &str| GqlError::new(format!("Unknown type \"{n}\"."));
        match self.types.get(&self.query) {
            Some(t) if t.kind == TypeKind::Object => {}
            Some(_) => return Err(GqlError::new(format!("Query root type must be Object type, it cannot be {}.", self.query))),
            None => return Err(GqlError::new(format!("Query root type \"{}\" is not defined (add `type Query {{ … }}`).", self.query))),
        }
        for root in [&self.mutation, &self.subscription].into_iter().flatten() {
            if self.types.get(root).is_none_or(|t| t.kind != TypeKind::Object) {
                return Err(missing(root));
            }
        }
        for t in self.types.values() {
            for f in &t.fields {
                if !self.types.contains_key(f.ty.named()) {
                    return Err(GqlError::new(format!("Unknown type \"{}\" (field {}.{}).", f.ty.named(), t.name, f.name)));
                }
                for a in &f.args {
                    if !self.types.contains_key(a.ty.named()) {
                        return Err(missing(a.ty.named()));
                    }
                }
            }
            for i in t.interfaces.iter().chain(&t.members) {
                if !self.types.contains_key(i) {
                    return Err(missing(i));
                }
            }
            for f in &t.input_fields {
                if !self.types.contains_key(f.ty.named()) {
                    return Err(missing(f.ty.named()));
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    pub fn root(&self, kind: OpKind) -> Option<&TypeDef> {
        match kind {
            OpKind::Query => self.types.get(&self.query),
            OpKind::Mutation => self.mutation.as_ref().and_then(|m| self.types.get(m)),
            OpKind::Subscription => self.subscription.as_ref().and_then(|m| self.types.get(m)),
        }
    }

    pub fn is_composite(&self, name: &str) -> bool {
        self.types.get(name).is_some_and(|t| matches!(t.kind, TypeKind::Object | TypeKind::Interface | TypeKind::Union))
    }

    pub fn is_input(&self, name: &str) -> bool {
        self.types.get(name).is_some_and(|t| matches!(t.kind, TypeKind::Scalar | TypeKind::Enum | TypeKind::InputObject))
    }

    pub fn is_leaf(&self, name: &str) -> bool {
        self.types.get(name).is_some_and(|t| matches!(t.kind, TypeKind::Scalar | TypeKind::Enum))
    }

    /// Concrete object types of a (possibly abstract) type.
    pub fn possible_types(&self, name: &str) -> Vec<&str> {
        match self.types.get(name) {
            Some(t) if t.kind == TypeKind::Object => vec![t.name.as_str()],
            Some(t) if t.kind == TypeKind::Union => t.members.iter().map(String::as_str).collect(),
            Some(t) if t.kind == TypeKind::Interface => {
                self.types.values().filter(|o| o.kind == TypeKind::Object && o.interfaces.iter().any(|i| i == name)).map(|o| o.name.as_str()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Whether a fragment on `cond` applies to an object of type `object`.
    pub fn type_applies(&self, cond: &str, object: &str) -> bool {
        cond == object || self.possible_types(cond).contains(&object)
    }

    /// Whether fragments on `a` and `b` can overlap (for spread validation).
    pub fn types_overlap(&self, a: &str, b: &str) -> bool {
        let pa = self.possible_types(a);
        self.possible_types(b).iter().any(|t| pa.contains(t))
    }

    /// Fields of an object or interface type (none for others).
    pub fn field_of(&self, type_name: &str, field: &str) -> Option<&FieldDef> {
        self.types.get(type_name).and_then(|t| t.field(field))
    }

    /// The introspection `__schema` value (every field, including deprecated ones; the
    /// executor filters per `includeDeprecated`).
    pub fn introspection(&self) -> Json {
        let types: Vec<Json> = self.types.values().map(|t| self.type_json(t)).collect();
        let directives: Vec<Json> = self
            .directives
            .values()
            .map(|d| {
                json!({
                    "name": d.name,
                    "description": d.description,
                    "isRepeatable": d.repeatable,
                    "locations": d.locations,
                    "args": d.args.iter().map(|a| self.input_json(a)).collect::<Vec<_>>(),
                })
            })
            .collect();
        let named = |n: Option<&String>| n.and_then(|n| self.types.get(n)).map(|t| self.type_json(t)).unwrap_or(Json::Null);
        json!({
            "description": Json::Null,
            "queryType": named(Some(&self.query)),
            "mutationType": named(self.mutation.as_ref()),
            "subscriptionType": named(self.subscription.as_ref()),
            "types": types,
            "directives": directives,
        })
    }

    /// `__Type` for a named type (full, one level of references resolved by name lookup: nested
    /// `__Type`s are expanded lazily by the executor through [`Self::type_ref_json`]).
    pub fn type_json(&self, t: &TypeDef) -> Json {
        let mut m = Map::new();
        m.insert("kind".into(), json!(t.kind.introspection_name()));
        m.insert("name".into(), json!(t.name));
        m.insert("description".into(), json!(t.description));
        m.insert("specifiedByURL".into(), json!(t.specified_by));
        m.insert("isOneOf".into(), if t.kind == TypeKind::InputObject { json!(false) } else { Json::Null });
        let is_obj = matches!(t.kind, TypeKind::Object | TypeKind::Interface);
        m.insert(
            "fields".into(),
            if is_obj {
                Json::Array(
                    t.fields
                        .iter()
                        .map(|f| {
                            json!({
                                "name": f.name,
                                "description": f.description,
                                "args": f.args.iter().map(|a| self.input_json(a)).collect::<Vec<_>>(),
                                "type": self.type_ref_json(&f.ty),
                                "isDeprecated": f.deprecated.is_some(),
                                "deprecationReason": f.deprecated,
                            })
                        })
                        .collect(),
                )
            } else {
                Json::Null
            },
        );
        m.insert("interfaces".into(), if is_obj { Json::Array(t.interfaces.iter().map(|i| self.named_ref(i)).collect()) } else { Json::Null });
        m.insert(
            "possibleTypes".into(),
            if matches!(t.kind, TypeKind::Interface | TypeKind::Union) { Json::Array(self.possible_types(&t.name).into_iter().map(|p| self.named_ref(p)).collect()) } else { Json::Null },
        );
        m.insert(
            "enumValues".into(),
            if t.kind == TypeKind::Enum {
                Json::Array(
                    t.enum_values
                        .iter()
                        .map(|e| json!({ "name": e.name, "description": e.description, "isDeprecated": e.deprecated.is_some(), "deprecationReason": e.deprecated }))
                        .collect(),
                )
            } else {
                Json::Null
            },
        );
        m.insert("inputFields".into(), if t.kind == TypeKind::InputObject { Json::Array(t.input_fields.iter().map(|a| self.input_json(a)).collect()) } else { Json::Null });
        m.insert("ofType".into(), Json::Null);
        Json::Object(m)
    }

    fn input_json(&self, a: &InputValueDef) -> Json {
        json!({
            "name": a.name,
            "description": a.description,
            "type": self.type_ref_json(&a.ty),
            "defaultValue": a.default.as_ref().map(Value::print),
            "isDeprecated": false,
            "deprecationReason": Json::Null,
        })
    }

    /// A reference to a named type: `{"kind", "name", "$ref": name}` — the executor expands it to
    /// the full type when more than kind/name is selected.
    fn named_ref(&self, name: &str) -> Json {
        let kind = self.types.get(name).map_or("SCALAR", |t| t.kind.introspection_name());
        json!({ "kind": kind, "name": name, "ofType": Json::Null, "$ref": name })
    }

    pub fn type_ref_json(&self, t: &TypeRef) -> Json {
        match t {
            TypeRef::Named(n) => self.named_ref(n),
            TypeRef::List(inner) => json!({ "kind": "LIST", "name": Json::Null, "ofType": self.type_ref_json(inner) }),
            TypeRef::NonNull(inner) => json!({ "kind": "NON_NULL", "name": Json::Null, "ofType": self.type_ref_json(inner) }),
        }
    }

    /// Root fields of the query type (for the UI).
    pub fn summary(&self) -> Json {
        let roots = |t: Option<&TypeDef>| t.map(|t| t.fields.iter().map(|f| format!("{}: {}", f.name, f.ty)).collect::<Vec<_>>()).unwrap_or_default();
        json!({
            "types": self.types.keys().filter(|k| !k.starts_with("__") && !BUILTIN_SCALARS.contains(&k.as_str())).count(),
            "query": roots(self.root(OpKind::Query)),
            "mutation": roots(self.root(OpKind::Mutation)),
            "subscription": roots(self.root(OpKind::Subscription)),
        })
    }
}

fn type_ref_of(v: Option<&Json>) -> Result<TypeRef, GqlError> {
    let v = v.ok_or_else(|| GqlError::new("introspection: type reference missing"))?;
    match v.get("kind").and_then(Json::as_str) {
        Some("NON_NULL") => Ok(TypeRef::NonNull(Box::new(type_ref_of(v.get("ofType"))?))),
        Some("LIST") => Ok(TypeRef::List(Box::new(type_ref_of(v.get("ofType"))?))),
        _ => v.get("name").and_then(Json::as_str).map(|n| TypeRef::Named(n.to_string())).ok_or_else(|| GqlError::new("introspection: named type without a name")),
    }
}

/// Parses a constant value (`defaultValue` strings in introspection results).
pub fn parse_const_value(text: &str) -> Result<Value, GqlError> {
    let doc = parse(&format!("{{ f(v: {text}) }}"))?;
    let op = doc.operations().next().ok_or_else(|| GqlError::new("bad value"))?;
    match op.selection.first() {
        Some(Selection::Field(f)) => f.args.first().map(|(_, v)| v.clone()).ok_or_else(|| GqlError::new("bad value")),
        _ => Err(GqlError::new("bad value")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const SDL: &str = r#"
        type Query { user(id: ID!): User, users(first: Int = 2): [User!]!, search(term: String!): [Result!]!, node(id: ID!): Node }
        type Mutation { createUser(input: UserInput!): User! }
        interface Node { id: ID! }
        type User implements Node { id: ID! name: String! email: String role: Role! posts: [Post!]! }
        type Post implements Node { id: ID! title: String! }
        union Result = User | Post
        enum Role { ADMIN MEMBER }
        input UserInput { name: String!, role: Role = MEMBER }
    "#;

    #[test]
    fn sdl_schema_and_introspection_round_trip() {
        let s = Schema::from_sdl(&[SDL]).unwrap();
        assert_eq!(s.mutation.as_deref(), Some("Mutation"));
        assert_eq!(s.possible_types("Node"), vec!["User", "Post"]);
        assert!(s.type_applies("Result", "Post"));
        assert!(s.is_leaf("Role") && s.is_composite("Result") && s.is_input("UserInput"));
        let intro = json!({ "data": { "__schema": strip_refs(&s.introspection()) } });
        let back = Schema::from_introspection(&intro).unwrap();
        assert_eq!(back.types.get("User").unwrap().fields.len(), 5);
        assert_eq!(back.types.get("UserInput").unwrap().input_fields[1].default, Some(Value::Enum("MEMBER".into())));
        assert_eq!(back.possible_types("Result"), vec!["User", "Post"]);
    }

    /// Drops the executor's `$ref` markers (references keep kind/name, which is all
    /// `from_introspection` needs).
    fn strip_refs(v: &Json) -> Json {
        match v {
            Json::Object(o) => Json::Object(o.iter().filter(|(k, _)| *k != "$ref").map(|(k, v)| (k.clone(), strip_refs(v))).collect()),
            Json::Array(a) => Json::Array(a.iter().map(strip_refs).collect()),
            other => other.clone(),
        }
    }

    #[test]
    fn schema_errors() {
        assert!(Schema::from_sdl(&["type User { id: ID }"]).unwrap_err().message.contains("Query root type"));
        assert!(Schema::from_sdl(&["type Query { a: Nope }"]).unwrap_err().message.contains("Unknown type \"Nope\""));
        assert!(Schema::from_sdl(&["type Query { a: Int }", "extend type Missing { b: Int }"]).unwrap_err().message.contains("Cannot extend"));
        let s = Schema::from_sdl(&["type Query { a: Int }", "extend type Query { b: String }"]).unwrap();
        assert_eq!(s.types["Query"].fields.len(), 2);
        assert!(Schema::from_sdl(&["type Query { a: Int } query { a }"]).is_err());
    }
}
