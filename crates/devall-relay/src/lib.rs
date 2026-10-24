//! Client for the /dev/all relay (Group 2 — hosted, or self-hosted for free): reverse tunnels to
//! localhost and the hosted webhook inbox.
//!
//! - [`RelayClient`] — one relay (hosted `wss://relay.devall.dev` or a self-hosted URL) with
//!   Firebase (hosted) or static-token (self-hosted) credentials kept in the OS keychain.
//! - [`RelayClient::start_tunnel`] — public `https://<sub>.<domain>` → `http://127.0.0.1:PORT`,
//!   HTTP with streaming bodies both ways and WebSocket pass-through, multiplexed over one
//!   WebSocket on port 443, with auto-reconnect.
//! - [`RelayClient::connect_inbox`] — webhooks captured by the relay while the laptop was away,
//!   delivered in order and acknowledged after the app's handler stored them.
//! - [`signin`] — loopback browser sign-in for the hosted relay.
//! - [`RelayClient::start_tcp_tunnel`] / [`RelayClient::tcp_connect`] — raw TCP tunnels (TCP over
//!   WebSocket; owner account or a `dtcp_…` access secret), see [`tcp`].
//! - [`RelayClient::api`] / [`RelayClient::publish_mock`] / [`RelayClient::function`] — the hosted
//!   extras: hosted mocks ([`hosted_mock`] checks stubs against the edge's WireMock subset first),
//!   shared snapshots, scheduled monitors and custom domains (PROTOCOL.md §6).
//!
//! The wire protocol is specified in `relay/PROTOCOL.md`; the edge lives in `relay/worker`.

pub mod account;
pub mod api;
pub mod auth;
pub mod client;
pub mod error;
pub mod hosted;
pub mod hosted_mock;
pub mod inbox;
pub mod protocol;
pub mod session;
pub mod signin;
pub mod tcp;
pub mod transport;
pub mod tunnel;

pub use account::{AccountInbox, AccountInfo, AccountTunnel, CustomDomain, CustomDomainRecord, HostedMockRef};
pub use api::{MockPublished, MockWarning};
pub use hosted_mock::{MockPayload, MockSettings, PublishPlan};
pub use auth::{RelayAuth, TokenProvider};
pub use client::{RelayClient, RelayConfig, normalize_endpoint};
pub use error::{RelayError, Result};
pub use inbox::{InboxDelivery, InboxHandle, InboxHandler, InboxOptions, InboxState, InboxUpdate, handler};
/// HTTP method for [`RelayClient::api`] (re-exported so callers don't need reqwest).
pub use reqwest::Method as ApiMethod;
pub use protocol::{InboxEvent, InboxResponse, Limits, OfflineInbox, QuotaPayload, Usage};
pub use session::{ConnState, ReconnectPolicy};
pub use signin::SignedIn;
pub use tcp::{TcpAccess, TcpConnLog, TcpConnectEvent, TcpConnectHandle, TcpConnectOptions, TcpConnectStatus, TcpTunnelEvent, TcpTunnelHandle, TcpTunnelOptions, TcpTunnelStatus};
pub use tunnel::{RequestLog, TunnelEvent, TunnelHandle, TunnelOptions, TunnelStatus};
