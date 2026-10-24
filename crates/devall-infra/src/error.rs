use std::error::Error as StdError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No reachable container engine socket.
    #[error("no container engine found. Tried: {0}")]
    NoEngine(String),
    #[error("{0}")]
    Docker(String),
    #[error("{0}")]
    Kube(String),
    #[error("kubeconfig: {0}")]
    Kubeconfig(String),
    /// A kubeconfig exec credential plugin (gke-gcloud-auth-plugin, aws, kubelogin, …) is missing
    /// or failed. The message is written for humans and includes an install hint.
    #[error("{0}")]
    AuthPlugin(String),
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("{0}")]
    Request(String),
    #[error("`{command}` failed: {message}")]
    Command { command: String, message: String },
    /// A command-line tool (docker, gcloud, …) could not be found.
    #[error("{0}")]
    ToolMissing(String),
    /// A write against a real cloud project while the target is read-only.
    #[error("{0}")]
    ReadOnly(String),
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Core(#[from] devall_core::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<bollard::errors::Error> for Error {
    fn from(e: bollard::errors::Error) -> Self {
        use bollard::errors::Error as B;
        match e {
            B::DockerResponseServerError { status_code, message } => Error::Docker(format!("{status_code}: {message}")),
            other => Error::Docker(chain(&other)),
        }
    }
}

impl From<kube::Error> for Error {
    fn from(e: kube::Error) -> Self {
        if let kube::Error::Api(status) = &e {
            let reason = if status.reason.is_empty() { String::new() } else { format!(" {}", status.reason) };
            return Error::Kube(format!("{}{reason}: {}", status.code, status.message));
        }
        // Auth plugin failures arrive wrapped (Service → AuthError); surface them readably.
        let mut src: Option<&(dyn StdError + 'static)> = Some(&e);
        while let Some(s) = src {
            if let Some(auth) = s.downcast_ref::<kube::client::AuthError>() {
                return Error::AuthPlugin(describe_auth_error(auth));
            }
            src = s.source();
        }
        Error::Kube(chain(&e))
    }
}

impl From<kube::config::KubeconfigError> for Error {
    fn from(e: kube::config::KubeconfigError) -> Self {
        Error::Kubeconfig(chain(&e))
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Request(chain(&e))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Invalid(format!("invalid JSON: {e}"))
    }
}

fn describe_auth_error(e: &kube::client::AuthError) -> String {
    use kube::client::AuthError as A;
    match e {
        A::AuthExecStart(io) => format!(
            "could not start the kubeconfig auth plugin: {io}. Install it or put it on PATH, then retry (see the context's install hint)."
        ),
        A::AuthExecRun { cmd, status, out } => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let detail = if stderr.trim().is_empty() { stdout.trim().to_string() } else { stderr.trim().to_string() };
            let detail: String = detail.chars().take(1200).collect();
            format!(
                "the auth plugin {cmd} exited with {status}: {detail}\nIf it needs an interactive login (browser or device code), run `kubectl get ns` once in a terminal, then retry."
            )
        }
        other => format!("kubeconfig authentication failed: {}", chain(other)),
    }
}

/// `error: cause: cause` — the full source chain on one line, without repeats.
pub(crate) fn chain(e: &(dyn StdError + 'static)) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        let text = s.to_string();
        if !text.is_empty() && !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        src = s.source();
    }
    out
}
