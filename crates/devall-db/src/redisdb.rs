//! Redis / Valkey: SCAN-based key browser (type + TTL, paged), type-aware value reads (string,
//! hash, list, set, zset, stream, RedisJSON), edits, a `redis-cli`-style console with a command
//! classification table (writes are blocked in read-only sessions), INFO / SLOWLOG / MEMORY USAGE,
//! and pub/sub subscriptions streamed over a callback.
//!
//! Single node first-class; Sentinel is supported by resolving the master
//! (`options.sentinelMaster`); cluster mode is detected and works best-effort on the node you
//! connect to (no MOVED redirects).

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures_util::StreamExt;
use redis::aio::MultiplexedConnection;
use redis::{AsyncConnectionConfig, ConnectionAddr, IntoConnectionInfo, RedisConnectionInfo, Value as RValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::AbortHandle;

use crate::error::{DbError, Result, invalid};
use crate::model::TlsMode;
use crate::resolve::ResolvedConnection;

// ---------- command line parsing ----------

/// Splits a `redis-cli` command line into arguments: whitespace-separated, `"double quoted"`
/// with `\n \r \t \b \a \\ \" \xHH` escapes, or `'single quoted'` with `\'`.
pub fn parse_command_line(line: &str) -> Result<Vec<Vec<u8>>> {
    let b = line.as_bytes();
    let mut args = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let mut cur = Vec::new();
        match b[i] {
            b'"' => {
                i += 1;
                loop {
                    let Some(&c) = b.get(i) else { return Err(invalid("unbalanced double quote")) };
                    match c {
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\\' => {
                            let Some(&e) = b.get(i + 1) else { return Err(invalid("dangling backslash")) };
                            if e == b'x'
                                && let Some(h) = line.get(i + 2..i + 4)
                                && let Ok(v) = u8::from_str_radix(h, 16)
                            {
                                cur.push(v);
                                i += 4;
                                continue;
                            }
                            cur.push(match e {
                                b'n' => b'\n',
                                b'r' => b'\r',
                                b't' => b'\t',
                                b'b' => 8,
                                b'a' => 7,
                                other => other,
                            });
                            i += 2;
                        }
                        other => {
                            cur.push(other);
                            i += 1;
                        }
                    }
                }
            }
            b'\'' => {
                i += 1;
                loop {
                    let Some(&c) = b.get(i) else { return Err(invalid("unbalanced single quote")) };
                    if c == b'\\' && b.get(i + 1) == Some(&b'\'') {
                        cur.push(b'\'');
                        i += 2;
                    } else if c == b'\'' {
                        i += 1;
                        break;
                    } else {
                        cur.push(c);
                        i += 1;
                    }
                }
            }
            _ => {
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    cur.push(b[i]);
                    i += 1;
                }
            }
        }
        if i < b.len() && !b[i].is_ascii_whitespace() {
            return Err(invalid("closing quote must be followed by a space"));
        }
        args.push(cur);
    }
    Ok(args)
}

// ---------- classification ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandClass {
    Read,
    Write,
    /// Server administration (FLUSHALL, CONFIG SET, SHUTDOWN, …) — blocked in read-only mode.
    Admin,
    /// SUBSCRIBE / MONITOR — need a dedicated connection (use the Pub/Sub view).
    Streaming,
    /// Would block the shared connection (BLPOP, XREAD BLOCK, WAIT, …).
    Blocking,
}

const READ_COMMANDS: &[&str] = &[
    "GET", "MGET", "GETRANGE", "SUBSTR", "STRLEN", "GETBIT", "BITCOUNT", "BITPOS", "BITFIELD_RO", "LCS", "EXISTS", "TYPE", "TTL", "PTTL",
    "EXPIRETIME", "PEXPIRETIME", "KEYS", "SCAN", "RANDOMKEY", "DUMP", "TOUCH", "HGET", "HMGET", "HGETALL", "HKEYS", "HVALS", "HLEN", "HEXISTS",
    "HSTRLEN", "HSCAN", "HRANDFIELD", "HTTL", "HPTTL", "HEXPIRETIME", "HPEXPIRETIME", "LRANGE", "LLEN", "LINDEX", "LPOS", "SMEMBERS",
    "SISMEMBER", "SMISMEMBER", "SCARD", "SRANDMEMBER", "SSCAN", "SINTER", "SUNION", "SDIFF", "SINTERCARD", "ZRANGE", "ZRANGEBYSCORE",
    "ZREVRANGE", "ZREVRANGEBYSCORE", "ZRANGEBYLEX", "ZREVRANGEBYLEX", "ZCARD", "ZCOUNT", "ZLEXCOUNT", "ZSCORE", "ZMSCORE", "ZRANK", "ZREVRANK",
    "ZSCAN", "ZRANDMEMBER", "ZINTER", "ZUNION", "ZDIFF", "ZINTERCARD", "XRANGE", "XREVRANGE", "XLEN", "XREAD", "XPENDING", "GEOPOS",
    "GEODIST", "GEOHASH", "GEORADIUS_RO", "GEORADIUSBYMEMBER_RO", "GEOSEARCH", "PFCOUNT", "SORT_RO", "DBSIZE", "INFO", "PING", "ECHO", "TIME",
    "LASTSAVE", "ROLE", "LOLWUT", "SELECT", "AUTH", "HELLO", "READONLY", "READWRITE", "MULTI", "EXEC", "DISCARD", "WATCH", "UNWATCH", "QUIT",
    "RESET", "EVAL_RO", "EVALSHA_RO", "FCALL_RO", "JSON.GET", "JSON.MGET", "JSON.TYPE", "JSON.STRLEN", "JSON.ARRLEN", "JSON.ARRINDEX",
    "JSON.OBJKEYS", "JSON.OBJLEN", "JSON.RESP", "JSON.DEBUG", "FT.SEARCH", "FT.AGGREGATE", "FT.INFO", "FT._LIST", "FT.EXPLAIN",
    "FT.PROFILE", "FT.TAGVALS", "FT.SPELLCHECK", "TS.GET", "TS.MGET", "TS.RANGE", "TS.REVRANGE", "TS.MRANGE", "TS.MREVRANGE", "TS.INFO",
    "TS.QUERYINDEX", "BF.EXISTS", "BF.MEXISTS", "BF.INFO", "BF.CARD", "CF.EXISTS", "CF.MEXISTS", "CF.COUNT", "CF.INFO", "TOPK.QUERY",
    "TOPK.LIST", "TOPK.INFO", "TOPK.COUNT", "CMS.QUERY", "CMS.INFO", "TDIGEST.INFO", "TDIGEST.QUANTILE", "TDIGEST.MIN", "TDIGEST.MAX",
    "VCARD", "VDIM", "VEMB", "VGETATTR", "VINFO", "VLINKS", "VRANDMEMBER", "VSIM", "VISMEMBER",
];

