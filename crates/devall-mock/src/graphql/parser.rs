//! A GraphQL lexer and recursive-descent parser for executable documents (queries) and the type
//! system definition language (SDL), following the October 2021 spec. Errors read like
//! graphql-js (`Syntax Error: Expected Name, found "}".`) with a 1-based line/column.

use super::GqlError;
use super::ast::*;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Punct(char),
    Spread,
    Name(String),
    Int(String),
    Float(String),
    Str(String),
    BlockStr(String),
    Eof,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Punct(c) => format!("\"{c}\""),
            Tok::Spread => "\"...\"".into(),
            Tok::Name(n) => format!("Name \"{n}\""),
            Tok::Int(i) => format!("Int \"{i}\""),
            Tok::Float(f) => format!("Float \"{f}\""),
            Tok::Str(s) | Tok::BlockStr(s) => format!("String {}", serde_json::to_string(s).unwrap_or_default()),
            Tok::Eof => "<EOF>".into(),
        }
    }
}

struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    i: usize,
    line: usize,
    line_start: usize,
}

fn syntax(pos: Pos, msg: impl Into<String>) -> GqlError {
    GqlError::at(format!("Syntax Error: {}", msg.into()), pos)
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Self { src: text.as_bytes(), text, i: 0, line: 1, line_start: 0 }
    }

    fn pos(&self) -> Pos {
        Pos { line: self.line, column: self.text[self.line_start..self.i.min(self.text.len())].chars().count() + 1 }
    }

    fn newline(&mut self) {
        self.line += 1;
        self.line_start = self.i;
    }

    fn skip_ignored(&mut self) {
        while self.i < self.src.len() {
            match self.src[self.i] {
                b' ' | b'\t' | b',' => self.i += 1,
                b'\n' => {
                    self.i += 1;
                    self.newline();
                }
                b'\r' => {
                    self.i += 1;
                    if self.src.get(self.i) == Some(&b'\n') {
                        self.i += 1;
                    }
                    self.newline();
                }
                b'#' => {
                    while self.i < self.src.len() && !matches!(self.src[self.i], b'\n' | b'\r') {
                        self.i += 1;
                    }
                }
                0xEF if self.src[self.i..].starts_with(&[0xEF, 0xBB, 0xBF]) => self.i += 3,
                _ => break,
            }
        }
    }

    fn next(&mut self) -> Result<(Tok, Pos), GqlError> {
        self.skip_ignored();
        let pos = self.pos();
        let Some(&c) = self.src.get(self.i) else { return Ok((Tok::Eof, pos)) };
        match c {
            b'!' | b'$' | b'&' | b'(' | b')' | b':' | b'=' | b'@' | b'[' | b']' | b'{' | b'|' | b'}' => {
                self.i += 1;
                Ok((Tok::Punct(c as char), pos))
            }
            b'.' => {
                if self.src[self.i..].starts_with(b"...") {
                    self.i += 3;
                    Ok((Tok::Spread, pos))
                } else {
                    Err(syntax(pos, "Unexpected \".\"."))
                }
            }
            b'_' | b'a'..=b'z' | b'A'..=b'Z' => {
                let start = self.i;
                while self.i < self.src.len() && (self.src[self.i].is_ascii_alphanumeric() || self.src[self.i] == b'_') {
                    self.i += 1;
                }
                Ok((Tok::Name(self.text[start..self.i].to_string()), pos))
            }
            b'-' | b'0'..=b'9' => self.number(pos),
            b'"' => {
                if self.src[self.i..].starts_with(b"\"\"\"") {
                    self.block_string(pos)
                } else {
                    self.string(pos)
                }
            }
            _ => {
                let ch = self.text[self.i..].chars().next().unwrap_or('?');
                Err(syntax(pos, format!("Unexpected character: {}.", serde_json::to_string(&ch.to_string()).unwrap_or_default())))
            }
        }
    }

    fn number(&mut self, pos: Pos) -> Result<(Tok, Pos), GqlError> {
        let start = self.i;
        if self.src[self.i] == b'-' {
            self.i += 1;
        }
        let digits = |l: &mut Self| {
            let s = l.i;
            while l.i < l.src.len() && l.src[l.i].is_ascii_digit() {
                l.i += 1;
            }
            l.i > s
        };
        if self.src.get(self.i) == Some(&b'0') {
            self.i += 1;
            if self.src.get(self.i).is_some_and(u8::is_ascii_digit) {
                return Err(syntax(pos, "Invalid number, unexpected digit after 0."));
            }
        } else if !digits(self) {
            return Err(syntax(pos, "Invalid number, expected digit."));
        }
        let mut float = false;
        if self.src.get(self.i) == Some(&b'.') {
            float = true;
            self.i += 1;
            if !digits(self) {
                return Err(syntax(pos, "Invalid number, expected digit."));
            }
        }
        if matches!(self.src.get(self.i), Some(b'e' | b'E')) {
            float = true;
            self.i += 1;
            if matches!(self.src.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return Err(syntax(pos, "Invalid number, expected digit."));
            }
        }
        if self.src.get(self.i).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_' || *c == b'.') {
            return Err(syntax(pos, "Invalid number, expected digit."));
        }
        let text = self.text[start..self.i].to_string();
        Ok((if float { Tok::Float(text) } else { Tok::Int(text) }, pos))
    }

    fn string(&mut self, pos: Pos) -> Result<(Tok, Pos), GqlError> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let Some(&c) = self.src.get(self.i) else { return Err(syntax(pos, "Unterminated string.")) };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok((Tok::Str(out), pos));
                }
                b'\n' | b'\r' => return Err(syntax(pos, "Unterminated string.")),
                b'\\' => {
                    let e = self.src.get(self.i + 1).copied().ok_or_else(|| syntax(pos, "Unterminated string."))?;
                    self.i += 2;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let ch = self.unicode_escape(pos)?;
                            out.push(ch);
                        }
                        other => return Err(syntax(pos, format!("Invalid character escape sequence: \"\\{}\".", other as char))),
                    }
                }
                _ => {
                    let ch = self.text[self.i..].chars().next().unwrap_or('\u{fffd}');
                    out.push(ch);
                    self.i += ch.len_utf8();
                }
            }
        }
    }

    fn unicode_escape(&mut self, pos: Pos) -> Result<char, GqlError> {
        let bad = || syntax(pos, "Invalid Unicode escape sequence.");
        if self.src.get(self.i) == Some(&b'{') {
            let end = self.text[self.i..].find('}').ok_or_else(bad)? + self.i;
            let code = u32::from_str_radix(&self.text[self.i + 1..end], 16).map_err(|_| bad())?;
            self.i = end + 1;
            return char::from_u32(code).ok_or_else(bad);
        }
        let hex = self.text.get(self.i..self.i + 4).ok_or_else(bad)?;
        let code = u32::from_str_radix(hex, 16).map_err(|_| bad())?;
        self.i += 4;
        if (0xD800..0xDC00).contains(&code) && self.text[self.i..].starts_with("\\u") {
            let lo = self.text.get(self.i + 2..self.i + 6).and_then(|h| u32::from_str_radix(h, 16).ok()).ok_or_else(bad)?;
            self.i += 6;
            return char::from_u32(0x10000 + ((code - 0xD800) << 10) + (lo - 0xDC00)).ok_or_else(bad);
        }
        char::from_u32(code).ok_or_else(bad)
    }

    fn block_string(&mut self, pos: Pos) -> Result<(Tok, Pos), GqlError> {
        self.i += 3;
        let mut raw = String::new();
        loop {
            if self.i >= self.src.len() {
                return Err(syntax(pos, "Unterminated string."));
            }
            if self.src[self.i..].starts_with(b"\"\"\"") {
                self.i += 3;
                return Ok((Tok::BlockStr(block_string_value(&raw)), pos));
            }
            if self.src[self.i..].starts_with(b"\\\"\"\"") {
                raw.push_str("\"\"\"");
                self.i += 4;
                continue;
            }
            let ch = self.text[self.i..].chars().next().unwrap_or('\u{fffd}');
            self.i += ch.len_utf8();
            if ch == '\n' || (ch == '\r' && self.src.get(self.i) != Some(&b'\n')) {
                self.newline();
            }
            raw.push(ch);
        }
    }
}

