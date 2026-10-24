//! Format detection and per-line parsing.
//!
//! Every line yields a cheap [`Meta`] (timestamp, level, "is this structured") used while
//! indexing, and on demand a full [`LogEntry`]. Multi-line stack traces (Java, JS, Python, Rust,
//! Go, .NET) are grouped into the previous entry: in files where most lines carry a timestamp any
//! line without one continues the previous entry; elsewhere only lines that look like stack frames
//! or exception headers do.

use std::borrow::Cow;
use std::fmt;

use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::time::{self, TimeCtx};

// ---------- level ----------

/// Normalized severity. Ordered `trace < debug < info < warn < error < fatal`; `unknown` is
/// outside the order (never matches `level>=x`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Level {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
    Fatal = 5,
    #[default]
    Unknown = 6,
}

impl Level {
    /// Index order used by histogram/pattern level counts.
    pub const ALL: [Level; 7] = [Level::Trace, Level::Debug, Level::Info, Level::Warn, Level::Error, Level::Fatal, Level::Unknown];

    pub fn from_u8(v: u8) -> Level {
        Self::ALL.get(v as usize).copied().unwrap_or(Level::Unknown)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Fatal => "fatal",
            Level::Unknown => "unknown",
        }
    }

    /// Severity rank; `None` for unknown.
    pub fn rank(self) -> Option<u8> {
        (self != Level::Unknown).then_some(self as u8)
    }

    /// Level words in any case: `WARN`, `warning`, `E`… (single letters only via [`Self::from_letter`]).
    pub fn parse(s: &str) -> Option<Level> {
        let s = s.trim();
        if s.is_empty() || s.len() > 13 {
            return None;
        }
        let mut buf = [0u8; 13];
        for (i, c) in s.bytes().enumerate() {
            buf[i] = c.to_ascii_lowercase();
        }
        Some(match &buf[..s.len()] {
            b"trace" | b"trc" | b"trce" | b"verbose" | b"vrb" | b"finest" | b"finer" | b"all" => Level::Trace,
            b"debug" | b"dbg" | b"dbug" | b"fine" | b"config" => Level::Debug,
            b"info" | b"inf" | b"information" | b"informational" | b"notice" | b"note" => Level::Info,
            b"warn" | b"warning" | b"wrn" | b"wrng" => Level::Warn,
            b"error" | b"err" | b"eror" | b"fail" | b"failure" | b"severe" => Level::Error,
            b"fatal" | b"ftl" | b"crit" | b"critical" | b"crt" | b"emerg" | b"emergency" | b"alert" | b"panic" | b"dpanic" => {
                Level::Fatal
            }
            _ => return None,
        })
    }

    /// klog/glog style single letters.
    pub fn from_letter(c: u8) -> Option<Level> {
        Some(match c {
            b'T' => Level::Trace,
            b'D' => Level::Debug,
            b'I' => Level::Info,
            b'W' => Level::Warn,
            b'E' => Level::Error,
            b'F' | b'C' => Level::Fatal,
            _ => return None,
        })
    }

    /// Numeric levels: pino/bunyan (10…60) or syslog severities (0…7).
    pub fn from_number(n: f64) -> Option<Level> {
        if !n.is_finite() || n < 0.0 {
            return None;
        }
        let n = n as i64;
        Some(match n {
            0..=7 => Self::from_syslog_severity(n as u8),
            10..=19 => Level::Trace,
            20..=29 => Level::Debug,
            30..=39 => Level::Info,
            40..=49 => Level::Warn,
            50..=59 => Level::Error,
            60..=99 => Level::Fatal,
            _ => return None,
        })
    }

    pub fn from_syslog_severity(sev: u8) -> Level {
        match sev {
            0..=2 => Level::Fatal,
            3 => Level::Error,
            4 => Level::Warn,
            5 | 6 => Level::Info,
            _ => Level::Debug,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------- format ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    /// JSON object per line (any keys; common names mapped).
    Json,
    /// Docker json-file driver: `{"log":"…","stream":"stdout","time":"…"}`.
    Docker,
    /// Kubernetes CRI: `2024-01-02T03:04:05.123Z stdout F message`.
    Cri,
    Logfmt,
    /// Apache/Nginx common or combined access log.
    AccessLog,
    /// RFC 3164 (BSD) syslog.
    Syslog,
    Syslog5424,
    /// Kubernetes/glog: `I0102 03:04:05.123456 1 file.go:12] message`.
    Klog,
    /// `[timestamp] LEVEL message` and friends (log4j, Python, tracing, nginx error, Serilog…).
    Prefixed,
    #[default]
    Plain,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Json => "JSON lines",
            Format::Docker => "Docker JSON",
            Format::Cri => "Kubernetes CRI",
            Format::Logfmt => "logfmt",
            Format::AccessLog => "Access log",
            Format::Syslog => "Syslog (RFC 3164)",
            Format::Syslog5424 => "Syslog (RFC 5424)",
            Format::Klog => "klog",
            Format::Prefixed => "Timestamped text",
            Format::Plain => "Plain text",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Docker => "docker",
            Format::Cri => "cri",
            Format::Logfmt => "logfmt",
            Format::AccessLog => "accessLog",
            Format::Syslog => "syslog",
            Format::Syslog5424 => "syslog5424",
            Format::Klog => "klog",
            Format::Prefixed => "prefixed",
            Format::Plain => "plain",
        }
    }
}

// ---------- entry ----------

/// One log entry (a line, or a line plus its stack-trace continuation lines).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Position in its view (filters keep the original position).
    pub index: u64,
    /// 1-based physical line number of the first line.
    pub line_no: u64,
    /// Physical lines in the entry (> 1 for grouped stack traces).
    pub line_count: u32,
    /// Unix ms (UTC) when the entry carried a parseable timestamp.
    pub ts: Option<i64>,
    pub level: Level,
    /// The message (continuation lines appended after `\n`).
    pub message: String,
    /// Structured fields (JSON object keys, logfmt pairs, access-log parts, `k=v` in messages).
    pub fields: Map<String, Value>,
    pub raw: String,
    pub format: Format,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Source name in merged views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl LogEntry {
    /// Field by name: exact key first, then a dotted path through nested objects.
    pub fn field(&self, name: &str) -> Option<&Value> {
        lookup(&self.fields, name)
    }

    /// First line of the message.
    pub fn first_line(&self) -> &str {
        self.message.split('\n').next().unwrap_or("")
    }
}

pub fn lookup<'a>(fields: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    if let Some(v) = fields.get(name) {
        return Some(v);
    }
    if !name.contains('.') {
        return None;
    }
    let mut parts = name.split('.');
    let mut cur = fields.get(parts.next()?)?;
    for p in parts {
        cur = match cur {
            Value::Object(o) => o.get(p)?,
            Value::Array(a) => a.get(p.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Cheap per-line facts gathered while indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Meta {
    pub ts: Option<i64>,
    pub level: Level,
    /// The line parsed as a self-delimiting record (JSON, logfmt, syslog, access log…).
    pub structured: bool,
}

/// One parsed physical line.
#[derive(Debug, Clone, Default)]
pub struct ParsedLine {
    pub ts: Option<i64>,
    pub level: Level,
    pub message: String,
    pub fields: Map<String, Value>,
    pub format: Format,
    pub correlation_id: Option<String>,
}

// ---------- key names ----------

const TS_KEYS: &[&str] = &[
    "@timestamp", "timestamp", "time", "ts", "@t", "datetime", "date", "t", "eventTime", "event_time", "logged_at", "__REALTIME_TIMESTAMP",
];
const LEVEL_KEYS: &[&str] =
    &["level", "severity", "lvl", "@l", "loglevel", "log_level", "levelname", "level_name", "severityText", "severity_text", "@level", "PRIORITY"];
const MSG_KEYS: &[&str] =
    &["msg", "message", "@m", "@message", "@mt", "log", "text", "event", "body", "short_message", "full_message", "error"];
const CORR_KEYS: &[&str] = &[
    "trace_id", "traceId", "traceID", "trace.id", "correlation_id", "correlationId", "correlationID", "request_id", "requestId", "requestID", "req_id",
    "reqId", "x-request-id", "x_request_id", "transaction.id",
];

fn key_rank(keys: &[&str], k: &str) -> Option<u8> {
    keys.iter().position(|c| c.eq_ignore_ascii_case(k)).map(|i| i as u8)
}

// ---------- JSON meta (no allocation for ignored values) ----------

struct Key<'a>(Cow<'a, str>);

impl<'de> Deserialize<'de> for Key<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Key<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a key")
            }
            fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Borrowed(v)))
            }
            fn visit_str<E>(self, v: &str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Owned(v.to_string())))
            }
            fn visit_string<E>(self, v: String) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Owned(v)))
            }
        }
        d.deserialize_str(V)
    }
}

