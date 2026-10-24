use std::collections::BTreeSet;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("{0}")]
    Invalid(String),
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("TLS: {0}")]
    Tls(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("the connection is closed")]
    Closed,
    #[error("descriptors: {0}")]
    Descriptor(String),
    #[error(transparent)]
    Core(#[from] devall_core::Error),
}

impl ProtoError {
    /// Message with every secret value masked (for errors shown in the UI).
    pub fn redacted(&self, secrets: &BTreeSet<String>) -> String {
        devall_core::vars::redact_values(&self.to_string(), secrets)
    }
}

pub type Result<T, E = ProtoError> = std::result::Result<T, E>;

/// Full `source()` chain of an error, deduplicated (library top-level messages are often vague).
pub(crate) fn chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let msg = s.to_string();
        if !parts.iter().any(|p| p.contains(&msg)) {
            parts.push(msg);
        }
        cur = s.source();
    }
    parts.join(": ")
}
