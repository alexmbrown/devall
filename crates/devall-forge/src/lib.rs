//! Git hosting for Source control: open a pull request (GitHub, GitHub Enterprise) or merge
//! request (GitLab.com, self-managed GitLab) from a comparison — for any repository, private and
//! self-hosted included.
//!
//! - [`detect`]: which host a remote URL is (credentials in the URL are never kept), its API and
//!   web addresses. github.com / `*github*` hosts are GitHub, `*gitlab*` hosts GitLab; any other
//!   host is whatever the user connected it as ([`Account`]).
//! - Tokens live only in the OS keychain ([`TOKEN_SCOPE`], one entry per host). The list of
//!   connected hosts (no tokens) is `<app data>/git-forges.json`.
//! - [`create`] calls the host's REST API; [`cli`] falls back to an installed, signed-in `gh` /
//!   `glab`. Nothing here talks to the network unless the user asked (connect, create).
//! - Errors never contain the token ([`clean`]).

pub mod cli;
mod github;
mod gitlab;

use std::path::{Path, PathBuf};
use std::time::Duration;

use devall_core::secrets::{ALL_ENVIRONMENTS, SecretScope};
use devall_core::SecretStore;
use serde::{Deserialize, Serialize};

pub const ACCOUNTS_FILE: &str = "git-forges.json";

/// Keychain scope of host tokens (entry name = host, e.g. `github.com`).
pub fn token_scope() -> SecretScope {
    SecretScope::new("devall-forge", ALL_ENVIRONMENTS)
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("{0}")]
    Api(String),
    #[error("{0}")]
    Invalid(String),
    #[error("no token for {0} — connect it first")]
    NoToken(String),
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Cli(String),
}

pub type Result<T> = std::result::Result<T, ForgeError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForgeKind {
    Github,
    Gitlab,
}

impl ForgeKind {
    pub fn name(self) -> &'static str {
        match self {
            ForgeKind::Github => "GitHub",
            ForgeKind::Gitlab => "GitLab",
        }
    }
    /// "pull request" / "merge request".
    pub fn noun(self) -> &'static str {
        match self {
            ForgeKind::Github => "pull request",
            ForgeKind::Gitlab => "merge request",
        }
    }
}

/// A connected host (its token is in the keychain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// `github.com`, `gitlab.acme.dev:8443`.
    pub host: String,
    pub kind: ForgeKind,
    /// The user the token belongs to.
    #[serde(default)]
    pub user: Option<String>,
    /// Override of the API base (tests, unusual installations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
}

/// A repository on a host, from its remote URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub kind: ForgeKind,
    /// Host (with a non-default port for web URLs) — the key of its token.
    pub host: String,
    /// `owner/repo`, `group/sub/project`.
    pub path: String,
    /// `https://github.com/owner/repo`.
    pub web_url: String,
    /// `https://api.github.com`, `https://ghe.acme.dev/api/v3`, `https://gitlab.com/api/v4`.
    pub api_url: String,
}

/// (scheme, host with port for http(s), path) of a remote URL; credentials dropped.
pub fn parse_url(url: &str) -> Option<(String, String, String)> {
    let u = url.trim();
    let strip_git = |p: &str| p.trim_matches('/').trim_end_matches(".git").trim_matches('/').to_string();
    if let Some((scheme, rest)) = u.split_once("://") {
        let (auth, path) = rest.split_once('/')?;
        let hostport = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
        let scheme = scheme.to_ascii_lowercase();
        let host = match scheme.as_str() {
            "http" | "https" => hostport.to_ascii_lowercase(),
            // ssh://git@host:2222/path — the web UI isn't on the SSH port.
            _ => hostport.split(':').next()?.to_ascii_lowercase(),
        };
        let path = strip_git(path);
        return (!host.is_empty() && !path.is_empty()).then_some((scheme, host, path));
    }
    // scp-like: git@host:owner/repo.git
    let (left, path) = u.split_once(':')?;
    if path.starts_with('/') && left.len() == 1 {
        return None; // C:/path
    }
    let host = left.rsplit_once('@').map(|(_, h)| h).unwrap_or(left).to_ascii_lowercase();
    let path = strip_git(path);
    (!host.is_empty() && !path.is_empty() && !host.contains('/')).then(|| ("ssh".to_string(), host, path))
}