enum Scalar<'a> {
    Str(Cow<'a, str>),
    Num(f64),
    /// An object; `level` holds its `level` member (ECS `log.level`).
    Obj(Option<Box<Scalar<'a>>>),
    Other,
}

impl<'de> Deserialize<'de> for Scalar<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Scalar<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Str(Cow::Borrowed(v)))
            }
            fn visit_str<E>(self, v: &str) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Str(Cow::Owned(v.to_string())))
            }
            fn visit_string<E>(self, v: String) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Str(Cow::Owned(v)))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Num(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Num(v as f64))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Num(v))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Other)
            }
            fn visit_unit<E>(self) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Other)
            }
            fn visit_none<E>(self) -> Result<Scalar<'de>, E> {
                Ok(Scalar::Other)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Scalar<'de>, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Scalar::Other)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Scalar<'de>, A::Error> {
                let mut level = None;
                while let Some(k) = map.next_key::<Key<'de>>()? {
                    if k.0 == "level" {
                        level = Some(Box::new(map.next_value::<Scalar<'de>>()?));
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(Scalar::Obj(level))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Default)]
struct JsonMeta<'a> {
    ts: Option<(u8, Scalar<'a>)>,
    level: Option<(u8, Scalar<'a>)>,
    /// Docker `log` member.
    log: Option<Cow<'a, str>>,
    stream: bool,
}

impl<'de> Deserialize<'de> for JsonMeta<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = JsonMeta<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsonMeta<'de>, A::Error> {
                let mut m = JsonMeta::default();
                while let Some(k) = map.next_key::<Key<'de>>()? {
                    let k = k.0;
                    if k == "log" {
                        match map.next_value::<Scalar<'de>>()? {
                            Scalar::Str(s) => m.log = Some(s),
                            Scalar::Obj(Some(level)) if m.level.as_ref().is_none_or(|(r, _)| *r > 50) => {
                                m.level = Some((50, *level));
                            }
                            _ => {}
                        }
                    } else if k == "stream" {
                        m.stream = true;
                        map.next_value::<IgnoredAny>()?;
                    } else if let Some(r) = key_rank(TS_KEYS, &k) {
                        let v = map.next_value::<Scalar<'de>>()?;
                        if m.ts.as_ref().is_none_or(|(old, _)| *old > r) {
                            m.ts = Some((r, v));
                        }
                    } else if let Some(r) = key_rank(LEVEL_KEYS, &k) {
                        let v = map.next_value::<Scalar<'de>>()?;
                        if m.level.as_ref().is_none_or(|(old, _)| *old > r) {
                            m.level = Some((r, v));
                        }
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(m)
            }
        }
        d.deserialize_map(V)
    }
}

fn scalar_ts(s: &Scalar, ctx: &TimeCtx) -> Option<i64> {
    match s {
        Scalar::Str(s) => time::parse_str(s, ctx),
        Scalar::Num(n) => time::from_epoch(*n),
        _ => None,
    }
}

fn scalar_level(s: &Scalar) -> Option<Level> {
    match s {
        Scalar::Str(s) => Level::parse(s).or_else(|| s.trim().parse::<f64>().ok().and_then(Level::from_number)),
        Scalar::Num(n) => Level::from_number(*n),
        Scalar::Obj(Some(inner)) => scalar_level(inner),
        _ => None,
    }
}

fn json_meta(line: &str, ctx: &TimeCtx) -> Option<(Meta, bool)> {
    let t = line.trim();
    if !t.starts_with('{') {
        return None;
    }
    let m: JsonMeta = serde_json::from_str(t).ok()?;
    if let (Some(log), true) = (&m.log, m.stream) {
        // Docker wrapper: the inner text decides the level (and maybe the timestamp).
        let inner = auto_meta(log.trim_end_matches(['\n', '\r']), ctx);
        let ts = inner.ts.or_else(|| m.ts.as_ref().and_then(|(_, s)| scalar_ts(s, ctx)));
        return Some((Meta { ts, level: inner.level, structured: true }, true));
    }
    let ts = m.ts.as_ref().and_then(|(_, s)| scalar_ts(s, ctx));
    let level = m.level.as_ref().and_then(|(_, s)| scalar_level(s)).unwrap_or_default();
    Some((Meta { ts, level, structured: true }, false))
}

// ---------- logfmt ----------

/// `key=value key2="quoted value" flag` pairs. `None` unless the line starts with a pair and
/// has at least two of them.
pub fn logfmt_pairs(line: &str) -> Option<Vec<(&str, Cow<'_, str>)>> {
    let pairs = kv_pairs(line, true);
    (pairs.len() >= 2).then_some(pairs)
}

fn is_key_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'@' | b'/')
}

/// Scans `k=v` tokens. With `strict`, stops at the first token that isn't a pair (so the line
/// must *be* logfmt); otherwise skips non-pair words (pairs embedded in a message).
fn kv_pairs(line: &str, strict: bool) -> Vec<(&str, Cow<'_, str>)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let ks = i;
        while i < b.len() && is_key_char(b[i]) {
            i += 1;
        }
        let key_ok = i > ks && (b[ks].is_ascii_alphabetic() || b[ks] == b'_' || b[ks] == b'@');
        if key_ok && i < b.len() && b[i] == b'=' {
            let key = &line[ks..i];
            i += 1;
            if i < b.len() && b[i] == b'"' {
                i += 1;
                let vs = i;
                let mut escaped = false;
                let mut has_escape = false;
                while i < b.len() {
                    if escaped {
                        escaped = false;
                    } else if b[i] == b'\\' {
                        escaped = true;
                        has_escape = true;
                    } else if b[i] == b'"' {
                        break;
                    }
                    i += 1;
                }
                let raw = &line[vs..i.min(b.len())];
                i += 1;
                let v = if has_escape { Cow::Owned(unescape(raw)) } else { Cow::Borrowed(raw) };
                out.push((key, v));
            } else {
                let vs = i;
                while i < b.len() && b[i] != b' ' && b[i] != b'\t' {
                    i += 1;
                }
                out.push((key, Cow::Borrowed(&line[vs..i])));
            }
        } else {
            if strict && !(key_ok && (i >= b.len() || b[i] == b' ' || b[i] == b'\t') && !out.is_empty()) {
                // A bare flag is fine once the line has proven to be logfmt.
                return if out.len() >= 2 { out } else { Vec::new() };
            }
            while i < b.len() && b[i] != b' ' && b[i] != b'\t' {
                i += 1;
            }
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn scalar_value(v: &str) -> Value {
    if let Ok(i) = v.parse::<i64>() {
        return Value::from(i);
    }
    if v.contains(['.', 'e', 'E'])
        && let Ok(f) = v.parse::<f64>()
        && f.is_finite()
        && v.bytes().any(|c| c.is_ascii_digit())
        && !v.starts_with('.')
    {
        return Value::from(f);
    }
    match v {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(v.to_string()),
    }
}

fn logfmt_meta(line: &str, ctx: &TimeCtx) -> Option<Meta> {
    let pairs = logfmt_pairs(line)?;
    let mut ts: Option<(u8, i64)> = None;
    let mut level: Option<(u8, Level)> = None;
    for (k, v) in &pairs {
        if let Some(r) = key_rank(TS_KEYS, k) {
            if ts.is_none_or(|(o, _)| o > r)
                && let Some(ms) = time::parse_str(v, ctx)
            {
                ts = Some((r, ms));
            }
        } else if let Some(r) = key_rank(LEVEL_KEYS, k)
            && level.is_none_or(|(o, _)| o > r)
            && let Some(l) = Level::parse(v)
        {
            level = Some((r, l));
        }
    }
    Some(Meta { ts: ts.map(|t| t.1), level: level.map(|l| l.1).unwrap_or_default(), structured: true })
}

// ---------- access log ----------

struct Access<'a> {
    host: &'a str,
    ident: &'a str,
    user: &'a str,
    ts: Option<i64>,
    request: &'a str,
    status: Option<u16>,
    bytes: Option<u64>,
    referer: Option<&'a str>,
    agent: Option<&'a str>,
    rest: &'a str,
}

fn token(s: &str) -> Option<(&str, &str)> {
    let i = s.find(' ')?;
    Some((&s[..i], &s[i + 1..]))
}

fn quoted(s: &str) -> Option<(&str, &str)> {
    let s = s.strip_prefix('"')?;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some((&s[..i], s[i + 1..].trim_start_matches(' '))),
            _ => i += 1,
        }
    }
    None
}

