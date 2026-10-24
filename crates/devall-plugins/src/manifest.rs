//! `plugin.yaml`: what a plugin is, what it may do (capabilities) and what it adds to the app
//! (contribution points). Unknown keys are rejected so a typo in a capability name can't silently
//! drop a permission the author expected to need.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{PluginError, Result};

/// The plugin ABI / contract version this host implements.
pub const API_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "plugin.yaml";
pub const WASM_FILE: &str = "plugin.wasm";
/// Largest accepted `plugin.wasm` (bigger modules are almost certainly debug builds).
pub const MAX_WASM_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// `[a-z0-9][a-z0-9._-]{0,63}`; also the install folder name.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Contract version the plugin was built for (currently 1).
    pub api_version: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Module file next to the manifest (default `plugin.wasm`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub contributes: Contributions,
}

/// What a plugin may do beyond pure computation. Everything is denied unless listed here *and*
/// approved by the user at install / trust time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Outbound HTTP(S) through the host, only to these hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpCapability>,
    /// Named plugin secrets (values the user stores in the OS keychain for this plugin).
    #[serde(default, rename = "secrets.read", skip_serializing_if = "Vec::is_empty")]
    pub secrets_read: Vec<String>,
    /// Read files inside the open project (never `.git/` or `.devall/`).
    #[serde(default, rename = "fs.project.read", skip_serializing_if = "std::ops::Not::not")]
    pub fs_project_read: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCapability {
    /// Host names (`api.example.com`), wildcard subdomains (`*.example.com`) or `host:port`.
    pub hosts: Vec<String>,
}

impl Capabilities {
    pub fn is_empty(&self) -> bool {
        self.http.as_ref().is_none_or(|h| h.hosts.is_empty()) && self.secrets_read.is_empty() && !self.fs_project_read
    }

