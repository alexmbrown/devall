use std::path::Path;

/// Everything that can go wrong loading, installing or running a plugin. Messages are stable and
/// deterministic (no addresses, no backtraces) so they can be shown as-is and asserted in tests.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    #[error("invalid plugin manifest: {0}")]
    Manifest(String),
    #[error("plugin {0} is not installed")]
    NotFound(String),
    #[error("plugin {plugin} has no {kind} contribution `{id}`")]
    NoContribution { plugin: String, kind: &'static str, id: String },
    #[error("plugin {0} is disabled")]
    Disabled(String),
    #[error("plugin {0} is not trusted on this machine (review its capabilities and trust it first)")]
    NotTrusted(String),
    #[error("plugin {0} changed since it was approved (its code or capabilities differ): review and approve it again")]
    Changed(String),
    #[error("plugin {plugin}: invalid WebAssembly module: {detail}")]
    InvalidModule { plugin: String, detail: String },
    #[error("plugin {plugin}: {detail}")]
    Abi { plugin: String, detail: String },
    #[error("plugin {plugin}: exceeded its instruction budget ({fuel} fuel units)")]
    OutOfFuel { plugin: String, fuel: u64 },
    #[error("plugin {plugin}: timed out after {ms} ms")]
    Timeout { plugin: String, ms: u64 },
    #[error("plugin {plugin}: exceeded its memory limit ({mib} MiB)")]
    MemoryLimit { plugin: String, mib: u64 },
    #[error("plugin {plugin}: crashed ({detail})")]
    Trap { plugin: String, detail: String },
    #[error("plugin {plugin}: capability denied: {detail}")]
    CapabilityDenied { plugin: String, detail: String },
    /// The plugin ran and reported an error itself (`{"error": "…"}`).
    #[error("plugin {plugin}: {message}")]
    Failed { plugin: String, message: String },
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Invalid(String),
}

impl PluginError {
    pub(crate) fn io(path: &Path, e: impl std::fmt::Display) -> Self {
        PluginError::Io(format!("{}: {e}", path.display()))
    }
}

impl From<devall_core::Error> for PluginError {
    fn from(e: devall_core::Error) -> Self {
        PluginError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, PluginError>;
