use std::collections::BTreeSet;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Core(#[from] devall_core::Error),
    #[error(transparent)]
    Data(#[from] devall_data::DataError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("{0} not found")]
    NotFound(String),
    /// `{{references}}` that could not be resolved (variable or secret names, never values).
    #[error("unresolved {}: {} — define {} in the active environment (secrets: Environments → Secrets)", if .0.len() == 1 { "reference" } else { "references" }, .0.iter().map(|r| format!("{{{{{r}}}}}")).collect::<Vec<_>>().join(", "), if .0.len() == 1 { "it" } else { "them" })]
    Unresolved(Vec<String>),
    /// A statement/command was blocked because the session is read-only.
    #[error("{0}")]
    ReadOnly(String),
    #[error("cannot connect: {0}")]
    Connect(String),
    #[error("{0}")]
    Query(String),
    #[error("timed out after {0}")]
    Timeout(String),
    #[error("cancelled")]
    Cancelled,
    #[error("TLS: {0}")]
    Tls(String),
    #[error("SSH: {0}")]
    Ssh(String),
    /// The SSH server's host key is not in `known_hosts`; the UI asks the user to trust it
    /// (trust on first use) and reconnects with the fingerprint.
    #[error("the SSH host key of {host}:{port} is not known ({key_type} {fingerprint})")]
    HostKeyUnknown { host: String, port: u16, key_type: String, fingerprint: String },
    #[error(
        "the SSH host key of {host}:{port} does not match ~/.ssh/known_hosts (line {line}); presented {fingerprint}. This can mean a man-in-the-middle attack — fix known_hosts if the server was legitimately re-keyed"
    )]
    HostKeyChanged { host: String, port: u16, fingerprint: String, line: usize },
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("not connected (the session was closed)")]
    Closed,
}

pub type Result<T, E = DbError> = std::result::Result<T, E>;

impl DbError {
    /// The error message with every secret value masked.
    pub fn redacted(&self, secrets: &BTreeSet<String>) -> String {
        devall_core::vars::redact_values(&self.to_string(), secrets)
    }
}

pub(crate) fn invalid(msg: impl Into<String>) -> DbError {
    DbError::Invalid(msg.into())
}