fn access<'a>(line: &'a str, ctx: &TimeCtx) -> Option<Access<'a>> {
    let (host, rest) = token(line)?;
    if host.is_empty() || !host.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b':' | b'-' | b'_')) {
        return None;
    }
    let (ident, rest) = token(rest)?;
    let (user, rest) = token(rest)?;
    let rest = rest.strip_prefix('[')?;
    let close = rest.find(']')?;
    let ts = time::parse_prefix(&rest[..close], ctx).map(|t| t.0);
    let rest = rest[close + 1..].trim_start_matches(' ');
    let (request, rest) = quoted(rest)?;
    let (status, rest) = match token(rest) {
        Some(t) => t,
        None => (rest, ""),
    };
    let status = if status == "-" { None } else { Some(status.parse::<u16>().ok().filter(|s| (100..600).contains(s))?) };
    let (bytes, mut rest) = match token(rest) {
        Some(t) => t,
        None => (rest, ""),
    };
    let bytes = if bytes == "-" || bytes.is_empty() { None } else { Some(bytes.parse::<u64>().ok()?) };
    let mut referer = None;
    let mut agent = None;
    if let Some((r, after)) = quoted(rest) {
        referer = Some(r);
        rest = after;
        if let Some((a, after)) = quoted(rest) {
            agent = Some(a);
            rest = after;
        }
    }
    Some(Access { host, ident, user, ts, request, status, bytes, referer, agent, rest })
}

fn status_level(status: Option<u16>) -> Level {
    match status {
        Some(s) if s >= 500 => Level::Error,
        Some(s) if s >= 400 => Level::Warn,
        Some(_) => Level::Info,
        None => Level::Unknown,
    }
}

// ---------- syslog ----------

fn pri(line: &str) -> Option<(u8, &str)> {
    let rest = line.strip_prefix('<')?;
    let close = rest.find('>')?;
    if close == 0 || close > 3 {
        return None;
    }
    let p: u8 = rest[..close].parse().ok()?;
    (p <= 191).then_some((p, &rest[close + 1..]))
}

struct Syslog<'a> {
    pri: Option<u8>,
    ts: Option<i64>,
    host: &'a str,
    app: &'a str,
    pid: Option<&'a str>,
    msg: &'a str,
}

fn syslog3164<'a>(line: &'a str, ctx: &TimeCtx) -> Option<Syslog<'a>> {
    let (p, rest) = match pri(line) {
        Some((p, r)) => (Some(p), r),
        None => (None, line),
    };
    let first = *rest.as_bytes().first()?;
    // Without PRI insist on the classic `Mmm dd hh:mm:ss` stamp (ISO lines are "prefixed").
    if p.is_none() && !first.is_ascii_alphabetic() {
        return None;
    }
    let (ts, n) = time::parse_prefix(rest, ctx)?;
    let rest = rest[n..].strip_prefix(' ')?;
    let (host, rest) = token(rest)?;
    if host.is_empty() || host.ends_with(':') {
        return None;
    }
    // TAG[pid]: msg
    let colon = rest.find(": ").or_else(|| rest.strip_suffix(':').map(|r| r.len()))?;
    let tag = &rest[..colon];
    if tag.is_empty() || tag.len() > 64 || tag.contains(' ') {
        return None;
    }
    let (app, pid) = match tag.find('[') {
        Some(i) if tag.ends_with(']') => (&tag[..i], Some(&tag[i + 1..tag.len() - 1])),
        _ => (tag, None),
    };
    let msg = rest.get(colon + 2..).unwrap_or("");
    Some(Syslog { pri: p, ts: Some(ts), host, app, pid, msg })
}

struct Syslog5424<'a> {
    pri: u8,
    ts: Option<i64>,
    host: &'a str,
    app: &'a str,
    procid: &'a str,
    msgid: &'a str,
    sd: &'a str,
    msg: &'a str,
}

fn syslog5424<'a>(line: &'a str, ctx: &TimeCtx) -> Option<Syslog5424<'a>> {
    let (p, rest) = pri(line)?;
    let rest = rest.strip_prefix("1 ")?;
    let (ts_s, rest) = token(rest)?;
    let ts = if ts_s == "-" { None } else { Some(time::parse_str(ts_s, ctx)?) };
    let (host, rest) = token(rest)?;
    let (app, rest) = token(rest)?;
    let (procid, rest) = token(rest)?;
    let (msgid, rest) = match token(rest) {
        Some(t) => t,
        None => (rest, ""),
    };
    let (sd, msg) = if rest.starts_with('-') {
        ("", rest.get(2..).unwrap_or(""))
    } else if rest.starts_with('[') {
        let end = sd_end(rest)?;
        (&rest[..end], rest[end..].trim_start_matches(' '))
    } else {
        ("", rest)
    };
    Some(Syslog5424 { pri: p, ts, host, app, procid, msgid, sd, msg: msg.trim_start_matches('\u{feff}') })
}

/// End of the structured-data section (`[id k="v"][id2 …]`).
fn sd_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] == b'[' {
        let mut in_q = false;
        i += 1;
        while i < b.len() {
            match b[i] {
                b'\\' if in_q => i += 1,
                b'"' => in_q = !in_q,
                b']' if !in_q => break,
                _ => {}
            }
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        i += 1;
    }
    Some(i)
}

fn sd_fields(sd: &str) -> Map<String, Value> {
    let mut out = Map::new();
    let mut rest = sd;
    while let Some(r) = rest.strip_prefix('[') {
        let end = sd_end(rest).unwrap_or(rest.len());
        let body = &r[..end.saturating_sub(2).min(r.len())];
        let (id, params) = match body.find(' ') {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => (body, ""),
        };
        let obj: Map<String, Value> = kv_pairs(params, false).into_iter().map(|(k, v)| (k.to_string(), Value::String(v.into_owned()))).collect();
        out.insert(id.to_string(), Value::Object(obj));
        rest = &rest[end.min(rest.len())..];
    }
    out
}

// ---------- klog ----------

