//! Infrastructure viewers for /dev/all: Docker (any engine socket), Kubernetes (kubeconfig with
//! exec auth plugins), a port-forward manager that binds forwarded URLs into HTTP environments,
//! and Google Cloud emulator-first clients (Pub/Sub, Firestore/Datastore, Storage, Logging).
//!
//! Pure Rust, no Tauri: streams report through [`stream::Sink`] callbacks and long-running work
//! is plain async functions the caller spawns (and aborts) as tasks.
//!
//! | Module | What |
//! |---|---|
//! | [`docker`] | engine detection, containers/images/volumes/networks, logs, stats, exec, compose |
//! | [`kube`] | contexts, clients, generic resources, watch, logs, exec, delete/restart/scale |
//! | [`forward`] | `connections/*.forward.yaml`, forwards, [`forward::RuntimeOverrides`] for `{{vars}}` |
//! | [`gcp`] | Pub/Sub, Firestore, Datastore, Cloud Storage, Cloud Logging |

pub mod docker;
pub mod error;
pub mod forward;
pub mod gcp;
pub mod kube;
pub mod stream;
pub mod sys;
pub mod tls;

pub use error::{Error, Result};
