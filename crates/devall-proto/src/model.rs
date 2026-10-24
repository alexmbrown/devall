//! On-disk models of the non-HTTP request files under `http/`. Field order is key order on disk;
//! defaults stay out of the file; unknown keys are preserved in `extra`. Every string field may
//! contain `{{variables}}` / `{{secret.NAME}}`.

use devall_core::project::item_kind;
use devall_core::{Extra, FORMAT_VERSION, KeyValue, Project, new_id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{ProtoError, Result};

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
fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}
fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 10_000;
fn default_connect_timeout() -> u64 {
    DEFAULT_CONNECT_TIMEOUT_MS
}
fn is_default_connect_timeout(n: &u64) -> bool {
    *n == DEFAULT_CONNECT_TIMEOUT_MS
}

/// How a composer message body is interpreted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum MessageFormat {
    #[default]
    #[serde(rename = "text")]
    Text,
    /// Sent as text; validated as JSON first.
    #[serde(rename = "json")]
    Json,
    /// Base64 in the file, sent as binary.
    #[serde(rename = "binary-base64")]
    BinaryBase64,
}

impl MessageFormat {
    pub fn is_text(&self) -> bool {
        *self == MessageFormat::Text
    }

    /// Bytes to put on the wire for `body` (already template-resolved).
    pub fn encode(&self, body: &str) -> Result<Vec<u8>> {
        use base64::Engine;
        match self {
            MessageFormat::Text => Ok(body.as_bytes().to_vec()),
            MessageFormat::Json => {
                serde_json::from_str::<serde_json::Value>(body).map_err(|e| ProtoError::Invalid(format!("message is not valid JSON: {e}")))?;
                Ok(body.as_bytes().to_vec())
            }
            MessageFormat::BinaryBase64 => {
                let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
                base64::engine::general_purpose::STANDARD
                    .decode(compact)
                    .map_err(|e| ProtoError::Invalid(format!("message is not valid base64: {e}")))
            }
        }
    }
}

/// A named message preset shown in the composer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SavedMessage {
    pub name: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "MessageFormat::is_text")]
    pub format: MessageFormat,
}

// ---------------------------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WsSettings {
    #[serde(default = "yes")]
    pub verify_tls: bool,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_ms: u64,
    /// Sends a ping every N milliseconds to keep idle connections open; 0 = off.
    #[serde(default)]
    pub ping_interval_ms: u64,
}

impl Default for WsSettings {
    fn default() -> Self {
        Self { verify_tls: true, connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS, ping_interval_ms: 0 }
    }
}

impl WsSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `http/**/<slug>.ws.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WsFile {
    #[serde(default = "format_version")]
    pub version: u32,
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// `ws://` or `wss://` (`http(s)://` is accepted and mapped).
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    /// Offered in `Sec-WebSocket-Protocol`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subprotocols: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<SavedMessage>,
    #[serde(default, skip_serializing_if = "WsSettings::is_default")]
    pub settings: WsSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl WsFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            id: new_id(),
            name: name.into(),
            url: "ws://localhost:8080/ws".into(),
            headers: Vec::new(),
            subprotocols: Vec::new(),
            messages: vec![SavedMessage { name: "Hello".into(), body: "{\n  \"type\": \"hello\"\n}".into(), format: MessageFormat::Json }],
            settings: WsSettings::default(),
            docs: None,
            extra: Extra::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// gRPC
// ---------------------------------------------------------------------------------------------

