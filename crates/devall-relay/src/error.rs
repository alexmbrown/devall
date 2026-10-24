use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("not signed in to the relay")]
    NotSignedIn,
    /// The relay answered the WebSocket upgrade with an HTTP error (PROTOCOL.md §1.1).
    #[error("{message}")]
    Rejected { status: u16, code: String, message: String, retry_after: Option<Duration> },
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("TLS: {0}")]
    Tls(String),
    #[error("WebSocket: {0}")]
    WebSocket(String),
    #[error("HTTP: {0}")]
    Http(String),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("keychain: {0}")]
    Secret(String),
    #[error("sign-in: {0}")]
    SignIn(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = RelayError> = std::result::Result<T, E>;

impl RelayError {
    /// Errors that won't go away by reconnecting (the session loop stops and reports them).
    pub fn is_fatal(&self) -> bool {
        match self {
            RelayError::NotSignedIn | RelayError::Config(_) | RelayError::Auth(_) | RelayError::Secret(_) => true,
            RelayError::Rejected { status, .. } => matches!(status, 400 | 401 | 403 | 404 | 409 | 426 | 451),
            _ => false,
        }
    }

    /// `code` of a relay rejection (`unauthorized`, `too_many_tunnels`, …).
    pub fn code(&self) -> Option<&str> {
        match self {
            RelayError::Rejected { code, .. } => Some(code),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            RelayError::Rejected { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl From<devall_core::Error> for RelayError {
    fn from(e: devall_core::Error) -> Self {
        RelayError::Secret(e.to_string())
    }
}

impl From<reqwest::Error> for RelayError {
    fn from(e: reqwest::Error) -> Self {
        RelayError::Http(chain(&e))
    }
}

/// An error with its sources joined (`a: b: c`), for messages users can act on.
pub fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        let m = c.to_string();
        if !s.contains(&m) {
            s.push_str(": ");
            s.push_str(&m);
        }
        cur = c.source();
    }
    s
}
