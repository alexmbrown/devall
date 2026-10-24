//! SSH tunnels (local port forwarding) with `russh` — pure Rust, no libssh2/OpenSSL.
//!
//! A tunnel listens on `127.0.0.1:<random port>` and forwards each accepted socket through a
//! `direct-tcpip` channel to the database host as seen from the SSH server. Tunnels are shared
//! and reference-counted: every session to the same `(ssh server, target)` holds a [`Tunnel`];
//! the SSH connection closes when the last one is dropped.
//!
//! Host keys are checked against `~/.ssh/known_hosts`. An unknown host fails with
//! [`DbError::HostKeyUnknown`] (carrying the SHA-256 fingerprint); after the user confirms it in
//! the app, connecting again with `trust = Some(fingerprint)` accepts exactly that key and appends
//! it to `known_hosts` (trust on first use). A changed key is always rejected.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use russh::client::{self, AuthResult, Handle};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::AbortHandle;

use crate::error::{DbError, Result};
use crate::model::HostKeyCheck;
use crate::resolve::{SshAuth, SshTarget, home_dir};

/// `~/.ssh/known_hosts`.
pub fn default_known_hosts() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".ssh").join("known_hosts"))
}

pub struct SshHandler {
    host: String,
    port: u16,
    check: HostKeyCheck,
    trust: Option<String>,
    known_hosts: Option<PathBuf>,
    issue: Arc<Mutex<Option<DbError>>>,
}

impl SshHandler {
    fn verdict(&mut self, key: &PublicKey) -> bool {
        if self.check == HostKeyCheck::Off {
            return true;
        }
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let known = match &self.known_hosts {
            Some(p) if p.is_file() => russh::keys::check_known_hosts_path(&self.host, self.port, key, p),
            _ => Ok(false),
        };
        match known {
            Ok(true) => true,
            Err(russh::keys::Error::KeyChanged { line }) => {
                *self.issue.lock().unwrap() = Some(DbError::HostKeyChanged { host: self.host.clone(), port: self.port, fingerprint, line });
                false
            }
            _ => {
                if self.check == HostKeyCheck::Ask && self.trust.as_deref() == Some(fingerprint.as_str()) {
                    if let Some(p) = &self.known_hosts
                        && let Err(e) = russh::keys::known_hosts::learn_known_hosts_path(&self.host, self.port, key, p)
                    {
                        // Still connect (the user confirmed this exact key); report nothing fatal.
                        let _ = e;
                    }
                    return true;
                }
                *self.issue.lock().unwrap() = Some(if self.check == HostKeyCheck::Strict {
                    DbError::Ssh(format!(
                        "{}:{} is not in ~/.ssh/known_hosts and strict host key checking is on ({fingerprint}); connect once with ssh, or set knownHostsCheck: ask",
                        self.host, self.port
                    ))
                } else {
                    DbError::HostKeyUnknown { host: self.host.clone(), port: self.port, key_type: key.algorithm().to_string(), fingerprint }
                });
                false
            }
        }
    }
}

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        let pk = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => PublicKey::from(c.public_key().clone()),
        };
        Ok(self.verdict(&pk))
    }
}

/// A live tunnel; clone to share. The SSH connection closes when the last clone is dropped.
#[derive(Clone)]
pub struct Tunnel(Arc<TunnelInner>);

struct TunnelInner {
    local_port: u16,
    handle: Option<Arc<Handle<SshHandler>>>,
    accept: AbortHandle,
}

impl Drop for TunnelInner {
    fn drop(&mut self) {
        self.accept.abort();
        if let (Some(h), Ok(rt)) = (self.handle.take(), tokio::runtime::Handle::try_current()) {
            rt.spawn(async move {
                let _ = h.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            });
        }
    }
}

impl Tunnel {
    /// Connect the database client here instead of the target host.
    pub fn local_port(&self) -> u16 {
        self.0.local_port
    }

    pub fn is_closed(&self) -> bool {
        self.0.handle.as_ref().is_none_or(|h| h.is_closed())
    }
}

#[derive(Default)]
pub struct TunnelManager {
    tunnels: tokio::sync::Mutex<HashMap<String, Weak<TunnelInner>>>,
}

impl TunnelManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the shared tunnel to `target_host:target_port` through `ssh`, opening it if needed.
    pub async fn acquire(&self, ssh: &SshTarget, target_host: &str, target_port: u16, trust: Option<&str>, timeout: Duration) -> Result<Tunnel> {
        let key = format!("{}@{}:{}|{}:{}", ssh.user, ssh.host, ssh.port, target_host, target_port);
        let mut map = self.tunnels.lock().await;
        if let Some(t) = map.get(&key).and_then(Weak::upgrade).map(Tunnel)
            && !t.is_closed()
        {
            return Ok(t);
        }
        map.retain(|_, w| w.strong_count() > 0);
        let t = open_tunnel(ssh, target_host, target_port, trust, timeout).await?;
        map.insert(key, Arc::downgrade(&t.0));
        Ok(t)
    }

    /// Open tunnels (for diagnostics).
    pub async fn active(&self) -> usize {
        self.tunnels.lock().await.values().filter(|w| w.strong_count() > 0).count()
    }
}

