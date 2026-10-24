//! Rewrite rules: ordered, individually enabled, matched on method / host / path / header, each
//! with one or more actions.
//!
//! Evaluation: every enabled rule that matches contributes, in order. Header edits accumulate;
//! the first Map Remote, the first terminal action (Map Local, Mock response, Block), the first
//! Throttle and the first Validate against OpenAPI win; breakpoints apply if any matching rule
//! asks for them. A terminal action means the upstream is never contacted.
//!
//! *Validate against OpenAPI* loads the spec once when the rules are installed and checks each
//! matching exchange with `devall-openapi` (undocumented path/method/status, request and
//! response bodies against their schemas): it flags the exchange (badge + findings), or in
//! `block` mode answers a non-conforming request with `422` (never forwarded) and replaces a
//! non-conforming response with `502`, both as `application/problem+json`.
//!
//! Rules are per user (`<app data>/proxy/rules.json`) and export/import as the same JSON:
//!
//! ```json
//! { "version": 1, "rules": [ {
//!     "id": "01J…", "name": "API to localhost", "enabled": true,
//!     "match": { "methods": ["GET"], "host": "api.example.com", "path": "/v1/*" },
//!     "actions": [ { "type": "mapRemote", "scheme": "http", "host": "localhost", "port": 3000 } ]
//! } ] }
//! ```

use std::path::Path;
use std::sync::Arc;

use devall_openapi::Conformance;
use http::HeaderMap;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{IoContext, ProxyError, Result};
use crate::util::glob_regex;

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn ok_status() -> u16 {
    200
}
fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesFile {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, rename = "match")]
    pub matcher: RuleMatch,
    #[serde(default)]
    pub actions: Vec<RuleAction>,
}

/// All set conditions must hold. An empty match matches everything.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuleMatch {
    /// Any of these methods (empty = any).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    /// Glob over the host (`*.example.com`), case-insensitive; with `:port` it also matches the
    /// port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Glob over the path without the query (`/api/*/users`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Regex searched in the path including the query (anchor it with `^…$` if needed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_regex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<HeaderMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderMatch {
    pub name: String,
    /// Glob over the value; absent = the header just has to be present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameValue {
    pub name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HeaderTarget {
    #[default]
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockMode {
    /// Answer `403 Forbidden`.
    #[default]
    Forbidden,
    /// Close the connection without a response.
    Close,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidateMode {
    /// Badge the exchange and list the findings.
    #[default]
    Flag,
    /// Also answer non-conforming requests with `422` (not forwarded) and replace
    /// non-conforming responses with `502`.
    Block,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RuleAction {
    /// Pause matching requests and/or responses for editing.
    Breakpoint {
        #[serde(default = "yes")]
        request: bool,
        #[serde(default)]
        response: bool,
    },
    /// Serve a local file (or a directory, joined with the request path) or an inline body.
    MapLocal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default = "ok_status")]
        status: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        headers: Vec<NameValue>,
    },
    /// Send the request somewhere else (e.g. production → localhost).
    MapRemote {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scheme: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        /// Replace this path prefix…
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path_from: Option<String>,
        /// …with this one (without `pathFrom`: prepended).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path_to: Option<String>,
        /// Keep the original `Host` header instead of the new host.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        preserve_host: bool,
    },
    /// Answer without contacting the server.
    MockResponse {
        #[serde(default = "ok_status")]
        status: u16,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        headers: Vec<NameValue>,
        #[serde(default)]
        body: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        delay_ms: u64,
    },
    SetHeader {
        #[serde(default)]
        target: HeaderTarget,
        name: String,
        #[serde(default)]
        value: String,
    },
    RemoveHeader {
        #[serde(default)]
        target: HeaderTarget,
        name: String,
    },
    /// Add latency before forwarding and/or cap the download rate (kilobytes per second).
    Throttle {
        #[serde(default, skip_serializing_if = "is_zero")]
        latency_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kbps: Option<u64>,
    },
    Block {
        #[serde(default)]
        mode: BlockMode,
    },
    /// Check matching traffic against an OpenAPI 3.x spec (absolute path, usually a file in the
    /// project).
    ValidateSpec {
        spec: String,
        #[serde(default)]
        mode: ValidateMode,
        /// Check every matching request, not only those to the spec's `servers` hosts.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_hosts: bool,
    },
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl RuleAction {
    pub fn kind(&self) -> &'static str {
        match self {
            RuleAction::Breakpoint { .. } => "breakpoint",
            RuleAction::MapLocal { .. } => "mapLocal",
            RuleAction::MapRemote { .. } => "mapRemote",
            RuleAction::MockResponse { .. } => "mockResponse",
            RuleAction::SetHeader { .. } => "setHeader",
            RuleAction::RemoveHeader { .. } => "removeHeader",
            RuleAction::Throttle { .. } => "throttle",
            RuleAction::Block { .. } => "block",
            RuleAction::ValidateSpec { .. } => "validateSpec",
        }
    }
}

