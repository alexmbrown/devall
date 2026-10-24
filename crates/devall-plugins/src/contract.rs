//! The JSON contracts between the host and a plugin. Every call is one JSON document in and one
//! out (see `docs/PLUGINS.md`); these types are mirrored by the author SDK
//! (`sdk/rust/devall-plugin`), so field names here are part of the stable ABI.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn is_false(b: &bool) -> bool {
    !*b
}

/// Host → plugin: `devall_call` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallEnvelope {
    pub api_version: u32,
    /// One of the `KIND_*` constants.
    pub kind: String,
    /// Contribution id inside the plugin.
    pub id: String,
    pub input: Value,
}

pub const KIND_UTILITY: &str = "utility";
pub const KIND_PRE_REQUEST: &str = "hook.preRequest";
pub const KIND_POST_RESPONSE: &str = "hook.postResponse";
pub const KIND_IMPORT_DETECT: &str = "import.detect";
pub const KIND_IMPORT_PARSE: &str = "import.parse";
pub const KIND_CODEGEN: &str = "codegen";
pub const KIND_AUTH: &str = "auth";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self { name: name.into(), value: value.into() }
    }
}

/// A resolved HTTP request (variables and secrets already substituted).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    /// UTF-8 text, or Base64 when `bodyBase64` is true. Absent = no body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub body_base64: bool,
}

impl PluginRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub body_base64: bool,
    #[serde(default)]
    pub time_ms: u64,
}

// ---------------------------------------------------------------- utility

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UtilityInput {
    pub input: String,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UtilityOutput {
    /// A string (shown as-is) or any JSON value (pretty-printed).
    pub output: Value,
}

// ---------------------------------------------------------------- hooks

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreRequestInput {
    pub request: PluginRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PreRequestOutput {
    /// The request to send instead; absent = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<PluginRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostResponseInput {
    pub request: PluginRequest,
    pub response: PluginResponse,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PostResponseOutput {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<TestResult>,
    /// Runtime variables to set (like captures).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<String>,
}

// ---------------------------------------------------------------- importers

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportInput {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DetectOutput {
    /// 0 = not mine, 1 = certainly mine. ≥ 0.5 counts as detected.
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ImportedBody {
    Json { content: String },
    Text {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    Form { fields: Vec<Header> },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedRequest {
    pub name: String,
    #[serde(default = "get")]
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<Header>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ImportedBody>,
    /// `/`-separated folder path inside the import (`users/admin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

fn get() -> String {
    "GET".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub requests: Vec<ImportedRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------- codegen / auth

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CodegenInput {
    pub request: PluginRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CodegenOutput {
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuthInput {
    pub request: PluginRequest,
    /// The auth's config with `{{variables}}` and `{{secret.X}}` resolved.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuthOutput {
    /// Headers to add (replacing same-named ones).
    #[serde(default)]
    pub headers: Vec<Header>,
    /// Query parameters to append.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<Header>,
}

// ---------------------------------------------------------------- host calls

/// Plugin → host: `devall.host_call` input. Each op needs a capability (except time/random).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all_fields = "camelCase")]
pub enum HostCall {
    /// Needs `http` with the URL's host in its allow-list. Redirects are not followed.
    #[serde(rename = "http.request")]
    HttpRequest {
        request: PluginRequest,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    /// Needs `secrets.read` listing the name, granted by the user. Result: `{"value": "…"}`.
    #[serde(rename = "secrets.get")]
    SecretGet { name: String },
    /// Needs `fs.project.read`; `path` is relative to the project root. Result:
    /// `{"content": "…", "base64": false}`.
    #[serde(rename = "fs.read")]
    FsRead { path: String },
    /// `{"unixMs": …, "iso": "…"}`.
    #[serde(rename = "time.now")]
    Now,
    /// `{"hex": "…"}` — `len` ≤ 1024 bytes from the OS RNG.
    #[serde(rename = "random.bytes")]
    RandomBytes { len: u32 },
}