/// `http/**/<slug>.grpc.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrpcFile {
    #[serde(default = "format_version")]
    pub version: u32,
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// `grpc://host:port` or `http://` = plaintext (h2c); `grpcs://` or `https://` = TLS.
    /// A bare `host:port` is plaintext.
    #[serde(default)]
    pub url: String,
    /// Fully-qualified service name, e.g. `helloworld.Greeter`.
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub method: String,
    /// Request message as JSON (proto3 JSON mapping). For client-streaming methods a JSON array
    /// sends one message per element.
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<KeyValue>,
    /// Ask the server for its schema (gRPC server reflection v1, falling back to v1alpha).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub use_reflection: bool,
    /// `.proto` files (relative to the project root) used when reflection is off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proto_files: Vec<String>,
    /// Extra import directories (relative to the project root). The files' own directories and
    /// the project root are always searched; Google's well-known types are built in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import_paths: Vec<String>,
    /// Skip TLS certificate verification (self-signed development servers).
    #[serde(default, skip_serializing_if = "is_false")]
    pub insecure: bool,
    /// Call deadline in milliseconds (sent as `grpc-timeout`); 0 = none.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub deadline_ms: u64,
    /// Saved messages for streaming calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<SavedMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl GrpcFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            id: new_id(),
            name: name.into(),
            url: "grpc://localhost:50051".into(),
            service: String::new(),
            method: String::new(),
            message: "{}".into(),
            metadata: Vec::new(),
            use_reflection: true,
            proto_files: Vec::new(),
            import_paths: Vec::new(),
            insecure: false,
            deadline_ms: 0,
            messages: Vec::new(),
            docs: None,
            extra: Extra::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// MQTT
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum MqttVersion {
    #[default]
    #[serde(rename = "3.1.1")]
    V311,
    #[serde(rename = "5")]
    V5,
}