    /// Human-readable lines for install / trust prompts.
    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(h) = self.http.as_ref().filter(|h| !h.hosts.is_empty()) {
            out.push(format!("Make HTTP requests to {}", h.hosts.join(", ")));
        }
        if !self.secrets_read.is_empty() {
            out.push(format!("Read its secrets {}", self.secrets_read.join(", ")));
        }
        if self.fs_project_read {
            out.push("Read files in the open project".into());
        }
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contributions {
    /// Text → text/JSON operations (Utilities, command palette, `devall util`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub utilities: Vec<UtilityContribution>,
    /// Pre-request / post-response transforms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<HookContribution>,
    /// Text → requests, offered by the Import dialog.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub importers: Vec<ImporterContribution>,
    /// Request → code, offered by Generate code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codegen: Vec<CodegenContribution>,
    /// Compute auth headers / query parameters for a request (`auth: { type: plugin }`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth: Vec<AuthContribution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArgSpec {
    pub name: String,
    #[serde(default)]
    pub label: String,
    /// `text`, `secret`, `number`, `textarea`, `select:<a>|<b>`.
    #[serde(default = "text_kind")]
    pub kind: String,
    #[serde(default)]
    pub default: String,
}

fn text_kind() -> String {
    "text".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UtilityContribution {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// `text` (default) or `json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default)]
    pub no_input: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<ArgSpec>,
    /// Sample input for the Utilities page's Example button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HookEvent {
    PreRequest,
    PostResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookContribution {
    pub id: String,
    pub name: String,
    pub events: Vec<HookEvent>,
    /// Only requests to these hosts (same syntax as the http capability); empty = every request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImporterContribution {
    pub id: String,
    pub name: String,
    /// File extensions this importer understands (`.myfmt`), used as a detection hint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodegenContribution {
    pub id: String,
    pub name: String,
    /// Editor language id for highlighting (`kotlin`, `python`, …).
    #[serde(default = "text_language")]
    pub language: String,
}

fn text_language() -> String {
    "plaintext".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthContribution {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Config fields shown in the auth editor (values may use `{{secret.NAME}}`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<ArgSpec>,
}

fn valid_plugin_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

fn valid_item_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0].is_ascii_alphanumeric() && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

fn valid_host_pattern(h: &str) -> bool {
    let (name, _) = split_pattern(&h.trim().to_ascii_lowercase());
    let name = name.strip_prefix("*.").unwrap_or(&name);
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self> {
        let m: Manifest = serde_yaml_ng::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| PluginError::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| PluginError::io(&path, e))?;
        Self::parse(&text)
    }

    pub fn wasm_file(&self) -> &str {
        self.wasm.as_deref().filter(|w| !w.trim().is_empty()).unwrap_or(WASM_FILE)
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(PluginError::Manifest(m));
        if !valid_plugin_id(&self.id) {
            return bad(format!("id `{}` must be 1–64 characters of a-z 0-9 . _ - (starting with a letter or digit)", self.id));
        }
        if self.name.trim().is_empty() {
            return bad("name is required".into());
        }
        if self.version.trim().is_empty() {
            return bad("version is required".into());
        }
        if self.api_version != API_VERSION {
            return bad(format!("apiVersion {} is not supported (this /dev/all implements apiVersion {API_VERSION})", self.api_version));
        }
        let wasm = self.wasm_file();
        if wasm.contains(['/', '\\']) || wasm.starts_with('.') || !wasm.ends_with(".wasm") {
            return bad(format!("wasm `{wasm}` must be a .wasm file name next to plugin.yaml"));
        }
        if let Some(h) = &self.capabilities.http {
            if h.hosts.is_empty() {
                return bad("capabilities.http.hosts must list at least one host".into());
            }
            if let Some(x) = h.hosts.iter().find(|h| !valid_host_pattern(h)) {
                return bad(format!("capabilities.http.hosts: `{x}` is not a host name, *.domain or host:port"));
            }
        }
        if let Some(s) = self.capabilities.secrets_read.iter().find(|s| !valid_item_id(s)) {
            return bad(format!("capabilities.secrets.read: `{s}` is not a valid secret name"));
        }
        let c = &self.contributes;
        let groups: [(&str, Vec<&str>); 5] = [
            ("utilities", c.utilities.iter().map(|x| x.id.as_str()).collect()),
            ("hooks", c.hooks.iter().map(|x| x.id.as_str()).collect()),
            ("importers", c.importers.iter().map(|x| x.id.as_str()).collect()),
            ("codegen", c.codegen.iter().map(|x| x.id.as_str()).collect()),
            ("auth", c.auth.iter().map(|x| x.id.as_str()).collect()),
        ];
        for (kind, ids) in &groups {
            for (i, id) in ids.iter().enumerate() {
                if !valid_item_id(id) {
                    return bad(format!("contributes.{kind}: id `{id}` must be 1–64 characters of A-Z a-z 0-9 . _ -"));
                }
                if ids[..i].contains(id) {
                    return bad(format!("contributes.{kind}: duplicate id `{id}`"));
                }
            }
        }
        if let Some(h) = c.hooks.iter().find(|h| h.events.is_empty()) {
            return bad(format!("contributes.hooks: `{}` must list at least one event (preRequest, postResponse)", h.id));
        }
        if let Some((h, x)) = c.hooks.iter().find_map(|h| h.hosts.iter().find(|x| !valid_host_pattern(x)).map(|x| (h, x))) {
            return bad(format!("contributes.hooks: `{}` host `{x}` is not a host name, *.domain or host:port", h.id));
        }
        if let Some(u) = c.utilities.iter().find(|u| u.output.as_deref().is_some_and(|o| o != "text" && o != "json")) {
            return bad(format!("contributes.utilities: `{}` output must be text or json", u.id));
        }
        if groups.iter().all(|(_, ids)| ids.is_empty()) {
            return bad("contributes must declare at least one utility, hook, importer, codegen target or auth provider".into());
        }
        Ok(())
    }
}

/// `name[:port]`, `[v6]:port` → (name without brackets, port).
fn split_pattern(p: &str) -> (String, Option<u16>) {
    if let Some(rest) = p.strip_prefix('[')
        && let Some((addr, tail)) = rest.split_once(']')
    {
        return (addr.to_string(), tail.strip_prefix(':').and_then(|x| x.parse().ok()));
    }
    match p.rsplit_once(':') {
        Some((n, port)) if !n.contains(':') => match port.parse::<u16>() {
            Ok(port) => (n.to_string(), Some(port)),
            Err(_) => (p.to_string(), None),
        },
        _ => (p.to_string(), None),
    }
}

/// Whether `host` (optionally on `port`) matches an allow-list pattern.
pub fn host_allowed(patterns: &[String], host: &str, port: Option<u16>) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    patterns.iter().any(|p| {
        let p = p.trim().to_ascii_lowercase();
        let (name, pport) = split_pattern(&p);
        if pport.is_some() && pport != port {
            return false;
        }
        match name.strip_prefix("*.") {
            Some(domain) => host.ends_with(&format!(".{domain}")),
            None => host == name,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
id: hmac-signer
name: HMAC signer
version: 0.1.0
apiVersion: 1
capabilities:
  http:
    hosts: [api.example.com, "*.internal.test", "localhost:8080"]
  secrets.read: [HMAC_KEY]
  fs.project.read: true
contributes:
  utilities:
    - id: sign
      name: HMAC sign
      args: [{ name: key, label: Key, kind: secret }]
  auth:
    - id: hmac
      name: HMAC signature
      fields: [{ name: keyId }]
"#;

    #[test]
    fn parses_and_validates() {
        let m = Manifest::parse(GOOD).unwrap();
        assert_eq!(m.id, "hmac-signer");
        assert_eq!(m.capabilities.secrets_read, vec!["HMAC_KEY"]);
        assert!(m.capabilities.fs_project_read);
        assert_eq!(m.contributes.utilities[0].args[0].kind, "secret");
        assert_eq!(m.contributes.auth[0].fields[0].kind, "text");
        assert_eq!(m.wasm_file(), "plugin.wasm");
        assert_eq!(m.capabilities.describe().len(), 3);
    }

    #[test]
    fn rejects_bad_manifests() {
        let e = |s: &str| Manifest::parse(s).unwrap_err().to_string();
        assert!(e("id: X\nname: a\nversion: '1'\napiVersion: 1\ncontributes: {utilities: [{id: a, name: a}]}").contains("id `X`"));
        assert!(e("id: x\nname: a\nversion: '1'\napiVersion: 2\ncontributes: {utilities: [{id: a, name: a}]}").contains("apiVersion 2"));
        assert!(e("id: x\nname: a\nversion: '1'\napiVersion: 1\ncontributes: {}").contains("at least one"));
        assert!(e("id: x\nname: a\nversion: '1'\napiVersion: 1\ncapabilities: {net: true}\ncontributes: {utilities: [{id: a, name: a}]}").contains("unknown field"));
        assert!(e("id: x\nname: a\nversion: '1'\napiVersion: 1\nwasm: ../x.wasm\ncontributes: {utilities: [{id: a, name: a}]}").contains("wasm"));
        assert!(e("id: x\nname: a\nversion: '1'\napiVersion: 1\ncontributes: {utilities: [{id: a, name: a}, {id: a, name: b}]}").contains("duplicate"));
        assert!(e("id: x\nname: a\nversion: '1'\napiVersion: 1\ncontributes: {hooks: [{id: a, name: a, events: []}]}").contains("event"));
        assert!(e("id: x\nname: a\nversion: '1'\napiVersion: 1\ncapabilities: {http: {hosts: ['https://x']}}\ncontributes: {utilities: [{id: a, name: a}]}").contains("host"));
    }

    #[test]
    fn host_patterns() {
        let p = vec!["api.example.com".to_string(), "*.internal.test".to_string(), "localhost:8080".to_string()];
        assert!(host_allowed(&p, "api.example.com", Some(443)));
        assert!(host_allowed(&p, "API.example.com", None));
        assert!(!host_allowed(&p, "evil.example.com", Some(443)));
        assert!(host_allowed(&p, "a.b.internal.test", Some(80)));
        assert!(!host_allowed(&p, "internal.test", Some(80)));
        assert!(host_allowed(&p, "localhost", Some(8080)));
        assert!(!host_allowed(&p, "localhost", Some(9090)));
    }
}