/// Opens a dedicated (unshared) tunnel.
pub async fn open_tunnel(ssh: &SshTarget, target_host: &str, target_port: u16, trust: Option<&str>, timeout: Duration) -> Result<Tunnel> {
    if ssh.host.is_empty() {
        return Err(DbError::Ssh("set the SSH host".into()));
    }
    if ssh.user.is_empty() {
        return Err(DbError::Ssh("set the SSH user".into()));
    }
    let issue = Arc::new(Mutex::new(None));
    let handler = SshHandler {
        host: ssh.host.clone(),
        port: ssh.port,
        check: ssh.check,
        trust: trust.map(str::to_string),
        known_hosts: ssh.known_hosts.clone().or_else(default_known_hosts),
        issue: issue.clone(),
    };
    let config = Arc::new(client::Config { keepalive_interval: Some(Duration::from_secs(30)), keepalive_max: 4, nodelay: true, ..Default::default() });
    let tcp = tokio::time::timeout(timeout, TcpStream::connect((ssh.host.as_str(), ssh.port)))
        .await
        .map_err(|_| DbError::Timeout(format!("{:?} connecting to SSH server {}:{}", timeout, ssh.host, ssh.port)))?
        .map_err(|e| DbError::Ssh(format!("cannot reach {}:{}: {e}", ssh.host, ssh.port)))?;
    let _ = tcp.set_nodelay(true);
    let connected = tokio::time::timeout(timeout, client::connect_stream(config, tcp, handler)).await;
    let mut handle = match connected {
        Err(_) => return Err(DbError::Timeout(format!("{timeout:?} during the SSH handshake with {}", ssh.host))),
        Ok(Err(e)) => {
            if let Some(i) = issue.lock().unwrap().take() {
                return Err(i);
            }
            return Err(DbError::Ssh(format!("handshake with {}:{} failed: {e}", ssh.host, ssh.port)));
        }
        Ok(Ok(h)) => h,
    };
    let authed = tokio::time::timeout(timeout, authenticate(&mut handle, ssh))
        .await
        .map_err(|_| DbError::Timeout(format!("{timeout:?} during SSH authentication with {}", ssh.host)))??;
    if !authed {
        let how = match ssh.auth {
            SshAuth::Key { .. } => "key",
            SshAuth::Password(_) => "password",
            SshAuth::Agent => "SSH agent keys",
        };
        return Err(DbError::Ssh(format!("authentication as {} on {} failed ({how})", ssh.user, ssh.host)));
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let local_port = listener.local_addr()?.port();
    let handle = Arc::new(handle);
    let accept_handle = handle.clone();
    let target = target_host.to_string();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut sock, peer)) = listener.accept().await else { break };
            let _ = sock.set_nodelay(true);
            let h = accept_handle.clone();
            let target = target.clone();
            tokio::spawn(async move {
                let opened = h.channel_open_direct_tcpip(target, u32::from(target_port), peer.ip().to_string(), u32::from(peer.port())).await;
                drop(h);
                if let Ok(ch) = opened {
                    let mut stream = ch.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut sock, &mut stream).await;
                }
            });
        }
    });
    Ok(Tunnel(Arc::new(TunnelInner { local_port, handle: Some(handle), accept: task.abort_handle() })))
}

async fn authenticate(handle: &mut Handle<SshHandler>, ssh: &SshTarget) -> Result<bool> {
    let user = ssh.user.clone();
    let res = match &ssh.auth {
        SshAuth::Key { path, passphrase } => {
            let key = russh::keys::load_secret_key(path, passphrase.as_deref()).map_err(|e| {
                DbError::Ssh(match e {
                    russh::keys::Error::KeyIsEncrypted => format!("{} is encrypted: set the key passphrase (as a secret)", path.display()),
                    other => format!("cannot load private key {}: {other}", path.display()),
                })
            })?;
            let hash = if key.algorithm().is_rsa() { handle.best_supported_rsa_hash().await.ok().flatten().flatten() } else { None };
            handle.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await
        }
        SshAuth::Password(p) => handle.authenticate_password(user, p.clone()).await,
        SshAuth::Agent => return agent_auth(handle, &ssh.user).await,
    };
    match res {
        Ok(AuthResult::Success) => Ok(true),
        Ok(AuthResult::Failure { .. }) => Ok(false),
        Err(e) => Err(DbError::Ssh(format!("authentication error: {e}"))),
    }
}

#[cfg(any(unix, windows))]
async fn agent_auth(handle: &mut Handle<SshHandler>, user: &str) -> Result<bool> {
    use russh::keys::agent::AgentIdentity;
    use russh::keys::agent::client::AgentClient;
    #[cfg(unix)]
    let mut agent = AgentClient::connect_env().await.map_err(|e| DbError::Ssh(format!("no key or password set and no SSH agent is available (SSH_AUTH_SOCK): {e}")))?;
    #[cfg(windows)]
    let mut agent = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
        .await
        .map_err(|e| DbError::Ssh(format!("no key or password set and the Windows OpenSSH agent is not running: {e}")))?;
    let ids = agent.request_identities().await.map_err(|e| DbError::Ssh(format!("SSH agent: {e}")))?;
    if ids.is_empty() {
        return Err(DbError::Ssh("the SSH agent has no keys (ssh-add one, or set a private key file)".into()));
    }
    for id in ids {
        let AgentIdentity::PublicKey { key, .. } = id else { continue };
        let hash = if key.algorithm().is_rsa() { handle.best_supported_rsa_hash().await.ok().flatten().flatten() } else { None };
        if let Ok(AuthResult::Success) = handle.authenticate_publickey_with(user, key, hash, &mut agent).await {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(not(any(unix, windows)))]
async fn agent_auth(_: &mut Handle<SshHandler>, _: &str) -> Result<bool> {
    Err(DbError::Ssh("SSH agent authentication is not supported on this platform; set a key or password".into()))
}