impl MqttVersion {
    pub fn is_default(&self) -> bool {
        *self == MqttVersion::V311
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MqttSubscription {
    /// Topic filter; `+` and `#` wildcards allowed.
    pub topic: String,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub qos: u8,
    /// Subscribed automatically on connect when enabled.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MqttPublish {
    pub name: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub payload: String,
    #[serde(default, skip_serializing_if = "MessageFormat::is_text")]
    pub format: MessageFormat,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub qos: u8,
    #[serde(default, skip_serializing_if = "is_false")]
    pub retain: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MqttLastWill {
    pub topic: String,
    #[serde(default)]
    pub payload: String,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub qos: u8,
    #[serde(default, skip_serializing_if = "is_false")]
    pub retain: bool,
}

pub const DEFAULT_KEEP_ALIVE_SECS: u64 = 60;
fn default_keep_alive() -> u64 {
    DEFAULT_KEEP_ALIVE_SECS
}
fn is_default_keep_alive(n: &u64) -> bool {
    *n == DEFAULT_KEEP_ALIVE_SECS
}

/// `http/**/<slug>.mqtt.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MqttFile {
    #[serde(default = "format_version")]
    pub version: u32,
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// `mqtt://host:1883`, `mqtts://host:8883`, `ws://host:8083/mqtt` or `wss://host/mqtt`.
    #[serde(default)]
    pub broker_url: String,
    /// Empty = a random `devall-…` id per connection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// Use `{{secret.NAME}}`; never a literal password in a shared file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// Keep-alive interval in seconds.
    #[serde(default = "default_keep_alive", skip_serializing_if = "is_default_keep_alive")]
    pub keep_alive: u64,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub clean_session: bool,
    /// Protocol version: `3.1.1` (default) or `5`.
    #[serde(default, skip_serializing_if = "MqttVersion::is_default")]
    pub mqtt_version: MqttVersion,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<MqttSubscription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publishes: Vec<MqttPublish>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_will: Option<MqttLastWill>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub verify_tls: bool,
    #[serde(default = "default_connect_timeout", skip_serializing_if = "is_default_connect_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl MqttFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            id: new_id(),
            name: name.into(),
            broker_url: "mqtt://localhost:1883".into(),
            client_id: String::new(),
            username: String::new(),
            password: String::new(),
            keep_alive: DEFAULT_KEEP_ALIVE_SECS,
            clean_session: true,
            mqtt_version: MqttVersion::V311,
            subscriptions: vec![MqttSubscription { topic: "devall/#".into(), qos: 0, enabled: true }],
            publishes: vec![MqttPublish {
                name: "Hello".into(),
                topic: "devall/hello".into(),
                payload: "{\n  \"hello\": \"world\"\n}".into(),
                format: MessageFormat::Json,
                qos: 0,
                retain: false,
            }],
            last_will: None,
            verify_tls: true,
            connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            docs: None,
            extra: Extra::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Socket.IO
// ---------------------------------------------------------------------------------------------

pub const DEFAULT_SIO_PATH: &str = "/socket.io/";
fn default_sio_path() -> String {
    DEFAULT_SIO_PATH.into()
}
fn is_default_sio_path(p: &String) -> bool {
    p == DEFAULT_SIO_PATH || p.is_empty()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SioEmit {
    pub name: String,
    #[serde(default)]
    pub event: String,
    /// JSON. An array is spread into the event's arguments; any other value is one argument;
    /// empty = no arguments.
    #[serde(default)]
    pub args: String,
    /// Ask the server for an acknowledgement.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ack: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SioSettings {
    #[serde(default = "yes")]
    pub verify_tls: bool,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_ms: u64,
}

impl Default for SioSettings {
    fn default() -> Self {
        Self { verify_tls: true, connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS }
    }
}

impl SioSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `http/**/<slug>.sio.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SioFile {
    #[serde(default = "format_version")]
    pub version: u32,
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// Server URL; its path is the namespace, like the JS client (`http://host:3000/admin`).
    #[serde(default)]
    pub url: String,
    /// Engine.IO endpoint path.
    #[serde(default = "default_sio_path", skip_serializing_if = "is_default_sio_path")]
    pub path: String,
    /// JSON object sent as the handshake `auth` payload.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    /// Extra query parameters on the handshake URL.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<KeyValue>,
    /// Event names to show. Empty = every event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listen: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emits: Vec<SioEmit>,
    #[serde(default, skip_serializing_if = "SioSettings::is_default")]
    pub settings: SioSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl SioFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: FORMAT_VERSION,
            id: new_id(),
            name: name.into(),
            url: "http://localhost:3000".into(),
            path: DEFAULT_SIO_PATH.into(),
            auth: String::new(),
            headers: Vec::new(),
            query: Vec::new(),
            listen: Vec::new(),
            emits: vec![SioEmit { name: "Hello".into(), event: "message".into(), args: "[\"hello\"]".into(), ack: false }],
            settings: SioSettings::default(),
            docs: None,
            extra: Extra::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Kind dispatch
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ProtoKind {
    #[serde(rename = "ws")]
    Ws,
    #[serde(rename = "grpc")]
    Grpc,
    #[serde(rename = "mqtt")]
    Mqtt,
    #[serde(rename = "socketio")]
    Socketio,
}

impl ProtoKind {
    pub const ALL: [ProtoKind; 4] = [ProtoKind::Ws, ProtoKind::Grpc, ProtoKind::Mqtt, ProtoKind::Socketio];

    /// File extension (matches `devall_core::project::ITEM_KINDS`).
    pub fn ext(self) -> &'static str {
        match self {
            ProtoKind::Ws => ".ws.yaml",
            ProtoKind::Grpc => ".grpc.yaml",
            ProtoKind::Mqtt => ".mqtt.yaml",
            ProtoKind::Socketio => ".sio.yaml",
        }
    }

    /// Protocol name used by the request tree (`TreeNode::Request::protocol`).
    pub fn protocol(self) -> &'static str {
        match self {
            ProtoKind::Ws => "ws",
            ProtoKind::Grpc => "grpc",
            ProtoKind::Mqtt => "mqtt",
            ProtoKind::Socketio => "socketio",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProtoKind::Ws => "WebSocket",
            ProtoKind::Grpc => "gRPC",
            ProtoKind::Mqtt => "MQTT",
            ProtoKind::Socketio => "Socket.IO",
        }
    }

    /// Accepts protocol names and common aliases (`websocket`, `sio`, `socket.io`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ws" | "wss" | "websocket" => Some(ProtoKind::Ws),
            "grpc" => Some(ProtoKind::Grpc),
            "mqtt" => Some(ProtoKind::Mqtt),
            "socketio" | "socket.io" | "sio" => Some(ProtoKind::Socketio),
            _ => None,
        }
    }