const ADMIN_COMMANDS: &[&str] = &[
    "FLUSHALL", "FLUSHDB", "SHUTDOWN", "DEBUG", "SAVE", "BGSAVE", "BGREWRITEAOF", "REPLICAOF", "SLAVEOF", "MIGRATE", "FAILOVER", "SYNC",
    "PSYNC", "SWAPDB", "MOVE", "RESTORE", "RESTORE-ASKING",
];

const BLOCKING_COMMANDS: &[&str] =
    &["BLPOP", "BRPOP", "BRPOPLPUSH", "BLMOVE", "BLMPOP", "BZPOPMIN", "BZPOPMAX", "BZMPOP", "WAIT", "WAITAOF"];

/// Container commands whose subcommand decides (`CONFIG GET` reads, `CONFIG SET` administers).
fn sub_class(cmd: &str, sub: &str) -> Option<CommandClass> {
    use CommandClass::*;
    let read_subs: &[&str] = match cmd {
        "CONFIG" => &["GET", "HELP"],
        "CLIENT" => &["LIST", "INFO", "GETNAME", "ID", "TRACKINGINFO", "GETREDIR", "HELP", "NO-EVICT", "NO-TOUCH", "SETNAME", "SETINFO", "REPLY"],
        "SLOWLOG" => &["GET", "LEN", "HELP"],
        "CLUSTER" => &["INFO", "NODES", "SLOTS", "SHARDS", "KEYSLOT", "COUNTKEYSINSLOT", "GETKEYSINSLOT", "MYID", "MYSHARDID", "LINKS", "HELP", "REPLICAS", "SLAVES"],
        "FUNCTION" => &["LIST", "DUMP", "STATS", "HELP"],
        "SCRIPT" => &["EXISTS", "HELP"],
        "ACL" => &["WHOAMI", "LIST", "USERS", "GETUSER", "CAT", "LOG", "GENPASS", "DRYRUN", "HELP"],
        "MEMORY" => &["USAGE", "STATS", "DOCTOR", "MALLOC-STATS", "HELP"],
        "OBJECT" => &["ENCODING", "FREQ", "IDLETIME", "REFCOUNT", "HELP"],
        "XINFO" => &["STREAM", "GROUPS", "CONSUMERS", "HELP"],
        "COMMAND" => &["COUNT", "DOCS", "GETKEYS", "GETKEYSANDFLAGS", "INFO", "LIST", "HELP"],
        "LATENCY" => &["LATEST", "HISTORY", "GRAPH", "DOCTOR", "HISTOGRAM", "HELP"],
        "MODULE" => &["LIST", "HELP"],
        "PUBSUB" => &["CHANNELS", "NUMSUB", "NUMPAT", "SHARDCHANNELS", "SHARDNUMSUB", "HELP"],
        _ => return None,
    };
    if sub.is_empty() || read_subs.contains(&sub) {
        return Some(Read);
    }
    Some(match cmd {
        "CONFIG" | "CLUSTER" | "MODULE" | "ACL" | "CLIENT" | "LATENCY" | "SLOWLOG" => Admin,
        _ => Write,
    })
}

/// Classifies a command by name (and subcommand / options when they matter).
pub fn classify_command(args: &[Vec<u8>]) -> CommandClass {
    use CommandClass::*;
    let Some(name) = args.first() else { return Read };
    let name = String::from_utf8_lossy(name).to_ascii_uppercase();
    let sub = args.get(1).map(|s| String::from_utf8_lossy(s).to_ascii_uppercase()).unwrap_or_default();
    if let Some(c) = sub_class(&name, &sub) {
        return c;
    }
    match name.as_str() {
        "SUBSCRIBE" | "PSUBSCRIBE" | "SSUBSCRIBE" | "UNSUBSCRIBE" | "PUNSUBSCRIBE" | "SUNSUBSCRIBE" | "MONITOR" => Streaming,
        "XREAD" | "XREADGROUP" if args.iter().skip(1).any(|a| a.eq_ignore_ascii_case(b"BLOCK")) => Blocking,
        // XREADGROUP updates the group's pending list: a write.
        "XREADGROUP" => Write,
        n if BLOCKING_COMMANDS.contains(&n) => Blocking,
        n if ADMIN_COMMANDS.contains(&n) => Admin,
        n if READ_COMMANDS.contains(&n) => Read,
        _ => Write,
    }
}

// ---------- values ----------

/// UTF-8 text as-is; other bytes `redis-cli`-escaped (`\xNN`).
pub fn display_bytes(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let mut out = String::with_capacity(b.len() * 2);
            for &c in b {
                match c {
                    b'\\' => out.push_str("\\\\"),
                    b'\n' => out.push_str("\\n"),
                    b'\r' => out.push_str("\\r"),
                    b'\t' => out.push_str("\\t"),
                    0x20..=0x7e => out.push(c as char),
                    _ => out.push_str(&format!("\\x{c:02x}")),
                }
            }
            out
        }
    }
}

fn is_binary(b: &[u8]) -> bool {
    std::str::from_utf8(b).is_err()
}

/// A reply as JSON (bulk strings as text; nested arrays/maps preserved).
pub fn reply_to_json(v: &RValue) -> Value {
    match v {
        RValue::Nil => Value::Null,
        RValue::Int(i) => json!(i),
        RValue::BulkString(b) => Value::String(display_bytes(b)),
        RValue::Array(a) | RValue::Set(a) => Value::Array(a.iter().map(reply_to_json).collect()),
        RValue::SimpleString(s) => Value::String(s.clone()),
        RValue::Okay => Value::String("OK".into()),
        RValue::Map(m) => {
            let all_str = m.iter().all(|(k, _)| matches!(k, RValue::BulkString(_) | RValue::SimpleString(_)));
            if all_str {
                Value::Object(m.iter().map(|(k, v)| (reply_to_json(k).as_str().unwrap_or_default().to_string(), reply_to_json(v))).collect())
            } else {
                Value::Array(m.iter().map(|(k, v)| json!([reply_to_json(k), reply_to_json(v)])).collect())
            }
        }
        RValue::Attribute { data, .. } => reply_to_json(data),
        RValue::Double(d) => json!(d),
        RValue::Boolean(b) => json!(b),
        RValue::VerbatimString { text, .. } => Value::String(text.clone()),
        RValue::BigNumber(n) => Value::String(String::from_utf8_lossy(n).into_owned()),
        RValue::Push { data, .. } => Value::Array(data.iter().map(reply_to_json).collect()),
        RValue::ServerError(e) => json!({ "error": e.to_string() }),
        _ => Value::Null,
    }
}

