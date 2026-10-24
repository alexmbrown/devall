#[derive(Debug, thiserror::Error)]
pub enum DataError {
    /// The schema is malformed (bad parameter, unknown reference, …).
    #[error("{0}")]
    Invalid(String),
    #[error("unknown entity '{0}'")]
    UnknownEntity(String),
    /// Entities reference each other in a loop, or fields depend on each other in a loop.
    #[error("dependency cycle: {0}")]
    Cycle(String),
    /// A unique field ran out of distinct values.
    #[error("{0}")]
    Unique(String),
    #[error("cannot parse input: {0}")]
    Parse(String),
    #[error(transparent)]
    Core(#[from] devall_core::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = DataError> = std::result::Result<T, E>;

pub(crate) fn invalid(msg: impl Into<String>) -> DataError {
    DataError::Invalid(msg.into())
}