fn klog<'a>(line: &'a str, ctx: &TimeCtx) -> Option<(Level, Option<i64>, &'a str, &'a str)> {
    let b = line.as_bytes();
    if b.len() < 22 {
        return None;
    }
    let level = Level::from_letter(b[0]).filter(|_| matches!(b[0], b'I' | b'W' | b'E' | b'F'))?;
    let mo: u32 = line.get(1..3)?.parse().ok()?;
    let d: u32 = line.get(3..5)?.parse().ok()?;
    if b[5] != b' ' {
        return None;
    }
    let mut c = time::Civil { y: ctx.year, mo, d, ..Default::default() };
    c.h = line.get(6..8)?.parse().ok()?;
    if b[8] != b':' || b[11] != b':' {
        return None;
    }
    c.mi = line.get(9..11)?.parse().ok()?;
    c.s = line.get(12..14)?.parse().ok()?;
    let mut i = 14;
    if b.get(i) == Some(&b'.') {
        let fs = i + 1;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let frac = &line[fs..i];
        c.ms = frac.get(..3.min(frac.len())).and_then(|f| format!("{f:0<3}").parse().ok()).unwrap_or(0);
    }
    let ts = ctx.civil_to_ms(&c, None);
    let rest = line[i..].trim_start();
    let close = rest.find("] ")?;
    let head = &rest[..close];
    let caller = head.rsplit(' ').next().unwrap_or("");
    Some((level, ts, caller, &rest[close + 2..]))
}

// ---------- prefixed / plain ----------

struct Prefixed<'a> {
    ts: Option<i64>,
    level: Option<Level>,
    msg: &'a str,
    target: Option<&'a str>,
}

/// Level word in a token like `INFO`, `[warn]`, `<E>`, `level:error`, `core:error`, `INFO:root:msg`.
fn level_token(tok: &str) -> Option<Level> {
    let t = tok.trim_matches(|c: char| matches!(c, '[' | ']' | '(' | ')' | '<' | '>' | '{' | '}' | ':' | '|' | ',' | '"'));
    if t.is_empty() {
        return None;
    }
    if let Some(l) = Level::parse(t) {
        return Some(l);
    }
    if t.contains([':', '.']) {
        // `INFO:root:msg`, `core:error`, Laravel's `local.ERROR`.
        let first = t.split([':', '.']).next().and_then(Level::parse);
        return first.or_else(|| t.rsplit([':', '.']).next().and_then(Level::parse));
    }
    None
}

fn skip_clock(b: &[u8]) -> usize {
    // hh:mm:ss[.fff]
    if b.len() >= 8 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[2] == b':' && b[3].is_ascii_digit() && b[5] == b':' && b[6].is_ascii_digit() {
        let mut i = 8;
        if i < b.len() && (b[i] == b'.' || b[i] == b',') {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
        i
    } else {
        0
    }
}

fn prefixed<'a>(line: &'a str, ctx: &TimeCtx) -> Option<Prefixed<'a>> {
    let b = line.as_bytes();
    let mut i = 0;
    let mut ts = None;
    let mut stamped = false;
    let bracket = b.first() == Some(&b'[');
    let start = usize::from(bracket);
    if let Some((ms, n)) = time::parse_prefix(&line[start..], ctx) {
        ts = Some(ms);
        stamped = true;
        i = start + n;
    } else {
        let n = skip_clock(&b[start..]);
        if n > 0 {
            stamped = true;
            i = start + n;
        }
    }
    let mut closed = !bracket || !stamped;
    if stamped && bracket && b.get(i) == Some(&b']') {
        i += 1;
        closed = true;
    }
    let after_ts = i;
    let max_tokens = if stamped { 4 } else { 1 };
    let mut level = None;
    let mut msg_start = after_ts;
    let mut j = after_ts;
    for k in 0..max_tokens {
        while j < b.len() && matches!(b[j], b' ' | b'\t' | b'|') {
            j += 1;
        }
        // Python's " - " separators.
        if j + 1 < b.len() && b[j] == b'-' && (b[j + 1] == b' ' || b[j + 1] == b'\t') {
            j += 2;
            continue;
        }
        if j >= b.len() {
            break;
        }
        let ts_start = j;
        while j < b.len() && b[j] != b' ' && b[j] != b'\t' {
            j += 1;
        }
        let tok = &line[ts_start..j];
        if let Some(l) = level_token(tok) {
            level = Some(l);
            if k == 0 {
                msg_start = j;
            }
            break;
        }
    }
    if ts.is_none() && level.is_none() {
        return None;
    }
    let mut target = None;
    let mut rest = &line[msg_start.min(line.len())..];
    if !closed && let Some(close) = rest.find(']') {
        // env_logger: `[2024-… INFO  my_crate] msg`
        let t = rest[..close].trim();
        if !t.is_empty() && !t.contains(' ') {
            target = Some(t);
        }
        rest = &rest[close + 1..];
    }
    let msg = rest.trim_start_matches([' ', '\t', ':', '-', '|', ']']);
    Some(Prefixed { ts, level, msg, target })
}

// ---------- stack traces ----------

/// Lines that continue a previous entry even in files without timestamps.
pub fn is_stack_line(line: &str) -> bool {
    let b = line.as_bytes();
    let Some(&first) = b.first() else { return false };
    if first == b' ' || first == b'\t' {
        return !line.trim().is_empty();
    }
    const PREFIXES: &[&str] = &[
        "at ",
        "Caused by:",
        "Suppressed:",
        "... ",
        "Traceback (most recent call last)",
        "During handling of the above exception",
        "The above exception was the direct cause",
        "goroutine ",
        "created by ",
        "stack backtrace:",
        "note: run with `RUST_BACKTRACE",
        "--- End of ",
        "Exception in thread ",
        "[CIRCULAR REFERENCE",
    ];
    if PREFIXES.iter().any(|p| line.starts_with(p)) {
        return true;
    }
    // Exception headers: `java.lang.IllegalStateException: …`, `ValueError: …`, `TypeError: …`.
    let head_end = line.find(':').unwrap_or(line.len());
    let head = &line[..head_end];
    if !head.is_empty()
        && !head.contains(' ')
        && head.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'$'))
        && ["Error", "Exception", "Throwable", "Panic", "Fault", "Warning"].iter().any(|s| head.ends_with(s))
    {
        return true;
    }
    // Go frames: `main.main()` / `main.(*T).m(0x1, 0x2)`.
    if line.ends_with(')')
        && let Some(open) = line.find('(')
        && open > 0
        && !line[..open].contains(' ')
        && line[..open].contains('.')
        && !line.contains(": ")
    {
        return true;
    }
    false
}

// ---------- detection ----------

/// Detected file layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Detection {
    pub format: Format,
    /// Most entries carry a timestamp: lines without one continue the previous entry.
    pub ts_mode: bool,
}

fn try_format(line: &str, f: Format, ctx: &TimeCtx) -> Option<Meta> {
    match f {
        Format::Json => json_meta(line, ctx).filter(|(_, docker)| !docker).map(|m| m.0),
        Format::Docker => json_meta(line, ctx).filter(|(_, docker)| *docker).map(|m| m.0),
        Format::Cri => cri(line, ctx).map(|(ts, _, _, inner)| {
            let m = auto_meta(inner, ctx);
            Meta { ts: Some(ts).or(m.ts), level: m.level, structured: true }
        }),
        Format::Logfmt => logfmt_meta(line, ctx),
        Format::AccessLog => access(line, ctx).map(|a| Meta { ts: a.ts, level: status_level(a.status), structured: true }),
        Format::Syslog => syslog3164(line, ctx).map(|s| Meta {
            ts: s.ts,
            level: s.pri.map(|p| Level::from_syslog_severity(p % 8)).or_else(|| first_word_level(s.msg)).unwrap_or_default(),
            structured: true,
        }),
        Format::Syslog5424 => syslog5424(line, ctx).map(|s| Meta { ts: s.ts, level: Level::from_syslog_severity(s.pri % 8), structured: true }),
        Format::Klog => klog(line, ctx).map(|(level, ts, _, _)| Meta { ts, level, structured: true }),
        Format::Prefixed => prefixed(line, ctx).map(|p| Meta { ts: p.ts, level: p.level.unwrap_or_default(), structured: false }),
        Format::Plain => Some(Meta { ts: None, level: first_word_level(line).unwrap_or_default(), structured: false }),
    }
}

