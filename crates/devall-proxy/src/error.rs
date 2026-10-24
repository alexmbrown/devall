use std::path::PathBuf;

use crate::model::ExchangeId;

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("could not listen on {addr}: {source}")]
    Bind { addr: String, source: std::io::Error },
    #[error("I/O error at {path}: {source}")]
    IoAt { path: PathBuf, source: std::io::Error },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("certificate error: {0}")]
    Cert(String),
    #[error("TLS configuration error: {0}")]
    Tls(String),
    #[error("exchange {0} is not in the capture (it may have been evicted or cleared)")]
    NotFound(ExchangeId),
    #[error("exchange {0} is not paused at a breakpoint")]
    NotPaused(ExchangeId),
    #[error("invalid rule \"{rule}\": {message}")]
    InvalidRule { rule: String, message: String },
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Core(#[from] devall_core::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ProxyError>;

pub(crate) trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| ProxyError::IoAt { path: path.into(), source })
    }
}
