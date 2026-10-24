//! SSH tunnels against an in-process russh server that forwards `direct-tcpip` channels to a local
//! echo server. Keys are derived from fixed seeds at runtime (no key material in the repo).

use std::sync::Arc;
use std::time::Duration;

use devall_db::DbError;
use devall_db::model::HostKeyCheck;
use devall_db::resolve::{SshAuth, SshTarget};
use devall_db::ssh::TunnelManager;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::{LineEnding, PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, MethodKind, MethodSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const T: Duration = Duration::from_secs(10);

fn key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

#[derive(Clone)]
struct Server {
    client_key: PublicKey,
}

impl russh::server::Handler for Server {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "tester" && password == "pw-123" { Auth::Accept } else { Auth::reject() })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(if user == "tester" && key.key_data() == self.client_key.key_data() { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let target = format!("{host}:{port}");
        reply.accept().await;
        tokio::spawn(async move {
            if let Ok(mut tcp) = TcpStream::connect(target).await {
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
            }
        });
        Ok(())
    }
}

/// Starts the SSH server; returns its port.
async fn ssh_server() -> u16 {
    let mut config = russh::server::Config { auth_rejection_time: Duration::from_millis(10), auth_rejection_time_initial: Some(Duration::ZERO), ..Default::default() };
    config.keys = vec![key(7)];
    config.methods = MethodSet::from(&[MethodKind::Password, MethodKind::PublicKey][..]);
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handler = Server { client_key: key(9).public_key().clone() };
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { break };
            let (config, handler) = (config.clone(), handler.clone());
            tokio::spawn(async move {
                if let Ok(running) = russh::server::run_stream(config, sock, handler).await {
                    let _ = running.await;
                }
            });
        }
    });
    port
}

/// A TCP echo server standing in for the database; returns its port.
async fn echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

async fn roundtrip(port: u16, msg: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(msg).await.unwrap();
    let mut buf = vec![0u8; msg.len()];
    tokio::time::timeout(T, s.read_exact(&mut buf)).await.unwrap().unwrap();
    buf
}

#[tokio::test]
async fn tunnel_with_trust_on_first_use_and_sharing() {
    let ssh_port = ssh_server().await;
    let db_port = echo_server().await;
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let target = SshTarget {
        host: "127.0.0.1".into(),
        port: ssh_port,
        user: "tester".into(),
        auth: SshAuth::Password("pw-123".into()),
        check: HostKeyCheck::Ask,
        known_hosts: Some(known_hosts.clone()),
    };
    let mgr = TunnelManager::new();

    // Unknown host → the fingerprint is surfaced for confirmation; nothing is written.
    let fingerprint = match mgr.acquire(&target, "127.0.0.1", db_port, None, T).await {
        Err(DbError::HostKeyUnknown { fingerprint, key_type, .. }) => {
            assert_eq!(key_type, "ssh-ed25519");
            assert!(fingerprint.starts_with("SHA256:"));
            fingerprint
        }
        Err(e) => panic!("expected an unknown host key, got {e}"),
        Ok(_) => panic!("expected an unknown host key"),
    };
    assert!(!known_hosts.exists());
    // A different fingerprint is not accepted.
    assert!(matches!(mgr.acquire(&target, "127.0.0.1", db_port, Some("SHA256:nope"), T).await, Err(DbError::HostKeyUnknown { .. })));

    // Confirmed → connects and records the key.
    let t1 = mgr.acquire(&target, "127.0.0.1", db_port, Some(&fingerprint), T).await.unwrap();
    assert_eq!(roundtrip(t1.local_port(), b"hello through ssh").await, b"hello through ssh");
    let recorded = std::fs::read_to_string(&known_hosts).unwrap();
    // (russh starts a newly created file with an empty line.)
    assert!(recorded.trim_start().starts_with(&format!("[127.0.0.1]:{ssh_port} ssh-ed25519 ")), "{recorded}");

    // Shared and reference-counted: same tunnel while someone holds it.
    let t2 = mgr.acquire(&target, "127.0.0.1", db_port, None, T).await.unwrap();
    assert_eq!(t1.local_port(), t2.local_port());
    assert_eq!(mgr.active().await, 1);
    drop(t1);
    assert_eq!(roundtrip(t2.local_port(), b"still up").await, b"still up");
    drop(t2);
    assert_eq!(mgr.active().await, 0, "closed with the last user");

    // Known now: strict checking passes too.
    let strict = SshTarget { check: HostKeyCheck::Strict, ..target.clone() };
    let t3 = mgr.acquire(&strict, "127.0.0.1", db_port, None, T).await.unwrap();
    assert_eq!(roundtrip(t3.local_port(), b"x").await, b"x");
}

#[tokio::test]
async fn key_auth_changed_keys_and_bad_passwords() {
    let ssh_port = ssh_server().await;
    let db_port = echo_server().await;
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let key_file = dir.path().join("id_ed25519");
    std::fs::write(&key_file, key(9).to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
    let target = SshTarget {
        host: "127.0.0.1".into(),
        port: ssh_port,
        user: "tester".into(),
        auth: SshAuth::Key { path: key_file.clone(), passphrase: None },
        check: HostKeyCheck::Off,
        known_hosts: Some(known_hosts.clone()),
    };
    let t = devall_db::ssh::open_tunnel(&target, "127.0.0.1", db_port, None, T).await.unwrap();
    assert_eq!(roundtrip(t.local_port(), b"key auth").await, b"key auth");

    let wrong_pw = SshTarget { auth: SshAuth::Password("nope".into()), ..target.clone() };
    let e = devall_db::ssh::open_tunnel(&wrong_pw, "127.0.0.1", db_port, None, T).await.err().unwrap();
    assert!(e.to_string().contains("authentication as tester"), "{e}");

    // A different key recorded for this host → refused, never auto-accepted.
    let other = key(3);
    std::fs::write(&known_hosts, format!("[127.0.0.1]:{ssh_port} {}\n", other.public_key().to_openssh().unwrap())).unwrap();
    let ask = SshTarget { check: HostKeyCheck::Ask, ..target.clone() };
    match devall_db::ssh::open_tunnel(&ask, "127.0.0.1", db_port, Some("SHA256:anything"), T).await {
        Err(DbError::HostKeyChanged { line, .. }) => assert_eq!(line, 1),
        Err(e) => panic!("expected a changed host key, got {e}"),
        Ok(_) => panic!("expected a changed host key"),
    }
    let strict_unknown = SshTarget { check: HostKeyCheck::Strict, known_hosts: Some(dir.path().join("empty_known_hosts")), ..target.clone() };
    let e = devall_db::ssh::open_tunnel(&strict_unknown, "127.0.0.1", db_port, None, T).await.err().unwrap();
    assert!(e.to_string().contains("strict"), "{e}");
}