/// The host and repository a remote URL points at, or `None` for hosts that aren't GitHub or
/// GitLab (and weren't connected as one).
pub fn detect(url: &str, accounts: &[Account]) -> Option<Remote> {
    let (scheme, host, path) = parse_url(url)?;
    let web_scheme = if scheme == "http" { "http" } else { "https" };
    let bare = host.split(':').next().unwrap_or(&host).to_string();
    let account = accounts.iter().find(|a| a.host.eq_ignore_ascii_case(&host) || a.host.eq_ignore_ascii_case(&bare));
    let kind = match account {
        Some(a) => a.kind,
        None if bare == "github.com" || bare.contains("github") => ForgeKind::Github,
        None if bare.contains("gitlab") => ForgeKind::Gitlab,
        None => return None,
    };
    let key = account.map(|a| a.host.clone()).unwrap_or_else(|| host.clone());
    let web_url = format!("{web_scheme}://{host}/{path}");
    let api_url = account.and_then(|a| a.api_url.clone()).unwrap_or_else(|| match kind {
        ForgeKind::Github if bare == "github.com" => "https://api.github.com".into(),
        ForgeKind::Github => format!("{web_scheme}://{host}/api/v3"),
        ForgeKind::Gitlab => format!("{web_scheme}://{host}/api/v4"),
    });
    Some(Remote { kind, host: key, path, web_url, api_url })
}

/// Where to make a token and which scopes it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenHelp {
    pub url: String,
    pub scopes: String,
}

pub fn token_help(kind: ForgeKind, host: &str) -> TokenHelp {
    match kind {
        ForgeKind::Github => TokenHelp {
            url: format!("https://{host}/settings/tokens/new?scopes=repo&description=devall"),
            scopes: "classic token: repo (or public_repo for public repositories only) · fine-grained token: Pull requests read & write, Contents read".into(),
        },
        ForgeKind::Gitlab => TokenHelp { url: format!("https://{host}/-/user_settings/personal_access_tokens?name=devall&scopes=api"), scopes: "api".into() },
    }
}

// ---------- accounts (no tokens) ----------

pub fn accounts_path(app_data: &Path) -> PathBuf {
    app_data.join(ACCOUNTS_FILE)
}

pub fn read_accounts(path: &Path) -> Vec<Account> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn write_accounts(path: &Path, list: &[Account]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| ForgeError::Invalid(e.to_string()))?;
    }
    let json = serde_json::to_vec_pretty(list).map_err(|e| ForgeError::Invalid(e.to_string()))?;
    devall_core::fsutil::atomic_write(path, &json).map_err(|e| ForgeError::Invalid(e.to_string()))
}

/// Normalizes what the user typed as a host (`https://gitlab.acme.dev/` → `gitlab.acme.dev`).
pub fn normalize_host(h: &str) -> Result<String> {
    let h = h.trim().trim_end_matches('/');
    let h = h.split_once("://").map(|(_, r)| r).unwrap_or(h);
    let h = h.split('/').next().unwrap_or_default().rsplit('@').next().unwrap_or_default().to_ascii_lowercase();
    if h.is_empty() || h.chars().any(|c| c.is_whitespace() || c.is_control()) || h.starts_with('-') {
        return Err(ForgeError::Invalid("that isn't a host name".into()));
    }
    Ok(h)
}

pub fn get_token(store: &dyn SecretStore, host: &str) -> Option<String> {
    store.get(&token_scope(), host).ok().flatten().filter(|t| !t.trim().is_empty())
}

pub fn set_token(store: &dyn SecretStore, host: &str, token: &str) -> Result<()> {
    store.set(&token_scope(), host, token.trim()).map_err(|e| ForgeError::Invalid(format!("couldn't store the token in the keychain: {e}")))
}

pub fn delete_token(store: &dyn SecretStore, host: &str) -> Result<()> {
    store.delete(&token_scope(), host).map_err(|e| ForgeError::Invalid(e.to_string()))
}

/// Replaces the token wherever it shows up in a message.
pub fn clean(msg: &str, token: &str) -> String {
    let t = token.trim();
    if t.len() >= 4 { msg.replace(t, "••••••") } else { msg.to_string() }
}

// ---------- the API ----------

/// What to open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewRequest {
    pub title: String,
    pub body: String,
    /// Target branch (name on the host).
    pub base: String,
    /// Source branch (name on the host).
    pub head: String,
    pub draft: bool,
    /// User names (GitHub teams as `org/team`).
    pub reviewers: Vec<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub number: u64,
    pub url: String,
    /// Parts that didn't work (a reviewer that doesn't exist…) — the request itself was opened.
    pub warnings: Vec<String>,
    /// Opened through `gh` / `glab`.
    pub via_cli: bool,
}

pub(crate) fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("devall/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| ForgeError::Network(e.to_string()))
}

/// An HTTP response read in full: status and body (JSON or text).
pub(crate) struct Reply {
    pub status: u16,
    pub json: serde_json::Value,
    pub text: String,
}

pub(crate) async fn send(req: reqwest::RequestBuilder, token: &str) -> Result<Reply> {
    let resp = req.send().await.map_err(|e| ForgeError::Network(clean(&e.without_url().to_string(), token)))?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await.map_err(|e| ForgeError::Network(clean(&e.without_url().to_string(), token)))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(Reply { status, json: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null), text })
}

