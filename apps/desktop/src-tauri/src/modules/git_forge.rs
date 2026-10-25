//! Create pull / merge requests (GitHub, GitHub Enterprise, GitLab.com, self-managed GitLab) from
//! Compare & review. The host comes from the remote URL (`devall_forge::detect`); tokens are kept
//! only in the OS keychain (the app's `SecretStore`), added with "Connect GitHub/GitLab" — never
//! in project files, logs or the webview. Without a token, an installed `gh` / `glab` (signed in
//! on its own) does it. Network calls happen only on explicit actions: connect (checks the token)
//! and create (optionally pushing the branch first, after the UI asked).
//!
//! Registration: `pub mod git_forge;` and the commands below.

use std::path::PathBuf;

use devall_core::repos::{compare, git, ops};
use devall_forge::cli::{self, CliStatus};
use devall_forge::{Account, Created, ForgeKind, NewRequest, Remote, Template, TokenHelp};
use serde::Serialize;
use tauri::State;

use super::git::{blocking, repo_of};
use crate::state::AppState;

type R<T> = Result<T, String>;

fn accounts_file() -> R<PathBuf> {
    devall_core::local::app_data_dir().map(|d| devall_forge::accounts_path(&d)).ok_or_else(|| "no app data folder".to_string())
}

fn accounts() -> Vec<Account> {
    accounts_file().map(|p| devall_forge::read_accounts(&p)).unwrap_or_default()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeInfo {
    /// The git remote used (`origin`).
    remote_name: Option<String>,
    /// Its URL, credentials stripped.
    remote_url: Option<String>,
    /// `None`: not GitHub/GitLab (or a self-hosted host that isn't connected yet).
    forge: Option<Remote>,
    /// A token for the host is in the keychain.
    connected: bool,
    account: Option<Account>,
    token_help: Option<TokenHelp>,
    cli: Option<CliStatus>,
    templates: Vec<Template>,
    remotes: Vec<String>,
    /// The OS keychain works (otherwise a token lasts until the app quits).
    keychain: bool,
}

/// Which host the repository's remote is, whether it's connected, the `gh`/`glab` fallback and
/// the repository's PR/MR templates. No network.
#[tauri::command]
pub async fn git_forge_info(app: State<'_, AppState>, root: String, repo: Option<String>, remote: Option<String>) -> R<ForgeInfo> {
    let keychain = app.keychain_available;
    let r = blocking(move || repo_of(&root, repo.as_deref())).await?;
    let dir = r.path.clone();
    let (remote_name, url, remotes) = blocking(move || {
        let remotes = compare::remotes(&dir);
        let name = remote.filter(|n| remotes.contains(n)).or_else(|| compare::main_remote(&dir));
        let url = name.as_deref().and_then(|n| git::run(&dir, &["remote", "get-url", n]).ok()).map(|u| u.trim().to_string());
        Ok((name, url, remotes))
    })
    .await?;
    let accts = accounts();
    let forge = url.as_deref().and_then(|u| devall_forge::detect(u, &accts));
    let connected = forge.as_ref().is_some_and(|f| devall_forge::get_token(app.secrets.as_ref(), &f.host).is_some());
    let account = forge.as_ref().and_then(|f| accts.iter().find(|a| a.host == f.host).cloned());
    let dir = r.path.clone();
    let kind = forge.as_ref().map(|f| f.kind);
    let (cli, templates) = blocking(move || Ok((kind.map(cli::status), kind.map(|k| devall_forge::templates(&dir, k)).unwrap_or_default()))).await?;
    Ok(ForgeInfo {
        token_help: forge.as_ref().map(|f| devall_forge::token_help(f.kind, f.host.split(':').next().unwrap_or(&f.host))),
        remote_url: url.as_deref().map(devall_core::repos::strip_credentials),
        remote_name,
        forge,
        connected,
        account,
        cli,
        templates,
        remotes,
        keychain,
    })
}

#[tauri::command]
pub fn git_forge_accounts() -> Vec<Account> {
    accounts()
}

/// Checks the token with the host (network, on the user's "Connect"), then keeps it in the
/// keychain and the host in the list of connected accounts.
#[tauri::command]
pub async fn git_forge_connect(app: State<'_, AppState>, host: String, kind: ForgeKind, token: String, api_url: Option<String>) -> R<Account> {
    let host = devall_forge::normalize_host(&host).map_err(|e| e.to_string())?;
    if token.trim().is_empty() {
        return Err("paste a token".into());
    }
    let api = api_url.clone().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| devall_forge::api_url_for(kind, &host));
    let user = devall_forge::whoami(kind, &api, &token).await.map_err(|e| e.to_string())?;
    devall_forge::set_token(app.secrets.as_ref(), &host, &token).map_err(|e| e.to_string())?;
    let account = Account { host: host.clone(), kind, user: Some(user), api_url: api_url.filter(|u| !u.trim().is_empty()) };
    let path = accounts_file()?;
    let mut list = devall_forge::read_accounts(&path);
    list.retain(|a| a.host != host);
    list.push(account.clone());
    devall_forge::write_accounts(&path, &list).map_err(|e| e.to_string())?;
    Ok(account)
}

/// Forgets a host's token (keychain) and account.
#[tauri::command]
pub async fn git_forge_disconnect(app: State<'_, AppState>, host: String) -> R<()> {
    devall_forge::delete_token(app.secrets.as_ref(), &host).map_err(|e| e.to_string())?;
    let path = accounts_file()?;
    let mut list = devall_forge::read_accounts(&path);
    list.retain(|a| a.host != host);
    devall_forge::write_accounts(&path, &list).map_err(|e| e.to_string())
}

/// Opens the pull/merge request: pushes `request.head` to `remote` first when `push` (the UI
/// asked), then the host's API with the keychain token — or `gh`/`glab` when `useCli`.
#[tauri::command]
pub async fn git_forge_create(app: State<'_, AppState>, root: String, repo: Option<String>, remote: String, request: NewRequest, push: bool, use_cli: bool) -> R<Created> {
    let r = blocking(move || repo_of(&root, repo.as_deref())).await?;
    let dir = r.path.clone();
    let remote_name = remote.clone();
    let url = blocking(move || git::run(&dir, &["remote", "get-url", &remote_name]).map(|u| u.trim().to_string())).await?;
    let forge = devall_forge::detect(&url, &accounts()).ok_or_else(|| format!("{} isn't a GitHub or GitLab remote — connect its host first", devall_core::repos::strip_credentials(&url)))?;
    if push {
        let dir = r.path.clone();
        let (remote, head) = (remote.clone(), request.head.clone());
        blocking(move || ops::push_branch(&dir, &remote, &head)).await?;
    }
    if !use_cli {
        let token = devall_forge::get_token(app.secrets.as_ref(), &forge.host).ok_or_else(|| format!("no token for {} — connect it, or use the {} CLI", forge.host, cli::program(forge.kind)))?;
        return devall_forge::create(&forge, &token, &request).await.map_err(|e| e.to_string());
    }
    let dir = r.path.clone();
    blocking(move || cli::create(&dir, &forge, &request).map_err(|e| e.to_string())).await
}
