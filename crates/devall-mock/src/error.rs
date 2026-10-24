use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum MockError {
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Core(#[from] devall_core::Error),
    #[error("invalid stub mapping: {0}")]
    InvalidStub(String),
    #[error("{0}")]
    Invalid(String),
}

impl MockError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io { path: path.to_path_buf(), source }
    }
}

pub type Result<T, E = MockError> = std::result::Result<T, E>;
