//! MongoDB with the official driver (rustls, aws-lc). Filters, projections, sorts and pipelines
//! are Extended JSON — and, for convenience, the mongo-shell dialect people paste from docs:
//! unquoted keys, single quotes, comments, trailing commas, `ObjectId("…")`, `ISODate("…")`,
//! `NumberLong(…)`, `NumberDecimal("…")`, `UUID("…")`, `/regex/i`. Results are relaxed
//! Extended JSON. Writes (insert/update/delete, `$out`/`$merge`, mutating commands) are blocked
//! in read-only sessions.

use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use mongodb::bson::{self, Bson, Document, doc};
use mongodb::options::{ClientOptions, ServerAddress, Tls, TlsOptions};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::{DbError, Result, invalid};
use crate::model::TlsMode;
use crate::resolve::ResolvedConnection;

// ---------- relaxed (shell-style) JSON ----------

struct P<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
}

impl<'a> P<'a> {
    fn err<T>(&self, msg: &str) -> Result<T> {
        let line = self.src[..self.i.min(self.src.len())].matches('\n').count() + 1;
        Err(invalid(format!("{msg} (line {line}, offset {})", self.i)))
    }

    fn ws(&mut self) {
        loop {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"//") {
                while self.i < self.s.len() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if self.s[self.i..].starts_with(b"/*") {
                match self.src[self.i + 2..].find("*/") {
                    Some(p) => self.i += p + 4,
                    None => self.i = self.s.len(),
                }
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> Result<()> {
        if self.eat(c) { Ok(()) } else { self.err(&format!("expected '{}'", c as char)) }
    }

    fn value(&mut self) -> Result<Value> {
        self.ws();
        match self.peek() {
            None => self.err("unexpected end of input"),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"' | b'\'') => Ok(Value::String(self.string()?)),
            Some(b'/') => self.regex(),
            Some(c) if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {
                let word = self.ident();
                self.word_value(&word)
            }
            Some(c) => self.err(&format!("unexpected '{}'", c as char)),
        }
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'_' | b'$' | b'.')) {
            self.i += 1;
        }
        self.src[start..self.i].to_string()
    }

    fn string(&mut self) -> Result<String> {
        let q = self.s[self.i];
        self.i += 1;
        let mut out = String::new();
        loop {
            let rest = &self.src[self.i..];
            let Some(c) = rest.chars().next() else { return self.err("unterminated string") };
            self.i += c.len_utf8();
            match c {
                c if c as u32 == u32::from(q) => return Ok(out),
                '\\' => {
                    let Some(e) = self.src[self.i..].chars().next() else { return self.err("unterminated string") };
                    self.i += e.len_utf8();
                    match e {
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        '0' => out.push('\0'),
                        'u' => {
                            let hex = self.src.get(self.i..self.i + 4).ok_or_else(|| invalid("bad \\u escape"))?;
                            let cp = u32::from_str_radix(hex, 16).map_err(|_| invalid("bad \\u escape"))?;
                            self.i += 4;
                            // Surrogate pairs.
                            if (0xD800..0xDC00).contains(&cp) && self.src[self.i..].starts_with("\\u") {
                                let lo = self.src.get(self.i + 2..self.i + 6).and_then(|h| u32::from_str_radix(h, 16).ok()).unwrap_or(0);
                                if (0xDC00..0xE000).contains(&lo) {
                                    self.i += 6;
                                    out.push(char::from_u32(0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)).unwrap_or('\u{fffd}'));
                                    continue;
                                }
                            }
                            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn number(&mut self) -> Result<Value> {
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || matches!(self.s[self.i], b'-' | b'+' | b'.')) {
            self.i += 1;
        }
        let t = self.src[start..self.i].trim_start_matches('+');
        if let Ok(i) = t.parse::<i64>() {
            return Ok(if i32::try_from(i).is_ok() { json!(i) } else { json!({ "$numberLong": i.to_string() }) });
        }
        match t.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(json!(f)),
            _ => self.err(&format!("bad number '{t}'")),
        }
    }

    fn regex(&mut self) -> Result<Value> {
        self.i += 1;
        let mut pattern = String::new();
        loop {
            let Some(c) = self.src[self.i..].chars().next() else { return self.err("unterminated regex") };
            self.i += c.len_utf8();
            match c {
                '\\' => {
                    let Some(e) = self.src[self.i..].chars().next() else { return self.err("unterminated regex") };
                    self.i += e.len_utf8();
                    if e != '/' {
                        pattern.push('\\');
                    }
                    pattern.push(e);
                }
                '/' => break,
                other => pattern.push(other),
            }
        }
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_alphabetic() {
            self.i += 1;
        }
        let mut opts: Vec<char> = self.src[start..self.i].chars().collect();
        opts.sort_unstable();
        Ok(json!({ "$regularExpression": { "pattern": pattern, "options": opts.into_iter().collect::<String>() } }))
    }

    fn args(&mut self) -> Result<Vec<Value>> {
        self.expect(b'(')?;
        let mut out = Vec::new();
        if self.eat(b')') {
            return Ok(out);
        }
        loop {
            out.push(self.value()?);
            if self.eat(b')') {
                return Ok(out);
            }
            self.expect(b',')?;
            if self.eat(b')') {
                return Ok(out);
            }
        }
    }

    fn word_value(&mut self, word: &str) -> Result<Value> {
        let s = |v: &Value| -> String {
            match v {
                Value::String(s) => s.clone(),
                Value::Object(o) if o.contains_key("$numberLong") => o["$numberLong"].as_str().unwrap_or_default().to_string(),
                other => other.to_string(),
            }
        };
        Ok(match word {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "null" | "undefined" => Value::Null,
            "new" => {
                self.ws();
                let w = self.ident();
                return self.word_value(&w);
            }
            "ObjectId" => {
                let a = self.args()?;
                match a.first() {
                    Some(v) => json!({ "$oid": s(v) }),
                    None => return self.err("ObjectId needs a hex string"),
                }
            }
            "ISODate" | "Date" => {
                let a = self.args()?;
                match a.first() {
                    Some(Value::Number(n)) => json!({ "$date": { "$numberLong": n.to_string() } }),
                    Some(Value::Object(o)) if o.contains_key("$numberLong") => json!({ "$date": Value::Object(o.clone()) }),
                    Some(v) => {
                        let mut d = s(v);
                        // Date-only strings: midnight UTC.
                        if d.len() == 10 {
                            d.push_str("T00:00:00Z");
                        } else if !d.ends_with('Z') && !d.contains('+') && d.len() >= 19 && !d[10..].contains('-') {
                            d.push('Z');
                        }
                        json!({ "$date": d })
                    }
                    None => json!({ "$date": chrono_now_iso() }),
                }
            }
            "NumberLong" | "Long" => json!({ "$numberLong": s(self.args()?.first().unwrap_or(&json!("0"))) }),
            "NumberInt" | "Int32" => json!({ "$numberInt": s(self.args()?.first().unwrap_or(&json!("0"))) }),
            "NumberDecimal" | "Decimal128" => json!({ "$numberDecimal": s(self.args()?.first().unwrap_or(&json!("0"))) }),
            "Timestamp" => {
                let a = self.args()?;
                let n = |i: usize| a.get(i).and_then(|v| v.as_u64().or_else(|| s(v).parse().ok())).unwrap_or(0);
                json!({ "$timestamp": { "t": n(0), "i": n(1) } })
            }
            "UUID" => json!({ "$uuid": s(self.args()?.first().unwrap_or(&json!(""))) }),
            "BinData" => {
                let a = self.args()?;
                let sub = a.first().and_then(|v| v.as_u64()).unwrap_or(0);
                json!({ "$binary": { "base64": a.get(1).map(s).unwrap_or_default(), "subType": format!("{sub:02x}") } })
            }
            "MinKey" | "MaxKey" => {
                if self.peek_after_ws() == Some(b'(') {
                    self.args()?;
                }
                if word == "MinKey" { json!({ "$minKey": 1 }) } else { json!({ "$maxKey": 1 }) }
            }
            other => return self.err(&format!("unknown value '{other}'")),
        })
    }

    fn peek_after_ws(&mut self) -> Option<u8> {
        self.ws();
        self.peek()
    }

    fn object(&mut self) -> Result<Value> {
        self.i += 1;
        let mut m = Map::new();
        loop {
            self.ws();
            if self.eat(b'}') {
                return Ok(Value::Object(m));
            }
            let key = match self.peek() {
                Some(b'"' | b'\'') => self.string()?,
                Some(c) if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$') => self.ident(),
                _ => return self.err("expected a key"),
            };
            self.expect(b':')?;
            let v = self.value()?;
            m.insert(key, v);
            self.ws();
            if self.eat(b'}') {
                return Ok(Value::Object(m));
            }
            self.expect(b',')?;
        }
    }

    fn array(&mut self) -> Result<Value> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            if self.eat(b']') {
                return Ok(Value::Array(out));
            }
            out.push(self.value()?);
            if self.eat(b']') {
                return Ok(Value::Array(out));
            }
            self.expect(b',')?;
        }
    }
}

fn chrono_now_iso() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    bson::DateTime::from_millis(ms).try_to_rfc3339_string().unwrap_or_default()
}

/// Parses Extended JSON or mongo-shell syntax into (canonical) Extended JSON.
pub fn parse_relaxed(text: &str) -> Result<Value> {
    let mut p = P { s: text.as_bytes(), src: text, i: 0 };
    p.ws();
    if p.i >= text.len() {
        return Ok(Value::Object(Map::new()));
    }
    let v = p.value()?;
    p.ws();
    if p.i < text.len() {
        return p.err("unexpected text after the value");
    }
    Ok(v)
}

/// A document from relaxed JSON text (empty text = `{}`).
pub fn parse_document(text: &str) -> Result<Document> {
    match parse_relaxed(text)? {
        Value::Object(m) => Document::try_from(m).map_err(|e| invalid(format!("invalid Extended JSON: {e}"))),
        _ => Err(invalid("expected a JSON object")),
    }
}

/// An array of documents (pipelines, inserts). A single object is accepted as a one-element array.
pub fn parse_documents(text: &str) -> Result<Vec<Document>> {
    match parse_relaxed(text)? {
        Value::Array(a) => a
            .into_iter()
            .map(|v| match v {
                Value::Object(m) => Document::try_from(m).map_err(|e| invalid(format!("invalid Extended JSON: {e}"))),
                _ => Err(invalid("every element must be an object")),
            })
            .collect(),
        Value::Object(m) => Ok(vec![Document::try_from(m).map_err(|e| invalid(format!("invalid Extended JSON: {e}")))?]),
        _ => Err(invalid("expected an array of objects")),
    }
}

pub fn doc_to_json(d: Document) -> Value {
    Bson::Document(d).into_relaxed_extjson()
}

/// `$out` / `$merge` stages write.
pub fn pipeline_writes(pipeline: &[Document]) -> Option<&'static str> {
    pipeline.iter().find_map(|st| {
        if st.contains_key("$out") {
            Some("$out")
        } else if st.contains_key("$merge") {
            Some("$merge")
        } else {
            None
        }
    })
}

