//! Serde model of every project file. Field order here is the key order on disk, so keep the
//! most useful fields first. Unknown keys are preserved in `extra` so an older app version never
//! silently drops data written by a newer one.

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;

/// Unknown keys carried through a read → write round trip.
pub type Extra = IndexMap<String, serde_json::Value>;

/// An ordered name → value variable map.
pub type Variables = IndexMap<String, String>;

fn format_version() -> u32 {
    FORMAT_VERSION
}
fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_false(b: &bool) -> bool {
    !*b
}
fn default_method() -> String {
    "GET".into()
}

/// `devall.yaml` at the project root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifest {
    #[serde(default = "format_version")]
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Project-wide, non-secret variables (lowest precedence).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub variables: IndexMap<String, String>,
    /// Git repositories that belong to the project (multi-repo projects). Empty for a project that
    /// is its own repo (or lives inside one) — that repo is found without being listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<RepoRef>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// One repository of a multi-repo project (`repos:` in `devall.yaml`). Shared with the team, so
/// `path` is relative to the project folder and may climb out of it (`../api`); a machine whose
/// layout differs points at its own copy in the project data folder's `repos.json` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    /// Unique in the project; links refer to files in it as `repo:<name>/<path>`.
    pub name: String,
    /// Folder relative to the project folder, `/`-separated (`web`, `apps/admin`, `../api`).
    pub path: String,
    /// Clone URL, offered when the repository is missing on a machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Free label the app groups repositories by (e.g. `Frontend`, `Backend`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// A name/value row (query param, header, form field). Disabled rows stay in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyValue {
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl KeyValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self { name: name.into(), value: value.into(), enabled: true, description: None }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Auth {
    /// Use the nearest folder's auth (or none at the root).
    #[default]
    Inherit,
    None,
    Basic {
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
    },
    Bearer {
        #[serde(default)]
        token: String,
        /// Defaults to `Bearer`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
    },
    ApiKey {
        #[serde(default)]
        key: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        location: ApiKeyLocation,
    },
    /// OAuth 2.0: tokens are fetched (and refreshed) automatically and cached in the keychain.
    #[serde(rename = "oauth2")]
    OAuth2(OAuth2Config),
    /// AWS Signature Version 4.
    #[serde(rename = "awsSigV4")]
    AwsSigV4 {
        #[serde(default)]
        access_key_id: String,
        #[serde(default)]
        secret_access_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_token: Option<String>,
        #[serde(default)]
        region: String,
        #[serde(default)]
        service: String,
    },
    /// HTTP Digest (RFC 7616): answered automatically after the server's 401 challenge.
    Digest {
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
    },
    /// NTLM (NTLMv2 responses): negotiate → server challenge → authenticate, on one kept-alive
    /// connection. `username` may also be written `DOMAIN\user` or `user@domain`.
    Ntlm {
        #[serde(default)]
        username: String,
        /// Usually a `{{secret.NAME}}` reference.
        #[serde(default)]
        password: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        domain: Option<String>,
        /// Client machine name sent in the handshake (none by default).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workstation: Option<String>,
    },
    /// Computed by a WebAssembly plugin's auth provider (e.g. custom HMAC signing). `config`
    /// values accept `{{variables}}` and `{{secret.NAME}}`; the provider sees them resolved.
    Plugin {
        /// Plugin id.
        #[serde(default)]
        plugin: String,
        /// Auth provider id inside the plugin.
        #[serde(default)]
        provider: String,
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        config: IndexMap<String, String>,
    },
}

