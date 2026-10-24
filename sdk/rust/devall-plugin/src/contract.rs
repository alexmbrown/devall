//! The JSON contracts (apiVersion 1). Field names match `crates/devall-plugins/src/contract.rs`
//! in the host; see `docs/PLUGINS.md` for each call.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn is_false(b: &bool) -> bool {
    !*b
}

pub const API_VERSION: u32 = 1;
pub const KIND_UTILITY: &str = "utility";
pub const KIND_PRE_REQUEST: &str = "hook.preRequest";
pub const KIND_POST_RESPONSE: &str = "hook.postResponse";
pub const KIND_IMPORT_DETECT: &str = "import.detect";
pub const KIND_IMPORT_PARSE: &str = "import.parse";
pub const KIND_CODEGEN: &str = "codegen";
pub const KIND_AUTH: &str = "auth";

/// What the host passes to `devall_call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallEnvelope {
    pub api_version: u32,
    pub kind: String,
    pub id: String,
    pub input: Value,
}

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

/// A resolved request (variables and secrets substituted).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    /// UTF-8 text, or Base64 when `body_base64`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub body_base64: bool,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
    }

    /// Sets a header, replacing same-named ones.
    pub fn set_header(&mut self, name: &str, value: impl Into<String>) {
        self.headers.retain(|h| !h.name.eq_ignore_ascii_case(name));
        self.headers.push(Header::new(name, value));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
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

impl UtilityOutput {
    pub fn text(s: impl Into<String>) -> Self {
        Self { output: Value::String(s.into()) }
    }
    pub fn json(v: Value) -> Self {
        Self { output: v }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreRequestInput {
    pub request: Request,
    #[serde(default)]
    pub request_name: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PreRequestOutput {
    /// The request to send instead (`None` = unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<Request>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostResponseInput {
    pub request: Request,
    pub response: Response,
    #[serde(default)]
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
    /// Runtime variables to set.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportInput {
    pub text: String,
    #[serde(default)]
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DetectOutput {
    /// 0 = not mine … 1 = certainly mine (≥ 0.5 counts).
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
pub struct ImportedRequest {
    pub name: String,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<Header>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ImportedBody>,
    /// `/`-separated folder inside the import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub requests: Vec<ImportedRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CodegenInput {
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CodegenOutput {
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuthInput {
    pub request: Request,
    /// The auth config with variables and `{{secret.X}}` resolved.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuthOutput {
    /// Set on the request (replacing same-named headers).
    pub headers: Vec<Header>,
    /// Appended to the query string.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<Header>,
}