pub(crate) fn json_body(v: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

fn validate(req: &NewRequest) -> Result<()> {
    if req.title.trim().is_empty() {
        return Err(ForgeError::Invalid("give it a title".into()));
    }
    if req.base.trim().is_empty() || req.head.trim().is_empty() {
        return Err(ForgeError::Invalid("pick the source and target branches".into()));
    }
    if req.base == req.head {
        return Err(ForgeError::Invalid("source and target are the same branch".into()));
    }
    Ok(())
}

/// Opens the pull/merge request with the host's API.
pub async fn create(remote: &Remote, token: &str, req: &NewRequest) -> Result<Created> {
    validate(req)?;
    let client = client()?;
    match remote.kind {
        ForgeKind::Github => github::create(&client, remote, token, req).await,
        ForgeKind::Gitlab => gitlab::create(&client, remote, token, req).await,
    }
}

/// The user a token belongs to (checks the token works). Network.
pub async fn whoami(remote_kind: ForgeKind, api_url: &str, token: &str) -> Result<String> {
    let client = client()?;
    match remote_kind {
        ForgeKind::Github => github::whoami(&client, api_url, token).await,
        ForgeKind::Gitlab => gitlab::whoami(&client, api_url, token).await,
    }
}

/// The API base for a host the user connects by hand.
pub fn api_url_for(kind: ForgeKind, host: &str) -> String {
    let bare = host.split(':').next().unwrap_or(host);
    match kind {
        ForgeKind::Github if bare == "github.com" => "https://api.github.com".into(),
        ForgeKind::Github => format!("https://{host}/api/v3"),
        ForgeKind::Gitlab => format!("https://{host}/api/v4"),
    }
}

/// A message from an error response (`message`, `errors[].message`, GitLab's `message` arrays).
pub(crate) fn api_error(what: &str, r: &Reply, token: &str) -> ForgeError {
    let mut parts: Vec<String> = Vec::new();
    match &r.json["message"] {
        serde_json::Value::String(s) => parts.push(s.clone()),
        serde_json::Value::Array(a) => parts.extend(a.iter().filter_map(|x| x.as_str().map(str::to_string))),
        serde_json::Value::Object(o) => parts.extend(o.iter().map(|(k, v)| format!("{k} {}", v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_else(|| v.to_string())))),
        _ => {}
    }
    if let Some(errs) = r.json["errors"].as_array() {
        parts.extend(errs.iter().filter_map(|e| e["message"].as_str().map(str::to_string).or_else(|| e.as_str().map(str::to_string))));
    }
    if let Some(e) = r.json["error"].as_str() {
        parts.push(e.to_string());
    }
    if parts.is_empty() {
        parts.push(r.text.chars().take(300).collect());
    }
    let hint = match r.status {
        401 => " — the token is invalid or expired; connect again",
        403 => " — the token lacks a scope or access to this repository",
        404 => " — repository not found (or the token can't see it)",
        _ => "",
    };
    ForgeError::Api(clean(&format!("{what} failed ({}): {}{hint}", r.status, parts.join("; ")), token))
}

// ---------- templates ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub name: String,
    /// Relative to the repository.
    pub path: String,
    pub content: String,
}

const MAX_TEMPLATE: u64 = 64 * 1024;

fn read_template(repo: &Path, rel: &str) -> Option<Template> {
    let p = repo.join(rel);
    let meta = std::fs::metadata(&p).ok()?;
    if !meta.is_file() || meta.len() > MAX_TEMPLATE {
        return None;
    }
    let content = std::fs::read_to_string(&p).ok()?;
    let name = Path::new(rel).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| rel.to_string());
    Some(Template { name, path: rel.replace('\\', "/"), content })
}

fn dir_templates(repo: &Path, rel_dir: &str) -> Vec<Template> {
    let Ok(rd) = std::fs::read_dir(repo.join(rel_dir)) else { return vec![] };
    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.to_lowercase().ends_with(".md")).collect();
    names.sort_by_key(|n| (n.to_lowercase() != "default.md", n.to_lowercase()));
    names.iter().filter_map(|n| read_template(repo, &format!("{rel_dir}/{n}"))).collect()
}

/// The repository's PR / MR templates, the default one first.
pub fn templates(repo: &Path, kind: ForgeKind) -> Vec<Template> {
    let mut out = Vec::new();
    match kind {
        ForgeKind::Github => {
            for rel in [".github/pull_request_template.md", ".github/PULL_REQUEST_TEMPLATE.md", "docs/pull_request_template.md", "docs/PULL_REQUEST_TEMPLATE.md", "pull_request_template.md", "PULL_REQUEST_TEMPLATE.md"] {
                if let Some(t) = read_template(repo, rel)
                    && !out.iter().any(|x: &Template| x.content == t.content)
                {
                    out.push(t);
                }
            }
            for d in [".github/PULL_REQUEST_TEMPLATE", "docs/PULL_REQUEST_TEMPLATE", "PULL_REQUEST_TEMPLATE"] {
                out.extend(dir_templates(repo, d));
            }
        }
        ForgeKind::Gitlab => out.extend(dir_templates(repo, ".gitlab/merge_request_templates")),
    }
    out
}

#[cfg(test)]
mod tests;