fn first_word_level(s: &str) -> Option<Level> {
    let tok = s.split_whitespace().next()?;
    level_token(tok)
}

/// `ts stream tag message`.
fn cri<'a>(line: &'a str, ctx: &TimeCtx) -> Option<(i64, &'a str, &'a str, &'a str)> {
    let (ts, rest) = token(line)?;
    if !ts.as_bytes().first()?.is_ascii_digit() {
        return None;
    }
    let (stream, rest) = token(rest)?;
    if stream != "stdout" && stream != "stderr" {
        return None;
    }
    let (tag, msg) = match token(rest) {
        Some(t) => t,
        None => (rest, ""),
    };
    if tag.is_empty() || tag.len() > 8 || !tag.bytes().all(|c| c.is_ascii_uppercase() || c == b':') {
        return None;
    }
    let ms = time::parse_str(ts, ctx)?;
    Some((ms, stream, tag, msg))
}

/// Meta for text of unknown format (Docker/CRI inner text).
fn auto_meta(text: &str, ctx: &TimeCtx) -> Meta {
    for f in [Format::Json, Format::Klog, Format::Logfmt, Format::Prefixed] {
        if let Some(m) = try_format(text, f, ctx) {
            return Meta { structured: true, ..m };
        }
    }
    Meta { ts: None, level: first_word_level(text).unwrap_or_default(), structured: true }
}

const DETECT_ORDER: [Format; 9] = [
    Format::Docker,
    Format::Cri,
    Format::Json,
    Format::Syslog5424,
    Format::AccessLog,
    Format::Syslog,
    Format::Klog,
    Format::Logfmt,
    Format::Prefixed,
];

/// Picks the format that parses most sample lines, and whether timestamps delimit entries.
pub fn detect<'a>(lines: impl IntoIterator<Item = &'a str>, ctx: &TimeCtx) -> Detection {
    let lines: Vec<&str> = lines.into_iter().filter(|l| !l.trim().is_empty()).take(500).collect();
    if lines.is_empty() {
        return Detection::default();
    }
    let heads: Vec<&str> = lines.iter().copied().filter(|l| !is_stack_line(l)).collect();
    let denom = heads.len().max(1);
    let mut best = (Format::Plain, 0usize);
    for f in DETECT_ORDER {
        let n = heads.iter().filter(|l| try_format(l, f, ctx).is_some()).count();
        if n > best.1 {
            best = (f, n);
        }
    }
    let format = match best.0 {
        Format::Prefixed if best.1 * 10 >= denom * 3 => Format::Prefixed,
        Format::Prefixed => Format::Plain,
        f if best.1 * 2 >= denom => f,
        _ => {
            // A minority structured format: fall back to the generic prefix parser if it works.
            let n = heads.iter().filter(|l| try_format(l, Format::Prefixed, ctx).is_some()).count();
            if n * 10 >= denom * 3 { Format::Prefixed } else { Format::Plain }
        }
    };
    let with_ts = heads.iter().filter(|l| line_meta(l, format, ctx).ts.is_some()).count();
    Detection { format, ts_mode: with_ts > 0 && with_ts * 2 >= denom }
}

/// Meta for one line under the detected format (falls back to other parsers per line).
pub fn line_meta(line: &str, format: Format, ctx: &TimeCtx) -> Meta {
    if let Some(m) = try_format(line, format, ctx) {
        return m;
    }
    if format != Format::Json && format != Format::Docker && line.trim_start().starts_with('{')
        && let Some((m, _)) = json_meta(line, ctx)
    {
        return m;
    }
    if !matches!(format, Format::Prefixed | Format::Plain)
        && let Some(m) = try_format(line, Format::Prefixed, ctx)
    {
        return m;
    }
    Meta { ts: None, level: first_word_level(line).filter(|_| !line.starts_with([' ', '\t'])).unwrap_or_default(), structured: false }
}

/// Whether `line` starts a new entry (vs continuing the previous one).
pub fn starts_entry(line: &str, meta: &Meta, ts_mode: bool) -> bool {
    if meta.structured || meta.ts.is_some() {
        return true;
    }
    if ts_mode {
        return false;
    }
    !line.trim().is_empty() && !is_stack_line(line)
}

// ---------- full parse ----------

fn put(fields: &mut Map<String, Value>, k: &str, v: Value) {
    fields.insert(k.to_string(), v);
}

fn put_str(fields: &mut Map<String, Value>, k: &str, v: &str) {
    if !v.is_empty() && v != "-" {
        fields.insert(k.to_string(), Value::String(v.to_string()));
    }
}

/// Embedded `k=v` pairs in free text become fields (lnav-style), unless they'd clobber one.
fn embedded_pairs(fields: &mut Map<String, Value>, msg: &str) {
    if !msg.contains('=') {
        return;
    }
    for (k, v) in kv_pairs(msg, false) {
        if !fields.contains_key(k) && !v.is_empty() {
            fields.insert(k.to_string(), scalar_value(&v));
        }
    }
}

fn value_str(v: &Value) -> Option<Cow<'_, str>> {
    match v {
        Value::String(s) => Some(Cow::Borrowed(s)),
        Value::Number(n) => Some(Cow::Owned(n.to_string())),
        _ => None,
    }
}

fn best_key<'a>(fields: &'a Map<String, Value>, keys: &[&str], accept: impl Fn(&Value) -> bool) -> Option<&'a Value> {
    let mut best: Option<(u8, &Value)> = None;
    for (k, v) in fields {
        if let Some(r) = key_rank(keys, k)
            && best.is_none_or(|(o, _)| o > r)
            && accept(v)
        {
            best = Some((r, v));
        }
    }
    best.map(|b| b.1).or_else(|| {
        // Dotted names (`trace.id`, `log.level`) through nested objects.
        keys.iter().filter(|k| k.contains('.')).find_map(|k| lookup(fields, k).filter(|v| accept(v)))
    })
}

fn map_fields(fields: &Map<String, Value>, ctx: &TimeCtx) -> (Option<i64>, Level, Option<String>, Option<String>) {
    let ts = best_key(fields, TS_KEYS, |v| matches!(v, Value::String(_) | Value::Number(_))).and_then(|v| match v {
        Value::String(s) => time::parse_str(s, ctx),
        Value::Number(n) => n.as_f64().and_then(time::from_epoch),
        _ => None,
    });
    let level_of = |v: &Value| match v {
        Value::String(s) => Level::parse(s).or_else(|| s.trim().parse::<f64>().ok().and_then(Level::from_number)),
        Value::Number(n) => n.as_f64().and_then(Level::from_number),
        _ => None,
    };
    let level = best_key(fields, LEVEL_KEYS, |v| level_of(v).is_some())
        .and_then(level_of)
        .or_else(|| lookup(fields, "log.level").and_then(level_of))
        .unwrap_or_default();
    let msg = best_key(fields, MSG_KEYS, |v| matches!(v, Value::String(_))).and_then(|v| v.as_str().map(str::to_string));
    let corr = best_key(fields, CORR_KEYS, |v| matches!(v, Value::String(_) | Value::Number(_)))
        .and_then(value_str)
        .map(|s| s.into_owned())
        .filter(|s| !s.is_empty());
    (ts, level, msg, corr)
}