/// `redis-cli`-style rendering: `"text"`, `(integer) 5`, `(nil)`, numbered nested lists.
pub fn format_reply(v: &RValue) -> String {
    let mut out = String::new();
    fmt_reply(v, 0, &mut out);
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn quote(b: &[u8]) -> String {
    let mut s = String::from("\"");
    for &c in b {
        match c {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7e => s.push(c as char),
            _ if c >= 0x80 && std::str::from_utf8(b).is_ok() => {
                // Valid UTF-8: keep non-ASCII characters readable.
                s.clear();
                s.push('"');
                s.push_str(&String::from_utf8_lossy(b).replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"));
                s.push('"');
                return s;
            }
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s.push('"');
    s
}

fn fmt_reply(v: &RValue, indent: usize, out: &mut String) {
    let items: Option<Vec<&RValue>> = match v {
        RValue::Array(a) | RValue::Set(a) => Some(a.iter().collect()),
        RValue::Push { data, .. } => Some(data.iter().collect()),
        RValue::Map(m) => Some(m.iter().flat_map(|(k, v)| [k, v]).collect()),
        _ => None,
    };
    match (v, items) {
        (_, Some(items)) => {
            if items.is_empty() {
                out.push_str("(empty array)\n");
                return;
            }
            let width = items.len().to_string().len();
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(&" ".repeat(indent));
                }
                let prefix = format!("{:>width$}) ", i + 1);
                out.push_str(&prefix);
                fmt_reply(it, indent + prefix.len(), out);
            }
        }
        (RValue::Nil, _) => out.push_str("(nil)\n"),
        (RValue::Int(i), _) => out.push_str(&format!("(integer) {i}\n")),
        (RValue::BulkString(b), _) => {
            out.push_str(&quote(b));
            out.push('\n');
        }
        (RValue::SimpleString(s), _) => {
            out.push_str(s);
            out.push('\n');
        }
        (RValue::Okay, _) => out.push_str("OK\n"),
        (RValue::Double(d), _) => out.push_str(&format!("(double) {d}\n")),
        (RValue::Boolean(b), _) => out.push_str(&format!("({})\n", if *b { "true" } else { "false" })),
        (RValue::VerbatimString { text, .. }, _) => {
            out.push_str(text);
            out.push('\n');
        }
        (RValue::BigNumber(n), _) => out.push_str(&format!("(big number) {}\n", String::from_utf8_lossy(n))),
        (RValue::Attribute { data, .. }, _) => fmt_reply(data, indent, out),
        (RValue::ServerError(e), _) => out.push_str(&format!("(error) {e}\n")),
        _ => out.push_str("(unknown)\n"),
    }
}

// ---------- API types ----------

/// A key passed from the UI: text, or `{ "b64": … }` for keys that aren't valid UTF-8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RedisKey {
    Text(String),
    Raw { b64: String },
}

impl RedisKey {
    pub fn bytes(&self) -> Result<Vec<u8>> {
        match self {
            RedisKey::Text(s) => Ok(s.clone().into_bytes()),
            RedisKey::Raw { b64 } => base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| invalid(format!("bad key encoding: {e}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    /// Display form (`\xNN`-escaped when binary).
    pub key: String,
    /// Base64 of the raw key when it isn't valid UTF-8 (pass `{ b64 }` back).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_b64: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    /// Milliseconds to expiry; -1 = no expiry; -2 = key vanished.
    pub ttl_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanPage {
    /// Pass back to continue; `"0"` when the iteration is complete.
    pub cursor: String,
    pub keys: Vec<KeyInfo>,
    pub done: bool,
    /// Total keys in the database (DBSIZE), for "n of total".
    pub db_size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub field: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZMember {
    pub member: String,
    pub score: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEntry {
    pub id: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RedisValue {
    /// The key does not exist (any more).
    None,
    String {
        value: String,
        /// Raw bytes (base64) when the value isn't valid UTF-8, for a hex view.
        #[serde(skip_serializing_if = "Option::is_none")]
        base64: Option<String>,
        size: u64,
        truncated: bool,
    },
    Hash { fields: Vec<Field>, len: u64, cursor: String },
    List { items: Vec<String>, len: u64, offset: u64 },
    Set { members: Vec<String>, len: u64, cursor: String },
    Zset { members: Vec<ZMember>, len: u64, offset: u64 },
    Stream { entries: Vec<StreamEntry>, len: u64, groups: Value, next: Option<String> },
    Json { value: Value },
    Unsupported { type_name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyValue {
    pub kind: String,
    pub ttl_ms: i64,
    pub value: RedisValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RedisEdit {
    SetString {
        key: RedisKey,
        value: String,
        /// New TTL in ms; `None` keeps the current TTL.
        #[serde(default)]
        ttl_ms: Option<i64>,
    },
    HashSet { key: RedisKey, field: String, value: String },
    HashDelete { key: RedisKey, field: String },
    ListSet { key: RedisKey, index: i64, value: String },
    ListPush { key: RedisKey, value: String, #[serde(default)] head: bool },
    ListRemove { key: RedisKey, value: String, #[serde(default)] count: i64 },
    SetAdd { key: RedisKey, member: String },
    SetRemove { key: RedisKey, member: String },
    ZAdd { key: RedisKey, member: String, score: f64 },
    ZRemove { key: RedisKey, member: String },
    StreamAdd { key: RedisKey, #[serde(default)] id: Option<String>, fields: Vec<(String, String)> },
    StreamDelete { key: RedisKey, id: String },
    JsonSet { key: RedisKey, #[serde(default)] path: Option<String>, value: String },
    /// `ttl_ms: None` removes the expiry.
    Expire { key: RedisKey, #[serde(default)] ttl_ms: Option<i64> },
    Rename { key: RedisKey, new_key: String },
}

impl RedisEdit {
    /// The command it runs (for previews and logs).
    pub fn command(&self) -> Result<Vec<Vec<u8>>> {
        let s = |v: &str| v.as_bytes().to_vec();
        Ok(match self {
            RedisEdit::SetString { key, value, ttl_ms } => {
                let mut a = vec![s("SET"), key.bytes()?, s(value)];
                match ttl_ms {
                    Some(t) if *t > 0 => a.extend([s("PX"), s(&t.to_string())]),
                    _ => a.push(s("KEEPTTL")),
                }
                a
            }
            RedisEdit::HashSet { key, field, value } => vec![s("HSET"), key.bytes()?, s(field), s(value)],
            RedisEdit::HashDelete { key, field } => vec![s("HDEL"), key.bytes()?, s(field)],
            RedisEdit::ListSet { key, index, value } => vec![s("LSET"), key.bytes()?, s(&index.to_string()), s(value)],
            RedisEdit::ListPush { key, value, head } => vec![s(if *head { "LPUSH" } else { "RPUSH" }), key.bytes()?, s(value)],
            RedisEdit::ListRemove { key, value, count } => vec![s("LREM"), key.bytes()?, s(&count.to_string()), s(value)],
            RedisEdit::SetAdd { key, member } => vec![s("SADD"), key.bytes()?, s(member)],
            RedisEdit::SetRemove { key, member } => vec![s("SREM"), key.bytes()?, s(member)],
            RedisEdit::ZAdd { key, member, score } => vec![s("ZADD"), key.bytes()?, s(&score.to_string()), s(member)],
            RedisEdit::ZRemove { key, member } => vec![s("ZREM"), key.bytes()?, s(member)],
            RedisEdit::StreamAdd { key, id, fields } => {
                if fields.is_empty() {
                    return Err(invalid("a stream entry needs at least one field"));
                }
                let mut a = vec![s("XADD"), key.bytes()?, s(id.as_deref().filter(|i| !i.is_empty()).unwrap_or("*"))];
                for (k, v) in fields {
                    a.push(s(k));
                    a.push(s(v));
                }
                a
            }
            RedisEdit::StreamDelete { key, id } => vec![s("XDEL"), key.bytes()?, s(id)],
            RedisEdit::JsonSet { key, path, value } => {
                serde_json::from_str::<Value>(value).map_err(|e| invalid(format!("not valid JSON: {e}")))?;
                vec![s("JSON.SET"), key.bytes()?, s(path.as_deref().unwrap_or("$")), s(value)]
            }
            RedisEdit::Expire { key, ttl_ms } => match ttl_ms {
                Some(t) if *t > 0 => vec![s("PEXPIRE"), key.bytes()?, s(&t.to_string())],
                _ => vec![s("PERSIST"), key.bytes()?],
            },
            RedisEdit::Rename { key, new_key } => vec![s("RENAMENX"), key.bytes()?, s(new_key)],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandReply {
    /// The command as typed (or built).
    pub command: String,
    /// `redis-cli`-style text.
    pub text: String,
    pub value: Value,
    pub elapsed_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoSection {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowlogEntry {
    pub id: i64,
    pub timestamp: i64,
    pub duration_us: i64,
    pub command: String,
    pub client: String,
    pub client_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PubSubMessage {
    pub channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    pub payload: String,
    pub at: i64,
}

/// Parses `INFO` output into sections.
pub fn parse_info(text: &str) -> Vec<InfoSection> {
    let mut out: Vec<InfoSection> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("# ") {
            out.push(InfoSection { name: name.trim().to_string(), entries: Vec::new() });
        } else if let Some((k, v)) = line.split_once(':') {
            if out.is_empty() {
                out.push(InfoSection { name: "Info".into(), entries: Vec::new() });
            }
            if let Some(s) = out.last_mut() {
                s.entries.push((k.to_string(), v.to_string()));
            }
        }
    }
    out
}

fn r_err(e: redis::RedisError) -> DbError {
    DbError::Query(e.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn bytes_of(v: &RValue) -> Option<&[u8]> {
    match v {
        RValue::BulkString(b) => Some(b),
        RValue::SimpleString(s) => Some(s.as_bytes()),
        RValue::VerbatimString { text, .. } => Some(text.as_bytes()),
        _ => None,
    }
}

fn text_of(v: &RValue) -> String {
    match v {
        RValue::Int(i) => i.to_string(),
        RValue::Double(d) => d.to_string(),
        RValue::Nil => String::new(),
        other => bytes_of(other).map(display_bytes).unwrap_or_default(),
    }
}

fn int_of(v: &RValue) -> i64 {
    match v {
        RValue::Int(i) => *i,
        other => text_of(other).parse().unwrap_or(0),
    }
}

fn items(v: RValue) -> Vec<RValue> {
    match v {
        RValue::Array(a) | RValue::Set(a) => a,
        RValue::Map(m) => m.into_iter().flat_map(|(k, v)| [k, v]).collect(),
        RValue::Nil => Vec::new(),
        other => vec![other],
    }
}

const MAX_STRING: u64 = 1024 * 1024;

// ---------- connection ----------

pub struct RedisConn {
    client: redis::Client,
    conn: MultiplexedConnection,
    pub(crate) version: String,
    pub(crate) warnings: Vec<String>,
}

impl RedisConn {
    pub async fn connect(rc: &ResolvedConnection, host: &str, port: u16) -> Result<Self> {
        let timeout = rc.connect_timeout();
        let tunneled = host != rc.host;
        let (mut host, mut port) = (host.to_string(), port);
        let mode = if rc.tls_explicit { rc.tls } else { TlsMode::Disable };
        // Sentinel: ask it for the current master, then connect there.
        if let Some(master) = rc.option("sentinelMaster") {
            let s_info = ConnectionAddr::Tcp(host.clone(), port).into_connection_info().map_err(r_err)?;
            let s_info = match rc.option("sentinelPassword") {
                Some(pw) => s_info.set_redis_settings(RedisConnectionInfo::default().set_password(pw)),
                None => s_info,
            };
            let sc = redis::Client::open(s_info).map_err(r_err)?;
            let cfg = AsyncConnectionConfig::new().set_connection_timeout(Some(timeout)).set_response_timeout(Some(timeout));
            let mut c = sc.get_multiplexed_async_connection_with_config(&cfg).await.map_err(|e| DbError::Connect(format!("sentinel {host}:{port}: {e}")))?;
            let addr: Option<(String, u16)> = redis::cmd("SENTINEL").arg("get-master-addr-by-name").arg(master).query_async(&mut c).await.map_err(r_err)?;
            let (h, p) = addr.ok_or_else(|| DbError::Connect(format!("sentinel knows no master named '{master}'")))?;
            if tunneled {
                return Err(DbError::Unsupported("Sentinel through an SSH tunnel (the master's address is only reachable from the server side)".into()));
            }
            host = h;
            port = p;
        }
        let mut addr = match mode {
            TlsMode::Disable | TlsMode::Prefer => ConnectionAddr::Tcp(host.clone(), port),
            m => ConnectionAddr::TcpTls { host: host.clone(), port, insecure: m != TlsMode::VerifyFull, tls_params: None },
        };
        if mode == TlsMode::VerifyFull && tunneled {
            // The tunnel endpoint is 127.0.0.1: verify the chain, not the host name.
            addr.set_danger_accept_invalid_hostnames(true);
        }
        let mut settings = RedisConnectionInfo::default();
        if let Some(u) = &rc.user {
            settings = settings.set_username(u);
        }
        if let Some(p) = &rc.password {
            settings = settings.set_password(p);
        }
        if let Some(db) = rc.database.as_deref().filter(|d| !d.is_empty()) {
            settings = settings.set_db(db.trim().parse().map_err(|_| invalid(format!("the Redis database must be a number (got '{db}')")))?);
        }
        let info = addr.into_connection_info().map_err(r_err)?.set_redis_settings(settings);
        let client = match (&rc.ca_cert, mode == TlsMode::VerifyFull) {
            (Some(ca), true) => {
                let pem = std::fs::read(ca).map_err(|e| DbError::Tls(format!("{}: {e}", ca.display())))?;
                redis::Client::build_with_tls(info, redis::TlsCertificates { client_tls: None, root_cert: Some(pem) }).map_err(|e| DbError::Tls(e.to_string()))?
            }
            _ => redis::Client::open(info).map_err(r_err)?,
        };
        let cfg = AsyncConnectionConfig::new().set_connection_timeout(Some(timeout)).set_response_timeout(Some(Duration::from_secs(60)));
        let mut conn = tokio::time::timeout(timeout * 2, client.get_multiplexed_async_connection_with_config(&cfg))
            .await
            .map_err(|_| DbError::Timeout(format!("{timeout:?} connecting to {}:{}", rc.host, rc.port)))?
            .map_err(|e| DbError::Connect(e.to_string()))?;
        let info: String = redis::cmd("INFO").arg("server").query_async(&mut conn).await.map_err(r_err)?;
        let sections = parse_info(&info);
        let get = |k: &str| sections.iter().flat_map(|s| &s.entries).find(|(key, _)| key == k).map(|(_, v)| v.clone());
        let version = match (get("valkey_version"), get("redis_version")) {
            (Some(v), _) => format!("Valkey {v}"),
            (None, Some(v)) => format!("Redis {v}"),
            _ => "Redis".into(),
        };
        let mut warnings = Vec::new();
        let cluster: String = redis::cmd("INFO").arg("cluster").query_async(&mut conn).await.unwrap_or_default();
        if cluster.contains("cluster_enabled:1") {
            warnings.push("Cluster mode: only keys on the connected node are listed, and commands for keys on other nodes fail with MOVED.".into());
        }
        Ok(Self { client, conn, version, warnings })
    }

    fn c(&self) -> MultiplexedConnection {
        self.conn.clone()
    }

    async fn raw(&self, args: &[Vec<u8>]) -> Result<RValue> {
        let Some((name, rest)) = args.split_first() else { return Err(invalid("empty command")) };
        let mut cmd = redis::cmd(&String::from_utf8_lossy(name));
        for a in rest {
            cmd.arg(a.as_slice());
        }
        cmd.query_async::<RValue>(&mut self.c()).await.map_err(r_err)
    }

    pub async fn scan(&self, cursor: &str, pattern: Option<&str>, type_filter: Option<&str>, count: usize) -> Result<ScanPage> {
        let count = count.clamp(10, 5000);
        let mut cursor: u64 = cursor.trim().parse().unwrap_or(0);
        let mut raw_keys: Vec<Vec<u8>> = Vec::new();
        let started = Instant::now();
        let type_filter = type_filter.filter(|t| !t.is_empty()).map(str::to_ascii_lowercase);
        loop {
            let mut cmd = redis::cmd("SCAN");
            cmd.arg(cursor).arg("COUNT").arg(count);
            if let Some(p) = pattern.filter(|p| !p.is_empty() && *p != "*") {
                cmd.arg("MATCH").arg(p);
            }
            let (next, keys): (u64, Vec<Vec<u8>>) = cmd.query_async(&mut self.c()).await.map_err(r_err)?;
            raw_keys.extend(keys);
            cursor = next;
            // Sparse matches return few keys per call: keep going for a moment.
            if cursor == 0 || raw_keys.len() >= count || started.elapsed() > Duration::from_millis(400) {
                break;
            }
        }
        let mut keys = Vec::with_capacity(raw_keys.len());
        for chunk in raw_keys.chunks(500) {
            let mut pipe = redis::pipe();
            for k in chunk {
                pipe.cmd("TYPE").arg(k.as_slice()).cmd("PTTL").arg(k.as_slice());
            }
            let vals: Vec<RValue> = pipe.query_async(&mut self.c()).await.map_err(r_err)?;
            for (i, k) in chunk.iter().enumerate() {
                let kind = vals.get(i * 2).map(text_of).unwrap_or_default();
                if kind == "none" {
                    continue;
                }
                if let Some(t) = &type_filter
                    && !kind.eq_ignore_ascii_case(t)
                {
                    continue;
                }
                keys.push(KeyInfo {
                    key: display_bytes(k),
                    key_b64: is_binary(k).then(|| base64::engine::general_purpose::STANDARD.encode(k)),
                    kind,
                    ttl_ms: vals.get(i * 2 + 1).map(int_of).unwrap_or(-1),
                });
            }
        }
        let db_size: u64 = redis::cmd("DBSIZE").query_async(&mut self.c()).await.unwrap_or(0);
        Ok(ScanPage { cursor: cursor.to_string(), done: cursor == 0, keys, db_size })
    }

    /// Reads a key by type. `cursor` continues HSCAN/SSCAN (or a stream id for streams);
    /// `offset` pages lists and sorted sets; `count` is the page size.
    pub async fn get(&self, key: &RedisKey, cursor: Option<&str>, offset: u64, count: u64) -> Result<KeyValue> {
        let k = key.bytes()?;
        let count = count.clamp(1, 10_000);
        let mut pipe = redis::pipe();
        pipe.cmd("TYPE").arg(k.as_slice()).cmd("PTTL").arg(k.as_slice());
        let tv: Vec<RValue> = pipe.query_async(&mut self.c()).await.map_err(r_err)?;
        let kind = tv.first().map(text_of).unwrap_or_default();
        let ttl_ms = tv.get(1).map(int_of).unwrap_or(-2);
        let ks = k.as_slice();
        let value = match kind.as_str() {
            "none" => RedisValue::None,
            "string" => {
                let size: u64 = redis::cmd("STRLEN").arg(ks).query_async(&mut self.c()).await.map_err(r_err)?;
                let truncated = size > MAX_STRING;
                let bytes: Vec<u8> = if truncated {
                    redis::cmd("GETRANGE").arg(ks).arg(0).arg(MAX_STRING - 1).query_async(&mut self.c()).await.map_err(r_err)?
                } else {
                    redis::cmd("GET").arg(ks).query_async::<Option<Vec<u8>>>(&mut self.c()).await.map_err(r_err)?.unwrap_or_default()
                };
                RedisValue::String {
                    value: display_bytes(&bytes),
                    base64: is_binary(&bytes).then(|| base64::engine::general_purpose::STANDARD.encode(&bytes)),
                    size,
                    truncated,
                }
            }
            "hash" => {
                let len: u64 = redis::cmd("HLEN").arg(ks).query_async(&mut self.c()).await.map_err(r_err)?;
                let (cursor, flat) = self.scan_collection("HSCAN", ks, cursor, count).await?;
                let fields = flat.chunks(2).map(|p| Field { field: text_of(&p[0]), value: p.get(1).map(text_of).unwrap_or_default() }).collect();
                RedisValue::Hash { fields, len, cursor }
            }
            "set" => {
                let len: u64 = redis::cmd("SCARD").arg(ks).query_async(&mut self.c()).await.map_err(r_err)?;
                let (cursor, flat) = self.scan_collection("SSCAN", ks, cursor, count).await?;
                RedisValue::Set { members: flat.iter().map(text_of).collect(), len, cursor }
            }
            "list" => {
                let len: u64 = redis::cmd("LLEN").arg(ks).query_async(&mut self.c()).await.map_err(r_err)?;
                let v: RValue = redis::cmd("LRANGE").arg(ks).arg(offset).arg(offset + count - 1).query_async(&mut self.c()).await.map_err(r_err)?;
                RedisValue::List { items: items(v).iter().map(text_of).collect(), len, offset }
            }
            "zset" => {
                let len: u64 = redis::cmd("ZCARD").arg(ks).query_async(&mut self.c()).await.map_err(r_err)?;
                let v: RValue = redis::cmd("ZRANGE").arg(ks).arg(offset).arg(offset + count - 1).arg("WITHSCORES").query_async(&mut self.c()).await.map_err(r_err)?;
                let flat = items(v);
                // RESP2: flat [member, score, …]; RESP3: [[member, score], …].
                let members = if flat.iter().all(|x| matches!(x, RValue::Array(_))) {
                    flat.into_iter()
                        .map(|p| {
                            let p = items(p);
                            ZMember { member: p.first().map(text_of).unwrap_or_default(), score: p.get(1).map(score_json).unwrap_or(Value::Null) }
                        })
                        .collect()
                } else {
                    flat.chunks(2).map(|p| ZMember { member: text_of(&p[0]), score: p.get(1).map(score_json).unwrap_or(Value::Null) }).collect()
                };
                RedisValue::Zset { members, len, offset }
            }
            "stream" => {
                let len: u64 = redis::cmd("XLEN").arg(ks).query_async(&mut self.c()).await.map_err(r_err)?;
                let end = match cursor.filter(|c| !c.is_empty()) {
                    Some(c) => format!("({c}"),
                    None => "+".into(),
                };
                let v: RValue = redis::cmd("XREVRANGE").arg(ks).arg(end).arg("-").arg("COUNT").arg(count).query_async(&mut self.c()).await.map_err(r_err)?;
                let entries: Vec<StreamEntry> = items(v)
                    .into_iter()
                    .map(|e| {
                        let e = items(e);
                        let id = e.first().map(text_of).unwrap_or_default();
                        let f = e.get(1).cloned().map(items).unwrap_or_default();
                        StreamEntry { id, fields: f.chunks(2).map(|p| Field { field: text_of(&p[0]), value: p.get(1).map(text_of).unwrap_or_default() }).collect() }
                    })
                    .collect();
                let next = (entries.len() as u64 == count).then(|| entries.last().map(|e| e.id.clone())).flatten();
                let groups = redis::cmd("XINFO").arg("GROUPS").arg(ks).query_async::<RValue>(&mut self.c()).await.map(|g| reply_to_json(&g)).unwrap_or(Value::Null);
                RedisValue::Stream { entries, len, groups, next }
            }
            "ReJSON-RL" => {
                let text: Option<String> = redis::cmd("JSON.GET").arg(ks).query_async(&mut self.c()).await.map_err(r_err)?;
                RedisValue::Json { value: text.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null) }
            }
            other => RedisValue::Unsupported { type_name: other.to_string() },
        };
        Ok(KeyValue { kind, ttl_ms, value })
    }

    async fn scan_collection(&self, cmd: &str, key: &[u8], cursor: Option<&str>, count: u64) -> Result<(String, Vec<RValue>)> {
        let mut cur: u64 = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
        let mut out = Vec::new();
        let per_item = if cmd == "HSCAN" { 2 } else { 1 };
        loop {
            let v: RValue = redis::cmd(cmd).arg(key).arg(cur).arg("COUNT").arg(count).query_async(&mut self.c()).await.map_err(r_err)?;
            let mut parts = items(v).into_iter();
            cur = parts.next().map(|c| int_of(&c) as u64).unwrap_or(0);
            out.extend(parts.next().map(items).unwrap_or_default());
            if cur == 0 || (out.len() / per_item) as u64 >= count {
                break;
            }
        }
        Ok((cur.to_string(), out))
    }

    pub async fn edit(&self, edit: &RedisEdit) -> Result<CommandReply> {
        let args = edit.command()?;
        let started = Instant::now();
        let v = self.raw(&args).await?;
        if let RedisEdit::Rename { new_key, .. } = edit
            && matches!(v, RValue::Int(0))
        {
            return Err(DbError::Query(format!("a key named '{new_key}' already exists")));
        }
        Ok(CommandReply {
            command: args.iter().map(|a| display_bytes(a)).collect::<Vec<_>>().join(" "),
            text: format_reply(&v),
            value: reply_to_json(&v),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            error: None,
        })
    }

    pub async fn delete(&self, keys: &[RedisKey]) -> Result<u64> {
        if keys.is_empty() {
            return Ok(0);
        }
        let raw: Vec<Vec<u8>> = keys.iter().map(|k| k.bytes()).collect::<Result<_>>()?;
        let mut cmd = redis::cmd("UNLINK");
        for k in &raw {
            cmd.arg(k.as_slice());
        }
        match cmd.query_async::<u64>(&mut self.c()).await {
            Ok(n) => Ok(n),
            Err(_) => {
                let mut cmd = redis::cmd("DEL");
                for k in &raw {
                    cmd.arg(k.as_slice());
                }
                cmd.query_async::<u64>(&mut self.c()).await.map_err(r_err)
            }
        }
    }

    /// Runs one console command. Blocked when `read_only` and it isn't a read.
    pub async fn command(&self, line: &str, read_only: bool) -> Result<CommandReply> {
        let args = parse_command_line(line)?;
        if args.is_empty() {
            return Err(invalid("empty command"));
        }
        check_command(&args, read_only)?;
        let started = Instant::now();
        let (text, value, error) = match self.raw(&args).await {
            Ok(v) => (format_reply(&v), reply_to_json(&v), None),
            Err(e) => (format!("(error) {e}"), Value::Null, Some(e.to_string())),
        };
        Ok(CommandReply { command: line.trim().to_string(), text, value, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0, error })
    }

    /// Runs a script of commands (one per line; `#` comments). In read-only mode the whole script
    /// is checked first.
    pub async fn run_script(&self, script: &str, read_only: bool) -> Result<Vec<CommandReply>> {
        let lines: Vec<&str> = script.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        for l in &lines {
            check_command(&parse_command_line(l)?, read_only)?;
        }
        let mut out = Vec::with_capacity(lines.len());
        for l in lines {
            out.push(self.command(l, read_only).await?);
        }
        Ok(out)
    }

    pub async fn info(&self, section: Option<&str>) -> Result<Vec<InfoSection>> {
        let mut cmd = redis::cmd("INFO");
        if let Some(s) = section.filter(|s| !s.is_empty()) {
            cmd.arg(s);
        }
        let text: String = cmd.query_async(&mut self.c()).await.map_err(r_err)?;
        Ok(parse_info(&text))
    }

    pub async fn slowlog(&self, count: usize) -> Result<Vec<SlowlogEntry>> {
        let v: RValue = redis::cmd("SLOWLOG").arg("GET").arg(count.clamp(1, 1000)).query_async(&mut self.c()).await.map_err(r_err)?;
        Ok(items(v)
            .into_iter()
            .map(|e| {
                let e = items(e);
                SlowlogEntry {
                    id: e.first().map(int_of).unwrap_or(0),
                    timestamp: e.get(1).map(int_of).unwrap_or(0),
                    duration_us: e.get(2).map(int_of).unwrap_or(0),
                    command: e.get(3).cloned().map(items).unwrap_or_default().iter().map(text_of).collect::<Vec<_>>().join(" "),
                    client: e.get(4).map(text_of).unwrap_or_default(),
                    client_name: e.get(5).map(text_of).unwrap_or_default(),
                }
            })
            .collect())
    }

    pub async fn memory_usage(&self, key: &RedisKey) -> Result<Option<u64>> {
        redis::cmd("MEMORY").arg("USAGE").arg(key.bytes()?).arg("SAMPLES").arg(0).query_async(&mut self.c()).await.map_err(r_err)
    }

    /// `HSET <prefix><id> field value …` per row (`id`/`_id` or the 1-based index), pipelined in
    /// batches of 500. Nested values are stored as JSON text.
    pub async fn hset_rows(&self, prefix: &str, rows: &[serde_json::Map<String, Value>]) -> Result<u64> {
        let mut n = 0u64;
        for (ci, chunk) in rows.chunks(500).enumerate() {
            let mut pipe = redis::pipe();
            for (i, row) in chunk.iter().enumerate() {
                if row.is_empty() {
                    continue;
                }
                let text = |v: &Value| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
                let id = row.get("id").or_else(|| row.get("_id")).map(text).unwrap_or_else(|| (ci * 500 + i + 1).to_string());
                let mut cmd = redis::cmd("HSET");
                cmd.arg(format!("{prefix}{id}"));
                for (k, v) in row {
                    cmd.arg(k).arg(text(v));
                }
                pipe.add_command(cmd).ignore();
                n += 1;
            }
            pipe.query_async::<()>(&mut self.c()).await.map_err(r_err)?;
        }
        Ok(n)
    }

    pub async fn publish(&self, channel: &str, message: &str) -> Result<i64> {
        redis::cmd("PUBLISH").arg(channel).arg(message).query_async(&mut self.c()).await.map_err(r_err)
    }

    /// Subscribes on a dedicated connection; messages go to `on_message` until the returned
    /// handle is aborted.
    pub async fn subscribe(&self, channels: &[String], patterns: &[String], on_message: Arc<dyn Fn(PubSubMessage) + Send + Sync>) -> Result<AbortHandle> {
        if channels.is_empty() && patterns.is_empty() {
            return Err(invalid("subscribe to at least one channel or pattern"));
        }
        let mut ps = self.client.get_async_pubsub().await.map_err(r_err)?;
        for c in channels {
            ps.subscribe(c).await.map_err(r_err)?;
        }
        for p in patterns {
            ps.psubscribe(p).await.map_err(r_err)?;
        }
        let task = tokio::spawn(async move {
            let mut stream = ps.into_on_message();
            while let Some(msg) = stream.next().await {
                on_message(PubSubMessage {
                    channel: msg.get_channel_name().to_string(),
                    pattern: msg.get_pattern::<Option<String>>().ok().flatten(),
                    payload: display_bytes(msg.get_payload_bytes()),
                    at: now_ms(),
                });
            }
        });
        Ok(task.abort_handle())
    }
}

fn score_json(v: &RValue) -> Value {
    match v {
        RValue::Double(d) => crate::sql::types::float_value(*d),
        other => {
            let t = text_of(other);
            t.parse::<f64>().ok().filter(|f| f.is_finite()).map(crate::sql::types::float_value).unwrap_or(Value::String(t))
        }
    }
}

/// Rejects commands the console can't or mustn't run.
pub fn check_command(args: &[Vec<u8>], read_only: bool) -> Result<()> {
    let name = args.first().map(|n| String::from_utf8_lossy(n).to_ascii_uppercase()).unwrap_or_default();
    match classify_command(args) {
        CommandClass::Read => Ok(()),
        CommandClass::Streaming => Err(invalid(format!("{name} needs its own connection — use the Pub/Sub tab"))),
        CommandClass::Blocking => Err(invalid(format!("{name} would block the shared connection; use a non-blocking variant (or omit BLOCK)"))),
        CommandClass::Write | CommandClass::Admin if read_only => Err(DbError::ReadOnly(format!(
            "Read-only session: `{name}` was not run because it {}. Turn off read-only for this connection to run it.",
            if classify_command(args) == CommandClass::Admin { "administers the server" } else { "modifies data" }
        ))),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        parse_command_line(line).unwrap().into_iter().map(|a| String::from_utf8(a).unwrap()).collect()
    }

    #[test]
    fn command_lines() {
        assert_eq!(args(r#"SET "my key" 'it\'s' plain"#), vec!["SET", "my key", "it's", "plain"]);
        assert_eq!(args(r#"SET k "a\nb\x41\"""#), vec!["SET", "k", "a\nbA\""]);
        assert_eq!(parse_command_line(r#"SET k "\xff""#).unwrap()[2], vec![0xff]);
        assert!(args("   ").is_empty());
        assert!(parse_command_line(r#"GET "unterminated"#).is_err());
        assert!(parse_command_line(r#"GET "a"b"#).is_err());
    }

    fn class(line: &str) -> CommandClass {
        classify_command(&parse_command_line(line).unwrap())
    }

    #[test]
    fn classification() {
        use CommandClass::*;
        for l in ["GET k", "hgetall h", "SCAN 0 MATCH x*", "CONFIG GET maxmemory", "CLIENT LIST", "MEMORY USAGE k", "JSON.GET doc", "XREAD COUNT 1 STREAMS s 0", "INFO", "SLOWLOG GET 10", "TYPE k"] {
            assert_eq!(class(l), Read, "{l}");
        }
        for l in ["SET k v", "DEL k", "HSET h f v", "EXPIRE k 10", "EVAL 'return 1' 0", "XADD s * a b", "XREADGROUP GROUP g c STREAMS s >", "PUBLISH c m", "SOMEMODULE.CMD x"] {
            assert_eq!(class(l), Write, "{l}");
        }
        for l in ["FLUSHALL", "CONFIG SET maxmemory 1", "SHUTDOWN", "CLIENT KILL ID 3", "SLOWLOG RESET", "DEBUG SLEEP 1"] {
            assert_eq!(class(l), Admin, "{l}");
        }
        assert_eq!(class("SUBSCRIBE c"), Streaming);
        assert_eq!(class("MONITOR"), Streaming);
        assert_eq!(class("BLPOP q 0"), Blocking);
        assert_eq!(class("XREAD BLOCK 0 STREAMS s $"), Blocking);
        assert!(check_command(&parse_command_line("DEL k").unwrap(), true).is_err());
        assert!(check_command(&parse_command_line("DEL k").unwrap(), false).is_ok());
        assert!(check_command(&parse_command_line("GET k").unwrap(), true).is_ok());
        let msg = check_command(&parse_command_line("flushall").unwrap(), true).unwrap_err().to_string();
        assert!(msg.contains("`FLUSHALL`") && msg.contains("administers"), "{msg}");
        assert!(check_command(&parse_command_line("subscribe x").unwrap(), false).is_err());
    }

    #[test]
    fn replies() {
        let v = RValue::Array(vec![RValue::BulkString(b"a".to_vec()), RValue::Int(3), RValue::Nil, RValue::Array(vec![RValue::BulkString(b"x\xff".to_vec())])]);
        assert_eq!(format_reply(&v), "1) \"a\"\n2) (integer) 3\n3) (nil)\n4) 1) \"x\\xff\"");
        assert_eq!(reply_to_json(&v), json!(["a", 3, null, ["x\\xff"]]));
        assert_eq!(format_reply(&RValue::Okay), "OK");
        assert_eq!(format_reply(&RValue::Array(vec![])), "(empty array)");
        assert_eq!(format_reply(&RValue::BulkString("héllo".as_bytes().to_vec())), "\"héllo\"");
        assert_eq!(display_bytes(b"\xffa\n"), "\\xffa\\n", "binary values are escaped");
        assert_eq!(display_bytes("é\n".as_bytes()), "é\n", "valid UTF-8 is kept as is");
    }

    #[test]
    fn info_and_edits() {
        let s = parse_info("# Server\r\nredis_version:7.2.4\r\nuptime_in_seconds:10\r\n\r\n# Keyspace\r\ndb0:keys=3,expires=0\r\n");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].entries[0], ("redis_version".into(), "7.2.4".into()));
        let e = RedisEdit::SetString { key: RedisKey::Text("k".into()), value: "v".into(), ttl_ms: Some(1500) };
        assert_eq!(e.command().unwrap(), vec![b"SET".to_vec(), b"k".to_vec(), b"v".to_vec(), b"PX".to_vec(), b"1500".to_vec()]);
        let e = RedisEdit::Expire { key: RedisKey::Raw { b64: "/w==".into() }, ttl_ms: None };
        assert_eq!(e.command().unwrap(), vec![b"PERSIST".to_vec(), vec![0xff]]);
        assert!(RedisEdit::JsonSet { key: RedisKey::Text("k".into()), path: None, value: "{bad".into() }.command().is_err());
        let parsed: RedisEdit = serde_json::from_value(json!({"type": "hashSet", "key": {"b64": "AA=="}, "field": "f", "value": "v"})).unwrap();
        assert!(matches!(parsed, RedisEdit::HashSet { key: RedisKey::Raw { .. }, .. }));
    }
}