const READ_COMMANDS: &[&str] = &[
    "find", "aggregate", "count", "distinct", "listCollections", "listIndexes", "listDatabases", "dbStats", "collStats", "serverStatus",
    "hello", "isMaster", "ismaster", "buildInfo", "buildinfo", "ping", "explain", "connectionStatus", "hostInfo", "getLog", "currentOp", "top",
    "dataSize", "getParameter", "getCmdLineOpts", "replSetGetStatus", "replSetGetConfig", "usersInfo", "rolesInfo", "getMore", "killCursors",
    "listCommands", "whatsmyuri", "validate", "lockInfo", "features", "profile", "dbHash", "mapReduce",
];

/// Whether a `runCommand` document only reads.
pub fn command_is_read(cmd: &Document) -> std::result::Result<(), String> {
    let Some(name) = cmd.keys().next() else { return Err("empty command".into()) };
    if !READ_COMMANDS.contains(&name.as_str()) {
        return Err(format!("`{name}` is not a known read-only command"));
    }
    match name.as_str() {
        "aggregate" => {
            if let Ok(p) = cmd.get_array("pipeline") {
                let stages: Vec<Document> = p.iter().filter_map(|b| b.as_document().cloned()).collect();
                if let Some(op) = pipeline_writes(&stages) {
                    return Err(format!("the pipeline writes with {op}"));
                }
            }
            Ok(())
        }
        "mapReduce" => match cmd.get("out") {
            Some(Bson::Document(o)) if o.contains_key("inline") => Ok(()),
            _ => Err("mapReduce writes its output to a collection".into()),
        },
        // profile with level > 0 changes settings; level -1 only reads.
        "profile" => match cmd.get("profile") {
            Some(Bson::Int32(-1)) | Some(Bson::Int64(-1)) => Ok(()),
            _ => Err("profile changes the profiler level".into()),
        },
        "explain" => match cmd.get_document("explain") {
            Ok(inner) => command_is_read(inner),
            Err(_) => Ok(()),
        },
        _ => Ok(()),
    }
}