fn parse_json_line(line: &str, ctx: &TimeCtx) -> Option<ParsedLine> {
    let t = line.trim();
    if !t.starts_with('{') {
        return None;
    }
    let mut fields: Map<String, Value> = serde_json::from_str(t).ok()?;
    if let (Some(Value::String(log)), true) = (fields.get("log"), fields.contains_key("stream")) {
        let inner_text = log.trim_end_matches(['\n', '\r']).to_string();
        let mut inner = parse_auto(&inner_text, ctx);
        let wrapper_ts = fields.get("time").and_then(|v| v.as_str()).and_then(|s| time::parse_str(s, ctx));
        if let Some(Value::String(stream)) = fields.remove("stream") {
            inner.fields.insert("stream".into(), Value::String(stream));
        }
        if let Some(attrs) = fields.remove("attrs") {
            inner.fields.insert("attrs".into(), attrs);
        }
        inner.ts = inner.ts.or(wrapper_ts);
        inner.format = Format::Docker;
        return Some(inner);
    }
    let (ts, level, msg, correlation_id) = map_fields(&fields, ctx);
    let message = msg.unwrap_or_else(|| {
        // No message key: show the remaining fields compactly.
        let rest: Map<String, Value> = fields
            .iter()
            .filter(|(k, _)| key_rank(TS_KEYS, k).is_none() && key_rank(LEVEL_KEYS, k).is_none())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        serde_json::to_string(&rest).unwrap_or_default()
    });
    Some(ParsedLine { ts, level, message, fields, format: Format::Json, correlation_id })
}

fn parse_logfmt_line(line: &str, ctx: &TimeCtx) -> Option<ParsedLine> {
    let pairs = logfmt_pairs(line)?;
    let fields: Map<String, Value> = pairs.iter().map(|(k, v)| (k.to_string(), scalar_value(v))).collect();
    let (ts, level, msg, correlation_id) = map_fields(&fields, ctx);
    let message = msg.unwrap_or_else(|| line.to_string());
    Some(ParsedLine { ts, level, message, fields, format: Format::Logfmt, correlation_id })
}

fn parse_access_line(line: &str, ctx: &TimeCtx) -> Option<ParsedLine> {
    let a = access(line, ctx)?;
    let mut f = Map::new();
    put_str(&mut f, "remote_addr", a.host);
    put_str(&mut f, "ident", a.ident);
    put_str(&mut f, "remote_user", a.user);
    let mut parts = a.request.splitn(3, ' ');
    let (method, path, proto) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    put_str(&mut f, "method", method);
    put_str(&mut f, "path", path);
    put_str(&mut f, "protocol", proto);
    if let Some(s) = a.status {
        put(&mut f, "status", Value::from(s));
    }
    if let Some(b) = a.bytes {
        put(&mut f, "bytes", Value::from(b));
    }
    if let Some(r) = a.referer {
        put_str(&mut f, "referer", r);
    }
    if let Some(u) = a.agent {
        put_str(&mut f, "user_agent", u);
    }
    let rest = a.rest.trim();
    if !rest.is_empty() {
        embedded_pairs(&mut f, rest);
        if !rest.contains('=') {
            put_str(&mut f, "extra", rest);
        }
    }
    let message = match a.status {
        Some(s) => format!("{} {s}", a.request),
        None => a.request.to_string(),
    };
    Some(ParsedLine { ts: a.ts, level: status_level(a.status), message, fields: f, format: Format::AccessLog, correlation_id: None })
}

fn parse_syslog_line(line: &str, ctx: &TimeCtx) -> Option<ParsedLine> {
    let s = syslog3164(line, ctx)?;
    let mut f = Map::new();
    put_str(&mut f, "host", s.host);
    put_str(&mut f, "app", s.app);
    if let Some(pid) = s.pid {
        put(&mut f, "pid", scalar_value(pid));
    }
    if let Some(p) = s.pri {
        put(&mut f, "facility", Value::from(p / 8));
        put(&mut f, "severity", Value::from(p % 8));
    }
    // The message may itself carry a level prefix or k=v pairs.
    let inner = prefixed(s.msg, ctx);
    let level = s.pri.map(|p| Level::from_syslog_severity(p % 8)).or_else(|| inner.as_ref().and_then(|p| p.level)).unwrap_or_default();
    embedded_pairs(&mut f, s.msg);
    Some(ParsedLine { ts: s.ts, level, message: s.msg.to_string(), fields: f, format: Format::Syslog, correlation_id: None })
}

fn parse_syslog5424_line(line: &str, ctx: &TimeCtx) -> Option<ParsedLine> {
    let s = syslog5424(line, ctx)?;
    let mut f = Map::new();
    put_str(&mut f, "host", s.host);
    put_str(&mut f, "app", s.app);
    put_str(&mut f, "procid", s.procid);
    put_str(&mut f, "msgid", s.msgid);
    put(&mut f, "facility", Value::from(s.pri / 8));
    put(&mut f, "severity", Value::from(s.pri % 8));
    if !s.sd.is_empty() {
        put(&mut f, "sd", Value::Object(sd_fields(s.sd)));
    }
    embedded_pairs(&mut f, s.msg);
    let correlation_id = best_key(&f, CORR_KEYS, |v| v.is_string()).and_then(|v| v.as_str().map(str::to_string));
    Some(ParsedLine {
        ts: s.ts,
        level: Level::from_syslog_severity(s.pri % 8),
        message: s.msg.to_string(),
        fields: f,
        format: Format::Syslog5424,
        correlation_id,
    })
}

fn parse_klog_line(line: &str, ctx: &TimeCtx) -> Option<ParsedLine> {
    let (level, ts, caller, msg) = klog(line, ctx)?;
    let mut f = Map::new();
    put_str(&mut f, "caller", caller);
    embedded_pairs(&mut f, msg);
    Some(ParsedLine { ts, level, message: msg.to_string(), fields: f, format: Format::Klog, correlation_id: None })
}

fn parse_prefixed_line(line: &str, ctx: &TimeCtx) -> Option<ParsedLine> {
    let p = prefixed(line, ctx)?;
    let mut f = Map::new();
    if let Some(t) = p.target {
        put_str(&mut f, "target", t);
    }
    embedded_pairs(&mut f, p.msg);
    let correlation_id = best_key(&f, CORR_KEYS, |v| matches!(v, Value::String(_) | Value::Number(_))).and_then(value_str).map(|s| s.into_owned());
    Some(ParsedLine {
        ts: p.ts,
        level: p.level.unwrap_or_default(),
        message: p.msg.to_string(),
        fields: f,
        format: Format::Prefixed,
        correlation_id,
    })
}

fn parse_cri_line(line: &str, ctx: &TimeCtx) -> Option<ParsedLine> {
    let (ts, stream, tag, msg) = cri(line, ctx)?;
    let mut inner = parse_auto(msg, ctx);
    inner.ts = inner.ts.or(Some(ts));
    inner.fields.insert("stream".into(), Value::String(stream.to_string()));
    if tag == "P" {
        inner.fields.insert("partial".into(), Value::Bool(true));
    }
    inner.format = Format::Cri;
    Some(inner)
}

fn plain(line: &str) -> ParsedLine {
    let mut fields = Map::new();
    embedded_pairs(&mut fields, line);
    ParsedLine {
        ts: None,
        level: first_word_level(line).filter(|_| !line.starts_with([' ', '\t'])).unwrap_or_default(),
        message: line.to_string(),
        fields,
        format: Format::Plain,
        correlation_id: None,
    }
}

fn parse_as(line: &str, f: Format, ctx: &TimeCtx) -> Option<ParsedLine> {
    match f {
        Format::Json | Format::Docker => parse_json_line(line, ctx),
        Format::Cri => parse_cri_line(line, ctx),
        Format::Logfmt => parse_logfmt_line(line, ctx),
        Format::AccessLog => parse_access_line(line, ctx),
        Format::Syslog => parse_syslog_line(line, ctx),
        Format::Syslog5424 => parse_syslog5424_line(line, ctx),
        Format::Klog => parse_klog_line(line, ctx),
        Format::Prefixed => parse_prefixed_line(line, ctx),
        Format::Plain => Some(plain(line)),
    }
}