impl Auth {
    pub fn is_inherit(&self) -> bool {
        matches!(self, Auth::Inherit)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2Grant {
    #[default]
    ClientCredentials,
    /// Authorization code, with PKCE by default; the browser redirects to a local callback.
    AuthorizationCode,
    /// Resource-owner password (legacy; prefer client credentials or authorization code).
    Password,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2ClientAuth {
    /// client_id/client_secret in an `Authorization: Basic` header.
    #[default]
    Basic,
    /// client_id/client_secret as form fields.
    Body,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2Config {
    #[serde(default)]
    pub grant: OAuth2Grant,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_url: String,
    #[serde(default)]
    pub token_url: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_secret: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    /// Password grant only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// Password grant only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub use_pkce: bool,
    /// Fixed local port for the authorization-code callback (must match the registered redirect URI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_port: Option<u16>,
    #[serde(default, skip_serializing_if = "is_default_client_auth")]
    pub client_auth: OAuth2ClientAuth,
    /// Defaults to `Bearer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_prefix: Option<String>,
    /// Extra parameters for the authorization and token requests (e.g. `resource`, `prompt`).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub extra_params: IndexMap<String, String>,
}

impl Default for OAuth2Config {
    /// Matches the file-format defaults (notably PKCE on).
    fn default() -> Self {
        Self {
            grant: OAuth2Grant::default(),
            auth_url: String::new(),
            token_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            scope: String::new(),
            audience: None,
            username: String::new(),
            password: String::new(),
            use_pkce: true,
            redirect_port: None,
            client_auth: OAuth2ClientAuth::default(),
            token_prefix: None,
            extra_params: IndexMap::new(),
        }
    }
}

fn is_default_client_auth(a: &OAuth2ClientAuth) -> bool {
    *a == OAuth2ClientAuth::Basic
}

/// Where a captured value comes from (see [`Capture`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "from", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CaptureSource {
    Status,
    /// Field is `header` (not `name`) because the capture's own `name` is the variable name.
    Header {
        header: String,
    },
    /// RFC 9535 JSONPath into the JSON response body, e.g. `$.data.token`.
    JsonPath {
        path: String,
    },
    /// First match of `pattern` in the body; `group` 0 = whole match.
    Regex {
        pattern: String,
        #[serde(default)]
        group: usize,
    },
    Cookie {
        cookie: String,
    },
    Body,
}

/// Stores part of a response in a runtime variable usable as `{{name}}` by later requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    pub name: String,
    #[serde(flatten)]
    pub source: CaptureSource,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "assert", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AssertTarget {
    Status,
    Header { name: String },
    JsonPath { path: String },
    Body,
    /// Total response time in milliseconds.
    ResponseTime,
    /// Body size in bytes.
    Size,
    ContentType,
    /// The response body matches a committed snapshot file (path relative to the project root,
    /// e.g. `http/__snapshots__/users/list.snap.json`). JSON is compared semantically; nodes
    /// selected by the `ignore` JSONPaths (timestamps, generated ids) are left out.
    Snapshot {
        file: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ignore: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AssertOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Matches,
    Exists,
    NotExists,
    IsEmpty,
    IsNotEmpty,
    /// `value` is one of string, number, boolean, array, object, null.
    IsType,
    /// `value` is a comma-separated list.
    OneOf,
}

/// A check on the response, e.g. `status eq 200` or `jsonPath $.items[0].id exists`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Assertion {
    #[serde(flatten)]
    pub target: AssertTarget,
    pub op: AssertOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

/// JavaScript run around a request in a sandbox (QuickJS) with a Postman-compatible `pm` API.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Scripts {
    /// Runs before variables are resolved; may set variables and edit the request.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pre_request: String,
    /// Runs after the response; may set variables and add `pm.test(...)` results.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub post_response: String,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self.pre_request.trim().is_empty() && self.post_response.trim().is_empty()
    }
}

fn scripts_empty(s: &Option<Scripts>) -> bool {
    s.as_ref().is_none_or(Scripts::is_empty)
}

/// Client certificate for mutual TLS (PEM files, paths relative to the project root or absolute).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientCert {
    /// PEM certificate (chain).
    pub cert: String,
    /// PEM private key; may be omitted when `cert` contains the key too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MultipartPart {
    pub name: String,
    #[serde(default)]
    pub value: String,
    /// Path of a file to upload (relative to the project root, or absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Body {
    #[default]
    None,
    Json {
        #[serde(default)]
        content: String,
    },
    Text {
        #[serde(default)]
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    Xml {
        #[serde(default)]
        content: String,
    },
    Form {
        #[serde(default)]
        fields: Vec<KeyValue>,
    },
    Multipart {
        #[serde(default)]
        parts: Vec<MultipartPart>,
    },
    Graphql {
        #[serde(default)]
        query: String,
        /// JSON text.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        variables: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_name: Option<String>,
    },
    Binary {
        #[serde(default)]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

impl Body {
    pub fn is_none(&self) -> bool {
        matches!(self, Body::None)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestSettings {
    #[serde(default = "RequestSettings::default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "yes")]
    pub follow_redirects: bool,
    #[serde(default = "yes")]
    pub verify_tls: bool,
    /// Proxy URL (`http://`, `https://`, `socks5://`), `none` to bypass, or absent for the system proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<ClientCert>,
}

impl RequestSettings {
    fn default_timeout() -> u64 {
        30_000
    }
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for RequestSettings {
    fn default() -> Self {
        Self { timeout_ms: Self::default_timeout(), follow_redirects: true, verify_tls: true, proxy: None, client_cert: None }
    }
}

/// `http/**/<slug>.req.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequestFile {
    #[serde(default = "format_version")]
    pub version: u32,
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Auth::is_inherit")]
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "Body::is_none")]
    pub body: Body,
    #[serde(default, skip_serializing_if = "RequestSettings::is_default")]
    pub settings: RequestSettings,
    /// Values to extract from the response into runtime variables (request chaining).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<Capture>,
    /// Checks run against every response; failures fail `devall run` in CI.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<Assertion>,
    #[serde(default, skip_serializing_if = "scripts_empty")]
    pub scripts: Option<Scripts>,
    /// Free-form markdown notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl HttpRequestFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            id: new_id(),
            name: name.into(),
            method: default_method(),
            url: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            auth: Auth::Inherit,
            body: Body::None,
            settings: RequestSettings::default(),
            captures: Vec::new(),
            tests: Vec::new(),
            scripts: None,
            docs: None,
            extra: Extra::new(),
        }
    }
}

/// `_folder.yaml` inside any folder under `http/`. Everything is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderFile {
    #[serde(default = "format_version")]
    pub version: u32,
    /// Display name; defaults to the directory name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Child file/directory names in display order. Children not listed sort after, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// Headers applied to every request below this folder (requests override by name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    /// Auth for requests below this folder that use `inherit`.
    #[serde(default, skip_serializing_if = "Auth::is_inherit")]
    pub auth: Auth,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub variables: IndexMap<String, String>,
    /// Scripts run for every request below this folder (outermost folder first).
    #[serde(default, skip_serializing_if = "scripts_empty")]
    pub scripts: Option<Scripts>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `environments/<slug>.env.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFile {
    #[serde(default = "format_version")]
    pub version: u32,
    pub name: String,
    /// Chip colour: a preset (`local`, `dev`, `test`, `staging`, `prod`) or a CSS hex colour (`#7c3aed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Short chip text (e.g. `QA`, `EU-PROD`); defaults to the colour preset's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Ask before sending anything but GET/HEAD/OPTIONS while this environment is active.
    #[serde(default, skip_serializing_if = "is_false")]
    pub protected: bool,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub variables: IndexMap<String, String>,
    /// Secret *names* only. Values live in the OS keychain (or a git-ignored `.env`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    /// Optional secret-manager references by secret name, e.g. `API_TOKEN: op://Eng/github/token`.
    /// References are safe to commit; each teammate resolves them with their own CLI login.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub secret_sources: IndexMap<String, String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl EnvironmentFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            name: name.into(),
            color: None,
            label: None,
            protected: false,
            variables: IndexMap::new(),
            secrets: Vec::new(),
            secret_sources: IndexMap::new(),
            extra: Extra::new(),
        }
    }
}

/// A new stable identifier (ULID): sortable, URL-safe, unique without coordination.
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}