    /// Kind of a file under `http/` by its extension.
    pub fn from_path(rel: &str) -> Option<Self> {
        item_kind(rel).and_then(|(_, protocol)| Self::parse(protocol))
    }
}

/// Any of the protocol files. Serializes as the inner file (untagged).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ProtoFile {
    Ws(WsFile),
    Grpc(GrpcFile),
    Mqtt(MqttFile),
    Socketio(SioFile),
}

impl ProtoFile {
    /// A starter file with sensible example content.
    pub fn new(kind: ProtoKind, name: &str) -> Self {
        match kind {
            ProtoKind::Ws => ProtoFile::Ws(WsFile::new(name)),
            ProtoKind::Grpc => ProtoFile::Grpc(GrpcFile::new(name)),
            ProtoKind::Mqtt => ProtoFile::Mqtt(MqttFile::new(name)),
            ProtoKind::Socketio => ProtoFile::Socketio(SioFile::new(name)),
        }
    }

    pub fn kind(&self) -> ProtoKind {
        match self {
            ProtoFile::Ws(_) => ProtoKind::Ws,
            ProtoFile::Grpc(_) => ProtoKind::Grpc,
            ProtoFile::Mqtt(_) => ProtoKind::Mqtt,
            ProtoFile::Socketio(_) => ProtoKind::Socketio,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ProtoFile::Ws(f) => &f.name,
            ProtoFile::Grpc(f) => &f.name,
            ProtoFile::Mqtt(f) => &f.name,
            ProtoFile::Socketio(f) => &f.name,
        }
    }