/// Target of a Map Remote rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapRemote {
    pub scheme: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub path_from: Option<String>,
    pub path_to: Option<String>,
    pub preserve_host: bool,
}

/// `scheme`, `host`, `port`, `path?query` of a request target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
}

impl Target {
    pub fn path(&self) -> &str {
        self.path_and_query.split('?').next().unwrap_or("/")
    }

    pub fn authority(&self) -> String {
        crate::util::authority_for(&self.scheme, &self.host, self.port)
    }

    pub fn url(&self) -> String {
        format!("{}://{}{}", self.scheme, self.authority(), self.path_and_query)
    }

    pub fn parse(url: &str) -> Option<Target> {
        let (scheme, authority, path) = crate::model::split_url(url);
        if scheme.is_empty() || authority.is_empty() {
            return None;
        }
        let default = if scheme == "https" || scheme == "wss" { 443 } else { 80 };
        let (host, port) = crate::util::split_host_port(&authority, default);
        Some(Target { scheme, host, port, path_and_query: if path.starts_with('/') { path } else { format!("/{path}") } })
    }
}

fn default_port(scheme: &str) -> u16 {
    if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") { 443 } else { 80 }
}

impl MapRemote {
    pub fn apply(&self, t: &Target) -> Target {
        let mut out = t.clone();
        if let Some(s) = self.scheme.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let s = s.to_ascii_lowercase();
            if self.port.is_none() && t.port == default_port(&t.scheme) {
                out.port = default_port(&s);
            }
            out.scheme = s;
        }
        if let Some(h) = self.host.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
            out.host = h.to_string();
        }
        if let Some(p) = self.port {
            out.port = p;
        }
        let (path, query) = match t.path_and_query.split_once('?') {
            Some((p, q)) => (p.to_string(), Some(q.to_string())),
            None => (t.path_and_query.clone(), None),
        };
        let to = self.path_to.clone().unwrap_or_default();
        let new_path = match self.path_from.as_deref().filter(|f| !f.is_empty()) {
            Some(from) => match path.strip_prefix(from) {
                Some(rest) => join_path(&to, rest),
                None => path,
            },
            None if !to.is_empty() => join_path(&to, &path),
            None => path,
        };
        let new_path = if new_path.starts_with('/') { new_path } else { format!("/{new_path}") };
        out.path_and_query = match query {
            Some(q) => format!("{new_path}?{q}"),
            None => new_path,
        };
        out
    }
}