fn read_only_err(what: &str) -> DbError {
    DbError::ReadOnly(format!("Read-only session: {what}. Turn off read-only for this connection to run it."))
}

fn m_err(e: mongodb::error::Error) -> DbError {
    DbError::Query(e.to_string())
}

// ---------- API types ----------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FindRequest {
    pub database: String,
    pub collection: String,
    /// Relaxed JSON (empty = `{}`).
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub projection: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub skip: Option<u64>,
    /// Default 50, max 10 000.
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AggregateRequest {
    pub database: String,
    pub collection: String,
    /// A JSON array of stages.
    pub pipeline: String,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Documents {
    pub docs: Vec<Value>,
    /// More documents matched than were returned.
    pub more: bool,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub name: String,
    pub size_on_disk: u64,
    pub empty: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionInfo {
    pub name: String,
    /// `collection`, `view` or `timeseries`.
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MongoWrite {
    /// `documents`: one object or an array.
    Insert { database: String, collection: String, documents: String },
    Update {
        database: String,
        collection: String,
        filter: String,
        /// An update document (`{$set: …}`) or a pipeline array.
        update: String,
        #[serde(default)]
        many: bool,
        #[serde(default)]
        upsert: bool,
    },
    Replace { database: String, collection: String, filter: String, replacement: String },
    Delete {
        database: String,
        collection: String,
        filter: String,
        #[serde(default)]
        many: bool,
    },
}

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 10_000;
const MAX_TIME: Duration = Duration::from_secs(60);

// ---------- connection ----------

pub struct MongoConn {
    client: mongodb::Client,
    pub(crate) default_db: Option<String>,
    pub(crate) version: String,
}

impl MongoConn {
    pub async fn connect(rc: &ResolvedConnection, host: &str, port: u16) -> Result<Self> {
        let timeout = rc.connect_timeout();
        let tunneled = host != rc.host;
        let uri = match &rc.url {
            Some(u) => u.clone(),
            None => {
                let mut u = format!("mongodb://{}:{}/{}", rc.host, rc.port, rc.database.as_deref().unwrap_or(""));
                let params: Vec<String> = rc
                    .options
                    .iter()
                    .filter(|(k, v)| !matches!(k.as_str(), "connectTimeout" | "applicationName" | "statementTimeout" | "authSource") && !v.is_empty())
                    .map(|(k, v)| format!("{}={}", k, percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC)))
                    .collect();
                if !params.is_empty() {
                    u.push('?');
                    u.push_str(&params.join("&"));
                }
                u
            }
        };
        if tunneled && uri.starts_with("mongodb+srv://") {
            return Err(DbError::Unsupported("mongodb+srv:// through an SSH tunnel — use mongodb://host:port of one member".into()));
        }
        let mut opts = tokio::time::timeout(timeout, ClientOptions::parse(&uri))
            .await
            .map_err(|_| DbError::Timeout(format!("{timeout:?} resolving {}", rc.display_target())))?
            .map_err(|e| DbError::Connect(e.to_string()))?;
        if tunneled {
            opts.hosts = vec![ServerAddress::Tcp { host: host.to_string(), port: Some(port) }];
            opts.direct_connection = Some(true);
            opts.repl_set_name = None;
        }
        if let Some(user) = &rc.user {
            let mut cred = opts.credential.take().unwrap_or_default();
            cred.username = Some(user.clone());
            if rc.password.is_some() {
                cred.password = rc.password.clone();
            }
            if let Some(src) = rc.option("authSource") {
                cred.source = Some(src.to_string());
            }
            opts.credential = Some(cred);
        } else if let (Some(src), Some(cred)) = (rc.option("authSource"), opts.credential.as_mut()) {
            cred.source = Some(src.to_string());
        }
        if rc.tls_explicit {
            opts.tls = Some(match rc.tls {
                TlsMode::Disable => Tls::Disabled,
                TlsMode::Prefer | TlsMode::Require => Tls::Enabled(TlsOptions::builder().allow_invalid_certificates(true).build()),
                TlsMode::VerifyFull => {
                    if tunneled {
                        return Err(DbError::Unsupported("verify-full TLS through an SSH tunnel (the certificate names the real host) — use require".into()));
                    }
                    Tls::Enabled(TlsOptions::builder().ca_file_path(rc.ca_cert.clone()).build())
                }
            });
        }
        opts.connect_timeout = Some(timeout);
        opts.server_selection_timeout = Some(timeout);
        opts.app_name = Some(rc.option("applicationName").unwrap_or("devall").to_string());
        let default_db = rc.database.clone().or_else(|| opts.default_database.clone());
        let client = mongodb::Client::with_options(opts).map_err(|e| DbError::Connect(e.to_string()))?;
        let info = client.database("admin").run_command(doc! { "buildInfo": 1 }).await.map_err(|e| DbError::Connect(e.to_string()))?;
        let version = format!("MongoDB {}", info.get_str("version").unwrap_or("?"));
        Ok(Self { client, default_db, version })
    }

    pub async fn databases(&self) -> Result<Vec<DatabaseInfo>> {
        match self.client.list_databases().await {
            Ok(list) => Ok(list.into_iter().map(|d| DatabaseInfo { name: d.name, size_on_disk: d.size_on_disk, empty: d.empty }).collect()),
            // Users limited to one database may not list databases: show the default one.
            Err(e) => match &self.default_db {
                Some(db) => Ok(vec![DatabaseInfo { name: db.clone(), size_on_disk: 0, empty: false }]),
                None => Err(m_err(e)),
            },
        }
    }

    pub async fn collections(&self, db: &str) -> Result<Vec<CollectionInfo>> {
        let cursor = self.client.database(db).list_collections().await.map_err(m_err)?;
        let specs: Vec<mongodb::results::CollectionSpecification> = cursor.try_collect().await.map_err(m_err)?;
        let mut out: Vec<CollectionInfo> = specs
            .into_iter()
            .map(|s| CollectionInfo {
                name: s.name,
                kind: match s.collection_type {
                    mongodb::results::CollectionType::View => "view",
                    mongodb::results::CollectionType::Timeseries => "timeseries",
                    _ => "collection",
                }
                .into(),
            })
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        Ok(out)
    }

    pub async fn find(&self, req: &FindRequest) -> Result<Documents> {
        let started = Instant::now();
        let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let coll = self.client.database(&req.database).collection::<Document>(&req.collection);
        let filter = parse_document(req.filter.as_deref().unwrap_or(""))?;
        let mut action = coll.find(filter).limit(limit + 1).max_time(MAX_TIME);
        if let Some(p) = req.projection.as_deref().filter(|p| !p.trim().is_empty()) {
            action = action.projection(parse_document(p)?);
        }
        if let Some(s) = req.sort.as_deref().filter(|s| !s.trim().is_empty()) {
            action = action.sort(parse_document(s)?);
        }
        if let Some(skip) = req.skip.filter(|s| *s > 0) {
            action = action.skip(skip);
        }
        let mut cursor = action.await.map_err(m_err)?;
        let mut docs = Vec::new();
        let mut more = false;
        while let Some(d) = cursor.try_next().await.map_err(m_err)? {
            if docs.len() as i64 >= limit {
                more = true;
                break;
            }
            docs.push(doc_to_json(d));
        }
        Ok(Documents { docs, more, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 })
    }

    pub async fn aggregate(&self, req: &AggregateRequest, read_only: bool) -> Result<Documents> {
        let started = Instant::now();
        let pipeline = parse_documents(&req.pipeline)?;
        if read_only && let Some(op) = pipeline_writes(&pipeline) {
            return Err(read_only_err(&format!("the pipeline was not run because its {op} stage writes to a collection")));
        }
        let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let coll = self.client.database(&req.database).collection::<Document>(&req.collection);
        let mut cursor = coll.aggregate(pipeline).max_time(MAX_TIME).allow_disk_use(true).await.map_err(m_err)?;
        let mut docs = Vec::new();
        let mut more = false;
        while let Some(d) = cursor.try_next().await.map_err(m_err)? {
            if docs.len() as i64 >= limit {
                more = true;
                break;
            }
            docs.push(doc_to_json(d));
        }
        Ok(Documents { docs, more, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 })
    }

    pub async fn count(&self, db: &str, coll: &str, filter: Option<&str>) -> Result<u64> {
        let filter = parse_document(filter.unwrap_or(""))?;
        let c = self.client.database(db).collection::<Document>(coll);
        if filter.is_empty() {
            return c.estimated_document_count().await.map_err(m_err);
        }
        c.count_documents(filter).max_time(MAX_TIME).await.map_err(m_err)
    }

    pub async fn indexes(&self, db: &str, coll: &str) -> Result<Vec<Value>> {
        let res = self.client.database(db).run_command(doc! { "listIndexes": coll }).await.map_err(m_err)?;
        let batch = res.get_document("cursor").and_then(|c| c.get_array("firstBatch")).map(|a| a.to_vec()).unwrap_or_default();
        Ok(batch.into_iter().map(Bson::into_relaxed_extjson).collect())
    }

    /// `queryPlanner` (or `executionStats` with `analyze`) explain of a find or an aggregate.
    pub async fn explain_find(&self, req: &FindRequest, analyze: bool) -> Result<Value> {
        let mut find = doc! { "find": &req.collection, "filter": parse_document(req.filter.as_deref().unwrap_or(""))? };
        if let Some(p) = req.projection.as_deref().filter(|p| !p.trim().is_empty()) {
            find.insert("projection", parse_document(p)?);
        }
        if let Some(s) = req.sort.as_deref().filter(|s| !s.trim().is_empty()) {
            find.insert("sort", parse_document(s)?);
        }
        if let Some(skip) = req.skip.filter(|s| *s > 0) {
            find.insert("skip", skip as i64);
        }
        find.insert("limit", req.limit.unwrap_or(DEFAULT_LIMIT));
        self.explain(&req.database, find, analyze).await
    }

    pub async fn explain_aggregate(&self, req: &AggregateRequest, analyze: bool) -> Result<Value> {
        let pipeline: Vec<Bson> = parse_documents(&req.pipeline)?.into_iter().map(Bson::Document).collect();
        self.explain(&req.database, doc! { "aggregate": &req.collection, "pipeline": pipeline, "cursor": {} }, analyze).await
    }

    async fn explain(&self, db: &str, inner: Document, analyze: bool) -> Result<Value> {
        let verbosity = if analyze { "executionStats" } else { "queryPlanner" };
        let res = self.client.database(db).run_command(doc! { "explain": inner, "verbosity": verbosity }).await.map_err(m_err)?;
        Ok(doc_to_json(res))
    }

    pub async fn write(&self, op: &MongoWrite) -> Result<Value> {
        let coll = |db: &str, c: &str| self.client.database(db).collection::<Document>(c);
        Ok(match op {
            MongoWrite::Insert { database, collection, documents } => {
                let docs = parse_documents(documents)?;
                if docs.is_empty() {
                    return Err(invalid("nothing to insert"));
                }
                let r = coll(database, collection).insert_many(docs).await.map_err(m_err)?;
                let mut ids: Vec<(usize, Bson)> = r.inserted_ids.into_iter().collect();
                ids.sort_by_key(|(i, _)| *i);
                json!({ "insertedCount": ids.len(), "insertedIds": ids.into_iter().map(|(_, id)| id.into_relaxed_extjson()).collect::<Vec<_>>() })
            }
            MongoWrite::Update { database, collection, filter, update, many, upsert } => {
                let filter = parse_document(filter)?;
                let update: mongodb::options::UpdateModifications = match parse_relaxed(update)? {
                    Value::Array(_) => parse_documents(update)?.into(),
                    _ => parse_document(update)?.into(),
                };
                let c = coll(database, collection);
                let r = if *many { c.update_many(filter, update).upsert(*upsert).await } else { c.update_one(filter, update).upsert(*upsert).await }.map_err(m_err)?;
                json!({ "matchedCount": r.matched_count, "modifiedCount": r.modified_count, "upsertedId": r.upserted_id.map(Bson::into_relaxed_extjson) })
            }
            MongoWrite::Replace { database, collection, filter, replacement } => {
                let r = coll(database, collection).replace_one(parse_document(filter)?, parse_document(replacement)?).await.map_err(m_err)?;
                json!({ "matchedCount": r.matched_count, "modifiedCount": r.modified_count })
            }
            MongoWrite::Delete { database, collection, filter, many } => {
                let filter = parse_document(filter)?;
                let c = coll(database, collection);
                let r = if *many { c.delete_many(filter).await } else { c.delete_one(filter).await }.map_err(m_err)?;
                json!({ "deletedCount": r.deleted_count })
            }
        })
    }

    /// `db.runCommand(…)`. Read-only sessions only run known read commands.
    pub async fn command(&self, db: &str, text: &str, read_only: bool) -> Result<Value> {
        let cmd = parse_document(text)?;
        if read_only && let Err(why) = command_is_read(&cmd) {
            return Err(read_only_err(&format!("the command was not run because {why}")));
        }
        let res = self.client.database(db).run_command(cmd).await.map_err(m_err)?;
        Ok(doc_to_json(res))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_syntax() {
        let v = parse_relaxed(
            r#"{
              // comment
              _id: ObjectId("65f0a1b2c3d4e5f6a7b8c9d0"),
              'name': 'O\'Brien',
              age: {$gte: 21, $lt: NumberLong(99)},
              big: 12345678901,
              at: ISODate("2024-05-01"),
              when: new Date("2024-05-01T10:00:00"),
              price: NumberDecimal("9.99"),
              tags: ['a', "b",],
              re: /^ad\/a/i,
              /* block */ ok: true, none: null,
            }"#,
        )
        .unwrap();
        assert_eq!(v["_id"], json!({"$oid": "65f0a1b2c3d4e5f6a7b8c9d0"}));
        assert_eq!(v["name"], json!("O'Brien"));
        assert_eq!(v["age"]["$lt"], json!({"$numberLong": "99"}));
        assert_eq!(v["big"], json!({"$numberLong": "12345678901"}));
        assert_eq!(v["at"], json!({"$date": "2024-05-01T00:00:00Z"}));
        assert_eq!(v["when"], json!({"$date": "2024-05-01T10:00:00Z"}));
        assert_eq!(v["tags"], json!(["a", "b"]));
        assert_eq!(v["re"], json!({"$regularExpression": {"pattern": "^ad/a", "options": "i"}}));
        let d = parse_document(&v.to_string()).unwrap();
        assert!(matches!(d.get("_id"), Some(Bson::ObjectId(_))));
        assert!(matches!(d.get("at"), Some(Bson::DateTime(_))));
        assert!(matches!(d.get("price"), Some(Bson::Decimal128(_))));
        assert!(matches!(d.get("re"), Some(Bson::RegularExpression(_))));
        assert!(matches!(d.get_document("age").unwrap().get("$lt"), Some(Bson::Int64(99))));
        let back = doc_to_json(d);
        assert_eq!(back["_id"], json!({"$oid": "65f0a1b2c3d4e5f6a7b8c9d0"}));
        assert_eq!(parse_document("").unwrap(), Document::new());
        assert!(parse_document("{a: }").is_err());
        assert!(parse_document("[1]").is_err());
        assert!(parse_relaxed("{a: 1} trailing").is_err());
        assert_eq!(parse_relaxed(r#"{"s": "é😀"}"#).unwrap()["s"], json!("é😀"));
    }

    #[test]
    fn pipelines_and_commands() {
        let p = parse_documents("[{$match: {a: 1}}, {$out: 'copy'}]").unwrap();
        assert_eq!(pipeline_writes(&p), Some("$out"));
        assert_eq!(pipeline_writes(&parse_documents("[{$match: {}}]").unwrap()), None);
        assert_eq!(parse_documents("{a: 1}").unwrap().len(), 1);
        assert!(command_is_read(&parse_document("{ping: 1}").unwrap()).is_ok());
        assert!(command_is_read(&parse_document("{dropDatabase: 1}").unwrap()).is_err());
        assert!(command_is_read(&parse_document("{aggregate: 'c', pipeline: [{$merge: 'x'}], cursor: {}}").unwrap()).is_err());
        assert!(command_is_read(&parse_document("{explain: {find: 'c'}}").unwrap()).is_ok());
        assert!(command_is_read(&parse_document("{explain: {delete: 'c', deletes: []}}").unwrap()).is_err());
        assert!(command_is_read(&parse_document("{mapReduce: 'c', out: {inline: 1}}").unwrap()).is_ok());
        let w: MongoWrite = serde_json::from_value(json!({"type": "delete", "database": "d", "collection": "c", "filter": "{}", "many": true})).unwrap();
        assert!(matches!(w, MongoWrite::Delete { many: true, .. }));
    }
}
