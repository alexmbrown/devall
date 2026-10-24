//! Non-HTTP protocol clients for /dev/all: WebSocket, gRPC (server reflection or `.proto` files),
//! MQTT 3.1.1/5, Socket.IO and GraphQL schema introspection.
//!
//! Every protocol has:
//! - a file model in [`model`] (`*.ws.yaml`, `*.grpc.yaml`, `*.mqtt.yaml`, `*.sio.yaml` under the
//!   project's `http/` folder, next to HTTP requests) whose string fields accept `{{variables}}`;
//! - a `*Options::resolve(&file, &mut Resolver)` step that turns the file into concrete connection
//!   settings (the caller builds the [`devall_core::Resolver`] with environment, folder, project and
//!   runtime layers plus the keychain-backed secret lookup);
//! - a session API suited to a UI: `connect(options, sink)` returns a handle with `send`/`close`,
//!   and everything that happens on the wire is reported as a [`ProtoEvent`] through an
//!   [`EventSink`] (a callback, or a tokio channel via [`EventSink::channel`]).
//!
//! Network code is pure Rust (rustls with aws-lc-rs, the OS trust store via
//! `rustls-platform-verifier`, `protox` instead of `protoc`) so it builds on Windows and macOS
//! without system libraries.

pub mod error;
pub mod event;
pub mod graphql;
pub mod grpc;
pub mod model;
pub mod mqtt;
pub mod net;
pub mod sio;
pub mod ws;

pub use error::ProtoError;
pub use event::{Direction, EventSink, ProtoEvent};
pub use model::{
    GrpcFile, MessageFormat, MqttFile, MqttLastWill, MqttPublish, MqttSubscription, MqttVersion, ProtoFile, ProtoKind, SavedMessage, SioEmit,
    SioFile, SioSettings, WsFile, WsSettings,
};

/// Milliseconds since the Unix epoch (event timestamps).
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
