use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The input looked like the format but could not be read.
    #[error("{0}")]
    Parse(String),
    /// A known format (or version) we deliberately don't read.
    #[error("{0}")]
    Unsupported(String),
    #[error(
        "could not recognize the format (supported: Postman, Insomnia, Bruno, OpenCollection, OpenAPI/Swagger, HAR, .http, Hurl, cURL)"
    )]
    UnknownFormat,
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Core(#[from] devall_core::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn io_err(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io { path: path.to_path_buf(), source }
}
