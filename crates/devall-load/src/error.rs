use crate::validate::Issue;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Core(#[from] devall_core::Error),
    #[error("invalid load test:\n{}", .0.iter().map(|i| format!("  - {i}")).collect::<Vec<_>>().join("\n"))]
    Validation(Vec<Issue>),
    /// A scenario request could not be read or prepared.
    #[error("{path}: {message}")]
    Request { path: String, message: String },
    /// The run needs explicit confirmation (`--yes` on the CLI, a dialog in the app).
    #[error("confirmation required: {0}")]
    ConfirmationRequired(String),
    #[error("{0}")]
    Invalid(String),
    #[error("load engine failed: {0}")]
    Engine(String),
    /// A distributed run would send resolved secrets to agents without the user's consent.
    #[error("{0}")]
    SecretsToAgents(String),
    /// A load agent could not be reached, refused the plan or failed.
    #[error("agent {address}: {message}")]
    Agent { address: String, message: String },
}

pub type Result<T, E = LoadError> = std::result::Result<T, E>;