/// Text of unknown format (e.g. inside a Docker wrapper).
pub fn parse_auto(text: &str, ctx: &TimeCtx) -> ParsedLine {
    for f in [Format::Json, Format::Klog, Format::Logfmt, Format::Prefixed] {
        if let Some(p) = parse_as(text, f, ctx) {
            return p;
        }
    }
    plain(text)
}

/// Full parse of one physical line under the detected format, with per-line fallbacks.
pub fn parse_line(line: &str, format: Format, ctx: &TimeCtx) -> ParsedLine {
    if let Some(p) = parse_as(line, format, ctx) {
        return p;
    }
    if line.trim_start().starts_with('{')
        && let Some(p) = parse_json_line(line, ctx)
    {
        return p;
    }
    if !matches!(format, Format::Prefixed | Format::Plain)
        && let Some(p) = parse_prefixed_line(line, ctx)
    {
        return p;
    }
    plain(line)
}

/// Builds an entry from its physical lines (first line + continuations).
pub fn build_entry(text: &str, index: u64, line_no: u64, format: Format, ctx: &TimeCtx) -> LogEntry {
    let mut lines = text.split('\n');
    let first = lines.next().unwrap_or("").trim_end_matches('\r');
    let p = parse_line(first, format, ctx);
    let mut message = p.message;
    let mut count = 1u32;
    for l in lines {
        message.push('\n');
        message.push_str(l.trim_end_matches('\r'));
        count += 1;
    }
    LogEntry {
        index,
        line_no,
        line_count: count,
        ts: p.ts,
        level: p.level,
        message,
        fields: p.fields,
        raw: text.to_string(),
        format: p.format,
        correlation_id: p.correlation_id,
        source: None,
    }
}

