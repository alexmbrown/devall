use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a /dev/all project (no devall.yaml in {0})")]
    NotAProject(PathBuf),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("{0} already exists")]
    AlreadyExists(String),
    #[error("{0} not found")]
    NotFound(String),
    #[error("secret store: {0}")]
    Secret(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) trait IoContext<T> {
    fn at(self, path: &Path) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: &Path) -> Result<T> {
        self.map_err(|source| Error::Io { path: path.to_path_buf(), source })
    }
}