fn join_path(prefix: &str, rest: &str) -> String {
    if rest.is_empty() {
        return if prefix.is_empty() { "/".into() } else { prefix.to_string() };
    }
    match (prefix.ends_with('/'), rest.starts_with('/')) {
        (true, true) => format!("{prefix}{}", &rest[1..]),
        (false, false) if !prefix.is_empty() => format!("{prefix}/{rest}"),
        _ => format!("{prefix}{rest}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeaderOp {
    Set(String, String),
    Remove(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Terminal {
    /// `prefix`: the literal start of the rule's path glob, stripped before joining a folder.
    MapLocal {
        file: Option<String>,
        body: Option<String>,
        status: u16,
        content_type: Option<String>,
        headers: Vec<NameValue>,
        prefix: String,
    },
    Mock {
        status: u16,
        headers: Vec<NameValue>,
        body: String,
        delay_ms: u64,
    },
    Block(BlockMode),
}

/// A compiled "Validate against OpenAPI" action.
#[derive(Debug, Clone)]
pub(crate) struct SpecCheck {
    pub rule: String,
    /// File name of the spec, for display.
    pub spec_name: String,
    pub conformance: Arc<Conformance>,
    pub mode: ValidateMode,
}

impl PartialEq for SpecCheck {
    fn eq(&self, other: &Self) -> bool {
        self.rule == other.rule && self.mode == other.mode && Arc::ptr_eq(&self.conformance, &other.conformance)
    }
}

/// What the matching rules ask for one request.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Plan {
    pub matched: Vec<String>,
    pub req_headers: Vec<HeaderOp>,
    pub resp_headers: Vec<HeaderOp>,
    pub map_remote: Option<MapRemote>,
    pub terminal: Option<Terminal>,
    pub bp_request: bool,
    pub bp_response: bool,
    pub latency_ms: u64,
    pub bytes_per_sec: Option<u64>,
    pub validate: Option<SpecCheck>,
}

pub(crate) struct RuleInput<'a> {
    pub method: &'a str,
    pub target: &'a Target,
    pub headers: &'a HeaderMap,
}

#[derive(Debug)]
struct CompiledRule {
    name: String,
    methods: Vec<String>,
    host: Option<(Regex, bool)>,
    path: Option<Regex>,
    /// Literal start of the path glob up to its last `/` (`/static/*` → `/static/`).
    path_prefix: String,
    path_regex: Option<Regex>,
    header: Option<(String, Option<Regex>)>,
    actions: Vec<RuleAction>,
    /// The spec of the first Validate action, parsed once.
    validate: Option<SpecCheck>,
}

fn literal_prefix(glob: &str) -> String {
    let lit = &glob[..glob.find(['*', '?']).unwrap_or(glob.len())];
    lit.rfind('/').map(|i| lit[..=i].to_string()).unwrap_or_default()
}

/// Enabled rules, validated and with their patterns compiled.
#[derive(Debug, Default)]
pub(crate) struct CompiledRules {
    rules: Vec<CompiledRule>,
}

fn invalid(rule: &Rule, message: impl Into<String>) -> ProxyError {
    ProxyError::InvalidRule { rule: if rule.name.trim().is_empty() { rule.id.clone() } else { rule.name.clone() }, message: message.into() }
}

fn compile_rule(rule: &Rule) -> Result<CompiledRule> {
    let m = &rule.matcher;
    let mut validate: Option<SpecCheck> = None;
    let opt = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let host = match opt(&m.host) {
        Some(h) => Some((glob_regex(&h, true).map_err(|e| invalid(rule, format!("host pattern: {e}")))?, h.contains(':'))),
        None => None,
    };
    let path = match opt(&m.path) {
        Some(p) => Some(glob_regex(&p, false).map_err(|e| invalid(rule, format!("path pattern: {e}")))?),
        None => None,
    };
    let path_regex = match opt(&m.path_regex) {
        Some(p) => Some(Regex::new(&p).map_err(|e| invalid(rule, format!("path regex: {e}")))?),
        None => None,
    };
    let header = match &m.header {
        Some(h) if !h.name.trim().is_empty() => {
            let v = match h.value.as_deref().filter(|v| !v.is_empty()) {
                Some(v) => Some(glob_regex(v, false).map_err(|e| invalid(rule, format!("header value: {e}")))?),
                None => None,
            };
            Some((h.name.trim().to_ascii_lowercase(), v))
        }
        _ => None,
    };
    for a in &rule.actions {
        match a {
            RuleAction::MapLocal { file, body, status, .. } => {
                if file.as_deref().is_none_or(|f| f.trim().is_empty()) && body.is_none() {
                    return Err(invalid(rule, "Map Local needs a file or an inline body"));
                }
                if let Some(f) = file.as_deref().filter(|f| !f.trim().is_empty())
                    && !Path::new(f.trim()).is_absolute()
                {
                    return Err(invalid(rule, "Map Local file must be an absolute path"));
                }
                check_status(rule, *status)?;
            }
            RuleAction::MockResponse { status, .. } => check_status(rule, *status)?,
            RuleAction::MapRemote { scheme, host, port, path_from, path_to, .. } => {
                if let Some(s) = scheme.as_deref().filter(|s| !s.is_empty())
                    && !s.eq_ignore_ascii_case("http")
                    && !s.eq_ignore_ascii_case("https")
                {
                    return Err(invalid(rule, "Map Remote scheme must be http or https"));
                }
                if scheme.is_none() && host.as_deref().is_none_or(str::is_empty) && port.is_none() && path_from.is_none() && path_to.is_none() {
                    return Err(invalid(rule, "Map Remote needs a scheme, host, port or path"));
                }
            }
            RuleAction::SetHeader { name, value, .. } => {
                http::HeaderName::from_bytes(name.trim().as_bytes()).map_err(|_| invalid(rule, format!("invalid header name {name:?}")))?;
                http::HeaderValue::from_str(value).map_err(|_| invalid(rule, format!("invalid value for header {name}")))?;
            }
            RuleAction::RemoveHeader { name, .. } if name.trim().is_empty() => {
                return Err(invalid(rule, "header name is empty"));
            }
            RuleAction::ValidateSpec { spec, mode, all_hosts } if validate.is_none() => {
                let path = Path::new(spec.trim());
                if spec.trim().is_empty() || !path.is_absolute() {
                    return Err(invalid(rule, "Validate against OpenAPI needs the absolute path of a spec file"));
                }
                let parsed = devall_openapi::Spec::load(path).map_err(|e| invalid(rule, format!("spec {}: {e}", path.display())))?;
                validate = Some(SpecCheck {
                    rule: if rule.name.trim().is_empty() { "Unnamed rule".into() } else { rule.name.clone() },
                    spec_name: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    conformance: Arc::new(Conformance::new(parsed, *all_hosts)),
                    mode: *mode,
                });
            }
            _ => {}
        }
    }
    Ok(CompiledRule {
        name: if rule.name.trim().is_empty() { "Unnamed rule".into() } else { rule.name.clone() },
        methods: m.methods.iter().map(|s| s.trim().to_ascii_uppercase()).filter(|s| !s.is_empty() && s != "ANY" && s != "*").collect(),
        host,
        path_prefix: opt(&m.path).map(|p| literal_prefix(&p)).unwrap_or_default(),
        path,
        path_regex,
        header,
        actions: rule.actions.clone(),
        validate,
    })
}

fn check_status(rule: &Rule, status: u16) -> Result<()> {
    if (100..=999).contains(&status) { Ok(()) } else { Err(invalid(rule, format!("invalid status {status}"))) }
}

/// Validates every rule (enabled or not) and returns the compiled enabled ones.
pub(crate) fn compile(rules: &[Rule]) -> Result<CompiledRules> {
    let mut out = Vec::new();
    for r in rules {
        let c = compile_rule(r)?;
        if r.enabled {
            out.push(c);
        }
    }
    Ok(CompiledRules { rules: out })
}

/// Checks rules without installing them (for the editor).
pub fn validate(rules: &[Rule]) -> Result<()> {
    compile(rules).map(|_| ())
}

impl CompiledRule {
    fn matches(&self, input: &RuleInput<'_>) -> bool {
        if !self.methods.is_empty() && !self.methods.iter().any(|m| m.eq_ignore_ascii_case(input.method)) {
            return false;
        }
        if let Some((re, with_port)) = &self.host {
            let t = input.target;
            let ok = if *with_port { re.is_match(&format!("{}:{}", t.host, t.port)) } else { re.is_match(&t.host) };
            if !ok {
                return false;
            }
        }
        if let Some(re) = &self.path
            && !re.is_match(input.target.path())
        {
            return false;
        }
        if let Some(re) = &self.path_regex
            && !re.is_match(&input.target.path_and_query)
        {
            return false;
        }
        if let Some((name, value)) = &self.header {
            let vals: Vec<&str> = input.headers.get_all(name.as_str()).iter().filter_map(|v| v.to_str().ok()).collect();
            if vals.is_empty() {
                return false;
            }
            if let Some(re) = value
                && !vals.iter().any(|v| re.is_match(v))
            {
                return false;
            }
        }
        true
    }
}

impl CompiledRules {
    pub fn plan(&self, input: &RuleInput<'_>) -> Plan {
        let mut plan = Plan::default();
        let mut throttled = false;
        for rule in self.rules.iter().filter(|r| r.matches(input)) {
            plan.matched.push(rule.name.clone());
            for a in &rule.actions {
                match a {
                    RuleAction::Breakpoint { request, response } => {
                        plan.bp_request |= *request;
                        plan.bp_response |= *response;
                    }
                    RuleAction::MapLocal { file, body, status, content_type, headers } => {
                        plan.terminal.get_or_insert_with(|| Terminal::MapLocal {
                            file: file.as_deref().map(str::trim).filter(|f| !f.is_empty()).map(str::to_string),
                            body: body.clone(),
                            status: *status,
                            content_type: content_type.clone(),
                            headers: headers.clone(),
                            prefix: rule.path_prefix.clone(),
                        });
                    }
                    RuleAction::MockResponse { status, headers, body, delay_ms } => {
                        plan.terminal.get_or_insert_with(|| Terminal::Mock {
                            status: *status,
                            headers: headers.clone(),
                            body: body.clone(),
                            delay_ms: *delay_ms,
                        });
                    }
                    RuleAction::Block { mode } => {
                        plan.terminal.get_or_insert(Terminal::Block(*mode));
                    }
                    RuleAction::MapRemote { scheme, host, port, path_from, path_to, preserve_host } => {
                        plan.map_remote.get_or_insert_with(|| MapRemote {
                            scheme: scheme.clone(),
                            host: host.clone(),
                            port: *port,
                            path_from: path_from.clone(),
                            path_to: path_to.clone(),
                            preserve_host: *preserve_host,
                        });
                    }
                    RuleAction::SetHeader { target, name, value } => {
                        let op = HeaderOp::Set(name.trim().to_string(), value.clone());
                        match target {
                            HeaderTarget::Request => plan.req_headers.push(op),
                            HeaderTarget::Response => plan.resp_headers.push(op),
                        }
                    }
                    RuleAction::RemoveHeader { target, name } => {
                        let op = HeaderOp::Remove(name.trim().to_string());
                        match target {
                            HeaderTarget::Request => plan.req_headers.push(op),
                            HeaderTarget::Response => plan.resp_headers.push(op),
                        }
                    }
                    RuleAction::ValidateSpec { .. } => {
                        if plan.validate.is_none() {
                            plan.validate = rule.validate.clone();
                        }
                    }
                    RuleAction::Throttle { latency_ms, kbps } => {
                        if !throttled {
                            throttled = true;
                            plan.latency_ms = *latency_ms;
                            plan.bytes_per_sec = kbps.filter(|k| *k > 0).map(|k| k * 1024);
                        }
                    }
                }
            }
        }
        plan
    }
}

pub(crate) fn apply_header_ops(headers: &mut HeaderMap, ops: &[HeaderOp]) {
    for op in ops {
        match op {
            HeaderOp::Set(name, value) => {
                if let (Ok(n), Ok(v)) = (http::HeaderName::from_bytes(name.as_bytes()), http::HeaderValue::from_str(value)) {
                    headers.insert(n, v);
                }
            }
            HeaderOp::Remove(name) => {
                headers.remove(name.as_str());
            }
        }
    }
}

/// Gives rules without an id a fresh one.
pub fn assign_ids(rules: &mut [Rule]) {
    for r in rules {
        if r.id.trim().is_empty() {
            r.id = devall_core::new_id();
        }
    }
}

/// Reads `rules.json` (missing file = no rules).
pub fn load_rules(path: &Path) -> Result<Vec<Rule>> {
    match std::fs::read(path) {
        Ok(b) => Ok(serde_json::from_slice::<RulesFile>(&b)?.rules),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(ProxyError::IoAt { path: path.to_path_buf(), source: e }),
    }
}

pub fn rules_json(rules: &[Rule]) -> Result<String> {
    let mut s = serde_json::to_string_pretty(&RulesFile { version: 1, rules: rules.to_vec() })?;
    s.push('\n');
    Ok(s)
}

/// Parses an exported rules file (or a bare JSON array of rules).
pub fn parse_rules(text: &str) -> Result<Vec<Rule>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let mut rules: Vec<Rule> = if v.is_array() { serde_json::from_value(v)? } else { serde_json::from_value::<RulesFile>(v)?.rules };
    assign_ids(&mut rules);
    validate(&rules)?;
    Ok(rules)
}

pub fn save_rules(path: &Path, rules: &[Rule]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    devall_core::fsutil::atomic_write(path, rules_json(rules)?.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str) -> Target {
        Target::parse(url).unwrap()
    }

    fn rule(json: serde_json::Value) -> Rule {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn targets_parse_and_render() {
        let t = target("https://API.example.com:8443/a/b?x=1");
        assert_eq!((t.scheme.as_str(), t.host.as_str(), t.port, t.path_and_query.as_str()), ("https", "API.example.com", 8443, "/a/b?x=1"));
        assert_eq!(t.path(), "/a/b");
        assert_eq!(target("http://a.com").url(), "http://a.com/");
        assert_eq!(target("https://a.com:443/x").url(), "https://a.com/x");
    }

    #[test]
    fn map_remote_rewrites() {
        let mr = MapRemote {
            scheme: Some("http".into()),
            host: Some("localhost".into()),
            port: Some(3000),
            path_from: Some("/v1".into()),
            path_to: Some("/api/v1".into()),
            preserve_host: false,
        };
        assert_eq!(mr.apply(&target("https://prod.example.com/v1/users?page=2")).url(), "http://localhost:3000/api/v1/users?page=2");
        let scheme_only = MapRemote { scheme: Some("http".into()), host: None, port: None, path_from: None, path_to: None, preserve_host: false };
        assert_eq!(scheme_only.apply(&target("https://a.com/x")).url(), "http://a.com/x");
        let prefix = MapRemote { scheme: None, host: None, port: None, path_from: None, path_to: Some("/mock".into()), preserve_host: false };
        assert_eq!(prefix.apply(&target("https://a.com/x")).url(), "https://a.com/mock/x");
        let untouched = MapRemote { path_from: Some("/other".into()), ..prefix };
        assert_eq!(untouched.apply(&target("https://a.com/x")).url(), "https://a.com/x");
    }

    #[test]
    fn plan_combines_rules_in_order() {
        let rules = vec![
            rule(
                serde_json::json!({"name": "hdr", "match": {"host": "*.example.com"}, "actions": [{"type": "setHeader", "name": "x-a", "value": "1"}, {"type": "removeHeader", "target": "response", "name": "server"}]}),
            ),
            rule(
                serde_json::json!({"name": "mock", "match": {"methods": ["get"], "path": "/api/*"}, "actions": [{"type": "mockResponse", "status": 201, "body": "{}"}]}),
            ),
            rule(serde_json::json!({"name": "block", "match": {"path": "/api/*"}, "actions": [{"type": "block"}]})),
            rule(serde_json::json!({"name": "off", "enabled": false, "actions": [{"type": "breakpoint"}]})),
            rule(
                serde_json::json!({"name": "hdrmatch", "match": {"header": {"name": "X-Debug", "value": "on*"}}, "actions": [{"type": "breakpoint", "response": true}]}),
            ),
        ];
        let compiled = compile(&rules).unwrap();
        let mut headers = HeaderMap::new();
        let t = target("https://api.example.com/api/users");
        let plan = compiled.plan(&RuleInput { method: "GET", target: &t, headers: &headers });
        assert_eq!(plan.matched, ["hdr", "mock", "block"]);
        assert_eq!(plan.req_headers, [HeaderOp::Set("x-a".into(), "1".into())]);
        assert_eq!(plan.resp_headers, [HeaderOp::Remove("server".into())]);
        assert!(matches!(plan.terminal, Some(Terminal::Mock { status: 201, .. })));
        assert!(!plan.bp_request);
        let plan = compiled.plan(&RuleInput { method: "POST", target: &t, headers: &headers });
        assert!(matches!(plan.terminal, Some(Terminal::Block(BlockMode::Forbidden))));
        headers.insert("x-debug", "online".parse().unwrap());
        let other = target("http://other.org/");
        let plan = compiled.plan(&RuleInput { method: "GET", target: &other, headers: &headers });
        assert_eq!(plan.matched, ["hdrmatch"]);
        assert!(plan.bp_request && plan.bp_response);
    }

    #[test]
    fn validation_errors_name_the_rule() {
        let bad = rule(serde_json::json!({"name": "R1", "match": {"pathRegex": "("}, "actions": []}));
        assert!(validate(&[bad]).unwrap_err().to_string().contains("R1"));
        let local = rule(serde_json::json!({"name": "L", "actions": [{"type": "mapLocal", "file": "relative/file.json"}]}));
        assert!(validate(&[local]).is_err());
        let empty_remote = rule(serde_json::json!({"name": "M", "actions": [{"type": "mapRemote"}]}));
        assert!(validate(&[empty_remote]).is_err());
    }

    #[test]
    fn literal_prefixes() {
        assert_eq!(literal_prefix("/static/*"), "/static/");
        assert_eq!(literal_prefix("/api/v1/data.json"), "/api/v1/");
        assert_eq!(literal_prefix("*.js"), "");
    }

    #[test]
    fn rules_round_trip_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        assert!(load_rules(&path).unwrap().is_empty());
        let mut rules = vec![rule(serde_json::json!({"name": "t", "actions": [{"type": "throttle", "latencyMs": 50, "kbps": 10}]}))];
        assign_ids(&mut rules);
        save_rules(&path, &rules).unwrap();
        assert_eq!(load_rules(&path).unwrap(), rules);
        let parsed = parse_rules(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed, rules);
        assert!(parse_rules("[{\"name\":\"x\",\"actions\":[]}]").unwrap()[0].id.len() > 10);
    }
}
