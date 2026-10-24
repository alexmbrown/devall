use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum NotebookError {
    /// The interpreter for a language isn't installed; the message says how to fix it.
    #[error("{0}")]
    InterpreterMissing(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("could not start {program}: {source}")]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Kernel(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Core(#[from] devall_core::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = NotebookError> = std::result::Result<T, E>;