    /// Parses JSON (e.g. from the UI) as the given kind.
    pub fn from_value(kind: ProtoKind, v: serde_json::Value) -> Result<Self> {
        let bad = |e: serde_json::Error| ProtoError::Invalid(format!("invalid {} file: {e}", kind.label()));
        Ok(match kind {
            ProtoKind::Ws => ProtoFile::Ws(serde_json::from_value(v).map_err(bad)?),
            ProtoKind::Grpc => ProtoFile::Grpc(serde_json::from_value(v).map_err(bad)?),
            ProtoKind::Mqtt => ProtoFile::Mqtt(serde_json::from_value(v).map_err(bad)?),
            ProtoKind::Socketio => ProtoFile::Socketio(serde_json::from_value(v).map_err(bad)?),
        })
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Reads `rel` (relative to `http/`); the kind comes from the extension.
    pub fn read(p: &Project, rel: &str) -> Result<Self> {
        let kind = ProtoKind::from_path(rel).ok_or_else(|| ProtoError::Invalid(format!("{rel} is not a WebSocket, gRPC, MQTT or Socket.IO file")))?;
        Ok(match kind {
            ProtoKind::Ws => ProtoFile::Ws(p.read_item(rel)?),
            ProtoKind::Grpc => ProtoFile::Grpc(p.read_item(rel)?),
            ProtoKind::Mqtt => ProtoFile::Mqtt(p.read_item(rel)?),
            ProtoKind::Socketio => ProtoFile::Socketio(p.read_item(rel)?),
        })
    }

    /// Writes the file at `rel`, whose extension must match the kind. Assigns an id when missing.
    pub fn save(&self, p: &Project, rel: &str) -> Result<()> {
        if ProtoKind::from_path(rel) != Some(self.kind()) {
            return Err(ProtoError::Invalid(format!("{rel} is not a {} file ({})", self.kind().label(), self.kind().ext())));
        }
        let mut f = self.clone();
        f.normalize();
        match &f {
            ProtoFile::Ws(x) => p.save_item(rel, x)?,
            ProtoFile::Grpc(x) => p.save_item(rel, x)?,
            ProtoFile::Mqtt(x) => p.save_item(rel, x)?,
            ProtoFile::Socketio(x) => p.save_item(rel, x)?,
        }
        Ok(())
    }

    /// Creates `<folder>/<slug(name)><ext>` with starter content; returns its path.
    pub fn create(p: &Project, folder: &str, kind: ProtoKind, name: &str) -> Result<String> {
        let name = if name.trim().is_empty() { format!("New {}", kind.label()) } else { name.trim().to_string() };
        let mut f = ProtoFile::new(kind, &name);
        f.normalize();
        let rel = match &f {
            ProtoFile::Ws(x) => p.create_item(folder, &name, kind.ext(), x)?,
            ProtoFile::Grpc(x) => p.create_item(folder, &name, kind.ext(), x)?,
            ProtoFile::Mqtt(x) => p.create_item(folder, &name, kind.ext(), x)?,
            ProtoFile::Socketio(x) => p.create_item(folder, &name, kind.ext(), x)?,
        };
        Ok(rel)
    }

    fn normalize(&mut self) {
        let (version, id) = match self {
            ProtoFile::Ws(f) => (&mut f.version, &mut f.id),
            ProtoFile::Grpc(f) => (&mut f.version, &mut f.id),
            ProtoFile::Mqtt(f) => (&mut f.version, &mut f.id),
            ProtoFile::Socketio(f) => (&mut f.version, &mut f.id),
        };
        *version = FORMAT_VERSION;
        if id.is_empty() {
            *id = new_id();
        }
    }
}

/// `(file name, JSON Schema)` for each protocol file type, generated from the serde model. The
/// committed copies live in `crates/devall-proto/schemas/` (regenerate with
/// `UPDATE_SCHEMAS=1 cargo test -p devall-proto schemas_are_up_to_date`).
pub fn schemas() -> Vec<(&'static str, serde_json::Value)> {
    use schemars::schema_for;
    let with_id = |name: &str, mut v: serde_json::Value, description: &str| {
        if let Some(o) = v.as_object_mut() {
            o.insert("$id".into(), format!("{}/{name}", devall_core::schema::SCHEMA_BASE_URL).into());
            o.insert("description".into(), description.into());
        }
        v
    };
    vec![
        ("ws.schema.json", with_id("ws.schema.json", schema_for!(WsFile).to_value(), "http/**/*.ws.yaml — a WebSocket connection with saved messages")),
        ("grpc.schema.json", with_id("grpc.schema.json", schema_for!(GrpcFile).to_value(), "http/**/*.grpc.yaml — a gRPC call")),
        ("mqtt.schema.json", with_id("mqtt.schema.json", schema_for!(MqttFile).to_value(), "http/**/*.mqtt.yaml — an MQTT connection with subscriptions and publishes")),
        ("sio.schema.json", with_id("sio.schema.json", schema_for!(SioFile).to_value(), "http/**/*.sio.yaml — a Socket.IO connection with listened and emitted events")),
    ]
}

/// Enabled, named rows of a key/value table, resolved.
pub(crate) fn resolve_pairs(rows: &[KeyValue], r: &mut devall_core::Resolver) -> Vec<(String, String)> {
    rows.iter().filter(|h| h.enabled && !h.name.trim().is_empty()).map(|h| (r.resolve(h.name.trim()), r.resolve(&h.value))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use devall_core::TreeNode;

    fn project() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Demo").unwrap();
        (dir, p)
    }

    #[test]
    fn create_read_save_round_trip_for_every_kind() {
        let (_d, p) = project();
        for kind in ProtoKind::ALL {
            let rel = ProtoFile::create(&p, "", kind, &format!("My {}", kind.label())).unwrap();
            assert!(rel.ends_with(kind.ext()), "{rel}");
            let f = ProtoFile::read(&p, &rel).unwrap();
            assert_eq!(f.kind(), kind);
            let path = p.item_path(&rel).unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.starts_with("version: 1\nid: "), "{text}");
            // Saving an unchanged file is byte-identical.
            f.save(&p, &rel).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            // JSON round trip (the UI path) keeps everything.
            let back = ProtoFile::from_value(kind, f.to_value()).unwrap();
            assert_eq!(back, f);
        }
        // The tree lists them with their protocol.
        let TreeNode::Folder { children, .. } = p.tree().unwrap() else { panic!() };
        let protocols: Vec<String> = children
            .iter()
            .filter_map(|c| match c {
                TreeNode::Request { protocol, .. } => Some(protocol.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(protocols, vec!["ws", "grpc", "mqtt", "socketio"]);
    }

    #[test]
    fn defaults_stay_out_of_files_and_unknown_keys_survive() {
        let (_d, p) = project();
        std::fs::write(p.http_root().join("x.mqtt.yaml"), "name: X\nbrokerUrl: mqtt://h\nfuture:\n  a: 1\n").unwrap();
        let f = ProtoFile::read(&p, "x.mqtt.yaml").unwrap();
        let ProtoFile::Mqtt(m) = &f else { panic!() };
        assert_eq!(m.keep_alive, 60);
        assert!(m.clean_session && m.verify_tls);
        f.save(&p, "x.mqtt.yaml").unwrap();
        let text = std::fs::read_to_string(p.http_root().join("x.mqtt.yaml")).unwrap();
        assert!(text.contains("future:"), "{text}");
        assert!(!text.contains("keepAlive") && !text.contains("cleanSession") && !text.contains("verifyTls"), "{text}");
        assert!(!text.contains("mqttVersion"), "{text}");
    }

    #[test]
    fn save_rejects_mismatched_extension() {
        let (_d, p) = project();
        let f = ProtoFile::new(ProtoKind::Ws, "a");
        assert!(f.save(&p, "a.grpc.yaml").is_err());
        assert!(f.save(&p, "a.req.yaml").is_err());
        assert!(f.save(&p, "../a.ws.yaml").is_err());
        f.save(&p, "a.ws.yaml").unwrap();
    }

    #[test]
    fn schemas_are_up_to_date() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas");
        let update = std::env::var_os("UPDATE_SCHEMAS").is_some();
        for (name, schema) in schemas() {
            assert_eq!(schema["type"], "object", "{name}");
            assert!(schema["properties"].get("version").is_some(), "{name}");
            let text = format!("{}\n", serde_json::to_string_pretty(&schema).unwrap());
            let path = dir.join(name);
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &text).unwrap();
            } else {
                let committed = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
                assert_eq!(committed, text, "{name} is stale — run UPDATE_SCHEMAS=1 cargo test -p devall-proto schemas_are_up_to_date");
            }
        }
    }

    #[test]
    fn formats_and_enums_use_documented_names() {
        let m = SavedMessage { name: "b".into(), body: "AAE=".into(), format: MessageFormat::BinaryBase64 };
        assert_eq!(serde_json::to_value(&m).unwrap()["format"], "binary-base64");
        assert_eq!(MessageFormat::BinaryBase64.encode("AA E=").unwrap(), vec![0, 1]);
        assert!(MessageFormat::Json.encode("{nope").is_err());
        assert_eq!(serde_json::to_value(MqttVersion::V5).unwrap(), "5");
        assert_eq!(serde_json::to_value(MqttVersion::V311).unwrap(), "3.1.1");
        assert_eq!(ProtoKind::parse("socket.io"), Some(ProtoKind::Socketio));
        assert_eq!(ProtoKind::from_path("a/b.sio.yaml"), Some(ProtoKind::Socketio));
        assert_eq!(ProtoKind::from_path("a/b.req.yaml"), None);
    }
}