/// Splits a whole text into entries (for small inputs: tests, pasted text, event data).
pub fn parse_text(text: &str, ctx: &TimeCtx) -> (Detection, Vec<LogEntry>) {
    let lines: Vec<&str> = text.lines().collect();
    let det = detect(lines.iter().copied(), ctx);
    let mut out: Vec<LogEntry> = Vec::new();
    let mut start: Option<usize> = None;
    let flush = |out: &mut Vec<LogEntry>, s: usize, e: usize| {
        let body = lines[s..e].join("\n");
        let idx = out.len() as u64;
        out.push(build_entry(&body, idx, s as u64 + 1, det.format, ctx));
    };
    for (i, l) in lines.iter().enumerate() {
        let m = line_meta(l, det.format, ctx);
        if start.is_none() || starts_entry(l, &m, det.ts_mode) {
            if let Some(s) = start {
                flush(&mut out, s, i);
            }
            start = Some(i);
        }
    }
    if let Some(s) = start {
        flush(&mut out, s, lines.len());
    }
    (det, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{NaiveTz, format_iso};

    fn ctx() -> TimeCtx {
        TimeCtx::new(NaiveTz::Utc, 2024)
    }

    fn one(line: &str) -> ParsedLine {
        let c = ctx();
        let d = detect([line], &c);
        parse_line(line, d.format, &c)
    }

    #[test]
    fn levels() {
        assert_eq!(Level::parse("WARNING"), Some(Level::Warn));
        assert_eq!(Level::parse("Information"), Some(Level::Info));
        assert_eq!(Level::parse("crit"), Some(Level::Fatal));
        assert_eq!(Level::parse("nope"), None);
        assert_eq!(Level::from_number(30.0), Some(Level::Info));
        assert_eq!(Level::from_number(3.0), Some(Level::Error));
        assert!(Level::Warn.rank() > Level::Info.rank());
        assert_eq!(Level::Unknown.rank(), None);
    }

    #[test]
    fn json_lines() {
        let c = ctx();
        let l = r#"{"ts":"2024-01-02T03:04:05Z","level":"error","msg":"boom","status":500,"user":{"id":42},"traceId":"abc123"}"#;
        assert_eq!(detect([l], &c).format, Format::Json);
        let p = one(l);
        assert_eq!(p.format, Format::Json);
        assert_eq!(p.level, Level::Error);
        assert_eq!(p.message, "boom");
        assert_eq!(format_iso(p.ts.unwrap()), "2024-01-02T03:04:05.000Z");
        assert_eq!(p.correlation_id.as_deref(), Some("abc123"));
        assert_eq!(lookup(&p.fields, "user.id"), Some(&Value::from(42)));
        // Meta agrees with the full parse.
        let m = line_meta(l, Format::Json, &c);
        assert_eq!((m.ts, m.level, m.structured), (p.ts, p.level, true));

        // pino: numeric level + epoch ms; ECS: @timestamp + log.level; Serilog CLEF.
        let pino = one(r#"{"level":50,"time":1704164645123,"msg":"pino"}"#);
        assert_eq!((pino.level, pino.ts), (Level::Error, Some(1_704_164_645_123)));
        let ecs = one(r#"{"@timestamp":"2024-01-02T03:04:05.000Z","log":{"level":"warn"},"message":"ecs","trace":{"id":"t-1"}}"#);
        assert_eq!((ecs.level, ecs.message.as_str(), ecs.correlation_id.as_deref()), (Level::Warn, "ecs", Some("t-1")));
        assert_eq!(line_meta(r#"{"@timestamp":"2024-01-02T03:04:05.000Z","log":{"level":"warn"}}"#, Format::Json, &c).level, Level::Warn);
        let clef = one(r#"{"@t":"2024-01-02T03:04:05Z","@mt":"Hello {User}","@l":"Warning","User":"ann"}"#);
        assert_eq!((clef.level, clef.message.as_str()), (Level::Warn, "Hello {User}"));
    }

    #[test]
    fn docker_and_cri() {
        let c = ctx();
        let l = r#"{"log":"2024-01-02 03:04:05,123 ERROR [main] boom\n","stream":"stderr","time":"2024-01-02T03:04:06.000000000Z"}"#;
        assert_eq!(detect([l], &c).format, Format::Docker);
        let p = one(l);
        assert_eq!(p.format, Format::Docker);
        assert_eq!(p.level, Level::Error);
        assert_eq!(p.fields["stream"], "stderr");
        assert_eq!(format_iso(p.ts.unwrap()), "2024-01-02T03:04:05.123Z");
        let m = line_meta(l, Format::Docker, &c);
        assert_eq!((m.ts, m.level), (p.ts, Level::Error));

        let inner_json = r#"{"log":"{\"level\":\"info\",\"msg\":\"hi\"}\n","stream":"stdout","time":"2024-01-02T03:04:06Z"}"#;
        let p = one(inner_json);
        assert_eq!((p.level, p.message.as_str()), (Level::Info, "hi"));
        assert_eq!(format_iso(p.ts.unwrap()), "2024-01-02T03:04:06.000Z");

        let cri_line = "2024-01-02T03:04:05.669794202Z stdout F level=warn msg=\"slow query\" took=2.5s";
        assert_eq!(detect([cri_line], &c).format, Format::Cri);
        let p = one(cri_line);
        assert_eq!((p.format, p.level, p.message.as_str()), (Format::Cri, Level::Warn, "slow query"));
        assert_eq!(p.fields["stream"], "stdout");
    }

    #[test]
    fn logfmt() {
        let l = r#"time=2024-01-02T03:04:05Z level=info msg="request done" status=200 dur=1.5 path=/api/x ok"#;
        assert_eq!(detect([l], &ctx()).format, Format::Logfmt);
        let p = one(l);
        assert_eq!((p.level, p.message.as_str()), (Level::Info, "request done"));
        assert_eq!(p.fields["status"], 200);
        assert_eq!(p.fields["dur"], 1.5);
        assert_eq!(p.fields["path"], "/api/x");
        assert!(logfmt_pairs("hello world").is_none());
        assert!(logfmt_pairs("2024-01-02 a=b c=d").is_none());
    }

    #[test]
    fn access_logs() {
        let l = r#"127.0.0.1 - frank [10/Oct/2000:13:55:36 -0700] "GET /apache_pb.gif HTTP/1.0" 503 2326 "http://ref/" "Mozilla/5.0 (X11)""#;
        assert_eq!(detect([l], &ctx()).format, Format::AccessLog);
        let p = one(l);
        assert_eq!(p.level, Level::Error);
        assert_eq!(p.fields["status"], 503);
        assert_eq!(p.fields["method"], "GET");
        assert_eq!(p.fields["path"], "/apache_pb.gif");
        assert_eq!(p.fields["user_agent"], "Mozilla/5.0 (X11)");
        assert_eq!(p.ts, Some(971_211_336_000));
        let common = one(r#"10.0.0.1 - - [10/Oct/2000:13:55:36 +0000] "POST /x HTTP/1.1" 404 -"#);
        assert_eq!((common.level, common.fields.get("bytes")), (Level::Warn, None));
    }

    #[test]
    fn syslog() {
        let l = "<34>Oct 11 22:14:15 mymachine su[123]: 'su root' failed for lonvick on /dev/pts/8";
        let p = one(l);
        assert_eq!(p.format, Format::Syslog);
        assert_eq!(p.level, Level::Fatal); // severity 2 (crit)
        assert_eq!(p.fields["app"], "su");
        assert_eq!(p.fields["pid"], 123);
        assert_eq!(p.message, "'su root' failed for lonvick on /dev/pts/8");
        let no_pri = one("Oct 11 22:14:15 host sshd: error: auth failed");
        assert_eq!((no_pri.format, no_pri.level), (Format::Syslog, Level::Error));

        let l5 = r#"<165>1 2003-10-11T22:14:15.003Z mymachine.example.com evntslog - ID47 [exampleSDID@32473 iut="3" eventSource="Application"] An application event"#;
        let p = one(l5);
        assert_eq!(p.format, Format::Syslog5424);
        assert_eq!(p.level, Level::Info); // severity 5 (notice)
        assert_eq!(p.message, "An application event");
        assert_eq!(p.fields["sd"]["exampleSDID@32473"]["eventSource"], "Application");
        assert_eq!(p.fields["app"], "evntslog");
    }

    #[test]
    fn prefixed_variants() {
        let cases = [
            ("2024-01-02 03:04:05,123 INFO [main] c.e.App - started", Level::Info, "[main] c.e.App - started"),
            ("2024-01-02 03:04:05,123 [main] WARN c.e.App - slow", Level::Warn, "[main] WARN c.e.App - slow"),
            ("2024-01-02 03:04:05,123 - app.db - ERROR - lost connection", Level::Error, "app.db - ERROR - lost connection"),
            ("[2024-01-02T03:04:05Z INFO  my_crate] hello", Level::Info, "hello"),
            ("2024-01-02T03:04:05.123456Z  WARN my_crate::db: slow query", Level::Warn, "my_crate::db: slow query"),
            ("2024/01/02 03:04:05 [error] 1234#0: *5 connect() failed", Level::Error, "1234#0: *5 connect() failed"),
            ("[2024-01-02 03:04:05] local.ERROR: Laravel says no", Level::Error, "Laravel says no"),
        ];
        for (line, level, msg) in cases {
            let p = one(line);
            assert_eq!(p.format, Format::Prefixed, "{line}");
            assert_eq!(p.level, level, "{line}");
            assert_eq!(p.message, msg, "{line}");
            assert_eq!(format_iso(p.ts.unwrap())[..19], *"2024-01-02T03:04:05", "{line}");
        }
        let env_logger = one("[2024-01-02T03:04:05Z INFO  my_crate] hello");
        assert_eq!(env_logger.fields["target"], "my_crate");
        let apache = one("[Wed Oct 11 14:32:52.123456 2000] [core:error] [pid 35708] AH00124: Request exceeded the limit");
        assert_eq!(apache.level, Level::Error);
        let serilog = one("[03:04:05 INF] Started host");
        assert_eq!((serilog.level, serilog.ts, serilog.message.as_str()), (Level::Info, None, "Started host"));
        let kv = one("2024-01-02T03:04:05Z INFO handled request_id=r-77 status=201 path=/users");
        assert_eq!(kv.fields["status"], 201);
        assert_eq!(kv.correlation_id.as_deref(), Some("r-77"));
        let klog = one("E0102 03:04:05.123456       1 reflector.go:138] Failed to watch *v1.Pod: unauthorized");
        assert_eq!((klog.format, klog.level), (Format::Klog, Level::Error));
        assert_eq!(klog.fields["caller"], "reflector.go:138");
        assert_eq!(format_iso(klog.ts.unwrap()), "2024-01-02T03:04:05.123Z");
        let plain = one("just some words");
        assert_eq!((plain.format, plain.level), (Format::Plain, Level::Unknown));
    }

    #[test]
    fn stack_traces_group() {
        let c = ctx();
        let java = "2024-01-02 03:04:05,123 ERROR [main] c.e.App - failed\njava.lang.IllegalStateException: boom\n\tat com.e.App.run(App.java:10)\n\tat com.e.App.main(App.java:5)\nCaused by: java.io.IOException: disk\n\t... 2 more\n2024-01-02 03:04:06,000 INFO [main] c.e.App - recovered";
        let (det, entries) = parse_text(java, &c);
        assert!(det.ts_mode);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].line_count, 6);
        assert_eq!(entries[0].level, Level::Error);
        assert!(entries[0].message.contains("Caused by"));
        assert_eq!(entries[1].line_no, 7);

        let python = "2024-01-02 03:04:05,123 ERROR app: crashed\nTraceback (most recent call last):\n  File \"x.py\", line 1, in <module>\n    main()\nValueError: bad\n2024-01-02 03:04:06,123 INFO app: ok";
        assert_eq!(parse_text(python, &c).1.iter().map(|e| e.line_count).collect::<Vec<_>>(), vec![5, 1]);

        let js = "{\"level\":\"error\",\"msg\":\"unhandled\"}\nTypeError: x is not a function\n    at Object.<anonymous> (/app/x.js:1:7)\n    at Module._compile (node:internal/modules/cjs/loader:1256:14)\n{\"level\":\"info\",\"msg\":\"next\"}";
        assert_eq!(parse_text(js, &c).1.iter().map(|e| e.line_count).collect::<Vec<_>>(), vec![4, 1]);

        let go = "panic: runtime error: index out of range\n\ngoroutine 1 [running]:\nmain.main()\n\t/app/main.go:5 +0x1d\nexit status 2";
        let (_, e) = parse_text(go, &c);
        assert_eq!(e.iter().map(|e| e.line_count).collect::<Vec<_>>(), vec![5, 1]);

        let rust = "thread 'main' panicked at src/main.rs:2:5:\nexplicit panic\nstack backtrace:\n   0: rust_begin_unwind\n   1: core::panicking::panic_fmt\nnote: run with `RUST_BACKTRACE=full` for a verbose backtrace.";
        let (_, e) = parse_text(rust, &c);
        assert_eq!(e.iter().map(|e| e.line_count).collect::<Vec<_>>(), vec![1, 5]);

        let dotnet = "fail: Microsoft.AspNetCore.Server.Kestrel[13]\n      Connection id \"0HM\" request failed\n      System.InvalidOperationException: nope\n         at App.Controllers.Home.Index() in C:\\src\\Home.cs:line 42\ninfo: Microsoft.Hosting.Lifetime[0]\n      Application started.";
        let (_, e) = parse_text(dotnet, &c);
        assert_eq!(e.iter().map(|e| (e.line_count, e.level)).collect::<Vec<_>>(), vec![(4, Level::Error), (2, Level::Info)]);
    }
}
