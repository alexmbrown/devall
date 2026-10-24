#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Parse(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, Error>;