/// Spec `BlockStringValue`: common indentation removed, blank leading/trailing lines dropped.
fn block_string_value(raw: &str) -> String {
    let lines: Vec<&str> = raw.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter_map(|l| {
            let n = l.len() - l.trim_start_matches([' ', '\t']).len();
            (n < l.len()).then_some(n)
        })
        .min()
        .unwrap_or(0);
    let mut out: Vec<String> = lines.iter().enumerate().map(|(i, l)| if i == 0 { l.to_string() } else { l.get(indent..).unwrap_or("").to_string() }).collect();
    while out.first().is_some_and(|l| l.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

pub struct Parser<'a> {
    lex: Lexer<'a>,
    tok: Tok,
    pos: Pos,
}

impl<'a> Parser<'a> {
    pub fn new(text: &'a str) -> Result<Self, GqlError> {
        let mut lex = Lexer::new(text);
        let (tok, pos) = lex.next()?;
        Ok(Self { lex, tok, pos })
    }

    fn bump(&mut self) -> Result<(Tok, Pos), GqlError> {
        let (t, p) = self.lex.next()?;
        Ok((std::mem::replace(&mut self.tok, t), std::mem::replace(&mut self.pos, p)))
    }

    fn unexpected<T>(&self, expected: &str) -> Result<T, GqlError> {
        Err(syntax(self.pos, format!("Expected {expected}, found {}.", self.tok.describe())))
    }

    fn is(&self, c: char) -> bool {
        self.tok == Tok::Punct(c)
    }

    fn eat(&mut self, c: char) -> Result<bool, GqlError> {
        if self.is(c) {
            self.bump()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn expect(&mut self, c: char) -> Result<Pos, GqlError> {
        if self.is(c) {
            return Ok(self.bump()?.1);
        }
        self.unexpected(&format!("\"{c}\""))
    }

    fn is_name(&self, n: &str) -> bool {
        matches!(&self.tok, Tok::Name(x) if x == n)
    }

    fn name(&mut self) -> Result<(String, Pos), GqlError> {
        match &self.tok {
            Tok::Name(_) => {
                let (t, p) = self.bump()?;
                match t {
                    Tok::Name(n) => Ok((n, p)),
                    _ => unreachable!(),
                }
            }
            _ => self.unexpected("Name"),
        }
    }

    fn keyword(&mut self, k: &str) -> Result<(), GqlError> {
        if self.is_name(k) {
            self.bump()?;
            return Ok(());
        }
        self.unexpected(&format!("\"{k}\""))
    }

    // ------------------------------------------------------------ documents

    pub fn document(mut self) -> Result<Document, GqlError> {
        let mut doc = Document::default();
        if self.tok == Tok::Eof {
            return Err(syntax(self.pos, "Unexpected <EOF>."));
        }
        while self.tok != Tok::Eof {
            doc.definitions.push(self.definition()?);
        }
        Ok(doc)
    }

    fn definition(&mut self) -> Result<Definition, GqlError> {
        if self.is('{') {
            let pos = self.pos;
            let selection = self.selection_set()?;
            return Ok(Definition::Operation(Operation { kind: OpKind::Query, name: None, variables: Vec::new(), directives: Vec::new(), selection, pos }));
        }
        let description = self.description()?;
        let Tok::Name(kw) = &self.tok else { return self.unexpected("a definition") };
        match kw.as_str() {
            "query" | "mutation" | "subscription" if description.is_none() => self.operation().map(Definition::Operation),
            "fragment" if description.is_none() => self.fragment().map(Definition::Fragment),
            "schema" => self.schema_def(),
            "scalar" | "type" | "interface" | "union" | "enum" | "input" => self.type_def(description, false),
            "directive" => self.directive_def(description),
            "extend" if description.is_none() => {
                self.bump()?;
                if self.is_name("schema") {
                    return self.schema_def_after_keyword();
                }
                self.type_def(None, true)
            }
            other => {
                let other = other.to_string();
                Err(syntax(self.pos, format!("Unexpected Name \"{other}\".")))
            }
        }
    }

    fn operation(&mut self) -> Result<Operation, GqlError> {
        let (kw, pos) = self.name()?;
        let kind = match kw.as_str() {
            "mutation" => OpKind::Mutation,
            "subscription" => OpKind::Subscription,
            _ => OpKind::Query,
        };
        let name = if matches!(self.tok, Tok::Name(_)) { Some(self.name()?.0) } else { None };
        let mut variables = Vec::new();
        if self.eat('(')? {
            while !self.eat(')')? {
                let vpos = self.expect('$')?;
                let (vname, _) = self.name()?;
                self.expect(':')?;
                let ty = self.type_ref()?;
                let default = if self.eat('=')? { Some(self.value(true)?) } else { None };
                self.directives(true)?;
                variables.push(VarDef { name: vname, ty, default, pos: vpos });
            }
        }
        let directives = self.directives(false)?;
        let selection = self.selection_set()?;
        Ok(Operation { kind, name, variables, directives, selection, pos })
    }

    fn fragment(&mut self) -> Result<Fragment, GqlError> {
        let (_, pos) = self.name()?;
        let (name, npos) = self.name()?;
        if name == "on" {
            return Err(syntax(npos, "Unexpected Name \"on\"."));
        }
        self.keyword("on")?;
        let (type_cond, _) = self.name()?;
        let directives = self.directives(false)?;
        let selection = self.selection_set()?;
        Ok(Fragment { name, type_cond, directives, selection, pos })
    }

    fn selection_set(&mut self) -> Result<Vec<Selection>, GqlError> {
        self.expect('{')?;
        if self.is('}') {
            return self.unexpected("Name");
        }
        let mut out = Vec::new();
        loop {
            out.push(self.selection()?);
            if self.eat('}')? {
                return Ok(out);
            }
        }
    }

    fn selection(&mut self) -> Result<Selection, GqlError> {
        if self.tok == Tok::Spread {
            let (_, pos) = self.bump()?;
            if matches!(&self.tok, Tok::Name(n) if n != "on") {
                let (name, _) = self.name()?;
                let directives = self.directives(false)?;
                return Ok(Selection::Spread { name, directives, pos });
            }
            let type_cond = if self.is_name("on") {
                self.bump()?;
                Some(self.name()?.0)
            } else {
                None
            };
            let directives = self.directives(false)?;
            let selection = self.selection_set()?;
            return Ok(Selection::Inline { type_cond, directives, selection, pos });
        }
        let (first, pos) = self.name()?;
        let (alias, name) = if self.eat(':')? { (Some(first), self.name()?.0) } else { (None, first) };
        let args = if self.is('(') { self.arguments(false)? } else { Vec::new() };
        let directives = self.directives(false)?;
        let selection = if self.is('{') { self.selection_set()? } else { Vec::new() };
        Ok(Selection::Field(Field { alias, name, args, directives, selection, pos }))
    }

    fn arguments(&mut self, constant: bool) -> Result<Vec<(String, Value)>, GqlError> {
        self.expect('(')?;
        let mut out = Vec::new();
        while !self.eat(')')? {
            let (n, _) = self.name()?;
            self.expect(':')?;
            out.push((n, self.value(constant)?));
        }
        Ok(out)
    }

    fn directives(&mut self, constant: bool) -> Result<Vec<Directive>, GqlError> {
        let mut out = Vec::new();
        while self.is('@') {
            let (_, pos) = self.bump()?;
            let (name, _) = self.name()?;
            let args = if self.is('(') { self.arguments(constant)? } else { Vec::new() };
            out.push(Directive { name, args, pos });
        }
        Ok(out)
    }

    fn value(&mut self, constant: bool) -> Result<Value, GqlError> {
        let pos = self.pos;
        match self.tok.clone() {
            Tok::Punct('$') if !constant => {
                self.bump()?;
                Ok(Value::Variable(self.name()?.0))
            }
            Tok::Punct('[') => {
                self.bump()?;
                let mut items = Vec::new();
                while !self.eat(']')? {
                    items.push(self.value(constant)?);
                }
                Ok(Value::List(items))
            }
            Tok::Punct('{') => {
                self.bump()?;
                let mut fields = Vec::new();
                while !self.eat('}')? {
                    let (n, _) = self.name()?;
                    self.expect(':')?;
                    fields.push((n, self.value(constant)?));
                }
                Ok(Value::Object(fields))
            }
            Tok::Int(i) => {
                self.bump()?;
                i.parse::<i64>().map(Value::Int).or_else(|_| i.parse::<f64>().map(Value::Float)).map_err(|_| syntax(pos, "Invalid number."))
            }
            Tok::Float(f) => {
                self.bump()?;
                f.parse::<f64>().map(Value::Float).map_err(|_| syntax(pos, "Invalid number."))
            }
            Tok::Str(s) | Tok::BlockStr(s) => {
                self.bump()?;
                Ok(Value::String(s))
            }
            Tok::Name(n) => {
                self.bump()?;
                Ok(match n.as_str() {
                    "true" => Value::Boolean(true),
                    "false" => Value::Boolean(false),
                    "null" => Value::Null,
                    _ => Value::Enum(n),
                })
            }
            _ => self.unexpected("a value"),
        }
    }

    fn type_ref(&mut self) -> Result<TypeRef, GqlError> {
        let inner = if self.eat('[')? {
            let t = self.type_ref()?;
            self.expect(']')?;
            TypeRef::List(Box::new(t))
        } else {
            TypeRef::Named(self.name()?.0)
        };
        Ok(if self.eat('!')? { TypeRef::NonNull(Box::new(inner)) } else { inner })
    }

    // ------------------------------------------------------------ SDL

    fn description(&mut self) -> Result<Option<String>, GqlError> {
        match &self.tok {
            Tok::Str(_) | Tok::BlockStr(_) => match self.bump()?.0 {
                Tok::Str(s) | Tok::BlockStr(s) => Ok(Some(s)),
                _ => unreachable!(),
            },
            _ => Ok(None),
        }
    }

    fn schema_def(&mut self) -> Result<Definition, GqlError> {
        self.keyword("schema")?;
        self.schema_body()
    }

    fn schema_def_after_keyword(&mut self) -> Result<Definition, GqlError> {
        self.keyword("schema")?;
        self.schema_body()
    }

    fn schema_body(&mut self) -> Result<Definition, GqlError> {
        self.directives(true)?;
        let mut roots = Vec::new();
        if self.eat('{')? {
            while !self.eat('}')? {
                let (op, pos) = self.name()?;
                let kind = match op.as_str() {
                    "query" => OpKind::Query,
                    "mutation" => OpKind::Mutation,
                    "subscription" => OpKind::Subscription,
                    _ => return Err(syntax(pos, format!("Unexpected Name \"{op}\"."))),
                };
                self.expect(':')?;
                roots.push((kind, self.name()?.0));
            }
        }
        Ok(Definition::Schema(roots))
    }

    fn deprecation(directives: &[Directive]) -> Option<String> {
        directives.iter().find(|d| d.name == "deprecated").map(|d| {
            d.args.iter().find(|(k, _)| k == "reason").and_then(|(_, v)| if let Value::String(s) = v { Some(s.clone()) } else { None }).unwrap_or_else(|| "No longer supported".into())
        })
    }

    fn input_values(&mut self, close: char) -> Result<Vec<InputValueDef>, GqlError> {
        let mut out = Vec::new();
        while !self.eat(close)? {
            let description = self.description()?;
            let (name, _) = self.name()?;
            self.expect(':')?;
            let ty = self.type_ref()?;
            let default = if self.eat('=')? { Some(self.value(true)?) } else { None };
            self.directives(true)?;
            out.push(InputValueDef { name, description, ty, default });
        }
        Ok(out)
    }

    fn type_def(&mut self, description: Option<String>, extend: bool) -> Result<Definition, GqlError> {
        let (kw, _) = self.name()?;
        let (name, _) = self.name()?;
        let kind = match kw.as_str() {
            "scalar" => TypeKind::Scalar,
            "type" => TypeKind::Object,
            "interface" => TypeKind::Interface,
            "union" => TypeKind::Union,
            "enum" => TypeKind::Enum,
            "input" => TypeKind::InputObject,
            _ => return self.unexpected("a type definition"),
        };
        let mut def = TypeDef::new(name, kind);
        def.description = description;
        if matches!(kind, TypeKind::Object | TypeKind::Interface) && self.is_name("implements") {
            self.bump()?;
            self.eat('&')?;
            def.interfaces.push(self.name()?.0);
            while self.eat('&')? || (matches!(&self.tok, Tok::Name(n) if n != "implements") && !self.is('{') && !self.is('@')) {
                if matches!(self.tok, Tok::Name(_)) {
                    def.interfaces.push(self.name()?.0);
                } else {
                    break;
                }
            }
        }
        let directives = self.directives(true)?;
        if kind == TypeKind::Scalar {
            def.specified_by = directives.iter().find(|d| d.name == "specifiedBy").and_then(|d| d.args.iter().find_map(|(k, v)| if let (true, Value::String(s)) = (k == "url", v) { Some(s.clone()) } else { None }));
        }
        match kind {
            TypeKind::Object | TypeKind::Interface => {
                if self.eat('{')? {
                    while !self.eat('}')? {
                        let description = self.description()?;
                        let (fname, _) = self.name()?;
                        let args = if self.eat('(')? { self.input_values(')')? } else { Vec::new() };
                        self.expect(':')?;
                        let ty = self.type_ref()?;
                        let d = self.directives(true)?;
                        def.fields.push(FieldDef { name: fname, description, args, ty, deprecated: Self::deprecation(&d) });
                    }
                }
            }
            TypeKind::Union => {
                if self.eat('=')? {
                    self.eat('|')?;
                    def.members.push(self.name()?.0);
                    while self.eat('|')? {
                        def.members.push(self.name()?.0);
                    }
                }
            }
            TypeKind::Enum => {
                if self.eat('{')? {
                    while !self.eat('}')? {
                        let description = self.description()?;
                        let (v, pos) = self.name()?;
                        if matches!(v.as_str(), "true" | "false" | "null") {
                            return Err(syntax(pos, format!("Name \"{v}\" is reserved and cannot be used for an enum value.")));
                        }
                        let d = self.directives(true)?;
                        def.enum_values.push(EnumValueDef { name: v, description, deprecated: Self::deprecation(&d) });
                    }
                }
            }
            TypeKind::InputObject => {
                if self.eat('{')? {
                    def.input_fields = self.input_values('}')?;
                }
            }
            TypeKind::Scalar => {}
        }
        Ok(Definition::Type { def, extend })
    }

    fn directive_def(&mut self, description: Option<String>) -> Result<Definition, GqlError> {
        self.keyword("directive")?;
        self.expect('@')?;
        let (name, _) = self.name()?;
        let args = if self.eat('(')? { self.input_values(')')? } else { Vec::new() };
        let repeatable = if self.is_name("repeatable") {
            self.bump()?;
            true
        } else {
            false
        };
        self.keyword("on")?;
        self.eat('|')?;
        let mut locations = vec![self.name()?.0];
        while self.eat('|')? {
            locations.push(self.name()?.0);
        }
        Ok(Definition::Directive(DirectiveDef { name, description, args, locations, repeatable }))
    }
}

/// Parses a document (operations, fragments and/or type system definitions).
pub fn parse(text: &str) -> Result<Document, GqlError> {
    Parser::new(text)?.document()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_queries() {
        let doc = parse(
            r#"
            # comment
            query GetUser($id: ID!, $n: Int = 3) @cached {
              user(id: $id) { id ...U posts(first: $n, filter: {tags: ["a", "b"], done: true}) @include(if: true) { title } }
              me: viewer { ... on Admin { level } ... @skip(if: false) { name } }
            }
            fragment U on User { name email }
            "#,
        )
        .unwrap();
        let ops: Vec<_> = doc.operations().collect();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name.as_deref(), Some("GetUser"));
        assert_eq!(ops[0].variables[0].ty.to_string(), "ID!");
        assert_eq!(ops[0].variables[1].default, Some(Value::Int(3)));
        let Selection::Field(user) = &ops[0].selection[0] else { panic!() };
        assert_eq!(user.args[0], ("id".to_string(), Value::Variable("id".into())));
        assert_eq!(user.selection.len(), 3);
        let Selection::Field(me) = &ops[0].selection[1] else { panic!() };
        assert_eq!(me.key(), "me");
        assert_eq!(doc.fragments().next().unwrap().type_cond, "User");
        assert_eq!(user.pos, Pos { line: 4, column: 15 });
    }

    #[test]
    fn parses_sdl() {
        let doc = parse(
            r#"
            schema { query: Q }
            """
            A user.
            """
            type User implements Node & Entity @key(fields: "id") {
              "the id" id: ID!
              posts(first: Int = 10, after: String): [Post!]! @deprecated(reason: "use feed")
            }
            interface Node { id: ID! }
            union Result = | User | Post
            enum Role { ADMIN USER @deprecated }
            input UserInput { name: String!, role: Role = USER }
            scalar DateTime @specifiedBy(url: "https://x")
            directive @key(fields: String!) repeatable on OBJECT | INTERFACE
            extend type User { age: Int }
            "#,
        )
        .unwrap();
        let types: Vec<&TypeDef> = doc.definitions.iter().filter_map(|d| if let Definition::Type { def, .. } = d { Some(def) } else { None }).collect();
        let user = types[0];
        assert_eq!(user.description.as_deref(), Some("A user."));
        assert_eq!(user.interfaces, vec!["Node", "Entity"]);
        assert_eq!(user.fields[1].args[0].default, Some(Value::Int(10)));
        assert_eq!(user.fields[1].deprecated.as_deref(), Some("use feed"));
        assert_eq!(types[2].members, vec!["User", "Post"]);
        assert_eq!(types[3].enum_values[1].deprecated.as_deref(), Some("No longer supported"));
        assert_eq!(types[4].input_fields[1].default, Some(Value::Enum("USER".into())));
        assert_eq!(types[5].specified_by.as_deref(), Some("https://x"));
    }

    #[test]
    fn syntax_errors_have_locations() {
        let e = parse("{ user { }").unwrap_err();
        assert_eq!(e.message, "Syntax Error: Expected Name, found \"}\".");
        assert_eq!(e.locations[0], Pos { line: 1, column: 10 });
        assert_eq!(parse("query { a(x: ) }").unwrap_err().message, "Syntax Error: Expected a value, found \")\".");
        assert!(parse("").unwrap_err().message.contains("<EOF>"));
        assert!(parse("{ a(s: \"open) }").unwrap_err().message.contains("Unterminated string"));
        assert_eq!(parse(r#"{ a(s: "\u00e9\n") }"#).unwrap().operations().count(), 1);
        assert_eq!(block_string_value("\n    hello\n      world\n  "), "hello\n  world");
    }
}
