//! Per-call approval of `send_request`.
//!
//! Three write modes ([`WriteMode`]): read-only (GET/HEAD/OPTIONS only), *confirm* (every other
//! method waits for the user's answer in the app) and allow (`--allow-writes`). Environments
//! marked `protected` always need an approval — whatever the method and the mode — and are
//! refused outright when nothing can ask (the stdio CLI started by an agent has no UI).
//!
//! The host supplies a [`Confirmer`]: the desktop app shows the request (method, URL,
//! environment, a body preview — secrets masked) with *Allow once* / *Allow for this session* /
//! *Deny*. No answer within [`APPROVAL_TIMEOUT`] is a denial. "For this session" lasts until the
//! MCP server stops and covers the same saved request, method and environment.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
pub(crate) const SAFE_METHODS: &[&str] = &["GET", "HEAD", "OPTIONS"];
/// Body preview length shown to the user.
pub const BODY_PREVIEW_CHARS: usize = 4000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WriteMode {
    /// Only GET, HEAD and OPTIONS are sent.
    #[default]
    ReadOnly,
    /// Other methods wait for the user's approval (needs a [`Confirmer`]).
    Confirm,
    /// Any method is sent (`--allow-writes`).
    Allow,
}

/// What the user is asked to approve. Every string is already redacted.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: String,
    pub tool: String,
    /// Saved request path (relative to `http/`).
    pub request_path: String,
    pub request_name: String,
    pub method: String,
    pub url: String,
    pub environment_id: Option<String>,
    pub environment: Option<String>,
    pub protected: bool,
    /// Why approval is needed, e.g. "POST changes data".
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body_preview: Option<String>,
    pub body_truncated: bool,
    pub timeout_secs: u64,
    /// Unix ms.
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    AllowOnce,
    AllowSession,
    Deny,
}

pub type ApprovalFuture<'a> = Pin<Box<dyn Future<Output = ApprovalDecision> + Send + 'a>>;

/// Asks the user. Implemented by the desktop app (an event + a pending-approval store + a
/// command that answers); tests use fakes.
pub trait Confirmer: Send + Sync {
    fn confirm(&self, request: ApprovalRequest) -> ApprovalFuture<'_>;
}

/// The confirmer plus what was allowed for the rest of the session.
pub struct Approvals {
    confirmer: Arc<dyn Confirmer>,
    allowed: Mutex<HashSet<String>>,
    timeout: Duration,
}

impl Approvals {
    pub fn new(confirmer: Arc<dyn Confirmer>) -> Self {
        Self { confirmer, allowed: Mutex::new(HashSet::new()), timeout: APPROVAL_TIMEOUT }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    fn key(r: &ApprovalRequest) -> String {
        format!("{}\u{0}{}\u{0}{}", r.environment_id.as_deref().unwrap_or(""), r.request_path, r.method)
    }

    /// Allowed already for this session, or asks (no answer in time = denied).
    pub async fn check(&self, request: ApprovalRequest) -> ApprovalDecision {
        let key = Self::key(&request);
        if self.allowed.lock().unwrap().contains(&key) {
            return ApprovalDecision::AllowSession;
        }
        let decision = tokio::time::timeout(self.timeout, self.confirmer.confirm(request)).await.unwrap_or(ApprovalDecision::Deny);
        if decision == ApprovalDecision::AllowSession {
            self.allowed.lock().unwrap().insert(key);
        }
        decision
    }
}

/// What happens to a send before anything is resolved or sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Allow,
    /// Needs the user's approval (the reason is shown).
    Ask(String),
    Refuse(String),
}

/// Decides whether `method` may be sent in `environment` (`(name, protected)`). `can_ask`: a
/// [`Confirmer`] is available.
pub fn send_gate(method: &str, environment: Option<(&str, bool)>, mode: WriteMode, can_ask: bool) -> Gate {
    let m = method.to_ascii_uppercase();
    let safe = SAFE_METHODS.contains(&m.as_str());
    if !safe && mode == WriteMode::ReadOnly {
        return Gate::Refuse(format!(
            "{m} requests are disabled: the MCP server is read-only (GET/HEAD/OPTIONS). Ask the user to allow writes in /dev/all (AI → Agents: \"Ask each time\" or \"Allowed\") or to restart it with `devall mcp --allow-writes`."
        ));
    }
    if let Some((name, true)) = environment {
        return if can_ask {
            Gate::Ask(format!("`{name}` is a protected environment"))
        } else {
            Gate::Refuse(format!(
                "environment `{name}` is protected — this MCP server can't ask the user for approval, so it never sends requests there. Use the /dev/all app (its MCP endpoint asks for each call)."
            ))
        };
    }
    if safe || mode == WriteMode::Allow {
        return Gate::Allow;
    }
    if can_ask {
        Gate::Ask(format!("{m} may change data"))
    } else {
        Gate::Refuse(format!("{m} requests need the user's approval, and this MCP server has no app to ask. Restart it with `devall mcp --allow-writes`, or use the /dev/all app's MCP endpoint."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates() {
        use WriteMode::*;
        assert_eq!(send_gate("get", None, ReadOnly, false), Gate::Allow);
        assert!(matches!(send_gate("POST", None, ReadOnly, true), Gate::Refuse(m) if m.contains("read-only")));
        assert!(matches!(send_gate("POST", None, Confirm, true), Gate::Ask(r) if r.contains("POST")));
        assert!(matches!(send_gate("POST", None, Confirm, false), Gate::Refuse(_)));
        assert_eq!(send_gate("DELETE", Some(("Dev", false)), Allow, false), Gate::Allow);
        // Protected: always asked when possible (even GET and allow-writes), else refused.
        assert!(matches!(send_gate("GET", Some(("Prod", true)), ReadOnly, true), Gate::Ask(r) if r.contains("protected")));
        assert!(matches!(send_gate("POST", Some(("Prod", true)), Allow, true), Gate::Ask(_)));
        assert!(matches!(send_gate("GET", Some(("Prod", true)), Allow, false), Gate::Refuse(m) if m.contains("protected")));
        // Read-only wins over protected for writes: nothing to ask about.
        assert!(matches!(send_gate("PUT", Some(("Prod", true)), ReadOnly, true), Gate::Refuse(m) if m.contains("read-only")));
    }
}
