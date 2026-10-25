//! Core of /dev/all: the on-disk project format, variable resolution and secrets.
//!
//! Everything a user creates lives as plain YAML files inside a project folder (see
//! `docs/IMPLEMENTATION_PLAN.md`, "Project format v1"). Secret *values* never touch those
//! files: they are referenced as `{{secret.NAME}}` and resolved from the OS keychain or a
//! git-ignored `.env` file at send time.

pub mod context;
pub mod curl;
pub mod error;
pub mod fsutil;
pub mod local;
pub mod model;
pub mod notebook;
pub mod notes;
pub mod project;
pub mod providers;
pub mod regexes;
pub mod repos;
pub mod scan;
pub mod schema;
pub mod secrets;
pub mod todos;
pub mod transfer;
pub mod tree;
pub mod usages;
pub mod vars;

pub use error::{Error, Result};
pub use model::*;
pub use project::{EnvironmentEntry, Project};
pub use secrets::{MemorySecretStore, SecretResolver, SecretScope, SecretStore};
pub use tree::TreeNode;
pub use vars::{Resolver, SecretLookup};

#[cfg(feature = "keychain")]
pub use secrets::KeychainSecretStore;
