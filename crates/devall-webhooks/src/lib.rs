//! Webhooks for /dev/all: a local inbox that receives, verifies, forwards, replays (with
//! re-signing) and converts webhooks — free and local — plus bring-your-own tunnels.
//!
//! * [`model`] — `webhooks/<slug>.webhook.yaml` inbox definitions (provider, secret reference,
//!   canned response, forward target, public URL, tolerance).
//! * [`receiver`] — one HTTP server per project on `127.0.0.1:4590` (or `0.0.0.0` for the LAN)
//!   routing by inbox path; unknown paths are captured too (catch-all).
//! * [`hub`] — the pipeline shared by the receiver and [`WebhookHub::ingest`] (the entry point for
//!   the hosted relay): store → verify ∥ forward; plus replay, re-sign and cURL.
//! * [`store`] — SQLite history in `<project data>/webhooks.sqlite` (on the device) with live notifications.
//! * [`verify`] — Stripe, GitHub, Shopify, Slack, Twilio, Standard Webhooks and generic HMAC
//!   verification with explanations, and re-signing with fresh timestamps.
//! * [`convert`] — event → HTTP request file (re-signing pre-request script), cURL, fixture.
//! * [`tunnels`] — detect and run ngrok / cloudflared / Tailscale Funnel.
//!
//! Secrets are only ever referenced (`{{secret.NAME}}`) and resolved through a
//! [`TemplateResolver`] supplied by the app; no secret value is returned or persisted.

pub mod convert;
pub mod event;
pub mod forward;
pub mod hub;
pub mod inbox;
pub mod model;
pub mod receiver;
pub mod store;
pub mod tunnels;
pub mod verify;

pub use event::{EventPage, EventQuery, EventSource, EventSummary, EventView, ForwardResult, IncomingEvent, Retention, StoreNotice, WebhookEvent};
pub use hub::{NoTemplates, ReceivedHook, ReplayEdits, ReplayOptions, StaticTemplates, TemplateResolver, WebhookHub};
pub use inbox::InboxEntry;
pub use model::{ForwardSpec, GenericHmac, HmacAlgorithm, InboxFile, Issue, ProviderInfo, ProviderKind, ResponseSpec, SignatureEncoding};
pub use receiver::{DEFAULT_PORT, Receiver, ReceiverConfig, ReceiverInfo};
pub use store::EventStore;
pub use tunnels::{ToolInfo, Tunnel, TunnelKind, TunnelState, TunnelStatus};
pub use verify::{VerifyResult, VerifyStatus};

#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error(transparent)]
    Project(#[from] devall_core::Error),
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Secret(String),
    #[error("could not listen on {addr}: {message}")]
    Bind { addr: String, message: String },
    #[error("{0}")]
    Io(String),
}

#[cfg(test)]
mod tests;
